//! E1 runtime directory、ticket 和 pipe record 的直接契约测试。
//!
//! 这些测试只在 `/private/tmp` 下创建带唯一 basename 的 owner-only child，并在每项测试末尾
//! 精确删除自己创建的文件或空目录；不启动 root、Authorization、网络或 utun。

use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use exv_vpn_darwin_ipc::{
    auth::AUTH_KEY_LEN,
    bootstrap::{
        BootstrapProtocolError, ENGINE_BOOTSTRAP_V1_LEN, ENGINE_TICKET_V1_LEN, EngineBootstrapExit,
        EngineBootstrapRecord, EngineBootstrapSequence, EngineTicketV1, TicketError,
        consume_engine_ticket, create_engine_ticket, guarded_remove_engine_ticket,
    },
    path::{RuntimeDir, RuntimeDirError, RuntimeOwner},
};

static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

fn unique_direct_child(label: &str) -> PathBuf {
    let sequence = NEXT_PATH.fetch_add(1, Ordering::Relaxed);
    PathBuf::from("/private/tmp").join(format!(
        "exv-e1-{label}-{:x}-{sequence:x}",
        std::process::id()
    ))
}

fn set_mode(path: &std::path::Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set exact test mode");
}

fn assert_open_error(result: Result<RuntimeDir, RuntimeDirError>, expected: RuntimeDirError) {
    assert_eq!(result.err(), Some(expected));
}

#[test]
fn runtime_dir_create_is_owner_only_and_held_identity_revalidates() {
    let owner = RuntimeOwner::current();
    let runtime = RuntimeDir::create(owner).expect("create fixed-parent runtime directory");
    let metadata = fs::symlink_metadata(runtime.as_path()).expect("read runtime metadata");
    assert!(runtime.as_path().starts_with("/private/tmp/"));
    assert_eq!(metadata.uid(), owner.uid());
    assert_eq!(runtime.runtime_gid(), metadata.gid());
    assert_eq!(metadata.mode() & 0o7777, 0o700);
    runtime
        .revalidate_path()
        .expect("held runtime and parent fd identities still match");
    runtime
        .cleanup_empty()
        .expect("empty held runtime directory cleans itself");
}

#[test]
fn runtime_dir_rejects_alias_nested_non_directory_and_wrong_owner_or_mode() {
    let owner = RuntimeOwner::current();
    let runtime = RuntimeDir::create(owner).expect("create runtime directory");
    let path = runtime.as_path().to_path_buf();
    let alias = PathBuf::from("/tmp").join(path.file_name().expect("runtime basename"));
    assert_open_error(
        RuntimeDir::open_existing_for_uid(alias, owner.uid()),
        RuntimeDirError::PathInvalid,
    );
    assert_open_error(
        RuntimeDir::open_existing_for_uid("/private/tmp/nested-runtime/child", owner.uid()),
        RuntimeDirError::PathInvalid,
    );
    assert_open_error(
        RuntimeDir::open_existing_for_uid(&path, owner.uid().saturating_add(1)),
        RuntimeDirError::Ownership,
    );

    set_mode(&path, 0o755);
    assert_open_error(
        RuntimeDir::open_existing_for_uid(&path, owner.uid()),
        RuntimeDirError::Metadata,
    );
    set_mode(&path, 0o700);
    runtime.cleanup_empty().expect("cleanup restored runtime");

    let file = unique_direct_child("ordinary-file");
    fs::write(&file, b"not a directory").expect("create direct child file");
    assert_open_error(
        RuntimeDir::open_existing_for_uid(&file, owner.uid()),
        RuntimeDirError::Open,
    );
    fs::remove_file(file).expect("remove exact direct child file");
}

#[test]
fn runtime_dir_rejects_direct_child_symlink_without_following_it() {
    let owner = RuntimeOwner::current();
    let target = RuntimeDir::create(owner).expect("create symlink target runtime");
    let link = unique_direct_child("symlink");
    symlink(target.as_path(), &link).expect("create direct child symlink");
    assert_open_error(
        RuntimeDir::open_existing_for_uid(&link, owner.uid()),
        RuntimeDirError::Open,
    );
    fs::remove_file(&link).expect("remove exact test symlink");
    target
        .cleanup_empty()
        .expect("cleanup symlink target runtime");
}

#[test]
fn runtime_dir_path_replacement_refuses_revalidation_and_cleanup() {
    let owner = RuntimeOwner::current();
    let runtime = RuntimeDir::create(owner).expect("create held runtime");
    let path = runtime.as_path().to_path_buf();
    let displaced = unique_direct_child("displaced");
    fs::rename(&path, &displaced).expect("move original held directory aside");
    fs::create_dir(&path).expect("create replacement direct child directory");
    set_mode(&path, 0o700);

    assert_eq!(runtime.revalidate_path(), Err(RuntimeDirError::PathChanged));
    assert_eq!(
        runtime.cleanup_empty(),
        Err(RuntimeDirError::CleanupRefused)
    );

    fs::remove_dir(&path).expect("remove exact replacement directory");
    fs::remove_dir(&displaced).expect("remove exact displaced directory");
}

#[test]
fn runtime_dir_empty_nonempty_and_replaced_cleanup_have_distinct_outcomes() {
    let owner = RuntimeOwner::current();
    let nonempty = RuntimeDir::create(owner).expect("create nonempty runtime");
    let nonempty_path = nonempty.as_path().to_path_buf();
    let child = nonempty_path.join("hold");
    fs::write(&child, b"test-only child").expect("create controlled child");
    assert_eq!(
        nonempty.cleanup_empty(),
        Err(RuntimeDirError::CleanupRefused)
    );
    fs::remove_file(&child).expect("remove exact controlled child");
    fs::remove_dir(&nonempty_path).expect("remove exact nonempty-test runtime");

    let empty = RuntimeDir::create(owner).expect("create empty runtime");
    empty
        .cleanup_empty()
        .expect("empty runtime cleanup succeeds");
}

#[test]
fn ticket_create_consume_is_owner_only_fixed_length_and_one_time() {
    let owner = RuntimeOwner::current();
    let runtime = RuntimeDir::create(owner).expect("create ticket runtime");
    let ticket_path = runtime.as_path().join("engine.ticket");
    let identity = create_engine_ticket(
        &runtime,
        EngineTicketV1::new(owner.uid(), 4_321, [0xA5; AUTH_KEY_LEN])
            .expect("construct fixed V1 ticket"),
    )
    .expect("atomically create engine ticket");
    let metadata = fs::symlink_metadata(&ticket_path).expect("read created ticket metadata");
    assert!(metadata.is_file());
    assert_eq!(metadata.uid(), owner.uid());
    assert_eq!(metadata.mode() & 0o7777, 0o600);
    assert_eq!(
        usize::try_from(metadata.size()),
        Ok(ENGINE_TICKET_V1_LEN),
        "ticket metadata size must fit usize and match the fixed V1 length"
    );

    assert_eq!(
        create_engine_ticket(
            &runtime,
            EngineTicketV1::new(owner.uid(), 4_322, [0xB6; AUTH_KEY_LEN])
                .expect("construct second ticket"),
        ),
        Err(TicketError::Exists),
        "existing ticket must never be overwritten"
    );

    let consumed = consume_engine_ticket(&runtime).expect("consume exact created ticket");
    assert_eq!(consumed.owner_uid(), owner.uid());
    assert_eq!(consumed.core_pid(), 4_321);
    assert!(
        !ticket_path.exists(),
        "successful consume must unlink through the held dirfd"
    );
    assert!(
        matches!(consume_engine_ticket(&runtime), Err(TicketError::Open)),
        "a ticket is one-time material"
    );

    let _ = identity;
    runtime
        .cleanup_empty()
        .expect("empty runtime cleans after ticket consume");
}

#[test]
fn ticket_rejects_symlink_nonregular_wrong_mode_and_malformed_content() {
    let owner = RuntimeOwner::current();
    let runtime = RuntimeDir::create(owner).expect("create ticket rejection runtime");
    let ticket_path = runtime.as_path().join("engine.ticket");
    let symlink_target = runtime.as_path().join("ticket-target");

    fs::write(&symlink_target, b"outside ticket").expect("create symlink target");
    set_mode(&symlink_target, 0o600);
    symlink(&symlink_target, &ticket_path).expect("create ticket symlink");
    assert!(matches!(
        consume_engine_ticket(&runtime),
        Err(TicketError::Open)
    ));
    assert!(ticket_path.is_symlink(), "symlink must be preserved");
    fs::remove_file(&ticket_path).expect("remove exact ticket symlink");
    fs::remove_file(&symlink_target).expect("remove exact symlink target");

    fs::create_dir(&ticket_path).expect("create nonregular ticket entry");
    assert!(matches!(
        consume_engine_ticket(&runtime),
        Err(TicketError::Metadata)
    ));
    assert!(ticket_path.is_dir(), "nonregular entry must be preserved");
    fs::remove_dir(&ticket_path).expect("remove exact ticket directory");

    let mode_identity = create_engine_ticket(
        &runtime,
        EngineTicketV1::new(owner.uid(), 4_323, [0xC7; AUTH_KEY_LEN])
            .expect("construct mode ticket"),
    )
    .expect("create mode ticket");
    set_mode(&ticket_path, 0o644);
    assert!(matches!(
        consume_engine_ticket(&runtime),
        Err(TicketError::Metadata)
    ));
    assert!(ticket_path.exists(), "wrong-mode ticket must be preserved");
    set_mode(&ticket_path, 0o600);
    guarded_remove_engine_ticket(&runtime, mode_identity)
        .expect("guarded removal accepts restored original inode");

    let malformed_identity = create_engine_ticket(
        &runtime,
        EngineTicketV1::new(owner.uid(), 4_324, [0xD8; AUTH_KEY_LEN])
            .expect("construct malformed ticket"),
    )
    .expect("create malformed ticket");
    let mut malformed = [0_u8; ENGINE_TICKET_V1_LEN];
    malformed[0..4].copy_from_slice(&2_u32.to_be_bytes());
    fs::write(&ticket_path, malformed).expect("replace ticket bytes without changing inode");
    assert!(matches!(
        consume_engine_ticket(&runtime),
        Err(TicketError::Malformed)
    ));
    assert!(ticket_path.exists(), "malformed ticket must be preserved");
    guarded_remove_engine_ticket(&runtime, malformed_identity)
        .expect("guarded removal removes known malformed original inode");

    runtime
        .cleanup_empty()
        .expect("cleanup ticket rejection runtime");
}

#[test]
fn guarded_ticket_cleanup_refuses_replacement_and_preserves_it() {
    let owner = RuntimeOwner::current();
    let runtime = RuntimeDir::create(owner).expect("create guarded-ticket runtime");
    let ticket_path = runtime.as_path().join("engine.ticket");
    let displaced_path = runtime.as_path().join("displaced.ticket");
    let identity = create_engine_ticket(
        &runtime,
        EngineTicketV1::new(owner.uid(), 4_325, [0xE9; AUTH_KEY_LEN])
            .expect("construct guarded ticket"),
    )
    .expect("create guarded ticket");

    fs::rename(&ticket_path, &displaced_path).expect("displace created ticket inode");
    fs::write(&ticket_path, [0x55; ENGINE_TICKET_V1_LEN]).expect("create replacement ticket inode");
    set_mode(&ticket_path, 0o600);
    assert_eq!(
        guarded_remove_engine_ticket(&runtime, identity),
        Err(TicketError::CleanupRefused)
    );
    assert!(ticket_path.exists(), "replacement ticket must remain");
    assert!(displaced_path.exists(), "displaced ticket must remain");

    fs::remove_file(&ticket_path).expect("remove exact replacement ticket");
    fs::remove_file(&displaced_path).expect("remove exact displaced ticket");
    runtime
        .cleanup_empty()
        .expect("cleanup guarded-ticket runtime");
}

#[test]
fn engine_bootstrap_record_is_exact_20_bytes_and_rejects_invalid_fields() {
    let ready = EngineBootstrapRecord::Ready { pid: 4_321 };
    let bytes = ready.encode().expect("encode valid Ready record");
    assert_eq!(bytes.len(), ENGINE_BOOTSTRAP_V1_LEN);
    assert_eq!(&bytes[0..4], b"EXVB");
    assert_eq!(bytes[4], 1);
    assert_eq!(bytes[5], 1);
    assert_eq!(&bytes[6..8], &[0, 0]);
    assert_eq!(u32::from_be_bytes(bytes[8..12].try_into().unwrap()), 4_321);
    assert_eq!(u32::from_be_bytes(bytes[12..16].try_into().unwrap()), 0);
    assert_eq!(&bytes[16..20], &[0, 0, 0, 0]);
    assert_eq!(EngineBootstrapRecord::decode(&bytes), Ok(ready));

    let mut invalid = bytes;
    invalid[0] = b'X';
    assert_eq!(
        EngineBootstrapRecord::decode(&invalid),
        Err(BootstrapProtocolError::Magic)
    );
    invalid = bytes;
    invalid[4] = 2;
    assert_eq!(
        EngineBootstrapRecord::decode(&invalid),
        Err(BootstrapProtocolError::Version)
    );
    invalid = bytes;
    invalid[5] = 9;
    assert_eq!(
        EngineBootstrapRecord::decode(&invalid),
        Err(BootstrapProtocolError::Tag)
    );
    invalid = bytes;
    invalid[6] = 1;
    assert_eq!(
        EngineBootstrapRecord::decode(&invalid),
        Err(BootstrapProtocolError::Reserved)
    );
    invalid = bytes;
    invalid[16] = 1;
    assert_eq!(
        EngineBootstrapRecord::decode(&invalid),
        Err(BootstrapProtocolError::Reserved)
    );
    invalid = bytes;
    invalid[8..12].copy_from_slice(&0_u32.to_be_bytes());
    assert_eq!(
        EngineBootstrapRecord::decode(&invalid),
        Err(BootstrapProtocolError::Pid)
    );
    invalid = bytes;
    invalid[8..12].copy_from_slice(&(i32::MAX as u32 + 1).to_be_bytes());
    assert_eq!(
        EngineBootstrapRecord::decode(&invalid),
        Err(BootstrapProtocolError::Pid)
    );
    invalid = bytes;
    invalid[12..16].copy_from_slice(&1_u32.to_be_bytes());
    assert_eq!(
        EngineBootstrapRecord::decode(&invalid),
        Err(BootstrapProtocolError::Code)
    );
    assert_eq!(
        EngineBootstrapRecord::ExitError {
            pid: 4_321,
            code: 0
        }
        .encode(),
        Err(BootstrapProtocolError::Code)
    );
    assert_eq!(
        EngineBootstrapRecord::ExitError {
            pid: 4_321,
            code: i32::MAX as u32 + 1
        }
        .encode(),
        Err(BootstrapProtocolError::Code)
    );

    let mut written = Vec::new();
    ready.write_to(&mut written).expect("write exact record");
    assert_eq!(written, bytes);
    let mut short = &bytes[..ENGINE_BOOTSTRAP_V1_LEN - 1];
    assert_eq!(
        EngineBootstrapRecord::read_from(&mut short),
        Err(BootstrapProtocolError::Truncated)
    );
}

#[test]
fn engine_bootstrap_sequence_allows_only_ready_then_matching_terminal() {
    let pid = 4_321;
    let mut ok_sequence = EngineBootstrapSequence::new();
    assert_eq!(
        ok_sequence.accept(EngineBootstrapRecord::Ready { pid }),
        Ok(None)
    );
    assert_eq!(
        ok_sequence.accept(EngineBootstrapRecord::ExitOk { pid }),
        Ok(Some(EngineBootstrapExit::Ok { pid }))
    );
    assert_eq!(
        ok_sequence.finish_on_eof(),
        Ok(EngineBootstrapExit::Ok { pid })
    );

    let mut error_sequence = EngineBootstrapSequence::new();
    assert_eq!(
        error_sequence.accept(EngineBootstrapRecord::Ready { pid }),
        Ok(None)
    );
    assert_eq!(
        error_sequence.accept(EngineBootstrapRecord::ExitError { pid, code: 9 }),
        Ok(Some(EngineBootstrapExit::Error { pid, code: 9 }))
    );
    assert_eq!(
        error_sequence.finish_on_eof(),
        Ok(EngineBootstrapExit::Error { pid, code: 9 })
    );

    let mut exit_before_ready = EngineBootstrapSequence::new();
    assert_eq!(
        exit_before_ready.accept(EngineBootstrapRecord::ExitOk { pid }),
        Err(BootstrapProtocolError::Sequence)
    );
    assert_eq!(
        exit_before_ready.finish_on_eof(),
        Err(BootstrapProtocolError::Sequence)
    );

    let mut mismatched_pid = EngineBootstrapSequence::new();
    mismatched_pid
        .accept(EngineBootstrapRecord::Ready { pid })
        .expect("accept initial Ready");
    assert_eq!(
        mismatched_pid.accept(EngineBootstrapRecord::ExitOk { pid: pid + 1 }),
        Err(BootstrapProtocolError::Sequence)
    );

    let mut extra_record = EngineBootstrapSequence::new();
    extra_record
        .accept(EngineBootstrapRecord::Ready { pid })
        .expect("accept initial Ready");
    extra_record
        .accept(EngineBootstrapRecord::ExitOk { pid })
        .expect("accept terminal ExitOk");
    assert_eq!(
        extra_record.accept(EngineBootstrapRecord::ExitOk { pid }),
        Err(BootstrapProtocolError::Sequence)
    );

    let mut missing_terminal = EngineBootstrapSequence::new();
    missing_terminal
        .accept(EngineBootstrapRecord::Ready { pid })
        .expect("accept initial Ready");
    assert_eq!(
        missing_terminal.finish_on_eof(),
        Err(BootstrapProtocolError::Sequence)
    );
}

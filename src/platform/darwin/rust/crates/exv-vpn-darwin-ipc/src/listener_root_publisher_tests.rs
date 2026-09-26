#![cfg(test)]

use std::{ffi::OsString, fs, os::unix::fs::MetadataExt, path::PathBuf, sync::Arc};

use super::*;
use crate::auth::AUTH_KEY_LEN;

fn same_uid_sealed_runtime() -> (RuntimeDir, RuntimeOwner) {
    let mut runtime =
        RuntimeDir::create(RuntimeOwner::current()).expect("create owner-only same-uid runtime");
    let core_owner = runtime.owner();
    runtime
        .seal_for_same_uid_publisher_fixture()
        .expect("same-uid sealed runtime fixture");
    (runtime, core_owner)
}

fn root_config(runtime_dir: RuntimeDir, owner: RuntimeOwner) -> RootListenerConfig {
    RootListenerConfig::new(
        runtime_dir,
        ExpectedPeer::new(owner.uid(), std::process::id()),
        AuthKey::from_bytes([0xB4; AUTH_KEY_LEN]),
    )
    .expect("sealed runtime is accepted by root publisher config")
}

struct RestoreUmask(libc::mode_t);

impl Drop for RestoreUmask {
    fn drop(&mut self) {
        // SAFETY: the isolated child restores the exact umask captured before its fixture.
        unsafe {
            libc::umask(self.0);
        }
    }
}

struct FailBeforeFchown;

impl RootPublishHook for FailBeforeFchown {
    fn before_fchownat(&self, _socket_path: &SocketPath) -> Result<(), PreauthError> {
        Err(PreauthError::EndpointOwnership)
    }

    fn before_recheck(&self, _socket_path: &SocketPath) -> Result<(), PreauthError> {
        Ok(())
    }
}

struct ReplaceBeforeRecheck {
    socket_path: PathBuf,
    displaced_path: PathBuf,
}

impl RootPublishHook for ReplaceBeforeRecheck {
    fn before_fchownat(&self, _socket_path: &SocketPath) -> Result<(), PreauthError> {
        Ok(())
    }

    fn before_recheck(&self, _socket_path: &SocketPath) -> Result<(), PreauthError> {
        fs::rename(&self.socket_path, &self.displaced_path).map_err(|_| PreauthError::Transport)?;
        let replacement = std::os::unix::net::UnixListener::bind(&self.socket_path)
            .map_err(|_| PreauthError::Transport)?;
        drop(replacement);
        Ok(())
    }
}

struct ReplaceAfterBindBeforeInitialRecord {
    socket_path: PathBuf,
    displaced_path: PathBuf,
}

impl AfterBindBeforeInitialRecordHook for ReplaceAfterBindBeforeInitialRecord {
    fn after_bind_before_initial_record(
        &self,
        _socket_path: &SocketPath,
    ) -> Result<(), PreauthError> {
        fs::rename(&self.socket_path, &self.displaced_path).map_err(|_| PreauthError::Transport)?;
        let replacement = std::os::unix::net::UnixListener::bind(&self.socket_path)
            .map_err(|_| PreauthError::Transport)?;
        drop(replacement);
        Err(PreauthError::Transport)
    }
}

fn assert_isolated_child_invocation() {
    assert_eq!(
        std::env::var_os("EXV_DARWIN_ROOT_PUBLISHER_TEST_CHILD"),
        Some(OsString::from("1")),
        "ignored body must only run from the fixed self-child launcher"
    );
}

fn assert_runtime_create_reasserts_0700_under_restrictive_umask() {
    // SAFETY: this isolated child temporarily uses the restrictive fixture umask only for the
    // Core directory creation assertion; RestoreUmask returns to the child baseline afterwards.
    let prior_umask = unsafe { libc::umask(0o177) };
    let _restore = RestoreUmask(prior_umask);
    assert_eq!(
        prior_umask, 0o027,
        "child must restore its distinct baseline before publisher assertions"
    );

    let runtime = RuntimeDir::create(RuntimeOwner::current())
        .expect("held-dirfd fchmod restores new runtime directory mode");
    let metadata = fs::symlink_metadata(runtime.as_path()).expect("read created runtime metadata");

    assert_eq!(metadata.mode() & 0o7777, 0o700);
    runtime
        .cleanup_empty()
        .expect("explicitly repaired runtime directory cleans normally");
}

fn assert_same_uid_publisher_is_0600_connectable_and_restores_umask() {
    let (runtime, owner) = same_uid_sealed_runtime();
    let runtime_path = runtime.as_path().to_path_buf();
    let socket_path = runtime
        .engine_socket_path()
        .expect("derive fixed socket path")
        .as_path()
        .to_path_buf();
    let config = root_config(runtime, owner);

    // SAFETY: this runs in the dedicated one-test child process that owns its umask state.
    let listener = unsafe { PreauthListener::bind_root_publisher_single_threaded(config) }
        .expect("same-uid sealed publisher succeeds");
    // SAFETY: setting the known child baseline also reveals the previous process umask. The
    // same value remains installed, so later publisher fixtures start from that baseline.
    let observed_umask = unsafe { libc::umask(0o027) };
    assert_eq!(
        observed_umask, 0o027,
        "publisher must restore the distinct prior umask"
    );

    let metadata = fs::symlink_metadata(&socket_path).expect("read published socket");
    assert!(metadata.file_type().is_socket());
    assert_eq!(metadata.uid(), owner.uid());
    assert_eq!(metadata.gid(), owner.gid());
    assert_eq!(metadata.mode() & 0o7777, 0o600);
    let core_connection = std::os::unix::net::UnixStream::connect(&socket_path)
        .expect("same-uid Core can connect to its 0600 socket");
    drop(core_connection);

    listener
        .cleanup()
        .expect("normal cleanup removes recorded socket and sealed runtime directory");
    assert!(!socket_path.exists());
    assert!(!runtime_path.exists());
}

fn assert_fchown_failure_after_record_has_no_listener_handoff_and_cleans_own_socket() {
    let (runtime, owner) = same_uid_sealed_runtime();
    let runtime_path = runtime.as_path().to_path_buf();
    let socket_path = runtime
        .engine_socket_path()
        .expect("derive fixed socket path")
        .as_path()
        .to_path_buf();
    let config = root_config(runtime, owner).with_root_publish_hook(Arc::new(FailBeforeFchown));

    // SAFETY: this runs in the dedicated one-test child process that owns its umask state.
    let result = unsafe { PreauthListener::bind_root_publisher_single_threaded(config) };
    assert!(matches!(result, Err(PreauthError::EndpointOwnership)));
    assert!(
        !socket_path.exists(),
        "recorded failed publisher socket is guarded-cleaned"
    );
    assert!(
        !runtime_path.exists(),
        "recorded failed publisher runtime is removed only after socket cleanup"
    );
}

fn assert_post_record_leaf_replacement_returns_no_listener_and_preserves_replacement() {
    let (runtime, owner) = same_uid_sealed_runtime();
    let runtime_path = runtime.as_path().to_path_buf();
    let socket_path = runtime
        .engine_socket_path()
        .expect("derive fixed socket path")
        .as_path()
        .to_path_buf();
    let displaced_path = runtime_path.join("recorded-original.sock");
    let hook = ReplaceBeforeRecheck {
        socket_path: socket_path.clone(),
        displaced_path: displaced_path.clone(),
    };
    let config = root_config(runtime, owner).with_root_publish_hook(Arc::new(hook));

    // SAFETY: this runs in the dedicated one-test child process that owns its umask state.
    let result = unsafe { PreauthListener::bind_root_publisher_single_threaded(config) };
    assert!(matches!(
        result,
        Err(PreauthError::PathInvalid | PreauthError::EndpointOwnership)
    ));
    assert!(
        socket_path.exists(),
        "post-record replacement must not be deleted by guarded cleanup"
    );
    assert!(
        displaced_path.exists(),
        "displaced recorded socket must remain for test-controlled cleanup"
    );
    assert!(
        runtime_path.exists(),
        "nonempty runtime with replacement must be preserved"
    );

    fs::remove_file(&socket_path).expect("remove exact replacement socket");
    fs::remove_file(&displaced_path).expect("remove exact displaced original socket");
    fs::remove_dir(&runtime_path).expect("remove exact replacement-test runtime");
}

fn assert_pre_record_leaf_replacement_returns_no_listener_and_preserves_replacement() {
    let (runtime, owner) = same_uid_sealed_runtime();
    let runtime_path = runtime.as_path().to_path_buf();
    let socket_path = runtime
        .engine_socket_path()
        .expect("derive fixed socket path")
        .as_path()
        .to_path_buf();
    let displaced_path = runtime_path.join("pre-record-original.sock");
    let hook = ReplaceAfterBindBeforeInitialRecord {
        socket_path: socket_path.clone(),
        displaced_path: displaced_path.clone(),
    };
    let config =
        root_config(runtime, owner).with_after_bind_before_initial_record_hook(Arc::new(hook));

    // SAFETY: this runs in the dedicated one-test child process that owns its umask state.
    let result = unsafe { PreauthListener::bind_root_publisher_single_threaded(config) };
    assert!(matches!(result, Err(PreauthError::Transport)));
    assert!(
        socket_path.exists(),
        "pre-record replacement must remain because no endpoint identity was recorded"
    );
    assert!(
        displaced_path.exists(),
        "displaced original leaf must remain because publisher performed zero unlink"
    );
    assert!(
        runtime_path.exists(),
        "pre-record replacement keeps the runtime directory nonempty and intact"
    );

    fs::remove_file(&socket_path).expect("remove exact pre-record replacement socket");
    fs::remove_file(&displaced_path).expect("remove exact pre-record displaced socket");
    fs::remove_dir(&runtime_path).expect("remove exact pre-record runtime");
}

#[test]
fn root_publisher_umask_scenarios_run_in_isolated_child() {
    let status = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .env("EXV_DARWIN_ROOT_PUBLISHER_TEST_CHILD", "1")
        .arg("--ignored")
        .arg("--exact")
        .arg("listener::listener_root_publisher_tests::root_publisher_umask_child")
        .arg("--test-threads=1")
        .status()
        .expect("run fixed root publisher test child");
    assert!(
        status.success(),
        "isolated root publisher test child failed"
    );
}

#[expect(
    clippy::ignore_without_reason,
    reason = "G0 守卫要求此 test-child 入口保持裸 #[ignore]，原因由本属性明确记录"
)]
#[ignore]
#[tokio::test(flavor = "current_thread")]
async fn root_publisher_umask_child() {
    assert_isolated_child_invocation();
    // SAFETY: only this one-test child changes its process-global umask. The distinct 0027
    // baseline proves the publisher restores the caller's value rather than its temporary 0177.
    // RestoreUmask returns to the value inherited by the child before the test exits.
    let original_umask = unsafe { libc::umask(0o027) };
    let _restore = RestoreUmask(original_umask);

    assert_runtime_create_reasserts_0700_under_restrictive_umask();
    assert_same_uid_publisher_is_0600_connectable_and_restores_umask();
    assert_fchown_failure_after_record_has_no_listener_handoff_and_cleans_own_socket();
    assert_post_record_leaf_replacement_returns_no_listener_and_preserves_replacement();
    assert_pre_record_leaf_replacement_returns_no_listener_and_preserves_replacement();
}

#[test]
fn root_publisher_rejects_unsealed_runtime_before_bind() {
    let runtime =
        RuntimeDir::create(RuntimeOwner::current()).expect("create unsealed runtime for rejection");
    let runtime_path = runtime.as_path().to_path_buf();
    let result = RootListenerConfig::new(
        runtime,
        ExpectedPeer::current_process(),
        AuthKey::from_bytes([0x91; AUTH_KEY_LEN]),
    );

    assert!(matches!(result, Err(PreauthError::PathInvalid)));
    assert!(
        runtime_path.is_dir(),
        "unsealed runtime must remain untouched"
    );
    fs::remove_dir(&runtime_path).expect("remove exact unsealed test runtime");
}

#[test]
fn root_publisher_rejects_existing_fixed_leaf_without_unlinking() {
    let (runtime, owner) = same_uid_sealed_runtime();
    let runtime_path = runtime.as_path().to_path_buf();
    let socket_path = runtime
        .engine_socket_path()
        .expect("derive fixed socket path")
        .as_path()
        .to_path_buf();
    fs::write(&socket_path, b"pre-existing leaf").expect("create exact existing leaf");
    let config = root_config(runtime, owner);

    // SAFETY: this error path rejects before umask mutation or UDS bind.
    let result = unsafe { PreauthListener::bind_root_publisher_single_threaded(config) };
    assert!(matches!(result, Err(PreauthError::EndpointExists)));
    assert!(socket_path.is_file(), "existing leaf must not be unlinked");

    fs::remove_file(&socket_path).expect("remove exact existing test leaf");
    fs::remove_dir(&runtime_path).expect("remove exact sealed test runtime");
}

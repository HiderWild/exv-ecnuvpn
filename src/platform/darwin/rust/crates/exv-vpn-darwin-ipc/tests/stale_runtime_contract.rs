
use std::{
    fs,
    os::unix::{fs::MetadataExt, net::UnixListener, prelude::PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use exv_vpn_darwin_ipc::{
    auth::AUTH_KEY_LEN,
    bootstrap::{
        EngineTicketV1, consume_engine_ticket, create_engine_ticket, guarded_remove_engine_ticket,
    },
    path::{RuntimeDir, RuntimeOwner},
};

static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

/// 测试自建条目的清理方式。
enum StaleEntry {
    /// 末尾 `remove_dir`（owner 测试进程总能删除自己的空目录）。
    Directory(PathBuf),
    /// 末尾 `remove_file`（普通文件或 socket 文件）。
    File(PathBuf),
}

fn unique_stale_child(label: &str) -> PathBuf {
    let sequence = NEXT_PATH.fetch_add(1, Ordering::Relaxed);
    PathBuf::from("/private/tmp").join(format!(
        "exv-s2-{label}-{:x}-{sequence:x}",
        std::process::id()
    ))
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set exact stale mode");
}

/// 预置真机盘点观察到的用户态可构造 stale 形态，返回需要末尾清理的清单。
fn seed_observed_stale_shapes() -> Vec<StaleEntry> {
    let mut stale = Vec::new();

    // 盘点 B 类：Core uid 未封印空 0700 目录（Core 创建 runtime 后进程强杀的孤儿）。
    let empty_dir = unique_stale_child("core-orphan-empty");
    fs::create_dir(&empty_dir).expect("create empty core-orphan stale directory");
    set_mode(&empty_dir, 0o700);
    stale.push(StaleEntry::Directory(empty_dir));

    // 盘点 A 类近似：0711 目录 + 0600 `engine.sock` 普通文件（真实形态为 root-owned
    // sealed 目录 + 孤儿 socket，非 root 测试不能 chown root，只能以同 uid 0711 逼近）。
    let sealed_lookalike = unique_stale_child("sealed-lookalike");
    fs::create_dir(&sealed_lookalike).expect("create sealed-lookalike stale directory");
    set_mode(&sealed_lookalike, 0o711);
    let orphan_sock = sealed_lookalike.join("engine.sock");
    fs::write(&orphan_sock, b"stale orphan socket placeholder")
        .expect("write orphan socket placeholder");
    set_mode(&orphan_sock, 0o600);
    stale.push(StaleEntry::File(orphan_sock));
    stale.push(StaleEntry::Directory(sealed_lookalike));

    // 孤儿 socket 活形态：在 `engine.sock` 名上真实 bind 一个 UDS listener 再释放，
    // 留下真实 socket 文件，验证残留 socket 文件与新会话 socket 的路径隔离。
    let live_socket_dir = unique_stale_child("live-socket");
    fs::create_dir(&live_socket_dir).expect("create live-socket stale directory");
    set_mode(&live_socket_dir, 0o700);
    let listener =
        UnixListener::bind(live_socket_dir.join("engine.sock")).expect("bind stale-dir listener");
    listener
        .local_addr()
        .expect("read stale listener local address");
    drop(listener);
    stale.push(StaleEntry::File(live_socket_dir.join("engine.sock")));
    stale.push(StaleEntry::Directory(live_socket_dir));

    // 盘点 ticket 形态（真机实测为零残留，防御性固定）：陌生目录内的 0600 垃圾
    // `engine.ticket` + 杂物 child，任何新会话流程都不得触碰或复用。
    let ticket_dir = unique_stale_child("stale-ticket");
    fs::create_dir(&ticket_dir).expect("create stale-ticket directory");
    set_mode(&ticket_dir, 0o700);
    let stale_ticket = ticket_dir.join("engine.ticket");
    fs::write(&stale_ticket, [0xEE_u8; 44]).expect("write stale ticket bytes");
    set_mode(&stale_ticket, 0o600);
    fs::write(ticket_dir.join("unrelated-child"), b"foreign junk")
        .expect("write unrelated stale child");
    stale.push(StaleEntry::File(stale_ticket));
    stale.push(StaleEntry::File(ticket_dir.join("unrelated-child")));
    stale.push(StaleEntry::Directory(ticket_dir));

    // 错误 mode 残留：0755 目录（sealed 空 root 目录 `0711` 的进一步权限异常近似）。
    let wrong_mode_dir = unique_stale_child("wrong-mode");
    fs::create_dir(&wrong_mode_dir).expect("create wrong-mode stale directory");
    set_mode(&wrong_mode_dir, 0o755);
    stale.push(StaleEntry::Directory(wrong_mode_dir));

    stale
}

fn stale_paths(stale: &[StaleEntry]) -> Vec<PathBuf> {
    stale
        .iter()
        .map(|entry| match entry {
            StaleEntry::Directory(path) | StaleEntry::File(path) => path.clone(),
        })
        .collect()
}

fn assert_stale_scene_preserved(stale: &[StaleEntry]) {
    for entry in stale {
        match entry {
            StaleEntry::Directory(path) => assert!(
                path.is_dir(),
                "stale directory {} must be preserved untouched",
                path.display()
            ),
            StaleEntry::File(path) => assert!(
                path.exists(),
                "stale file {} must be preserved untouched",
                path.display()
            ),
        }
    }
}

fn cleanup_stale_scene(stale: Vec<StaleEntry>) {
    for entry in stale {
        match entry {
            StaleEntry::Directory(path) => fs::remove_dir(path).expect("remove own stale dir"),
            StaleEntry::File(path) => fs::remove_file(path).expect("remove own stale file"),
        }
    }
}

fn fresh_ticket(owner: RuntimeOwner) -> EngineTicketV1 {
    EngineTicketV1::new(owner.uid(), std::process::id(), [0x5A; AUTH_KEY_LEN])
        .expect("construct contract ticket")
}

#[test]
fn observed_stale_residue_shapes_do_not_block_repeated_new_session_bootstrap() {
    let stale = seed_observed_stale_shapes();
    let owner = RuntimeOwner::current();

    // 连续两轮新会话 bootstrap：残留不仅不阻断单次建立，累积残留（宿主真实形态随
    // 崩溃次数增长）也不阻断。
    for round in 0..2 {
        let runtime = RuntimeDir::create(owner)
            .unwrap_or_else(|error| panic!("round {round}: create must ignore residue: {error:?}"));
        let path = runtime.as_path().to_path_buf();
        assert!(
            path.starts_with("/private/tmp/"),
            "round {round}: runtime must stay under the fixed parent"
        );
        assert!(
            !stale_paths(&stale).contains(&path),
            "round {round}: fresh runtime must be a new randomly named directory"
        );
        assert_eq!(
            fs::symlink_metadata(&path)
                .expect("read fresh runtime metadata")
                .mode()
                & 0o7777,
            0o700,
            "round {round}: fresh runtime keeps the owner-only 0700 contract"
        );

        let socket_path = runtime.engine_socket_path().unwrap_or_else(|error| {
            panic!("round {round}: socket path must stay valid: {error:?}")
        });
        assert!(
            socket_path.as_path().starts_with(&path),
            "round {round}: new session socket lives inside its own runtime directory"
        );

        let ticket_identity =
            create_engine_ticket(&runtime, fresh_ticket(owner)).unwrap_or_else(|error| {
                panic!("round {round}: ticket creation must ignore stale tickets: {error:?}")
            });

        // Core 启动失败路径（对齐 launch_fixed_engine_session 的 start_engine 失败
        // 分支）：只回收自己的 ticket 与空目录。
        guarded_remove_engine_ticket(&runtime, ticket_identity)
            .unwrap_or_else(|error| panic!("round {round}: own ticket cleanup: {error:?}"));
        runtime
            .cleanup_empty()
            .unwrap_or_else(|error| panic!("round {round}: own empty runtime cleanup: {error:?}"));
        assert!(
            !path.exists(),
            "round {round}: failure-path cleanup must leave no new orphan runtime directory"
        );

        assert_stale_scene_preserved(&stale);
    }

    cleanup_stale_scene(stale);
}

#[test]
fn stale_residue_does_not_conflict_with_the_full_core_engine_filesystem_handshake() {
    let stale = seed_observed_stale_shapes();
    let owner = RuntimeOwner::current();

    let runtime =
        RuntimeDir::create(owner).expect("create fresh runtime while stale residue exists");
    let fresh_path = runtime.as_path().to_path_buf();
    let socket_path = runtime
        .engine_socket_path()
        .expect("fresh socket path stays valid");

    let stale_socket_dir = stale
        .iter()
        .find_map(|entry| match entry {
            StaleEntry::Directory(path)
                if path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().contains("sealed-lookalike")) =>
            {
                Some(path.clone())
            }
            _ => None,
        })
        .expect("seeded sealed-lookalike stale directory");
    assert_ne!(
        socket_path.as_path().parent().map(Path::to_path_buf),
        Some(stale_socket_dir.clone()),
        "new session socket must live in its own directory, never beside a stale orphan socket"
    );
    assert!(
        stale_socket_dir.join("engine.sock").is_file(),
        "stale orphan socket placeholder must survive until the end of the test"
    );

    // 新会话自己的 ticket 建立成功：陌生目录里的 stale ticket 与新目录内 O_EXCL
    // 创建互不影响；Engine 侧的一次性消费也照常成功。
    create_engine_ticket(&runtime, fresh_ticket(owner))
        .expect("create fresh ticket despite stale residue");
    let consumed = consume_engine_ticket(&runtime).expect("engine consumes the fresh ticket");
    assert_eq!(consumed.core_pid(), std::process::id());
    assert!(
        !runtime.as_path().join("engine.ticket").exists(),
        "successful consume must unlink only the fresh ticket"
    );

    // 完整握手期间 stale 现场依旧完好（新会话流程没有任何按名字模式清理的行为）。
    assert_stale_scene_preserved(&stale);
    assert!(
        fresh_path.is_dir(),
        "fresh runtime survives until Core drops it"
    );
    runtime
        .cleanup_empty()
        .expect("empty fresh runtime cleanup after consume");
    cleanup_stale_scene(stale);
}

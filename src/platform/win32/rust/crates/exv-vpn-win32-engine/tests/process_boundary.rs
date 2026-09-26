
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use exv_vpn_domain::identity::{
    ConnectionBindingDigest, EffectId, OperationId, OperationLookupKey, OperationMethod,
    OwnershipVersion, PrincipalDigest, RequestDigest, ResourceIdentityDigest, RuntimeEpoch,
    TokenDigest,
};
use exv_vpn_domain::ports::{
    AdmissionWatermark, AuthorityEpoch, AuthorityFence, CanonicalInputDigest,
    JournalOperationIdentity, JournalRevision, PlatformAuthorityInstanceId,
};
use exv_vpn_resource::admission::{
    AdmissionRecord, AppliedFingerprint, AuthorizationSubject, MutationAdmitted, MutationKind,
    ObligationSeed, encode_record,
};
use exv_vpn_resource::authority::ConnectionBinding;
use exv_vpn_resource::journal::JournalRecord;
use exv_vpn_resource::operation::{AdmissionIndex, AdmissionOutcome, MutationAdmissionInput};
use exv_engine::composition::{
    compose_privileged_helper, ComposeConfig, ComposeError, CompositionPhase, HelperComposition,
};
use exv_engine::owner_lease::{LeaseIssue, OwnerLeaseManager};
use exv_engine::shutdown::{ShutdownOutcome, shutdown_composition};
use exv_vpn_win32_resource::journal_path::JournalPath;
use exv_vpn_win32_resource::journal_store::WinJournalStore;

use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A fresh, per-test temp directory: %TEMP%\exv-w26-<pid>-<tag>. Any stale copy from a
/// prior crashed run is removed so every test starts clean.
fn test_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("exv-w26-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A unique `Local\`-scoped authority mutex name per test (tag + pid), so parallel tests
/// and repeated runs never collide.
fn authority_name(tag: &str) -> String {
    format!(r"Local\ExvVpnW26Authority_{tag}_{}", std::process::id())
}

/// Compose the privileged helper for the given tag over a fresh temp journal dir, and
/// return (journal_dir, composition) so the test can shutdown and clean up.
fn compose(tag: &str) -> (PathBuf, HelperComposition) {
    let dir = test_dir(tag);
    let composition = compose_privileged_helper(ComposeConfig {
        authority_name: authority_name(tag),
        journal_dir: dir.clone(),
    })
    .expect("compose the privileged helper");
    (dir, composition)
}

/// The startup order the helper must record: authority first, recovery next, endpoint
/// last (架构 §5.1: 先 lock 后 scan；endpoint publication 是最后一步).
fn assert_full_startup(phases: &[CompositionPhase]) {
    let expected: &[CompositionPhase] = &[
        CompositionPhase::AuthorityAcquired,
        CompositionPhase::RecoveryCompleted,
        CompositionPhase::EndpointPublished,
    ];
    assert_eq!(
        phases, expected,
        "the endpoint must be published only AFTER the authority is acquired and startup \
         recovery completes (mutant: endpoint published first)"
    );
}

/// Deterministic 32-byte identity digest that differs across `n`.
fn digest(n: u8) -> [u8; 32] {
    let mut b = [0u8; 32];
    b[0] = n;
    b
}

/// Deterministic principal digest that differs across `n`.
fn principal(n: u8) -> PrincipalDigest {
    PrincipalDigest::try_from(digest(n)).expect("digest")
}

/// Deterministic request digest that differs across `n`.
fn request_digest(n: u8) -> RequestDigest {
    RequestDigest::try_from(digest(n)).expect("digest")
}

/// Deterministic candidate/held token digest that differs across `n`.
fn token(n: u8) -> TokenDigest {
    TokenDigest::try_from(digest(n)).expect("digest")
}

/// Deterministic connection binding that differs across `n`.
fn binding(n: u8) -> ConnectionBinding {
    let connection_digest = ConnectionBindingDigest::try_from(digest(n)).expect("digest");
    ConnectionBinding::try_from(connection_digest).expect("binding")
}

/// Deterministic non-zero ownership version for the given `n`.
fn version(n: u64) -> OwnershipVersion {
    OwnershipVersion::try_from(n).expect("version")
}

/// Deterministic non-nil runtime epoch for the given `n`.
fn epoch(n: u128) -> RuntimeEpoch {
    RuntimeEpoch::try_from(Uuid::from_u128(n)).expect("epoch")
}

/// Deterministic non-nil operation id for the given `n`.
fn operation(n: u128) -> OperationId {
    OperationId::try_from(Uuid::from_u128(n)).expect("operation")
}

/// Deterministic lookup key anchored to the given principal and method (fixed epoch/op).
fn lookup_key(principal_n: u8, method: OperationMethod) -> OperationLookupKey {
    OperationLookupKey::try_from((principal(principal_n), method, epoch(1), operation(1)))
        .expect("lookup key")
}

/// Deterministic authority fence minted at epoch 1, instance 1, watermark 0, revision 0.
fn fence() -> AuthorityFence {
    AuthorityFence {
        authority_epoch: AuthorityEpoch::try_from(1).expect("epoch 1"),
        platform_authority_instance_id: PlatformAuthorityInstanceId::try_from(Uuid::from_u128(1))
            .expect("non-nil instance"),
        admission_watermark: AdmissionWatermark::try_from(0).expect("watermark 0"),
        journal_revision: JournalRevision::try_from(0).expect("revision 0"),
    }
}

/// A canonical J51 mutation admission input for the given key/digest/ownership version
/// (W15-T 同款；ingress 的 durability gate 是 encode_record/decode_record 往返).
fn admission_input(
    key: &OperationLookupKey,
    request_digest: &RequestDigest,
    ownership_version: OwnershipVersion,
) -> MutationAdmissionInput {
    MutationAdmissionInput {
        key: key.clone(),
        request_digest: request_digest.clone(),
        effect_id: EffectId::try_from(Uuid::from_u128(1000)).expect("non-nil effect"),
        initiator_identity_digest: principal(9),
        canonical_input_digest: CanonicalInputDigest::try_from([0x11; 32]).expect("digest"),
        mutation_kind: MutationKind::External(OperationMethod::Connect),
        ownership_version,
        authorization_subject: AuthorizationSubject::LiveOwnershipTokenDigest(token(7)),
        resource_identity: ResourceIdentityDigest::try_from([0x33; 32]).expect("digest"),
        precondition_fingerprint: AppliedFingerprint::try_from([0x44; 32]).expect("digest"),
        desired_applied_fingerprint: AppliedFingerprint::try_from([0x55; 32]).expect("digest"),
        canonical_obligation_seed: ObligationSeed::try_from([0x66; 32]).expect("digest"),
    }
}

/// A canonical J51 `MutationAdmitted` sealed as a durable admission record (W25-T 同款；
/// `encode_record` codec 往返是 durability gate).
fn admitted(
    key: &OperationLookupKey,
    ownership_version: OwnershipVersion,
    subject: AuthorizationSubject,
    resource_identity: [u8; 32],
    desired_fingerprint: [u8; 32],
    mutation_kind: MutationKind,
) -> MutationAdmitted {
    let f = fence();
    MutationAdmitted {
        mutation_kind,
        journal_operation_identity: JournalOperationIdentity::External(key.clone()),
        effect_id: EffectId::try_from(Uuid::from_u128(1000)).expect("non-nil effect"),
        canonical_input_digest: CanonicalInputDigest::try_from([0x11; 32]).expect("digest"),
        initiator_identity_digest: principal(9),
        authority_epoch: f.authority_epoch,
        platform_authority_instance_id: f.platform_authority_instance_id,
        admission_watermark: f.admission_watermark,
        ownership_version,
        authorization_subject: subject,
        resource_identity: ResourceIdentityDigest::try_from(resource_identity).expect("digest"),
        precondition_fingerprint: AppliedFingerprint::try_from([0x44; 32]).expect("digest"),
        desired_applied_fingerprint: AppliedFingerprint::try_from(desired_fingerprint)
            .expect("digest"),
        canonical_obligation_seed: ObligationSeed::try_from([0x66; 32]).expect("digest"),
    }
}

/// Append one J51 admission as the next chained journal record (J50 frame + FlushFileBuffers
/// durability via the committed W14 store). Returns the record and its byte offset.
fn append_admission(
    store: &mut WinJournalStore,
    seq: u64,
    prev: [u8; 32],
    m: &MutationAdmitted,
) -> (JournalRecord, u64) {
    let rec = JournalRecord::new(seq, prev, encode_record(&AdmissionRecord::Admitted(m.clone())));
    let offset = store.append_synced(&rec).expect("append_synced a J51 admission record");
    (rec, offset)
}

/// The single journal file the store created in `dir` (never hard-code the file name).
fn journal_file_in(dir: &PathBuf) -> PathBuf {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("read the journal directory")
        .map(|e| e.expect("a directory entry").path())
        .filter(|p| p.is_file())
        .collect();
    assert_eq!(files.len(), 1, "the journal directory must contain exactly one file");
    files.remove(0)
}

// ---------------------------------------------------------------------------
// Child-process sentinels (two-instance authority race)
// ---------------------------------------------------------------------------

const CHILD_ENV: &str = "EXV_W26_CHILD";
const MUTEX_ENV: &str = "EXV_W26_MUTEX";
const JOURNAL_ENV: &str = "EXV_W26_JOURNAL";
/// The loser observed `AuthorityBusy` (WAIT_TIMEOUT=258) and exited cleanly, having done
/// NO mutation effect. This is the only acceptable loser exit code.
const CHILD_EXIT_BUSY: i32 = 250;
/// The second helper was (wrongly) granted authority — the double-master mutant.
const CHILD_EXIT_PUBLISHED: i32 = 255;
/// The child hit a ComposeError it did not expect.
const CHILD_EXIT_ERROR: i32 = 252;

/// Re-execute this test binary as the child. libtest's `--exact` runs only the named
/// test; that test checks `child_role()` first and exits before any parent work.
fn spawn_child(mode: &str, name: &str, journal_dir: &str, exact_test: &str) -> Child {
    Command::new(std::env::current_exe().expect("current test exe"))
        .env(CHILD_ENV, mode)
        .env(MUTEX_ENV, name)
        .env(JOURNAL_ENV, journal_dir)
        .arg("--exact")
        .arg(exact_test)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the child test process")
}

/// Child role entry, called at the top of every test that can be re-executed as a child.
/// Returns the exit code when running as a child; `None` in the parent role.
fn child_role() -> Option<i32> {
    let mode = std::env::var(CHILD_ENV).ok()?;
    let name = std::env::var(MUTEX_ENV).expect("child must receive the authority name");
    let journal_dir = std::env::var(JOURNAL_ENV).expect("child must receive the journal dir");
    match mode.as_str() {
        // Second-instance role: composing while the parent holds the authority must fail
        // with AuthorityBusy (WSP2 §1: the loser exits before any scan/observe/publish).
        // Getting Ok here is the "两个 helper 都接受 mutation" double-master mutant.
        "compose" => match compose_privileged_helper(ComposeConfig {
            authority_name: name,
            journal_dir: PathBuf::from(journal_dir),
        }) {
            Err(ComposeError::AuthorityBusy) => Some(CHILD_EXIT_BUSY),
            Ok(_) => Some(CHILD_EXIT_PUBLISHED),
            Err(_) => Some(CHILD_EXIT_ERROR),
        },
        other => panic!("unknown child mode {other}"),
    }
}

// ---------------------------------------------------------------------------
// The 6 pinned cases (Terra oracle plan §6.1 W26-T row, exact names)
// ---------------------------------------------------------------------------

/// Kills 'endpoint published before authority/recovery' (plan W26 mutant): the helper
/// acquires the singleton authority FIRST, runs startup recovery SECOND, and only then
/// publishes the endpoint (架构 §5.1: 先 lock 后 scan；plan §8 必测第 1 条).
#[test]
fn authority_and_recovery_finish_before_endpoint_publication() {
    let (dir, composition) = compose("order");
    assert_full_startup(composition.phases());
    shutdown_composition(composition).expect("shutdown the helper");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Kills 'mutation accepted without authentication' (plan §8 必测第 2 条): the helper's
/// ONLY mutation surface is the authenticated ingress gate — an ordinary request with no
/// durable J51 admission is refused before any native effect, and only a durable admission
/// (codec round-trip = durability) opens the gate.
#[test]
fn ordinary_user_cannot_call_native_mutation_directly() {
    let (dir, mut composition) = compose("ingress");

    // A live per-connection lease exists, but no J51 admission has been made durable yet.
    let mut manager = OwnerLeaseManager::new();
    let conn = binding(3);
    let key = lookup_key(1, OperationMethod::Connect);
    let d1 = request_digest(10);
    let v1 = version(1);
    assert!(matches!(
        manager.issue(&conn, key.clone(), d1.clone(), v1, token(20)),
        LeaseIssue::Issued { .. }
    ));
    let lease = manager.lease(&conn).expect("a live lease for the connection");

    // The ingress gate is the only mutation entry; the unauthenticated request is refused.
    let gate = composition.ingress();
    assert!(
        gate.admit(lease, key.clone(), d1.clone()).is_err(),
        "an ordinary (unauthenticated) mutation request must be refused: no durable \
         admission exists, so no native effect may happen"
    );

    // Only a durable J51 admission opens the gate (sync-before-reply, W15).
    let mut index = AdmissionIndex::new(fence());
    let admitted = match index.admit(admission_input(&key, &d1, v1)) {
        AdmissionOutcome::Admitted { record } => record,
        _ => panic!("expected a durable admission"),
    };
    assert!(
        gate.durable_before_reply(&admitted),
        "the durable MutationAdmitted must open the ingress gate"
    );
    assert!(
        gate.admit(lease, key, d1).is_ok(),
        "the same request must pass once its J51 admission is durable"
    );

    shutdown_composition(composition).expect("shutdown the helper");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Kills '两个 helper 都接受 mutation'（plan W26 mutant）: a second helper process composing
/// under the same authority name must observe `AuthorityBusy` and exit BEFORE any mutation
/// effect (WSP2 §1: WAIT_TIMEOUT loser exits without scan/observe/publish). Real
/// two-process race via the W13-T child pattern.
#[test]
fn one_process_has_one_helper_authority() {
    if let Some(code) = child_role() {
        std::process::exit(code);
    }

    let dir = test_dir("two-proc");
    let child_dir = test_dir("two-proc-child");
    let name = authority_name("two-proc");

    // The first process composes the helper and holds the singleton authority.
    let composition = compose_privileged_helper(ComposeConfig {
        authority_name: name.clone(),
        journal_dir: dir.clone(),
    })
    .expect("the first process composes the helper");
    assert_full_startup(composition.phases());

    // The second process (real child) composes the same authority name and must exit with
    // the Busy sentinel, having done NO mutation effect.
    let mut child = spawn_child(
        "compose",
        &name,
        &child_dir.to_string_lossy(),
        "one_process_has_one_helper_authority",
    );
    let output = child.wait_with_output().expect("the child finishes");
    let code = output.status.code().expect("the child exits with a code");
    assert_eq!(
        code,
        CHILD_EXIT_BUSY,
        "the second helper instance must observe AuthorityBusy and exit with sentinel \
         {CHILD_EXIT_BUSY} (no mutation effect); got exit {code} (double-master/error mutant)"
    );

    // The first helper still owns the authority and shuts down cleanly.
    shutdown_composition(composition).expect("shutdown the first helper");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&child_dir);
}

/// Kills 'shutdown detach'（plan W26 mutant）: shutdown must JOIN every native worker the
/// helper started before releasing the authority/final handle (架构 §6.2/§6.3: 不允许 detach
/// 未知 worker；W17/W24 join-before-final 的进程级投影). After shutdown no state is left:
/// the released authority is immediately reacquirable by a fresh helper (WSP2 §1:
/// mutex_reusable_after_release).
#[test]
fn shutdown_joins_native_workers() {
    let (dir, composition) = compose("shutdown");
    let name = authority_name("shutdown");

    let workers = composition.native_workers();
    let outcome = shutdown_composition(composition).expect("shutdown must complete");
    assert!(
        matches!(outcome, ShutdownOutcome::Joined { workers_joined } if workers_joined == workers),
        "shutdown must JOIN exactly the {workers} native workers the helper started \
         (detach mutant), got {outcome:?}"
    );

    // No state left when the helper exits: the authority is released, so a fresh helper
    // on the same name composes immediately.
    let restarted = compose_privileged_helper(ComposeConfig {
        authority_name: name,
        journal_dir: dir.clone(),
    })
    .expect("the released authority must be immediately reacquirable by a fresh helper");
    assert_full_startup(restarted.phases());
    shutdown_composition(restarted).expect("shutdown the restarted helper");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Kills 'restart 不重建 projection'（plan §8 必测第 5 条）: each helper lifetime re-projects
/// the durable journal at startup. A durable J51 admission landing between lifetimes must
/// change the projection digest; a restart over UNCHANGED durable state must reproduce the
/// IDENTICAL digest (W25 contract: 同一 projection 重复计算必须稳定).
#[test]
fn restart_rebuilds_durable_projection() {
    let (dir, first) = compose("restart");
    let name = authority_name("restart");
    let d_empty = first.projection_digest();
    shutdown_composition(first).expect("shutdown the first lifetime");

    // A durable J51 admission lands in the journal between lifetimes (W14 store).
    let key = lookup_key(1, OperationMethod::Connect);
    let m0 = admitted(
        &key,
        version(7),
        AuthorizationSubject::LiveOwnershipTokenDigest(token(7)),
        [0x33; 32],
        [0x55; 32],
        MutationKind::External(OperationMethod::Connect),
    );
    {
        let mut store =
            WinJournalStore::open(&JournalPath::from_dir(dir.clone())).expect("open the journal");
        append_admission(&mut store, 0, [0u8; 32], &m0);
    }

    // Restart: the projection must be rebuilt from the durable journal, not cached.
    let second = compose_privileged_helper(ComposeConfig {
        authority_name: name.clone(),
        journal_dir: dir.clone(),
    })
    .expect("the restart composes");
    let d_appended = second.projection_digest();
    assert_ne!(
        d_empty, d_appended,
        "a restart must rebuild the projection from the durable journal (the new admission \
         must be visible), not from a memory cache"
    );
    shutdown_composition(second).expect("shutdown the second lifetime");

    // Restart over UNCHANGED durable state reproduces the exact same projection.
    let third = compose_privileged_helper(ComposeConfig {
        authority_name: name,
        journal_dir: dir.clone(),
    })
    .expect("the third compose");
    assert_eq!(
        d_appended,
        third.projection_digest(),
        "a restart over unchanged durable state must reproduce the identical projection digest"
    );
    shutdown_composition(third).expect("shutdown the third lifetime");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Oracle kill for 'endpoint 先发布'（plan W26 mutant）: when startup recovery CANNOT
/// complete (corrupt middle record — WSP2 §4: 不跳过), the helper must NOT publish the
/// endpoint: compose fails typed `RecoveryCorrupt` carrying the corrupt frame offset, and
/// the failed compose leaves no state behind (the authority is released).
#[test]
fn oracle_kills_endpoint_before_recovery_mutant() {
    let dir = test_dir("corrupt");
    let name = authority_name("corrupt");
    let key = lookup_key(1, OperationMethod::Connect);
    let m0 = admitted(
        &key,
        version(7),
        AuthorizationSubject::LiveOwnershipTokenDigest(token(7)),
        [0x33; 32],
        [0x55; 32],
        MutationKind::External(OperationMethod::Connect),
    );
    let m1 = admitted(
        &key,
        version(7),
        AuthorizationSubject::LiveOwnershipTokenDigest(token(8)),
        [0x34; 32],
        [0x56; 32],
        MutationKind::External(OperationMethod::Connect),
    );
    let r1_offset;
    {
        let mut store =
            WinJournalStore::open(&JournalPath::from_dir(dir.clone())).expect("open the journal");
        let (r0, _) = append_admission(&mut store, 0, [0u8; 32], &m0);
        let (_, off) = append_admission(&mut store, 1, r0.digest, &m1);
        r1_offset = off;
    }

    // Corrupt a byte inside record 1's payload (frame starts at r1_offset, payload at +45,
    // W25-T 同款) so the projection reports Corrupt at record 1's frame offset.
    let file = journal_file_in(&dir);
    let mut bytes = std::fs::read(&file).expect("read the journal file");
    let payload_byte = (r1_offset as usize) + 45;
    assert!(payload_byte < bytes.len(), "record 1 payload byte must be within the file");
    bytes[payload_byte] ^= 0xFF;
    std::fs::write(&file, &bytes).expect("rewrite the corrupted journal file");

    // Startup recovery cannot complete over a corrupt projection: the endpoint MUST NOT be
    // published. The 'endpoint 先发布' mutant would return Ok here and die on expect_err.
    let err = compose_privileged_helper(ComposeConfig {
        authority_name: name.clone(),
        journal_dir: dir.clone(),
    })
    .expect_err("the endpoint must not be published when startup recovery cannot complete");
    assert!(
        matches!(err, ComposeError::RecoveryCorrupt { offset } if offset == r1_offset as usize),
        "a corrupt middle record must fail composition with the corrupt frame offset \
         {r1_offset}, got {err:?}"
    );

    // The failed composition leaves no state behind: the authority was released, so a fresh
    // helper on the same name composes immediately.
    let clean_dir = test_dir("corrupt-clean");
    let clean = compose_privileged_helper(ComposeConfig {
        authority_name: name,
        journal_dir: clean_dir.clone(),
    })
    .expect("after the failed compose the authority must be free for a fresh helper");
    shutdown_composition(clean).expect("shutdown the clean helper");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&clean_dir);
}


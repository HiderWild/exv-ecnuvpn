
use crate::error::{EffectCertainty, ErrorSubject, VpnError};
use crate::identity::{
    AttemptId, EffectId, EvidenceDigest, InteractionId, InventoryDigest, OperationLookupKey,
    OperationLookupKeyDigest, OwnerLeaseId, OwnershipVersion, RecoveryId, RequestDigest,
    ResourceIdentityDigest, RetirementOperationId, RuntimeEpoch, TokenDigest,
};
use crate::model::{
    CleanupProofRef, ConnectIntent, PacketLeaseRef, PlatformOwnershipRef, PlatformReadyProof,
    PromptDeadline, ProtocolSessionRef, RecoveryContext, RecoveryObligation,
};
use serde::Serialize;
use std::convert::Infallible;
use std::future::Future;
use std::net::Ipv4Addr;
use std::pin::Pin;
use uuid::Uuid;

pub type PortFuture<'a, T, E = VpnError> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct AuthorityEpoch(u64);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct PlatformAuthorityInstanceId(Uuid);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct AdmissionWatermark(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct JournalRevision(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct MonotonicTick(u64);

#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
pub struct CanonicalInputDigest([u8; 32]);

#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
pub struct CleanupTriggerDigest([u8; 32]);

#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ExternalOperationDigest([u8; 32]);

#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
pub struct JournalRootDigest([u8; 32]);

#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
pub struct TunnelIntentRef(ResourceIdentityDigest);

#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
pub struct AuthorityFence {
    pub authority_epoch: AuthorityEpoch,
    pub platform_authority_instance_id: PlatformAuthorityInstanceId,
    pub admission_watermark: AdmissionWatermark,
    pub journal_revision: JournalRevision,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
pub enum JournalOperationIdentity {
    External(OperationLookupKey),
    Retirement(RetirementOperationId),
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
pub struct MutationReceipt {
    pub journal_operation_identity: JournalOperationIdentity,
    pub canonical_input_digest: CanonicalInputDigest,
    pub effect_id: EffectId,
    pub authority_fence: AuthorityFence,
    pub ownership_version: OwnershipVersion,
    pub certainty: EffectCertainty,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
pub struct VersionedPlatformEvidence {
    pub kind_version: u32,
    pub digest: EvidenceDigest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Ipv4Route {
    pub network: Ipv4Addr,
    pub prefix_len: u8,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct TunnelPlan {
    pub ipv4_address: Ipv4Addr,
    pub ipv4_prefix_len: u8,
    pub mtu: u16,
    pub ipv4_routes: Vec<Ipv4Route>,
    pub dns_servers: Vec<Ipv4Addr>,
    pub control_bypass: Vec<Ipv4Addr>,
    pub opaque_intent: TunnelIntentRef,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
pub struct AttemptEffectFence {
    pub runtime_epoch: RuntimeEpoch,
    pub attempt_id: AttemptId,
    pub effect_id: EffectId,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
pub struct OwnedAttemptEffectFence {
    pub attempt_effect: AttemptEffectFence,
    pub platform_ownership: PlatformOwnershipRef,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
pub struct InteractionEffectFence {
    pub attempt_effect: AttemptEffectFence,
    pub interaction_id: InteractionId,
    pub deadline: PromptDeadline,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
pub enum PacketStopFence {
    Active(OwnedAttemptEffectFence),
    Recovery {
        runtime_epoch: RuntimeEpoch,
        recovery_id: RecoveryId,
        retirement_operation_id: RetirementOperationId,
        platform_ownership: PlatformOwnershipRef,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub enum CleanupTrigger {
    ExternalStop {
        lookup_key: OperationLookupKey,
        request_digest: RequestDigest,
    },
    ExternalReconcile {
        lookup_key: OperationLookupKey,
        request_digest: RequestDigest,
    },
    OwnerLost(OwnerLeaseId),
    PacketBoundaryLost(PacketLeaseRef),
    StartupRecovery(RecoveryId),
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct CleanProof {
    pub runtime_epoch: RuntimeEpoch,
    pub platform_authority_instance: PlatformAuthorityInstanceId,
    pub prior_ownership_version: OwnershipVersion,
    pub prior_platform_ownership_token_digest_if_issued: Option<TokenDigest>,
    pub retirement_operation_id: RetirementOperationId,
    pub cleanup_trigger_digest: CleanupTriggerDigest,
    pub external_trigger_operation_identity_digest_if_present: Option<ExternalOperationDigest>,
    pub canonical_obligation_inventory_digest: InventoryDigest,
    pub journal_root_or_projection_digest: JournalRootDigest,
    pub journal_revision: JournalRevision,
    pub platform_evidence: VersionedPlatformEvidence,
    pub unresolved_obligation_count: u32,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub enum CleanupOutcome {
    ProvenClean(CleanProof),
    StillOwned(RecoveryObligation),
    EffectUnknown(RecoveryObligation),
    ObservationFailed(RecoveryObligation),
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub enum OwnedStateObservation {
    NoOwnedState {
        authority: AuthorityFence,
    },
    Owned {
        authority: AuthorityFence,
        cleanup: CleanupOutcome,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub enum OperationTerminal {
    Succeeded {
        receipt: MutationReceipt,
    },
    Failed {
        receipt: MutationReceipt,
        error: VpnError,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct RejectionReason {
    pub error: VpnError,
    pub authority_fence: AuthorityFence,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub enum OperationState {
    Pending,
    Terminal(OperationTerminal),
    RejectedNoEffect {
        reason: RejectionReason,
    },
    AbsentNoEffect {
        lookup_key_digest: OperationLookupKeyDigest,
        authority_epoch: AuthorityEpoch,
        admission_watermark: AdmissionWatermark,
    },
    Unknown,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ObserveOwnedStateRequest {
    pub runtime_epoch: RuntimeEpoch,
    pub recovery_id: RecoveryId,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct AcquireOwnershipRequest {
    pub lookup_key: OperationLookupKey,
    pub request_digest: RequestDigest,
    pub fence: AttemptEffectFence,
    pub owner_lease_id: OwnerLeaseId,
    pub previous_cleanup_ref: Option<CleanupProofRef>,
}

pub struct OwnershipAcquired<T> {
    pub platform_ownership: PlatformOwnershipRef,
    pub token: T,
    pub receipt: MutationReceipt,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ApplyTunnelRequest {
    pub lookup_key: OperationLookupKey,
    pub request_digest: RequestDigest,
    pub fence: OwnedAttemptEffectFence,
    pub tunnel_plan: TunnelPlan,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct PlatformApplied {
    pub platform_ready: PlatformReadyProof,
    pub packet_lease: PacketLeaseRef,
    pub receipt: MutationReceipt,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct BeginStopRequest {
    pub trigger: CleanupTrigger,
    pub origin_subject: ErrorSubject,
    pub runtime_epoch: RuntimeEpoch,
    pub platform_ownership: PlatformOwnershipRef,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct BeginRecoveryStopRequest {
    pub context: RecoveryContext,
    pub trigger: CleanupTrigger,
    pub origin_subject: ErrorSubject,
    pub platform_ownership: PlatformOwnershipRef,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct RetirementStarted {
    pub trigger: CleanupTrigger,
    pub origin_subject: ErrorSubject,
    pub retirement_operation_id: RetirementOperationId,
    pub prior_platform_ownership: PlatformOwnershipRef,
    pub canonical_inventory_digest: InventoryDigest,
    pub journal_revision: JournalRevision,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ReconcileRequest {
    pub context: RecoveryContext,
    pub retirement_operation_id: RetirementOperationId,
    pub obligation: RecoveryObligation,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseOwnershipRequest {
    pub retirement_operation_id: RetirementOperationId,
    pub proof: CleanProof,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct OwnershipRetired {
    pub next_ownership_version: OwnershipVersion,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
pub struct GetOperationRequest {
    pub lookup_key: OperationLookupKey,
    pub request_digest: RequestDigest,
}

pub enum ProtocolProgress<H> {
    InteractionRequired,
    Established {
        protocol_session: ProtocolSessionRef,
        tunnel_plan: TunnelPlan,
        late_cleanup: H,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub enum ProtocolTerminal {
    Terminated,
    NotTerminated(VpnError),
}

pub struct PacketAttached<H> {
    pub packet_lease: PacketLeaseRef,
    pub late_cleanup: H,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub enum PacketTerminal {
    Terminated,
    NotTerminated(VpnError),
}

pub enum ProtocolLateCleanupOutcome<H> {
    Terminated,
    StillLive {
        error: VpnError,
        handle: H,
        obligation: RecoveryObligation,
    },
}

pub enum PacketLateCleanupOutcome<H> {
    Terminated,
    StillLive {
        error: VpnError,
        handle: H,
        obligation: RecoveryObligation,
    },
}

pub trait ProtocolPort: Send + Sync {
    type LateHandle: Send + 'static;

    fn connect(
        &self,
        fence: AttemptEffectFence,
        intent: ConnectIntent,
    ) -> PortFuture<'_, ProtocolProgress<Self::LateHandle>>;
    fn respond_interaction(
        &self,
        fence: InteractionEffectFence,
    ) -> PortFuture<'_, ProtocolProgress<Self::LateHandle>>;
    fn begin_stop(
        &self,
        fence: AttemptEffectFence,
        session: ProtocolSessionRef,
    ) -> PortFuture<'_, ProtocolTerminal>;
    fn cleanup_late(
        &self,
        handle: Self::LateHandle,
    ) -> PortFuture<'_, ProtocolLateCleanupOutcome<Self::LateHandle>, Infallible>;
}

pub trait Clock: Send + Sync {
    fn monotonic_now(&self) -> MonotonicTick;
}

pub trait EntropySource: Send + Sync {
    fn next_uuid(&self) -> Uuid;
}

// ---- D10-T construction seams (ports) ----

impl TryFrom<u64> for MonotonicTick {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Ok(MonotonicTick(value))
    }
}

impl MonotonicTick {
    // Crate-internal read of the raw tick backing; NOT a public getter (H00 D2).
    pub(crate) fn as_inner(self) -> u64 {
        self.0
    }

    /// Whether `later` is strictly after this tick, in the same monotonic clock domain.
    /// Exposes only ordering, never the raw backing value (H00 D2). Used for deadline expiry.
    #[must_use]
    pub fn exceeded_by(self, later: MonotonicTick) -> bool {
        later.0 > self.0
    }
}

impl TryFrom<u64> for AuthorityEpoch {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Ok(AuthorityEpoch(value))
    }
}

impl TryFrom<Uuid> for PlatformAuthorityInstanceId {
    type Error = &'static str;
    fn try_from(uuid: Uuid) -> Result<Self, Self::Error> {
        if uuid.is_nil() {
            return Err("platform authority instance: nil uuid");
        }
        Ok(PlatformAuthorityInstanceId(uuid))
    }
}

impl TryFrom<u64> for AdmissionWatermark {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Ok(AdmissionWatermark(value))
    }
}

impl TryFrom<u64> for JournalRevision {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Ok(JournalRevision(value))
    }
}

macro_rules! digest_try_from {
    ($ty:ident) => {
        impl TryFrom<[u8; 32]> for $ty {
            type Error = &'static str;
            fn try_from(bytes: [u8; 32]) -> Result<Self, Self::Error> {
                Ok($ty(bytes))
            }
        }
    };
}

digest_try_from!(CanonicalInputDigest);
digest_try_from!(CleanupTriggerDigest);
digest_try_from!(ExternalOperationDigest);
digest_try_from!(JournalRootDigest);

impl TryFrom<ResourceIdentityDigest> for TunnelIntentRef {
    type Error = &'static str;
    fn try_from(identity_digest: ResourceIdentityDigest) -> Result<Self, Self::Error> {
        if identity_digest.is_nil() {
            return Err("tunnel intent ref: nil identity");
        }
        Ok(TunnelIntentRef(identity_digest))
    }
}

impl
    TryFrom<(
        Ipv4Addr,
        u8,
        u16,
        Vec<Ipv4Route>,
        Vec<Ipv4Addr>,
        Vec<Ipv4Addr>,
        TunnelIntentRef,
    )> for TunnelPlan
{
    type Error = &'static str;
    fn try_from(
        (
            ipv4_address,
            ipv4_prefix_len,
            mtu,
            ipv4_routes,
            dns_servers,
            control_bypass,
            opaque_intent,
        ): (
            Ipv4Addr,
            u8,
            u16,
            Vec<Ipv4Route>,
            Vec<Ipv4Addr>,
            Vec<Ipv4Addr>,
            TunnelIntentRef,
        ),
    ) -> Result<Self, Self::Error> {
        if ipv4_prefix_len > 32 {
            return Err("tunnel plan: ipv4 prefix over 32");
        }
        if mtu < 576 {
            return Err("tunnel plan: mtu below minimum");
        }
        Ok(TunnelPlan {
            ipv4_address,
            ipv4_prefix_len,
            mtu,
            ipv4_routes,
            dns_servers,
            control_bypass,
            opaque_intent,
        })
    }
}


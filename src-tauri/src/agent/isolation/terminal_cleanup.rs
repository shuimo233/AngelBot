//! Durable terminal cleanup contract for delegated attempts.
//!
//! This module is deliberately a deep control-plane seam.  It accepts only
//! Main-Agent-issued opaque identities and hides ordering, idempotency,
//! retry, and failure projection behind [`TerminalCleanupPort`].  Concrete
//! filesystem/VCS adapters are injected later; no path is accepted or
//! resolved here.

use std::{
    collections::{BTreeSet, HashMap},
    fmt,
    sync::{Arc, Mutex},
};

use super::resource_binding::{
    CleanupStatus as ResourceCleanupStatus, ResourceBinding, ResourceBindingError,
    ResourceBindingIdentity, ResourceBindingRepository, ResourceKind,
};
use super::workspace_isolation::{WorktreeManifestIdentity, WorktreeManifestProvider};

const MAX_ID_LEN: usize = 256;
const MAX_SCOPE_DIGEST_LEN: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupContractError {
    InvalidIdentity(&'static str),
    IdentityMismatch(&'static str),
    InvalidScopeDigest,
    StaleLeaseEpoch,
}

impl fmt::Display for CleanupContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentity(field) => write!(f, "invalid opaque identity: {field}"),
            Self::IdentityMismatch(field) => write!(f, "cleanup identity mismatch: {field}"),
            Self::InvalidScopeDigest => f.write_str("invalid cleanup scope digest"),
            Self::StaleLeaseEpoch => f.write_str("stale cleanup lease epoch"),
        }
    }
}

impl std::error::Error for CleanupContractError {}

fn valid_opaque_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn validate_id(value: &str, field: &'static str) -> Result<(), CleanupContractError> {
    if valid_opaque_id(value) {
        Ok(())
    } else {
        Err(CleanupContractError::InvalidIdentity(field))
    }
}

fn validate_scope_digest(value: &str) -> Result<(), CleanupContractError> {
    if value.is_empty()
        || value.len() > MAX_SCOPE_DIGEST_LEN
        || value.contains(['\0', '\r', '\n'])
        || value.contains(['/', '\\'])
    {
        return Err(CleanupContractError::InvalidScopeDigest);
    }
    Ok(())
}

/// Opaque lease identity.  It is a CAS token projection, not a path or a
/// mutable lease record.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LeaseIdentity {
    pub id: String,
    pub attempt_id: String,
    pub lease_epoch: u64,
}

impl LeaseIdentity {
    pub fn new(
        id: impl Into<String>,
        attempt_id: impl Into<String>,
        lease_epoch: u64,
    ) -> Result<Self, CleanupContractError> {
        let identity = Self {
            id: id.into(),
            attempt_id: attempt_id.into(),
            lease_epoch,
        };
        validate_id(&identity.id, "lease_id")?;
        validate_id(&identity.attempt_id, "attempt_id")?;
        Ok(identity)
    }
}

/// Opaque workspace-admission row identity.  `state_version` is used for
/// release CAS and must never be guessed by a child agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AdmissionLeaseIdentity {
    pub id: String,
    pub attempt_id: String,
    pub lease_id: String,
    pub state_version: i64,
}

impl AdmissionLeaseIdentity {
    pub fn new(
        id: impl Into<String>,
        attempt_id: impl Into<String>,
        lease_id: impl Into<String>,
        state_version: i64,
    ) -> Result<Self, CleanupContractError> {
        let identity = Self {
            id: id.into(),
            attempt_id: attempt_id.into(),
            lease_id: lease_id.into(),
            state_version,
        };
        validate_id(&identity.id, "admission_id")?;
        validate_id(&identity.attempt_id, "attempt_id")?;
        validate_id(&identity.lease_id, "lease_id")?;
        if identity.state_version < 0 {
            return Err(CleanupContractError::InvalidIdentity("state_version"));
        }
        Ok(identity)
    }
}

/// Opaque sandbox identity.  The concrete sandbox adapter resolves this
/// through its own manifest; no path is representable at this seam.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SandboxIdentity {
    pub id: String,
    pub attempt_id: String,
    pub nonce: String,
}

impl SandboxIdentity {
    pub fn new(
        id: impl Into<String>,
        attempt_id: impl Into<String>,
        nonce: impl Into<String>,
    ) -> Result<Self, CleanupContractError> {
        let identity = Self {
            id: id.into(),
            attempt_id: attempt_id.into(),
            nonce: nonce.into(),
        };
        validate_id(&identity.id, "sandbox_id")?;
        validate_id(&identity.attempt_id, "attempt_id")?;
        validate_id(&identity.nonce, "sandbox_nonce")?;
        Ok(identity)
    }
}

/// Opaque worktree identity.  A future `GitWorktreeProvider` adapter can use
/// the handle id to load a provider-owned manifest after restart.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorktreeIdentity {
    pub id: String,
    pub work_package_id: String,
    pub attempt_id: String,
    pub lease_epoch: u64,
    pub provider_kind: String,
    pub scope_digest: String,
    pub manifest_locator: String,
    pub manifest_nonce: String,
    pub manifest_digest: String,
}

impl WorktreeIdentity {
    pub fn new(
        id: impl Into<String>,
        attempt_id: impl Into<String>,
        lease_epoch: u64,
    ) -> Result<Self, CleanupContractError> {
        let identity = Self {
            id: id.into(),
            work_package_id: "legacy_unresolved".into(),
            attempt_id: attempt_id.into(),
            lease_epoch,
            provider_kind: "legacy_unresolved".into(),
            scope_digest: "legacy_unresolved".into(),
            manifest_locator: "legacy_unresolved".into(),
            manifest_nonce: "legacy_unresolved".into(),
            manifest_digest: "legacy_unresolved".into(),
        };
        validate_id(&identity.id, "worktree_id")?;
        validate_id(&identity.attempt_id, "attempt_id")?;
        Ok(identity)
    }

    pub fn from_binding(binding: &ResourceBindingIdentity) -> Result<Self, CleanupContractError> {
        let identity = Self {
            id: binding.resource_ref.clone(),
            work_package_id: binding.work_package_id.clone(),
            attempt_id: binding.attempt_id.clone(),
            lease_epoch: binding.lease_epoch as u64,
            provider_kind: binding.provider_kind.clone(),
            scope_digest: binding.scope_digest.clone(),
            manifest_locator: binding.manifest_locator.clone(),
            manifest_nonce: binding.manifest_nonce.clone(),
            manifest_digest: binding.manifest_digest.clone(),
        };
        validate_id(&identity.id, "worktree_id")?;
        validate_id(&identity.work_package_id, "work_package_id")?;
        validate_id(&identity.attempt_id, "attempt_id")?;
        validate_id(&identity.provider_kind, "provider_kind")?;
        validate_id(&identity.manifest_locator, "manifest_locator")?;
        validate_id(&identity.manifest_nonce, "manifest_nonce")?;
        validate_scope_digest(&identity.scope_digest)?;
        validate_scope_digest(&identity.manifest_digest)?;
        if identity.lease_epoch == 0 {
            return Err(CleanupContractError::StaleLeaseEpoch);
        }
        Ok(identity)
    }
}

/// Trusted binding assembled by the Main Agent before terminal cleanup.
/// Every resource is bound to the same attempt and lease epoch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupBinding {
    pub attempt_id: String,
    pub scope_digest: String,
    pub lease_epoch: u64,
    pub sandbox_identity: SandboxIdentity,
    pub worktree_identity: WorktreeIdentity,
    pub admission_lease: AdmissionLeaseIdentity,
    pub lease: LeaseIdentity,
}

impl CleanupBinding {
    pub fn new(
        attempt_id: impl Into<String>,
        scope_digest: impl Into<String>,
        lease_epoch: u64,
        sandbox_identity: SandboxIdentity,
        worktree_identity: WorktreeIdentity,
        admission_lease: AdmissionLeaseIdentity,
        lease: LeaseIdentity,
    ) -> Result<Self, CleanupContractError> {
        let binding = Self {
            attempt_id: attempt_id.into(),
            scope_digest: scope_digest.into(),
            lease_epoch,
            sandbox_identity,
            worktree_identity,
            admission_lease,
            lease,
        };
        binding.validate()?;
        Ok(binding)
    }

    pub fn validate(&self) -> Result<(), CleanupContractError> {
        validate_id(&self.attempt_id, "attempt_id")?;
        validate_scope_digest(&self.scope_digest)?;
        if self.lease_epoch == 0 {
            return Err(CleanupContractError::StaleLeaseEpoch);
        }
        if self.sandbox_identity.attempt_id != self.attempt_id {
            return Err(CleanupContractError::IdentityMismatch("sandbox_attempt"));
        }
        if self.worktree_identity.attempt_id != self.attempt_id {
            return Err(CleanupContractError::IdentityMismatch("worktree_attempt"));
        }
        if self.admission_lease.attempt_id != self.attempt_id {
            return Err(CleanupContractError::IdentityMismatch("admission_attempt"));
        }
        if self.lease.attempt_id != self.attempt_id {
            return Err(CleanupContractError::IdentityMismatch("lease_attempt"));
        }
        if self.worktree_identity.lease_epoch != self.lease_epoch
            || self.lease.lease_epoch != self.lease_epoch
        {
            return Err(CleanupContractError::StaleLeaseEpoch);
        }
        if self.admission_lease.lease_id != self.lease.id {
            return Err(CleanupContractError::IdentityMismatch("admission_lease"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum CleanupStep {
    SandboxSeal,
    SandboxRevoke,
    AdmissionRelease,
    SandboxQuarantine,
    WorktreeCleanup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupEffect {
    Completed,
    AlreadyDone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupReport {
    pub attempt_id: String,
    pub already_clean: bool,
    pub completed_steps: Vec<CleanupStep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupPending {
    pub attempt_id: String,
    pub failed_step: CleanupStep,
    pub reason: String,
    pub completed_steps: Vec<CleanupStep>,
}

impl fmt::Display for CleanupPending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "terminal cleanup pending for {} at {:?}: {}",
            self.attempt_id, self.failed_step, self.reason
        )
    }
}

impl std::error::Error for CleanupPending {}

/// Adapter seam for concrete sandbox, admission, and worktree lifecycles.
/// Implementations must make each operation idempotent and reject stale or
/// mismatched identities.  The default residual scan is deliberately empty;
/// a production adapter may return only manifest-validated opaque bindings.
pub trait TerminalCleanupEffects {
    fn seal_sandbox(&mut self, identity: &SandboxIdentity) -> Result<CleanupEffect, String>;
    fn revoke_sandbox(&mut self, identity: &SandboxIdentity) -> Result<CleanupEffect, String>;
    fn release_admission(
        &mut self,
        identity: &AdmissionLeaseIdentity,
    ) -> Result<CleanupEffect, String>;
    fn quarantine_sandbox(&mut self, identity: &SandboxIdentity) -> Result<CleanupEffect, String>;
    fn cleanup_worktree(&mut self, identity: &WorktreeIdentity) -> Result<CleanupEffect, String>;

    fn residual_bindings(&self) -> Vec<CleanupBinding> {
        Vec::new()
    }
}

/// Adapter that composes the durable resource row with a provider-owned
/// manifest.  Cleanup never accepts a path: it reconstructs the manifest
/// identity from the trusted `ResourceBinding`, resolves it through the
/// provider, and only then invokes `release_verified`.
pub struct ManifestValidatedWorktreeCleanup<E, P> {
    inner: E,
    provider: P,
}

impl<E, P> ManifestValidatedWorktreeCleanup<E, P> {
    pub fn new(inner: E, provider: P) -> Self {
        Self { inner, provider }
    }

    pub fn inner(&self) -> &E {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut E {
        &mut self.inner
    }

    pub fn provider(&self) -> &P {
        &self.provider
    }
}

impl<E, P> TerminalCleanupEffects for ManifestValidatedWorktreeCleanup<E, P>
where
    E: TerminalCleanupEffects,
    P: WorktreeManifestProvider,
{
    fn seal_sandbox(&mut self, identity: &SandboxIdentity) -> Result<CleanupEffect, String> {
        self.inner.seal_sandbox(identity)
    }

    fn revoke_sandbox(&mut self, identity: &SandboxIdentity) -> Result<CleanupEffect, String> {
        self.inner.revoke_sandbox(identity)
    }

    fn release_admission(
        &mut self,
        identity: &AdmissionLeaseIdentity,
    ) -> Result<CleanupEffect, String> {
        self.inner.release_admission(identity)
    }

    fn quarantine_sandbox(&mut self, identity: &SandboxIdentity) -> Result<CleanupEffect, String> {
        self.inner.quarantine_sandbox(identity)
    }

    fn cleanup_worktree(&mut self, identity: &WorktreeIdentity) -> Result<CleanupEffect, String> {
        let manifest = WorktreeManifestIdentity {
            provider_kind: identity.provider_kind.clone(),
            handle_id: identity.id.clone(),
            work_package_id: identity.work_package_id.clone(),
            attempt_id: identity.attempt_id.clone(),
            scope_digest: identity.scope_digest.clone(),
            lease_epoch: identity.lease_epoch,
            manifest_locator: identity.manifest_locator.clone(),
            manifest_nonce: identity.manifest_nonce.clone(),
            manifest_digest: identity.manifest_digest.clone(),
        };
        let verified = self
            .provider
            .resolve_manifest(&manifest)
            .map_err(|error| format!("worktree manifest resolve refused: {error:?}"))?
            .ok_or_else(|| "worktree manifest unavailable; residual retained".to_owned())?;
        let receipt = self
            .provider
            .release_verified(&verified)
            .map_err(|error| format!("worktree manifest release refused: {error:?}"))?;
        if receipt.already_released {
            Ok(CleanupEffect::AlreadyDone)
        } else {
            Ok(CleanupEffect::Completed)
        }
    }

    fn residual_bindings(&self) -> Vec<CleanupBinding> {
        self.inner.residual_bindings()
    }
}

/// Small interface consumed by the service/composition root.  It is separate
/// from concrete filesystem adapters so the service never learns path or VCS
/// details.
pub trait TerminalCleanupPort {
    fn cleanup(&mut self, binding: &CleanupBinding) -> Result<CleanupReport, CleanupPending>;
    fn residual_bindings(&self) -> Vec<CleanupBinding>;

    /// Main-Agent service seam. Implementations backed by the durable resource
    /// ledger override this method; the legacy in-memory coordinator fails
    /// closed rather than treating an attempt id as filesystem authority.
    fn cleanup_attempt(&mut self, attempt_id: &str) -> Result<(), String> {
        Err(format!(
            "durable resource binding is required for attempt cleanup: {attempt_id}"
        ))
    }
}

/// Durable coordinator.  Progress is retained in-memory for the current
/// process and is also safe to replay after restart because every injected
/// effect is required to be idempotent.
pub struct DurableTerminalCleanup<E> {
    effects: E,
    progress: HashMap<String, BTreeSet<CleanupStep>>,
}

impl<E> DurableTerminalCleanup<E> {
    pub fn new(effects: E) -> Self {
        Self {
            effects,
            progress: HashMap::new(),
        }
    }

    pub fn effects(&self) -> &E {
        &self.effects
    }

    pub fn effects_mut(&mut self) -> &mut E {
        &mut self.effects
    }

    fn run_step(
        &mut self,
        binding: &CleanupBinding,
        step: CleanupStep,
    ) -> Result<(), CleanupPending>
    where
        E: TerminalCleanupEffects,
    {
        let done = self
            .progress
            .entry(binding.attempt_id.clone())
            .or_default()
            .contains(&step);
        if done {
            return Ok(());
        }
        let result = match step {
            CleanupStep::SandboxSeal => self.effects.seal_sandbox(&binding.sandbox_identity),
            CleanupStep::SandboxRevoke => self.effects.revoke_sandbox(&binding.sandbox_identity),
            CleanupStep::AdmissionRelease => {
                self.effects.release_admission(&binding.admission_lease)
            }
            CleanupStep::SandboxQuarantine => {
                self.effects.quarantine_sandbox(&binding.sandbox_identity)
            }
            CleanupStep::WorktreeCleanup => {
                self.effects.cleanup_worktree(&binding.worktree_identity)
            }
        };
        match result {
            Ok(CleanupEffect::Completed | CleanupEffect::AlreadyDone) => {
                self.progress
                    .entry(binding.attempt_id.clone())
                    .or_default()
                    .insert(step);
                Ok(())
            }
            Err(reason) => Err(CleanupPending {
                attempt_id: binding.attempt_id.clone(),
                failed_step: step,
                reason,
                completed_steps: self.completed_steps(&binding.attempt_id),
            }),
        }
    }

    fn completed_steps(&self, attempt_id: &str) -> Vec<CleanupStep> {
        self.progress
            .get(attempt_id)
            .map(|steps| steps.iter().copied().collect())
            .unwrap_or_default()
    }
}

impl<E: TerminalCleanupEffects> TerminalCleanupPort for DurableTerminalCleanup<E> {
    fn cleanup(&mut self, binding: &CleanupBinding) -> Result<CleanupReport, CleanupPending> {
        if let Err(error) = binding.validate() {
            return Err(CleanupPending {
                attempt_id: binding.attempt_id.clone(),
                failed_step: CleanupStep::SandboxSeal,
                reason: error.to_string(),
                completed_steps: Vec::new(),
            });
        }
        let was_complete = self
            .progress
            .get(&binding.attempt_id)
            .is_some_and(|steps| steps.len() == 5);
        for step in [
            CleanupStep::SandboxSeal,
            CleanupStep::SandboxRevoke,
            CleanupStep::AdmissionRelease,
            CleanupStep::SandboxQuarantine,
            CleanupStep::WorktreeCleanup,
        ] {
            self.run_step(binding, step)?;
        }
        Ok(CleanupReport {
            attempt_id: binding.attempt_id.clone(),
            already_clean: was_complete,
            completed_steps: self.completed_steps(&binding.attempt_id),
        })
    }

    fn residual_bindings(&self) -> Vec<CleanupBinding> {
        self.effects.residual_bindings()
    }
}

/// Resource-ledger-backed terminal cleanup used by the Main-Agent service.
/// Provider effects receive only opaque identities loaded from durable rows;
/// the legacy `delegation_attempts.sandbox_ref` is never read or resolved.
pub struct PersistentResourceCleanup<E> {
    repository: ResourceBindingRepository,
    effects: E,
}

impl<E> PersistentResourceCleanup<E> {
    pub fn new(repository: ResourceBindingRepository, effects: E) -> Self {
        Self {
            repository,
            effects,
        }
    }

    pub fn repository(&self) -> &ResourceBindingRepository {
        &self.repository
    }

    pub fn effects(&self) -> &E {
        &self.effects
    }

    pub fn effects_mut(&mut self) -> &mut E {
        &mut self.effects
    }
}

impl<E: TerminalCleanupEffects> PersistentResourceCleanup<E> {
    fn fail_pending(
        &self,
        attempt_id: &str,
        step: CleanupStep,
        reason: impl Into<String>,
    ) -> String {
        format!("cleanup {attempt_id} at {step:?}: {}", reason.into())
    }

    fn transition(
        &self,
        binding: &ResourceBinding,
        next: ResourceCleanupStatus,
        lifecycle: Option<super::resource_binding::LifecycleState>,
        step: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), String> {
        let changed = self
            .repository
            .transition_cleanup(
                &binding.identity.id,
                binding.cleanup_status,
                next,
                lifecycle,
                step,
                None,
                binding.quarantine_until,
                error,
            )
            .map_err(|error| error.to_string())?;
        if changed {
            Ok(())
        } else {
            Err("resource cleanup CAS token is stale".into())
        }
    }

    fn cleanup_sandbox(&mut self, mut binding: ResourceBinding) -> Result<(), String> {
        use super::resource_binding::LifecycleState;
        let attempt_id = binding.identity.attempt_id.clone();
        let sandbox = SandboxIdentity::new(
            binding.identity.resource_ref.clone(),
            attempt_id.clone(),
            binding.identity.manifest_nonce.clone(),
        )
        .map_err(|error| error.to_string())?;
        if binding.cleanup_status == ResourceCleanupStatus::Cleaned {
            return Ok(());
        }
        if binding.cleanup_status == ResourceCleanupStatus::None {
            match self.effects.seal_sandbox(&sandbox) {
                Ok(CleanupEffect::Completed | CleanupEffect::AlreadyDone) => {}
                Err(reason) => {
                    let _ = self.transition(
                        &binding,
                        ResourceCleanupStatus::Retry,
                        Some(LifecycleState::Unknown),
                        Some("seal"),
                        Some("seal_failed"),
                    );
                    return Err(self.fail_pending(&attempt_id, CleanupStep::SandboxSeal, reason));
                }
            }
            self.transition(
                &binding,
                ResourceCleanupStatus::Pending,
                Some(LifecycleState::Sealed),
                Some("seal"),
                None,
            )?;
            binding.cleanup_status = ResourceCleanupStatus::Pending;
        }

        match self.effects.revoke_sandbox(&sandbox) {
            Ok(CleanupEffect::Completed | CleanupEffect::AlreadyDone) => {}
            Err(reason) => {
                let _ = self.transition(
                    &binding,
                    ResourceCleanupStatus::Retry,
                    Some(LifecycleState::Sealed),
                    Some("revoke"),
                    Some("revoke_failed"),
                );
                return Err(self.fail_pending(&attempt_id, CleanupStep::SandboxRevoke, reason));
            }
        }
        self.transition(
            &binding,
            ResourceCleanupStatus::Pending,
            Some(LifecycleState::Revoked),
            Some("revoke"),
            None,
        )?;
        binding.cleanup_status = ResourceCleanupStatus::Pending;

        if let Some(admission) = binding.identity.admission.as_ref() {
            let admission = AdmissionLeaseIdentity::new(
                admission.id.clone(),
                attempt_id.clone(),
                binding.identity.lease_id.clone(),
                admission.state_version,
            )
            .map_err(|error| error.to_string())?;
            match self.effects.release_admission(&admission) {
                Ok(CleanupEffect::Completed | CleanupEffect::AlreadyDone) => {}
                Err(reason) => {
                    let _ = self.transition(
                        &binding,
                        ResourceCleanupStatus::Retry,
                        Some(LifecycleState::Revoked),
                        Some("admission_release"),
                        Some("admission_release_failed"),
                    );
                    return Err(self.fail_pending(
                        &attempt_id,
                        CleanupStep::AdmissionRelease,
                        reason,
                    ));
                }
            }
        }

        match self.effects.quarantine_sandbox(&sandbox) {
            Ok(CleanupEffect::Completed | CleanupEffect::AlreadyDone) => {}
            Err(reason) => {
                let _ = self.transition(
                    &binding,
                    ResourceCleanupStatus::Retry,
                    Some(LifecycleState::Revoked),
                    Some("quarantine"),
                    Some("quarantine_failed"),
                );
                return Err(self.fail_pending(&attempt_id, CleanupStep::SandboxQuarantine, reason));
            }
        }
        self.transition(
            &binding,
            ResourceCleanupStatus::Cleaned,
            Some(LifecycleState::Quarantined),
            Some("quarantine"),
            None,
        )?;
        Ok(())
    }

    fn cleanup_worktree(&mut self, binding: ResourceBinding) -> Result<(), String> {
        if binding.cleanup_status == ResourceCleanupStatus::Cleaned {
            return Ok(());
        }
        let attempt_id = binding.identity.attempt_id.clone();
        let worktree =
            WorktreeIdentity::from_binding(&binding.identity).map_err(|error| error.to_string())?;
        match self.effects.cleanup_worktree(&worktree) {
            Ok(CleanupEffect::Completed | CleanupEffect::AlreadyDone) => {}
            Err(reason) => {
                let _ = self.transition(
                    &binding,
                    ResourceCleanupStatus::Retry,
                    None,
                    Some("worktree"),
                    Some("worktree_failed"),
                );
                return Err(self.fail_pending(&attempt_id, CleanupStep::WorktreeCleanup, reason));
            }
        }
        self.transition(
            &binding,
            ResourceCleanupStatus::Cleaned,
            None,
            Some("worktree"),
            None,
        )
    }
}

impl<E: TerminalCleanupEffects> TerminalCleanupPort for PersistentResourceCleanup<E> {
    fn cleanup(&mut self, binding: &CleanupBinding) -> Result<CleanupReport, CleanupPending> {
        self.cleanup_attempt(&binding.attempt_id)
            .map(|_| CleanupReport {
                attempt_id: binding.attempt_id.clone(),
                already_clean: false,
                completed_steps: Vec::new(),
            })
            .map_err(|reason| CleanupPending {
                attempt_id: binding.attempt_id.clone(),
                failed_step: CleanupStep::SandboxSeal,
                reason,
                completed_steps: Vec::new(),
            })
    }

    fn residual_bindings(&self) -> Vec<CleanupBinding> {
        // Reconstructing the historical rich binding requires both resource
        // rows and an admission CAS token. Unknown/incomplete rows remain
        // retained; never synthesize identities from paths or legacy refs.
        Vec::new()
    }

    fn cleanup_attempt(&mut self, attempt_id: &str) -> Result<(), String> {
        let sandbox = match self
            .repository
            .load_for_attempt(attempt_id, ResourceKind::Sandbox)
        {
            Ok(binding) => Some(binding),
            Err(ResourceBindingError::NotFound) => None,
            Err(error) => return Err(error.to_string()),
        };
        let worktree = match self
            .repository
            .load_for_attempt(attempt_id, ResourceKind::Worktree)
        {
            Ok(binding) => Some(binding),
            Err(ResourceBindingError::NotFound) => None,
            Err(error) => return Err(error.to_string()),
        };
        if sandbox.is_none() && worktree.is_none() {
            return Err(
                "no durable resource binding; legacy sandbox_ref is not authoritative".into(),
            );
        }
        if let Some(binding) = sandbox {
            self.cleanup_sandbox(binding)?;
        }
        if let Some(binding) = worktree {
            self.cleanup_worktree(binding)?;
        }
        Ok(())
    }
}

/// Fail-closed service default. It is intentionally not a no-op cleanup path.
pub struct FailClosedTerminalCleanup;
impl TerminalCleanupPort for FailClosedTerminalCleanup {
    fn cleanup(&mut self, binding: &CleanupBinding) -> Result<CleanupReport, CleanupPending> {
        Err(CleanupPending {
            attempt_id: binding.attempt_id.clone(),
            failed_step: CleanupStep::SandboxSeal,
            reason: "terminal cleanup coordinator is not configured".into(),
            completed_steps: Vec::new(),
        })
    }

    fn residual_bindings(&self) -> Vec<CleanupBinding> {
        Vec::new()
    }
}

/// Fail-closed provider effects used by startup when the concrete sandbox and
/// worktree providers are unavailable.  The type is distinct from
/// `FailClosedTerminalCleanup` (the service-level port) so composition cannot
/// accidentally treat a missing provider as a successful cleanup.
#[derive(Debug, Clone, Copy, Default)]
pub struct FailClosedTerminalCleanupEffects;

impl TerminalCleanupEffects for FailClosedTerminalCleanupEffects {
    fn seal_sandbox(&mut self, _: &SandboxIdentity) -> Result<CleanupEffect, String> {
        Err("sandbox cleanup provider is not configured".into())
    }

    fn revoke_sandbox(&mut self, _: &SandboxIdentity) -> Result<CleanupEffect, String> {
        Err("sandbox cleanup provider is not configured".into())
    }

    fn release_admission(&mut self, _: &AdmissionLeaseIdentity) -> Result<CleanupEffect, String> {
        Err("workspace admission cleanup provider is not configured".into())
    }

    fn quarantine_sandbox(&mut self, _: &SandboxIdentity) -> Result<CleanupEffect, String> {
        Err("sandbox cleanup provider is not configured".into())
    }

    fn cleanup_worktree(&mut self, _: &WorktreeIdentity) -> Result<CleanupEffect, String> {
        Err("worktree cleanup provider is not configured".into())
    }
}

/// A deterministic fake that models durable idempotency and restart scans.
/// It intentionally stores only opaque identities and operation state.
#[derive(Debug, Clone, Default)]
pub struct FakeTerminalCleanupEffects {
    state: Arc<Mutex<FakeCleanupState>>,
}

#[derive(Debug, Default)]
struct FakeCleanupState {
    done: BTreeSet<(String, CleanupStep)>,
    events: Vec<(String, CleanupStep)>,
    fail_next: Option<CleanupStep>,
    residuals: HashMap<String, CleanupBinding>,
}

impl FakeTerminalCleanupEffects {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fail_next(&self, step: CleanupStep) {
        self.state.lock().unwrap().fail_next = Some(step);
    }

    pub fn events(&self) -> Vec<(String, CleanupStep)> {
        self.state.lock().unwrap().events.clone()
    }

    pub fn add_residual(&self, binding: CleanupBinding) {
        self.state
            .lock()
            .unwrap()
            .residuals
            .insert(binding.attempt_id.clone(), binding);
    }
}

impl FakeTerminalCleanupEffects {
    fn apply(&mut self, attempt_id: &str, step: CleanupStep) -> Result<CleanupEffect, String> {
        let mut state = self.state.lock().unwrap();
        let key = (attempt_id.to_owned(), step);
        if state.done.contains(&key) {
            return Ok(CleanupEffect::AlreadyDone);
        }
        state.events.push(key.clone());
        if state.fail_next == Some(step) {
            state.fail_next = None;
            return Err(format!("fake failure at {step:?}"));
        }
        state.done.insert(key);
        if step == CleanupStep::WorktreeCleanup {
            state.residuals.remove(attempt_id);
        }
        Ok(CleanupEffect::Completed)
    }
}

impl TerminalCleanupEffects for FakeTerminalCleanupEffects {
    fn seal_sandbox(&mut self, identity: &SandboxIdentity) -> Result<CleanupEffect, String> {
        self.apply(&identity.attempt_id, CleanupStep::SandboxSeal)
    }

    fn revoke_sandbox(&mut self, identity: &SandboxIdentity) -> Result<CleanupEffect, String> {
        self.apply(&identity.attempt_id, CleanupStep::SandboxRevoke)
    }

    fn release_admission(
        &mut self,
        identity: &AdmissionLeaseIdentity,
    ) -> Result<CleanupEffect, String> {
        self.apply(&identity.attempt_id, CleanupStep::AdmissionRelease)
    }

    fn quarantine_sandbox(&mut self, identity: &SandboxIdentity) -> Result<CleanupEffect, String> {
        self.apply(&identity.attempt_id, CleanupStep::SandboxQuarantine)
    }

    fn cleanup_worktree(&mut self, identity: &WorktreeIdentity) -> Result<CleanupEffect, String> {
        self.apply(&identity.attempt_id, CleanupStep::WorktreeCleanup)
    }

    fn residual_bindings(&self) -> Vec<CleanupBinding> {
        self.state
            .lock()
            .unwrap()
            .residuals
            .values()
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::resource_binding::{ResourceBindingRepository, ResourceKind};
    use crate::agent::shared_db::SharedDb;
    use rusqlite::Connection;

    fn binding() -> CleanupBinding {
        CleanupBinding::new(
            "attempt_1",
            "scope_digest_1",
            7,
            SandboxIdentity::new("sandbox_1", "attempt_1", "nonce_1").unwrap(),
            WorktreeIdentity::new("worktree_1", "attempt_1", 7).unwrap(),
            AdmissionLeaseIdentity::new("admission_1", "attempt_1", "lease_1", 3).unwrap(),
            LeaseIdentity::new("lease_1", "attempt_1", 7).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn all_resources_cleanup_in_strict_order() {
        let fake = FakeTerminalCleanupEffects::new();
        let mut cleanup = DurableTerminalCleanup::new(fake.clone());
        let report = cleanup.cleanup(&binding()).unwrap();
        assert!(!report.already_clean);
        assert_eq!(
            fake.events(),
            vec![
                ("attempt_1".into(), CleanupStep::SandboxSeal),
                ("attempt_1".into(), CleanupStep::SandboxRevoke),
                ("attempt_1".into(), CleanupStep::AdmissionRelease),
                ("attempt_1".into(), CleanupStep::SandboxQuarantine),
                ("attempt_1".into(), CleanupStep::WorktreeCleanup),
            ]
        );
    }

    #[test]
    fn one_failure_returns_pending_and_retry_completes_without_replaying_successes() {
        let fake = FakeTerminalCleanupEffects::new();
        fake.fail_next(CleanupStep::SandboxQuarantine);
        let mut cleanup = DurableTerminalCleanup::new(fake.clone());
        let pending = cleanup.cleanup(&binding()).unwrap_err();
        assert_eq!(pending.failed_step, CleanupStep::SandboxQuarantine);
        assert_eq!(pending.completed_steps.len(), 3);
        cleanup.cleanup(&binding()).unwrap();
        assert_eq!(
            fake.events()
                .iter()
                .filter(|(_, step)| *step == CleanupStep::SandboxSeal)
                .count(),
            1
        );
    }

    #[test]
    fn duplicate_cleanup_is_already_clean_and_effects_are_idempotent() {
        let fake = FakeTerminalCleanupEffects::new();
        let mut cleanup = DurableTerminalCleanup::new(fake.clone());
        cleanup.cleanup(&binding()).unwrap();
        let report = cleanup.cleanup(&binding()).unwrap();
        assert!(report.already_clean);
        assert_eq!(fake.events().len(), 5);
    }

    #[test]
    fn stale_epoch_and_path_like_identity_fail_closed() {
        let stale = CleanupBinding::new(
            "attempt_1",
            "scope_digest_1",
            7,
            SandboxIdentity::new("sandbox_1", "attempt_1", "nonce_1").unwrap(),
            WorktreeIdentity::new("worktree_1", "attempt_1", 8).unwrap(),
            AdmissionLeaseIdentity::new("admission_1", "attempt_1", "lease_1", 3).unwrap(),
            LeaseIdentity::new("lease_1", "attempt_1", 7).unwrap(),
        );
        assert_eq!(stale, Err(CleanupContractError::StaleLeaseEpoch));
        assert!(matches!(
            WorktreeIdentity::new("C:\\secret", "attempt_1", 7),
            Err(CleanupContractError::InvalidIdentity("worktree_id"))
        ));
        assert!(matches!(
            SandboxIdentity::new("sandbox", "attempt_1", "../escape"),
            Err(CleanupContractError::InvalidIdentity("sandbox_nonce"))
        ));
    }

    #[test]
    fn residual_scan_survives_coordinator_restart() {
        let fake = FakeTerminalCleanupEffects::new();
        let current = binding();
        fake.add_residual(current.clone());
        fake.fail_next(CleanupStep::WorktreeCleanup);
        let mut first = DurableTerminalCleanup::new(fake.clone());
        assert!(first.cleanup(&current).is_err());
        let restarted = DurableTerminalCleanup::new(fake.clone());
        assert_eq!(restarted.residual_bindings(), vec![current.clone()]);
        let mut restarted = restarted;
        restarted.cleanup(&current).unwrap();
        assert!(restarted.residual_bindings().is_empty());
    }

    #[test]
    fn persistent_cleanup_uses_opaque_binding_and_retries_with_cas() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO sessions(id,title,created_at,updated_at) VALUES ('s','s',1,1)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('m','s','user','m',1)", []).unwrap();
        conn.execute("INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES ('r','s','m','g','running','[]',1,1)", []).unwrap();
        conn.execute("INSERT INTO work_packages(id,session_id,owner_profile_id,workspace_key,task_shape,worker_profile,worker_policy_version,scope_digest,capability_scope_ref,capability_expires_at,candidate_version,created_at,updated_at,status) VALUES ('wp','s',1,'wk','explore','explorer',1,'scope','approved',1000,0,1,1,'active')", []).unwrap();
        conn.execute("INSERT INTO delegations(id,session_id,message_id,parent_run_id,work_package_id,objective,brief_json,status,state_version,created_at,updated_at) VALUES ('d','s','m','r','wp','g','{}','running',0,1,1)", []).unwrap();
        conn.execute("INSERT INTO delegation_attempts(id,delegation_id,attempt_number,status,sandbox_ref,created_at) VALUES ('a','d',1,'sealed','legacy-path',1)", []).unwrap();
        conn.execute("INSERT INTO delegation_capability_leases(id,attempt_id,read_roots_json,write_roots_json,tool_allowlist_json,network_hosts_json,budget_json,status,issued_at,expires_at,revoked_at) VALUES ('l','a','[]','[]','[]','[]','{}','revoked',1,1000,10)", []).unwrap();
        conn.execute("INSERT INTO delegation_resource_bindings(id,delegation_id,work_package_id,attempt_id,lease_id,resource_kind,provider_kind,resource_ref,manifest_locator,manifest_version,manifest_nonce,manifest_digest,scope_digest,lease_epoch,lifecycle_state,cleanup_status,created_at,updated_at) VALUES ('rb','d','wp','a','l','sandbox','sandbox-v1','sandbox-opaque','manifest-opaque',1,'nonce','digest','scope',1,'sealed','none',1,1)", []).unwrap();
        let db = SharedDb::new(conn);
        let repository = ResourceBindingRepository::with_clock(db.clone(), || 10);
        let fake = FakeTerminalCleanupEffects::new();
        fake.fail_next(CleanupStep::SandboxQuarantine);
        let mut cleanup = PersistentResourceCleanup::new(repository.clone(), fake.clone());
        assert!(cleanup.cleanup_attempt("a").is_err());
        let retry = repository
            .load_for_attempt("a", ResourceKind::Sandbox)
            .unwrap();
        assert_eq!(retry.cleanup_status, ResourceCleanupStatus::Retry);
        cleanup.cleanup_attempt("a").unwrap();
        let cleaned = repository.load("rb").unwrap();
        assert_eq!(cleaned.cleanup_status, ResourceCleanupStatus::Cleaned);
        assert_eq!(fake.events().len(), 4);
        assert!(!fake
            .events()
            .iter()
            .any(|(_, step)| *step == CleanupStep::WorktreeCleanup));
    }
}

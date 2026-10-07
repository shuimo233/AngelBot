//! Concrete, fail-closed adapters for the delegated sandbox lifecycle.
//!
//! The scheduler/runtime only knows an attempt id.  This module is the deep
//! seam that turns that id into an opaque, durable resource binding and then
//! asks the provider-owned manifest to verify the resource.  No method accepts
//! a path, a legacy `sandbox_ref`, or a child-supplied provider handle.

use std::{marker::PhantomData, path::Path, sync::Arc};

use super::{
    delegated_sandbox::{
        DelegatedSandboxLifecycle, SandboxError, SandboxManifestIdentity, SandboxState,
    },
    delegation_runtime::SandboxController,
    resource_binding::{
        ResourceBinding, ResourceBindingError, ResourceBindingRepository, ResourceKind,
    },
    shared_db::SharedDb,
    terminal_cleanup::{
        AdmissionLeaseIdentity, CleanupEffect, ManifestValidatedWorktreeCleanup, SandboxIdentity,
        TerminalCleanupEffects, WorktreeIdentity,
    },
    workspace_admission::WorkspaceAdmission,
    workspace_isolation::{
        GitWorktreeProvider, WorktreeManifestIdentity, WorktreeManifestProvider,
    },
};

const SANDBOX_PROVIDER_KIND: &str = "delegated_sandbox_v2";

pub type ProductionResourceCleanupEffects = ResourceCleanupEffects<
    WorktreeCleanupEffects<Arc<GitWorktreeProvider>>,
    Arc<GitWorktreeProvider>,
>;

/// Construct the provider-owned filesystem/VCS adapters below one trusted
/// application-data directory.  The caller supplies no model or child path;
/// roots are derived solely from the OS app-data location.
pub fn production_components(
    db: SharedDb,
    app_data_dir: &Path,
) -> Result<(DelegatedSandboxController, ProductionResourceCleanupEffects), String> {
    let (controller, effects, _) = production_components_with_provider(db, app_data_dir)?;
    Ok((controller, effects))
}

/// Variant used by the worker composition root so the same provider instance
/// serves both file operations and terminal cleanup. The returned Arc is the
/// only path-aware object; it never crosses the WorkerHost boundary.
pub fn production_components_with_provider(
    db: SharedDb,
    app_data_dir: &Path,
) -> Result<
    (
        DelegatedSandboxController,
        ProductionResourceCleanupEffects,
        Arc<GitWorktreeProvider>,
    ),
    String,
> {
    let sandbox_root = app_data_dir.join("delegated").join("sandboxes");
    let worktree_root = app_data_dir.join("delegated").join("worktrees");
    let manifest_root = worktree_root.join("manifests");
    let lifecycle = DelegatedSandboxLifecycle::new(&sandbox_root)
        .map_err(|error| format!("sandbox provider unavailable: {error:?}"))?;
    let provider = Arc::new(
        GitWorktreeProvider::new(&worktree_root, &manifest_root)
            .map_err(|error| format!("worktree provider unavailable: {error:?}"))?,
    );
    let repository = ResourceBindingRepository::new(db.clone());
    let controller = DelegatedSandboxController::new(lifecycle.clone(), repository.clone());
    let inner = WorktreeCleanupEffects::new(provider.clone());
    let validated = ManifestValidatedWorktreeCleanup::new(inner, provider.clone());
    let effects = ResourceCleanupEffects::new(
        db,
        repository,
        lifecycle,
        WorkspaceAdmission::new(),
        validated,
    );
    Ok((controller, effects, provider))
}

/// Runtime adapter used after durable lease revocation.  It resolves the
/// sandbox exclusively through the resource binding ledger and the provider's
/// manifest; a missing/legacy row is an error rather than an opportunity to
/// guess from `delegation_attempts.sandbox_ref`.
pub struct DelegatedSandboxController {
    lifecycle: DelegatedSandboxLifecycle,
    repository: ResourceBindingRepository,
}

impl DelegatedSandboxController {
    pub fn new(
        lifecycle: DelegatedSandboxLifecycle,
        repository: ResourceBindingRepository,
    ) -> Self {
        Self {
            lifecycle,
            repository,
        }
    }

    pub fn lifecycle(&self) -> &DelegatedSandboxLifecycle {
        &self.lifecycle
    }

    pub fn repository(&self) -> &ResourceBindingRepository {
        &self.repository
    }

    fn resolve(
        &self,
        attempt_id: &str,
    ) -> Result<super::delegated_sandbox::VerifiedSandboxRef, String> {
        let binding = self
            .repository
            .load_for_attempt(attempt_id, ResourceKind::Sandbox)
            .map_err(binding_error)?;
        let identity = sandbox_manifest(&binding)?;
        self.lifecycle
            .resolve_manifest(&identity)
            .map_err(sandbox_error)?
            .ok_or_else(|| "sandbox manifest unavailable; residual retained".to_owned())
    }

    fn seal_and_revoke_resolved(
        &self,
        verified: &super::delegated_sandbox::VerifiedSandboxRef,
    ) -> Result<(), String> {
        let mut current = verified.clone();
        if matches!(current.state(), SandboxState::Active | SandboxState::Sealed) {
            if current.state() == SandboxState::Active {
                current = self
                    .lifecycle
                    .seal_verified(&current)
                    .map_err(sandbox_error)?;
            }
            if matches!(current.state(), SandboxState::Sealed) {
                self.lifecycle
                    .revoke_verified(&current)
                    .map_err(sandbox_error)?;
            }
        }
        // Revoked and quarantined are terminal for this adapter.  The later
        // cleanup coordinator owns the quarantine/delete steps.
        Ok(())
    }
}

impl SandboxController for DelegatedSandboxController {
    fn seal_and_revoke(&mut self, attempt_id: &str) -> Result<(), String> {
        if attempt_id.trim().is_empty() {
            return Err("sandbox attempt identity is empty".into());
        }
        let verified = self.resolve(attempt_id)?;
        self.seal_and_revoke_resolved(&verified)
    }
}

/// Concrete cleanup effects for the durable resource ledger.  Worktree
/// cleanup remains an injected `ManifestValidatedWorktreeCleanup` adapter so
/// this module does not know VCS paths or branch names.  Sandbox and admission
/// effects resolve only opaque resource identities and are idempotent.
pub struct ResourceCleanupEffects<W, P> {
    db: SharedDb,
    repository: ResourceBindingRepository,
    lifecycle: DelegatedSandboxLifecycle,
    admission: WorkspaceAdmission,
    worktree: ManifestValidatedWorktreeCleanup<W, P>,
    _marker: PhantomData<fn() -> (W, P)>,
}

/// Provider-backed inner effect used by `ManifestValidatedWorktreeCleanup`.
/// The outer validator proves the manifest before calling this adapter; the
/// second resolution here is intentional and keeps the provider release
/// operation idempotent if a cleanup retry races a restart scan.
pub struct WorktreeCleanupEffects<P> {
    provider: P,
}

impl<P> WorktreeCleanupEffects<P> {
    pub fn new(provider: P) -> Self {
        Self { provider }
    }
}

impl<P> TerminalCleanupEffects for WorktreeCleanupEffects<P>
where
    P: WorktreeManifestProvider + Send + Sync,
{
    fn seal_sandbox(&mut self, _: &SandboxIdentity) -> Result<CleanupEffect, String> {
        Err("worktree provider cannot seal sandbox".into())
    }

    fn revoke_sandbox(&mut self, _: &SandboxIdentity) -> Result<CleanupEffect, String> {
        Err("worktree provider cannot revoke sandbox".into())
    }

    fn release_admission(&mut self, _: &AdmissionLeaseIdentity) -> Result<CleanupEffect, String> {
        Err("worktree provider cannot release admission".into())
    }

    fn quarantine_sandbox(&mut self, _: &SandboxIdentity) -> Result<CleanupEffect, String> {
        Err("worktree provider cannot quarantine sandbox".into())
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
        manifest
            .validate()
            .map_err(|error| format!("worktree manifest identity refused: {error:?}"))?;
        let verified = self
            .provider
            .resolve_manifest(&manifest)
            .map_err(|error| format!("worktree manifest resolve refused: {error:?}"))?
            .ok_or_else(|| "worktree manifest unavailable; residual retained".to_owned())?;
        let receipt = self
            .provider
            .release_verified(&verified)
            .map_err(|error| format!("worktree release refused: {error:?}"))?;
        Ok(if receipt.already_released {
            CleanupEffect::AlreadyDone
        } else {
            CleanupEffect::Completed
        })
    }
}

impl<W, P> ResourceCleanupEffects<W, P> {
    pub fn new(
        db: SharedDb,
        repository: ResourceBindingRepository,
        lifecycle: DelegatedSandboxLifecycle,
        admission: WorkspaceAdmission,
        worktree: ManifestValidatedWorktreeCleanup<W, P>,
    ) -> Self {
        Self {
            db,
            repository,
            lifecycle,
            admission,
            worktree,
            _marker: PhantomData,
        }
    }

    pub fn repository(&self) -> &ResourceBindingRepository {
        &self.repository
    }

    pub fn worktree(&self) -> &ManifestValidatedWorktreeCleanup<W, P> {
        &self.worktree
    }

    /// Return only known, durable rows that need reconciliation.  Provider
    /// residuals without a matching binding are intentionally absent and are
    /// retained for operator inspection rather than guessed or deleted.
    pub fn scan_restart(&self, now: i64) -> Result<Vec<ResourceBinding>, String> {
        self.repository.scan_restart(now).map_err(binding_error)
    }

    fn sandbox_binding(&self, identity: &SandboxIdentity) -> Result<ResourceBinding, String> {
        let binding = self
            .repository
            .load_for_attempt(&identity.attempt_id, ResourceKind::Sandbox)
            .map_err(binding_error)?;
        if binding.identity.resource_ref != identity.id
            || binding.identity.manifest_nonce != identity.nonce
            || binding.identity.provider_kind != SANDBOX_PROVIDER_KIND
        {
            return Err("sandbox binding identity mismatch".into());
        }
        Ok(binding)
    }

    fn resolve_sandbox(
        &self,
        identity: &SandboxIdentity,
    ) -> Result<
        (
            ResourceBinding,
            super::delegated_sandbox::VerifiedSandboxRef,
        ),
        String,
    > {
        let binding = self.sandbox_binding(identity)?;
        let manifest = sandbox_manifest(&binding)?;
        let verified = self
            .lifecycle
            .resolve_manifest(&manifest)
            .map_err(sandbox_error)?
            .ok_or_else(|| "sandbox manifest unavailable; residual retained".to_owned())?;
        Ok((binding, verified))
    }

    fn release_admission_internal(
        &self,
        identity: &AdmissionLeaseIdentity,
    ) -> Result<CleanupEffect, String> {
        let changed = self
            .db
            .with_conn_mut(|conn| {
                self.admission
                    .release_id(
                        conn,
                        &identity.id,
                        identity.state_version,
                        chrono::Utc::now().timestamp(),
                    )
                    .map_err(|error| error.to_string())
            })
            .map_err(|error| error.to_string())??;
        Ok(if changed {
            CleanupEffect::Completed
        } else {
            CleanupEffect::AlreadyDone
        })
    }
}

impl<W, P> TerminalCleanupEffects for ResourceCleanupEffects<W, P>
where
    W: TerminalCleanupEffects,
    P: WorktreeManifestProvider,
{
    fn seal_sandbox(&mut self, identity: &SandboxIdentity) -> Result<CleanupEffect, String> {
        let (_, verified) = self.resolve_sandbox(identity)?;
        if verified.state() == SandboxState::Active {
            self.lifecycle
                .seal_verified(&verified)
                .map_err(sandbox_error)?;
            Ok(CleanupEffect::Completed)
        } else {
            Ok(CleanupEffect::AlreadyDone)
        }
    }

    fn revoke_sandbox(&mut self, identity: &SandboxIdentity) -> Result<CleanupEffect, String> {
        let (_, verified) = self.resolve_sandbox(identity)?;
        let current = if verified.state() == SandboxState::Active {
            self.lifecycle
                .seal_verified(&verified)
                .map_err(sandbox_error)?
        } else {
            verified
        };
        if current.state() == SandboxState::Sealed {
            self.lifecycle
                .revoke_verified(&current)
                .map_err(sandbox_error)?;
            Ok(CleanupEffect::Completed)
        } else {
            Ok(CleanupEffect::AlreadyDone)
        }
    }

    fn release_admission(
        &mut self,
        identity: &AdmissionLeaseIdentity,
    ) -> Result<CleanupEffect, String> {
        self.release_admission_internal(identity)
    }

    fn quarantine_sandbox(&mut self, identity: &SandboxIdentity) -> Result<CleanupEffect, String> {
        let (_, verified) = self.resolve_sandbox(identity)?;
        if verified.state() == SandboxState::Revoked {
            self.lifecycle
                .quarantine_verified(&verified)
                .map_err(sandbox_error)?;
            Ok(CleanupEffect::Completed)
        } else if verified.state() == SandboxState::Quarantined {
            Ok(CleanupEffect::AlreadyDone)
        } else {
            Err("sandbox must be revoked before quarantine".into())
        }
    }

    fn cleanup_worktree(&mut self, identity: &WorktreeIdentity) -> Result<CleanupEffect, String> {
        self.worktree.cleanup_worktree(identity)
    }

    fn residual_bindings(&self) -> Vec<super::terminal_cleanup::CleanupBinding> {
        // Rich CleanupBinding reconstruction requires the historical lease
        // and admission CAS tuple; unknown/incomplete rows remain retained.
        Vec::new()
    }
}

fn sandbox_manifest(binding: &ResourceBinding) -> Result<SandboxManifestIdentity, String> {
    if binding.identity.resource_kind != ResourceKind::Sandbox
        || binding.identity.provider_kind != SANDBOX_PROVIDER_KIND
    {
        return Err("sandbox resource provider is not the concrete v2 provider".into());
    }
    SandboxManifestIdentity::new(
        binding.identity.delegation_id.clone(),
        binding.identity.attempt_id.clone(),
        binding.identity.scope_digest.clone(),
        binding.identity.lease_epoch as u64,
        binding.identity.manifest_locator.clone(),
        binding.identity.manifest_nonce.clone(),
        binding.identity.manifest_digest.clone(),
    )
    .map_err(sandbox_error)
}

fn binding_error(error: ResourceBindingError) -> String {
    match error {
        ResourceBindingError::NotFound => {
            "durable sandbox resource binding is required; legacy sandbox_ref is not authoritative"
                .into()
        }
        other => other.to_string(),
    }
}

fn sandbox_error(error: SandboxError) -> String {
    format!("sandbox manifest operation refused: {error:?}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::tempdir;

    fn db() -> SharedDb {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        SharedDb::new(conn)
    }

    #[test]
    fn legacy_or_tampered_binding_fails_closed() {
        let db = db();
        let root = tempdir().unwrap();
        let lifecycle = DelegatedSandboxLifecycle::new(root.path().join("sandbox")).unwrap();
        let controller =
            DelegatedSandboxController::new(lifecycle, ResourceBindingRepository::new(db));
        let mut controller = controller;
        assert!(controller.seal_and_revoke("a").is_err());
    }
}

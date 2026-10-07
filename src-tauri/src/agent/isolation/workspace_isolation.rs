//! Worktree and execution-isolation seams for delegated change work.
//!
//! This module intentionally stops before scheduler wiring or workspace
//! materialisation.  The deep interfaces here keep those decisions in one
//! place: a `WorkspaceProvider` owns VCS/worktree facts, an
//! `IsolationAdapter` owns execution roots and capability leases, and a
//! `ReviewArtifact` is the immutable hand-off between an implementer and a
//! separate verifier.  The v1 implementation uses ordinary app-managed paths;
//! an OS/container implementation can replace the adapter without changing
//! WorkPackage, scheduler, or Main-Agent code.

use std::{
    collections::HashMap,
    fs,
    fs::OpenOptions,
    io::{self, Write as _},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    resource_binding::{ResourceBinding, ResourceKind},
    work_package::{ChangeSet, WorkPackageScope},
};

pub const WORKSPACE_ISOLATION_SCHEMA_VERSION: u16 = 1;

/// Stable request used by the scheduler/issuance layer to create an
/// implementation worktree.  `scope.workspace_key` remains the durable
/// authorization key; `canonical_workspace` is the path used by the VCS
/// adapter and is never inferred from an untrusted child-agent value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRequest {
    pub scope: WorkPackageScope,
    pub work_package_id: String,
    pub attempt_id: String,
    pub canonical_workspace: PathBuf,
    pub base_revision: String,
    /// Lease epoch captured by the Main Agent.  A worktree handle is never
    /// valid for a restarted/renewed attempt with a different epoch.
    pub lease_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeHandle {
    // All filesystem/VCS paths remain provider-private.  The worker boundary
    // receives only these opaque identity projections below.
    provider_kind: String,
    scope: WorkPackageScope,
    work_package_id: String,
    attempt_id: String,
    lease_epoch: u64,
    handle_id: String,
    canonical_workspace: PathBuf,
    worktree_path: PathBuf,
    branch: String,
    base_revision: String,
    base_commit: String,
}

impl WorktreeHandle {
    pub fn opaque_id(&self) -> &str {
        &self.handle_id
    }

    pub fn provider_kind(&self) -> &str {
        &self.provider_kind
    }

    pub fn scope(&self) -> &WorkPackageScope {
        &self.scope
    }

    pub fn work_package_id(&self) -> &str {
        &self.work_package_id
    }

    pub fn attempt_id(&self) -> &str {
        &self.attempt_id
    }

    pub fn lease_epoch(&self) -> u64 {
        self.lease_epoch
    }
}

/// Provider-owned restart identity.  The tuple contains no checkout path or
/// branch authority; a provider may resolve it only through its own manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeManifestIdentity {
    pub provider_kind: String,
    pub handle_id: String,
    pub work_package_id: String,
    pub attempt_id: String,
    pub scope_digest: String,
    pub lease_epoch: u64,
    pub manifest_locator: String,
    pub manifest_nonce: String,
    pub manifest_digest: String,
}

impl WorktreeManifestIdentity {
    /// Reconstruct a provider-owned worktree identity from the sole durable
    /// resource ledger.  Callers never rebuild this tuple field by field or
    /// accept it from a worker; validation remains at the worktree seam.
    pub fn from_binding(binding: &ResourceBinding) -> Result<Self, WorkspaceError> {
        if binding.identity.resource_kind != ResourceKind::Worktree {
            return Err(WorkspaceError::InvalidIdentity);
        }
        let identity = Self {
            provider_kind: binding.identity.provider_kind.clone(),
            handle_id: binding.identity.resource_ref.clone(),
            work_package_id: binding.identity.work_package_id.clone(),
            attempt_id: binding.identity.attempt_id.clone(),
            scope_digest: binding.identity.scope_digest.clone(),
            lease_epoch: u64::try_from(binding.identity.lease_epoch)
                .map_err(|_| WorkspaceError::InvalidIdentity)?,
            manifest_locator: binding.identity.manifest_locator.clone(),
            manifest_nonce: binding.identity.manifest_nonce.clone(),
            manifest_digest: binding.identity.manifest_digest.clone(),
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn for_handle(
        handle: &WorktreeHandle,
        scope_digest: impl Into<String>,
    ) -> Result<Self, WorkspaceError> {
        let scope_digest = scope_digest.into();
        if !valid_scope_digest(&scope_digest) {
            return Err(WorkspaceError::InvalidIdentity);
        }
        let manifest_locator = format!("manifest_{}", handle.handle_id);
        let manifest_nonce = handle.handle_id.clone();
        let manifest_digest = worktree_manifest_digest(
            &handle.provider_kind,
            &handle.handle_id,
            &handle.work_package_id,
            &handle.attempt_id,
            &scope_digest,
            handle.lease_epoch,
            &manifest_nonce,
        );
        Ok(Self {
            provider_kind: handle.provider_kind.clone(),
            handle_id: handle.handle_id.clone(),
            work_package_id: handle.work_package_id.clone(),
            attempt_id: handle.attempt_id.clone(),
            scope_digest,
            lease_epoch: handle.lease_epoch,
            manifest_locator,
            manifest_nonce,
            manifest_digest,
        })
    }

    pub fn validate(&self) -> Result<(), WorkspaceError> {
        if !valid_identity(&self.provider_kind)
            || !valid_identity(&self.handle_id)
            || !valid_identity(&self.work_package_id)
            || !valid_identity(&self.attempt_id)
            || !valid_scope_digest(&self.scope_digest)
            || self.lease_epoch == 0
            || self.manifest_locator != format!("manifest_{}", self.handle_id)
            || self.manifest_nonce != self.handle_id
            || !valid_digest(&self.manifest_digest)
        {
            return Err(WorkspaceError::InvalidIdentity);
        }
        let expected = worktree_manifest_digest(
            &self.provider_kind,
            &self.handle_id,
            &self.work_package_id,
            &self.attempt_id,
            &self.scope_digest,
            self.lease_epoch,
            &self.manifest_nonce,
        );
        if expected != self.manifest_digest {
            return Err(WorkspaceError::PermissionDrift);
        }
        Ok(())
    }
}

/// Manifest-verified worktree capability.  The concrete handle (and all
/// paths) remain provider-private even after a restart scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedWorktreeHandle {
    identity: WorktreeManifestIdentity,
    handle: WorktreeHandle,
}

impl VerifiedWorktreeHandle {
    pub fn identity(&self) -> &WorktreeManifestIdentity {
        &self.identity
    }
}

/// Provider-owned restart record.  The checkout and canonical project paths
/// are persisted only for provider-side verification; they are never exposed
/// at the worker boundary.  `digest` covers every field except itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedWorktreeManifest {
    schema_version: u16,
    identity: WorktreeManifestIdentity,
    scope_session_id: String,
    scope_owner_profile_id: i64,
    scope_workspace_key: String,
    canonical_workspace: String,
    worktree_path: String,
    branch: String,
    base_revision: String,
    base_commit: String,
    state: String,
    digest: String,
}

/// Narrow persistence seam for providers that can recover worktree state from
/// a provider-owned manifest.  Providers that do not implement durable
/// manifest verification remain fail-closed; callers must not infer success
/// from an opaque handle or from the legacy in-memory lifecycle map.
pub trait WorktreeManifestProvider {
    fn persist_manifest(
        &self,
        handle: &WorktreeHandle,
        identity: &WorktreeManifestIdentity,
    ) -> Result<(), WorkspaceError>;

    fn resolve_manifest(
        &self,
        expected: &WorktreeManifestIdentity,
    ) -> Result<Option<VerifiedWorktreeHandle>, WorkspaceError>;

    /// Capture an immutable review artifact from a manifest-resolved checkout.
    /// Providers that cannot safely rehydrate a checkout must keep the default
    /// fail-closed behavior; callers must never reconstruct a `WorktreeHandle`
    /// from an opaque id themselves.
    fn capture_verified(
        &self,
        verified: &VerifiedWorktreeHandle,
        access: &WorktreeAccess,
        request: &ReviewCaptureRequest,
    ) -> Result<ReviewArtifact, WorkspaceError> {
        let _ = (verified, access, request);
        Err(WorkspaceError::ProviderUnavailable)
    }

    /// Resolve the canonical Git checkout only for a manifest-verified
    /// worktree.  This is intentionally provider-owned: workers never receive
    /// this path, while the Materializer can use it after parent confirmation.
    fn canonical_workspace_verified(
        &self,
        verified: &VerifiedWorktreeHandle,
    ) -> Result<PathBuf, WorkspaceError> {
        let _ = verified;
        Err(WorkspaceError::ProviderUnavailable)
    }

    fn release_verified(
        &self,
        verified: &VerifiedWorktreeHandle,
    ) -> Result<ReleaseReceipt, WorkspaceError>;
}

// Provider handles are shared between the cleanup validator and the runtime
// composition root.  Delegating the narrow manifest trait through `Arc` keeps
// one provider-owned lifecycle without cloning or exposing any checkout path.
impl<P> WorktreeManifestProvider for Arc<P>
where
    P: WorktreeManifestProvider + ?Sized,
{
    fn persist_manifest(
        &self,
        handle: &WorktreeHandle,
        identity: &WorktreeManifestIdentity,
    ) -> Result<(), WorkspaceError> {
        (**self).persist_manifest(handle, identity)
    }

    fn resolve_manifest(
        &self,
        expected: &WorktreeManifestIdentity,
    ) -> Result<Option<VerifiedWorktreeHandle>, WorkspaceError> {
        (**self).resolve_manifest(expected)
    }

    fn capture_verified(
        &self,
        verified: &VerifiedWorktreeHandle,
        access: &WorktreeAccess,
        request: &ReviewCaptureRequest,
    ) -> Result<ReviewArtifact, WorkspaceError> {
        (**self).capture_verified(verified, access, request)
    }

    fn canonical_workspace_verified(
        &self,
        verified: &VerifiedWorktreeHandle,
    ) -> Result<PathBuf, WorkspaceError> {
        (**self).canonical_workspace_verified(verified)
    }

    fn release_verified(
        &self,
        verified: &VerifiedWorktreeHandle,
    ) -> Result<ReleaseReceipt, WorkspaceError> {
        (**self).release_verified(verified)
    }
}

/// The operation class that the Main Agent grants to a delegated attempt.
/// A read-only reviewer can never be upgraded in-place to a writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeOperation {
    Read,
    Write,
}

/// Main-Agent-issued capability.  Roots are relative to the opaque worktree
/// and therefore cannot disclose or expand into the canonical workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeCapability {
    pub read_roots: Vec<String>,
    pub write_roots: Vec<String>,
    pub operation: WorktreeOperation,
    pub lease_epoch: u64,
    /// Stable canonical-workspace collaboration key.  It is compared against
    /// the trusted worktree scope and is never supplied by a child agent.
    pub collaboration_key: String,
}

/// Non-forgeable access token handed to a worker/reviewer after the Main Agent
/// grants a validated capability.  The handle id and capability are private;
/// callers can inspect only bounded identity projections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeAccess {
    handle_id: String,
    capability: WorktreeCapability,
    /// Present only for manifest-resolved access.  Keeping the identity here
    /// lets a file operation re-resolve provider state after a restart rather
    /// than treating an in-memory handle as authority.
    identity: Option<WorktreeManifestIdentity>,
}

impl WorktreeAccess {
    pub fn handle_id(&self) -> &str {
        &self.handle_id
    }

    pub fn capability(&self) -> &WorktreeCapability {
        &self.capability
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewCaptureRequest {
    /// The ChangeSet may be created after the worker has produced its commit.
    /// Binding is therefore explicit and can be performed exactly once before
    /// materialisation.
    pub change_set_id: Option<String>,
    /// Main-Agent/lease supplied allowlist.  An empty allowlist is an explicit
    /// no-write decision; the provider never expands it from worker output.
    pub allowed_relative_paths: Vec<String>,
    pub scope_digest: String,
    pub commit_message: String,
    pub created_at: i64,
}

/// A delegated worktree is an ephemeral execution resource.  Its branch and
/// checkout are not a second project tree: once the attempt is released they
/// are removed and the review artifact retains only immutable audit facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeLifecycle {
    Prepared,
    Captured,
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseDisposition {
    /// Remove both the checked-out worktree and its temporary branch.  This is
    /// the only supported lifecycle for delegated attempts.
    Cleanup,
    /// Explicit spelling retained for callers that want to document cleanup.
    DeleteBranch,
    /// Branch/worktree retention is intentionally forbidden.  Keeping this
    /// legacy variant makes old callers fail closed instead of silently
    /// retaining an execution tree after the v1 lifecycle boundary.
    RetainBranch,
}

impl Default for ReleaseDisposition {
    fn default() -> Self {
        Self::Cleanup
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseReceipt {
    pub worktree_path: PathBuf,
    /// Always false for delegated attempts.  The field remains explicit so an
    /// audit consumer cannot mistake a review artifact for a canonical path.
    pub branch_retained: bool,
    pub already_released: bool,
}

/// The VCS seam.  Implementations own the worktree lifecycle and return an
/// immutable commit-backed artifact; callers never shell out to Git directly.
pub trait WorkspaceProvider: Send + Sync {
    fn prepare(&self, request: &WorkspaceRequest) -> Result<WorktreeHandle, WorkspaceError>;

    fn capture(
        &self,
        handle: &WorktreeHandle,
        access: &WorktreeAccess,
        request: &ReviewCaptureRequest,
    ) -> Result<ReviewArtifact, WorkspaceError>;

    fn release(
        &self,
        handle: &WorktreeHandle,
        disposition: ReleaseDisposition,
    ) -> Result<ReleaseReceipt, WorkspaceError>;
}

/// A capability root exposed to an execution adapter.  The adapter may map a
/// path grant to a bind mount, a container mount, or a normal app-managed path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathGrant {
    pub root: PathBuf,
    pub mode: AccessMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsolationRequest {
    pub work_package_id: String,
    pub attempt_id: String,
    pub lease_id: String,
    pub lease_epoch: u64,
    pub grants: Vec<PathGrant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsolationHandle {
    /// Opaque durable identity.  It is not a bearer token and grants no
    /// authority on its own; every operation still checks the lease epoch.
    pub id: String,
    pub provider_kind: String,
    pub work_package_id: String,
    pub attempt_id: String,
    pub root: PathBuf,
    pub lease_id: String,
    pub lease_epoch: u64,
    pub capability_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IsolationCleanup {
    Cleaned,
    Retry,
    Refused,
}

/// Execution-isolation seam.  v1 can implement this with app-managed paths
/// and the existing capability lease.  A future OS/container adapter keeps
/// this interface and changes only the implementation behind it.
pub trait IsolationAdapter: Send + Sync {
    fn allocate(&self, request: &IsolationRequest) -> Result<IsolationHandle, IsolationError>;
    fn seal(&self, handle: &IsolationHandle) -> Result<(), IsolationError>;
    fn revoke(&self, handle: &IsolationHandle) -> Result<(), IsolationError>;
    fn cleanup(&self, handle: &IsolationHandle) -> IsolationCleanup;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Passed,
    Failed,
    NeedsDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewEvidence {
    /// Must belong to a distinct verifier attempt; an implementer cannot
    /// review its own commit.
    pub verifier_attempt_id: String,
    pub verdict: ReviewVerdict,
    pub reviewed_commit_sha: String,
    pub reviewed_diff_hash: String,
    pub evidence_refs: Vec<String>,
    pub reviewed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewArtifact {
    pub schema_version: u16,
    pub id: String,
    pub work_package_id: String,
    pub change_set_id: Option<String>,
    pub implementation_attempt_id: String,
    pub scope_digest: String,
    pub base_revision: String,
    pub base_commit: String,
    pub branch: String,
    /// Opaque internal audit reference.  It is deliberately not a filesystem
    /// path and must never be interpreted as the canonical project root.
    pub audit_ref: String,
    pub commit_sha: String,
    pub diff_hash: String,
    pub changed_paths: Vec<String>,
    pub allowed_relative_paths: Vec<String>,
    pub created_at: i64,
    pub review: Option<ReviewEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewArtifactError {
    Invalid,
    MissingChangeSet,
    ChangeSetAlreadyBound,
    ReviewAlreadyBound,
    ReviewMissing,
    ReviewAttemptMatchesImplementation,
    ReviewCommitMismatch,
    ReviewDiffMismatch,
    ReviewNotPassed,
    ScopeDigestMismatch,
}

impl ReviewArtifact {
    pub fn bind_change_set(mut self, change_set: &ChangeSet) -> Result<Self, ReviewArtifactError> {
        validate_artifact(&self)?;
        if self
            .change_set_id
            .as_deref()
            .is_some_and(|existing| existing != change_set.id)
        {
            return Err(ReviewArtifactError::ChangeSetAlreadyBound);
        }
        if change_set.package_id != self.work_package_id
            || change_set.base_revision != self.base_revision
            || change_set.scope_digest != self.scope_digest
        {
            return Err(ReviewArtifactError::ScopeDigestMismatch);
        }
        self.change_set_id = Some(change_set.id.clone());
        Ok(self)
    }

    pub fn bind_review(mut self, review: ReviewEvidence) -> Result<Self, ReviewArtifactError> {
        if self.review.is_some() {
            return Err(ReviewArtifactError::ReviewAlreadyBound);
        }
        if review.verifier_attempt_id == self.implementation_attempt_id {
            return Err(ReviewArtifactError::ReviewAttemptMatchesImplementation);
        }
        if review.reviewed_commit_sha != self.commit_sha {
            return Err(ReviewArtifactError::ReviewCommitMismatch);
        }
        if review.reviewed_diff_hash != self.diff_hash {
            return Err(ReviewArtifactError::ReviewDiffMismatch);
        }
        validate_review_evidence(&review)?;
        self.review = Some(review);
        Ok(self)
    }

    /// Materialisation is deliberately stricter than user confirmation:
    /// ChangeSet binding and an independent passed review are both required.
    pub fn ready_for_materialization(&self) -> Result<(), ReviewArtifactError> {
        validate_artifact(self)?;
        if self.change_set_id.is_none() {
            return Err(ReviewArtifactError::MissingChangeSet);
        }
        if self.scope_digest.is_empty() {
            return Err(ReviewArtifactError::ScopeDigestMismatch);
        }
        let Some(review) = self.review.as_ref() else {
            return Err(ReviewArtifactError::ReviewMissing);
        };
        if review.verdict != ReviewVerdict::Passed {
            return Err(ReviewArtifactError::ReviewNotPassed);
        }
        if review.verifier_attempt_id == self.implementation_attempt_id {
            return Err(ReviewArtifactError::ReviewAttemptMatchesImplementation);
        }
        if review.reviewed_commit_sha != self.commit_sha {
            return Err(ReviewArtifactError::ReviewCommitMismatch);
        }
        if review.reviewed_diff_hash != self.diff_hash {
            return Err(ReviewArtifactError::ReviewDiffMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceError {
    InvalidRequest,
    InvalidIdentity,
    InvalidRevision,
    InvalidPath,
    OutsideManagedRoot,
    WorktreeAlreadyExists,
    WorktreeNotFound,
    NoChanges,
    GitFailed,
    CommandUnavailable,
    ArtifactInvalid,
    PathNotAllowed,
    RetentionForbidden,
    AlreadyReleased,
    CapabilityInvalid,
    AccessDenied,
    LeaseEpochMismatch,
    CollaborationConflict,
    PermissionDrift,
    /// Provider-owned persistence is not available; residuals must remain
    /// retained and cleanup must fail closed.
    ProviderUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationError {
    InvalidRequest,
    LeaseRevoked,
    LeaseExpired,
    CapabilityRejected,
    ProviderUnavailable,
    InvalidHandle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommandOutput {
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
}

/// External command seam for Git.  Tests can verify arguments without making
/// a real repository or touching the canonical workspace.
pub trait GitCommandRunner: Send + Sync {
    fn run(&self, cwd: &Path, args: &[String]) -> io::Result<GitCommandOutput>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessGitCommandRunner;

impl GitCommandRunner for ProcessGitCommandRunner {
    fn run(&self, cwd: &Path, args: &[String]) -> io::Result<GitCommandOutput> {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .output()?;
        Ok(GitCommandOutput {
            status: output.status.code(),
            stdout: output.stdout,
        })
    }
}

/// Git-backed WorkspaceProvider.  Each branch/worktree is temporary and is
/// deleted by `release`; review/audit consumers use the immutable commit and
/// diff references in `ReviewArtifact`.  No method writes the canonical
/// workspace: Git writes only the app-managed worktree.
pub struct GitWorktreeProvider<R = ProcessGitCommandRunner> {
    root: PathBuf,
    active_root: PathBuf,
    manifest_root: PathBuf,
    runner: Arc<R>,
    lifecycle: Arc<Mutex<HashMap<PathBuf, WorktreeLifecycle>>>,
    admission: Arc<Mutex<WorktreeAdmissionState>>,
}

#[derive(Debug, Default)]
struct WorktreeAdmissionState {
    capabilities: HashMap<String, WorktreeCapability>,
    writers_by_workspace: HashMap<String, String>,
}

impl GitWorktreeProvider<ProcessGitCommandRunner> {
    pub fn new(
        root: impl AsRef<Path>,
        manifest_root: impl AsRef<Path>,
    ) -> Result<Self, WorkspaceError> {
        Self::with_runner_and_manifest_root(root, manifest_root, Arc::new(ProcessGitCommandRunner))
    }
}

impl<R: GitCommandRunner> GitWorktreeProvider<R> {
    #[cfg(test)]
    pub fn with_runner(root: impl AsRef<Path>, runner: Arc<R>) -> Result<Self, WorkspaceError> {
        let root = root.as_ref().to_path_buf();
        let manifest_root = root.join("manifests");
        Self::with_runner_and_manifest_root(root, manifest_root, runner)
    }

    /// Construct a provider with an explicitly trusted, provider-owned
    /// manifest root.  Child/model input must never reach this constructor.
    /// The root is required to be inside the provider storage root so a
    /// manifest can never authorize an unrelated filesystem location.
    pub fn with_runner_and_manifest_root(
        root: impl AsRef<Path>,
        manifest_root: impl AsRef<Path>,
        runner: Arc<R>,
    ) -> Result<Self, WorkspaceError> {
        fs::create_dir_all(root.as_ref()).map_err(|_| WorkspaceError::InvalidPath)?;
        let root = fs::canonicalize(root.as_ref()).map_err(|_| WorkspaceError::InvalidPath)?;
        fs::create_dir_all(manifest_root.as_ref()).map_err(|_| WorkspaceError::InvalidPath)?;
        let manifest_root =
            fs::canonicalize(manifest_root.as_ref()).map_err(|_| WorkspaceError::InvalidPath)?;
        if !within(&manifest_root, &root) {
            return Err(WorkspaceError::OutsideManagedRoot);
        }
        let active_root = root.join("active");
        fs::create_dir_all(&active_root).map_err(|_| WorkspaceError::InvalidPath)?;
        Ok(Self {
            root,
            active_root,
            manifest_root,
            runner,
            lifecycle: Arc::new(Mutex::new(HashMap::new())),
            admission: Arc::new(Mutex::new(WorktreeAdmissionState::default())),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest_root(&self) -> &Path {
        &self.manifest_root
    }

    fn run_ok(
        &self,
        cwd: &Path,
        operation: &'static str,
        args: Vec<String>,
    ) -> Result<Vec<u8>, WorkspaceError> {
        let output = self
            .runner
            .run(cwd, &args)
            .map_err(|_| WorkspaceError::CommandUnavailable)?;
        if output.status != Some(0) {
            // Never surface raw Git stderr (it can contain paths or user
            // configuration), but retain the fixed operation name for
            // operational diagnosis of an otherwise fail-closed workspace.
            eprintln!("[delegation] git worktree operation failed: {operation}");
            return Err(WorkspaceError::GitFailed);
        }
        Ok(output.stdout)
    }

    fn validate_handle(&self, handle: &WorktreeHandle) -> Result<(), WorkspaceError> {
        if !valid_identity(&handle.work_package_id)
            || !valid_identity(&handle.attempt_id)
            || !valid_branch(&handle.branch)
        {
            return Err(WorkspaceError::InvalidIdentity);
        }
        let active =
            fs::canonicalize(&self.active_root).map_err(|_| WorkspaceError::InvalidPath)?;
        let path = fs::canonicalize(&handle.worktree_path)
            .map_err(|_| WorkspaceError::WorktreeNotFound)?;
        if path.parent() != Some(active.as_path()) {
            return Err(WorkspaceError::OutsideManagedRoot);
        }
        Ok(())
    }

    fn lifecycle_of(&self, path: &Path) -> Result<WorktreeLifecycle, WorkspaceError> {
        self.lifecycle
            .lock()
            .map_err(|_| WorkspaceError::InvalidPath)?
            .get(path)
            .copied()
            .ok_or(WorkspaceError::WorktreeNotFound)
    }

    fn manifest_path(
        &self,
        identity: &WorktreeManifestIdentity,
    ) -> Result<PathBuf, WorkspaceError> {
        identity.validate()?;
        let root = fs::canonicalize(&self.manifest_root)
            .map_err(|_| WorkspaceError::ProviderUnavailable)?;
        let path = root.join(format!("{}.json", identity.manifest_locator));
        if path.parent() != Some(root.as_path()) {
            return Err(WorkspaceError::ProviderUnavailable);
        }
        Ok(path)
    }

    fn manifest_record(
        &self,
        handle: &WorktreeHandle,
        identity: &WorktreeManifestIdentity,
    ) -> Result<PersistedWorktreeManifest, WorkspaceError> {
        let state = match self.lifecycle_of(&handle.worktree_path)? {
            WorktreeLifecycle::Prepared => "prepared",
            WorktreeLifecycle::Captured => "captured",
            WorktreeLifecycle::Released => return Err(WorkspaceError::AlreadyReleased),
        };
        let canonical_workspace = fs::canonicalize(&handle.canonical_workspace)
            .map_err(|_| WorkspaceError::InvalidPath)?;
        let worktree_path = fs::canonicalize(&handle.worktree_path)
            .map_err(|_| WorkspaceError::WorktreeNotFound)?;
        self.validate_handle(handle)?;
        let mut record = PersistedWorktreeManifest {
            schema_version: WORKSPACE_ISOLATION_SCHEMA_VERSION,
            identity: identity.clone(),
            scope_session_id: handle.scope.session_id.clone(),
            scope_owner_profile_id: handle.scope.owner_profile_id,
            scope_workspace_key: handle.scope.workspace_key.clone(),
            canonical_workspace: canonical_workspace.to_string_lossy().into_owned(),
            worktree_path: worktree_path.to_string_lossy().into_owned(),
            branch: handle.branch.clone(),
            base_revision: handle.base_revision.clone(),
            base_commit: handle.base_commit.clone(),
            state: state.into(),
            digest: String::new(),
        };
        record.digest = persisted_manifest_digest(&record);
        Ok(record)
    }

    fn read_manifest(
        &self,
        expected: &WorktreeManifestIdentity,
    ) -> Result<Option<PersistedWorktreeManifest>, WorkspaceError> {
        let path = self.manifest_path(expected)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(WorkspaceError::ProviderUnavailable),
        };
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(WorkspaceError::ProviderUnavailable);
        }
        let bytes = fs::read(&path).map_err(|_| WorkspaceError::ProviderUnavailable)?;
        let record: PersistedWorktreeManifest =
            serde_json::from_slice(&bytes).map_err(|_| WorkspaceError::ProviderUnavailable)?;
        if record.identity != *expected {
            return Err(WorkspaceError::ProviderUnavailable);
        }
        if record.digest != persisted_manifest_digest(&record) {
            return Err(WorkspaceError::ProviderUnavailable);
        }
        Ok(Some(record))
    }

    fn verify_manifest_record(
        &self,
        expected: &WorktreeManifestIdentity,
        record: &PersistedWorktreeManifest,
    ) -> Result<WorktreeHandle, WorkspaceError> {
        if record.schema_version != WORKSPACE_ISOLATION_SCHEMA_VERSION
            || record.identity != *expected
            || !matches!(record.state.as_str(), "prepared" | "captured")
            || !valid_identity(&record.scope_session_id)
            || !valid_workspace_key(&record.scope_workspace_key)
            || !valid_revision(&record.base_revision)
            || !valid_commit(&record.base_commit)
            || !valid_branch(&record.branch)
            || record.digest != persisted_manifest_digest(record)
        {
            return Err(WorkspaceError::ProviderUnavailable);
        }
        let canonical_workspace = fs::canonicalize(&record.canonical_workspace)
            .map_err(|_| WorkspaceError::ProviderUnavailable)?;
        if !canonical_workspace.is_dir() {
            return Err(WorkspaceError::ProviderUnavailable);
        }
        let root_output = self
            .run_ok(
                &canonical_workspace,
                "rev-parse-root",
                vec!["rev-parse".into(), "--show-toplevel".into()],
            )
            .map_err(|_| WorkspaceError::ProviderUnavailable)?;
        let detected_root = trim_utf8(&root_output)
            .and_then(|value| fs::canonicalize(value).ok())
            .ok_or(WorkspaceError::ProviderUnavailable)?;
        if detected_root != canonical_workspace {
            return Err(WorkspaceError::ProviderUnavailable);
        }

        let active_root =
            fs::canonicalize(&self.active_root).map_err(|_| WorkspaceError::ProviderUnavailable)?;
        let worktree_path = fs::canonicalize(&record.worktree_path)
            .map_err(|_| WorkspaceError::ProviderUnavailable)?;
        let expected_path = active_root.join(format!(
            "{}-{}",
            record.identity.work_package_id, record.identity.attempt_id
        ));
        if worktree_path.parent() != Some(active_root.as_path())
            || worktree_path != expected_path
            || !worktree_path.is_dir()
        {
            return Err(WorkspaceError::ProviderUnavailable);
        }
        let expected_branch = format!(
            "angelbot/{}/{}",
            record.identity.work_package_id, record.identity.attempt_id
        );
        if record.branch != expected_branch
            || !worktree_list_contains(
                &self
                    .run_ok(
                        &canonical_workspace,
                        "worktree-list",
                        vec!["worktree".into(), "list".into(), "--porcelain".into()],
                    )
                    .map_err(|_| WorkspaceError::ProviderUnavailable)?,
                &worktree_path,
                &record.branch,
            )
        {
            return Err(WorkspaceError::ProviderUnavailable);
        }

        Ok(WorktreeHandle {
            provider_kind: record.identity.provider_kind.clone(),
            scope: WorkPackageScope {
                session_id: record.scope_session_id.clone(),
                owner_profile_id: record.scope_owner_profile_id,
                workspace_key: record.scope_workspace_key.clone(),
            },
            work_package_id: record.identity.work_package_id.clone(),
            attempt_id: record.identity.attempt_id.clone(),
            lease_epoch: record.identity.lease_epoch,
            handle_id: record.identity.handle_id.clone(),
            canonical_workspace,
            worktree_path,
            branch: record.branch.clone(),
            base_revision: record.base_revision.clone(),
            base_commit: record.base_commit.clone(),
        })
    }

    fn branch_present(&self, handle: &WorktreeHandle) -> Result<bool, WorkspaceError> {
        let output = self.run_ok(
            &handle.canonical_workspace,
            "branch-list",
            vec!["branch".into(), "--list".into(), handle.branch.clone()],
        )?;
        Ok(trim_utf8(&output).is_some())
    }

    fn cleanup_verified_handle(
        &self,
        handle: &WorktreeHandle,
    ) -> Result<ReleaseReceipt, WorkspaceError> {
        self.lifecycle
            .lock()
            .map_err(|_| WorkspaceError::InvalidPath)?
            .insert(handle.worktree_path.clone(), WorktreeLifecycle::Prepared);
        <Self as WorkspaceProvider>::release(self, handle, ReleaseDisposition::Cleanup)
    }

    /// Read-only lifecycle inspection for the scheduler/audit layer.  The
    /// returned state carries no filesystem authority and cannot be used to
    /// bypass `release` or capability revocation.
    pub fn lifecycle(&self, handle: &WorktreeHandle) -> Result<WorktreeLifecycle, WorkspaceError> {
        self.lifecycle_of(&handle.worktree_path)
    }

    /// Grant a Main-Agent capability after the provider has created the
    /// isolated worktree.  A second grant for the same handle must be byte-for
    /// byte equivalent; capability widening is permission drift.
    pub fn grant_access(
        &self,
        handle: &WorktreeHandle,
        capability: WorktreeCapability,
    ) -> Result<WorktreeAccess, WorkspaceError> {
        if self.lifecycle(handle)? == WorktreeLifecycle::Released {
            return Err(WorkspaceError::AlreadyReleased);
        }
        if capability.lease_epoch != handle.lease_epoch {
            return Err(WorkspaceError::LeaseEpochMismatch);
        }
        validate_capability(&capability, handle)?;

        let mut state = self
            .admission
            .lock()
            .map_err(|_| WorkspaceError::CapabilityInvalid)?;
        let digest = capability_digest(&capability);
        if let Some(previous) = state.capabilities.get(&handle.handle_id) {
            if capability_digest(previous) != digest {
                return Err(WorkspaceError::PermissionDrift);
            }
            return Ok(WorktreeAccess {
                handle_id: handle.handle_id.clone(),
                capability,
                identity: None,
            });
        }

        if capability.operation == WorktreeOperation::Write {
            let workspace = handle.scope.workspace_key.clone();
            if let Some(owner) = state.writers_by_workspace.get(&workspace) {
                if owner != &handle.handle_id {
                    return Err(WorkspaceError::CollaborationConflict);
                }
            }
            state
                .writers_by_workspace
                .insert(workspace, handle.handle_id.clone());
        }
        state
            .capabilities
            .insert(handle.handle_id.clone(), capability.clone());
        Ok(WorktreeAccess {
            handle_id: handle.handle_id.clone(),
            capability,
            identity: None,
        })
    }

    /// Validate an access token for one operation.  Read-only reviewer access
    /// can run concurrently; write access is admitted only once per canonical
    /// workspace and cannot be upgraded after grant.
    pub fn authorize(
        &self,
        handle: &WorktreeHandle,
        access: &WorktreeAccess,
        operation: WorktreeOperation,
    ) -> Result<(), WorkspaceError> {
        if self.lifecycle(handle)? == WorktreeLifecycle::Released {
            return Err(WorkspaceError::AlreadyReleased);
        }
        if access.handle_id != handle.handle_id {
            return Err(WorkspaceError::AccessDenied);
        }
        if access.capability.lease_epoch != handle.lease_epoch {
            return Err(WorkspaceError::LeaseEpochMismatch);
        }
        validate_capability(&access.capability, handle)?;
        let state = self
            .admission
            .lock()
            .map_err(|_| WorkspaceError::CapabilityInvalid)?;
        let Some(granted) = state.capabilities.get(&handle.handle_id) else {
            return Err(WorkspaceError::AccessDenied);
        };
        if capability_digest(granted) != capability_digest(&access.capability) {
            return Err(WorkspaceError::PermissionDrift);
        }
        if operation == WorktreeOperation::Write
            && access.capability.operation != WorktreeOperation::Write
        {
            return Err(WorkspaceError::AccessDenied);
        }
        Ok(())
    }

    /// Idempotent convenience entrypoint used by attempt cleanup/reapers.  It
    /// deliberately has no retention argument so callers cannot accidentally
    /// turn temporary delegated storage into a long-lived project branch.
    pub fn cleanup(&self, handle: &WorktreeHandle) -> Result<ReleaseReceipt, WorkspaceError> {
        <Self as WorkspaceProvider>::release(self, handle, ReleaseDisposition::Cleanup)
    }
}

/// Controlled file-operation seam for change workers.  The caller supplies an
/// opaque, manifest-backed access token and a relative path; the provider owns
/// path resolution, symlink checks, size limits, and lease/capability checks.
/// No canonical workspace path crosses this interface.
pub trait WorktreeFileProvider: Send + Sync {
    fn open_file_access(
        &self,
        identity: &WorktreeManifestIdentity,
        capability: WorktreeCapability,
    ) -> Result<WorktreeAccess, WorkspaceError>;

    fn read_file(
        &self,
        access: &WorktreeAccess,
        relative_path: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, WorkspaceError>;

    fn write_candidate(
        &self,
        access: &WorktreeAccess,
        relative_path: &str,
        contents: &[u8],
        max_bytes: usize,
    ) -> Result<(), WorkspaceError>;
}

impl<P> WorktreeFileProvider for Arc<P>
where
    P: WorktreeFileProvider + ?Sized,
{
    fn open_file_access(
        &self,
        identity: &WorktreeManifestIdentity,
        capability: WorktreeCapability,
    ) -> Result<WorktreeAccess, WorkspaceError> {
        (**self).open_file_access(identity, capability)
    }

    fn read_file(
        &self,
        access: &WorktreeAccess,
        relative_path: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, WorkspaceError> {
        (**self).read_file(access, relative_path, max_bytes)
    }

    fn write_candidate(
        &self,
        access: &WorktreeAccess,
        relative_path: &str,
        contents: &[u8],
        max_bytes: usize,
    ) -> Result<(), WorkspaceError> {
        (**self).write_candidate(access, relative_path, contents, max_bytes)
    }
}

impl<R: GitCommandRunner> WorkspaceProvider for GitWorktreeProvider<R> {
    fn prepare(&self, request: &WorkspaceRequest) -> Result<WorktreeHandle, WorkspaceError> {
        validate_workspace_request(request)?;
        let canonical = fs::canonicalize(&request.canonical_workspace)
            .map_err(|_| WorkspaceError::InvalidPath)?;
        if !canonical.is_dir() || within(&canonical, &self.root) || within(&self.root, &canonical) {
            return Err(WorkspaceError::OutsideManagedRoot);
        }
        let detected_output = self.run_ok(
            &canonical,
            "rev-parse-root",
            vec!["rev-parse".into(), "--show-toplevel".into()],
        )?;
        let detected = trim_utf8(&detected_output)
            .map(str::to_owned)
            .ok_or(WorkspaceError::GitFailed)?;
        let detected = fs::canonicalize(detected).map_err(|_| WorkspaceError::GitFailed)?;
        if detected != canonical {
            return Err(WorkspaceError::InvalidPath);
        }
        let base_output = self.run_ok(
            &canonical,
            "rev-parse-base",
            vec![
                "rev-parse".into(),
                "--verify".into(),
                format!("{}^{{commit}}", request.base_revision),
            ],
        )?;
        let base_commit = trim_utf8(&base_output)
            .filter(|value| valid_commit(value))
            .map(str::to_owned)
            .ok_or(WorkspaceError::InvalidRevision)?;
        let branch = format!(
            "angelbot/{}/{}",
            request.work_package_id, request.attempt_id
        );
        let path = self.active_root.join(format!(
            "{}-{}",
            request.work_package_id, request.attempt_id
        ));
        if path.exists() {
            return Err(WorkspaceError::WorktreeAlreadyExists);
        }
        self.run_ok(
            &canonical,
            "worktree-add",
            vec![
                "worktree".into(),
                "add".into(),
                "-b".into(),
                branch.clone(),
                git_path_string(&path),
                base_commit.clone(),
            ],
        )?;
        self.lifecycle
            .lock()
            .map_err(|_| WorkspaceError::InvalidPath)?
            .insert(path.clone(), WorktreeLifecycle::Prepared);
        Ok(WorktreeHandle {
            provider_kind: "git_worktree_v1".into(),
            scope: request.scope.clone(),
            work_package_id: request.work_package_id.clone(),
            attempt_id: request.attempt_id.clone(),
            lease_epoch: request.lease_epoch,
            handle_id: format!("worktree_handle_{}", Uuid::new_v4().simple()),
            canonical_workspace: canonical,
            worktree_path: path,
            branch,
            base_revision: request.base_revision.clone(),
            base_commit,
        })
    }

    fn capture(
        &self,
        handle: &WorktreeHandle,
        access: &WorktreeAccess,
        request: &ReviewCaptureRequest,
    ) -> Result<ReviewArtifact, WorkspaceError> {
        if self.lifecycle_of(&handle.worktree_path)? == WorktreeLifecycle::Released {
            return Err(WorkspaceError::AlreadyReleased);
        }
        self.validate_handle(handle)?;
        self.authorize(handle, access, WorktreeOperation::Write)?;
        if request.commit_message.trim().is_empty()
            || request.commit_message.len() > 200
            || request.commit_message.contains(['\0', '\r', '\n'])
            || request.scope_digest.trim().is_empty()
            || request.scope_digest.len() > 256
            || request.scope_digest.contains(['\0', '\r', '\n'])
        {
            return Err(WorkspaceError::InvalidRequest);
        }
        let mut allowed = request.allowed_relative_paths.clone();
        allowed.sort();
        allowed.dedup();
        if allowed.iter().any(|path| !safe_relative(path)) {
            return Err(WorkspaceError::PathNotAllowed);
        }
        if allowed
            .iter()
            .any(|path| !path_in_roots(path, &access.capability.write_roots))
        {
            return Err(WorkspaceError::PathNotAllowed);
        }
        let status = self.run_ok(
            &handle.worktree_path,
            "status",
            vec![
                "status".into(),
                "--porcelain=v1".into(),
                "--untracked-files=all".into(),
                "-z".into(),
            ],
        )?;
        if status.is_empty() {
            return Err(WorkspaceError::NoChanges);
        }
        let status_paths = parse_status_paths(&status)?;
        if status_paths.is_empty() || status_paths.iter().any(|path| !allowed.contains(path)) {
            return Err(WorkspaceError::PathNotAllowed);
        }
        let mut add_args = vec!["add".into(), "--".into()];
        add_args.extend(status_paths.iter().cloned());
        self.run_ok(&handle.worktree_path, "add", add_args)?;
        self.run_ok(
            &handle.worktree_path,
            "commit",
            vec!["commit".into(), "-m".into(), request.commit_message.clone()],
        )?;
        let commit_output = self.run_ok(
            &handle.worktree_path,
            "rev-parse-head",
            vec!["rev-parse".into(), "HEAD".into()],
        )?;
        let commit_sha = trim_utf8(&commit_output)
            .filter(|value| valid_commit(value))
            .map(str::to_owned)
            .ok_or(WorkspaceError::GitFailed)?;
        let range = format!("{}..{}", handle.base_commit, commit_sha);
        let names = self.run_ok(
            &handle.worktree_path,
            "diff-names",
            vec![
                "diff".into(),
                "--name-only".into(),
                "-z".into(),
                range.clone(),
            ],
        )?;
        let changed_paths = parse_paths(&names)?;
        if changed_paths.iter().any(|path| !allowed.contains(path)) {
            return Err(WorkspaceError::PathNotAllowed);
        }
        let diff = self.run_ok(
            &handle.worktree_path,
            "diff",
            vec!["diff".into(), "--binary".into(), range],
        )?;
        let diff_hash = hex::encode(Sha256::digest(&diff));
        let artifact = ReviewArtifact {
            schema_version: WORKSPACE_ISOLATION_SCHEMA_VERSION,
            id: format!("review_{}", Uuid::new_v4().simple()),
            work_package_id: handle.work_package_id.clone(),
            change_set_id: request.change_set_id.clone(),
            implementation_attempt_id: handle.attempt_id.clone(),
            scope_digest: request.scope_digest.clone(),
            base_revision: handle.base_revision.clone(),
            base_commit: handle.base_commit.clone(),
            branch: handle.branch.clone(),
            audit_ref: format!(
                "audit://{}/{}/{}",
                handle.work_package_id, handle.attempt_id, commit_sha
            ),
            commit_sha,
            diff_hash,
            changed_paths,
            allowed_relative_paths: allowed,
            created_at: request.created_at,
            review: None,
        };
        validate_artifact(&artifact).map_err(|_| WorkspaceError::ArtifactInvalid)?;
        self.lifecycle
            .lock()
            .map_err(|_| WorkspaceError::InvalidPath)?
            .insert(handle.worktree_path.clone(), WorktreeLifecycle::Captured);
        Ok(artifact)
    }

    fn release(
        &self,
        handle: &WorktreeHandle,
        disposition: ReleaseDisposition,
    ) -> Result<ReleaseReceipt, WorkspaceError> {
        let lifecycle = self.lifecycle_of(&handle.worktree_path)?;
        if lifecycle == WorktreeLifecycle::Released {
            return Ok(ReleaseReceipt {
                worktree_path: handle.worktree_path.clone(),
                branch_retained: false,
                already_released: true,
            });
        }
        self.validate_handle(handle)?;
        if matches!(disposition, ReleaseDisposition::RetainBranch) {
            return Err(WorkspaceError::RetentionForbidden);
        }
        self.run_ok(
            &handle.canonical_workspace,
            "worktree-remove",
            vec![
                "worktree".into(),
                "remove".into(),
                "--force".into(),
                git_path_string(&handle.worktree_path),
            ],
        )?;
        self.run_ok(
            &handle.canonical_workspace,
            "branch-delete",
            vec!["branch".into(), "-D".into(), handle.branch.clone()],
        )?;
        let mut admission = self
            .admission
            .lock()
            .map_err(|_| WorkspaceError::CapabilityInvalid)?;
        admission.capabilities.remove(&handle.handle_id);
        if admission
            .writers_by_workspace
            .get(&handle.scope.workspace_key)
            .is_some_and(|owner| owner == &handle.handle_id)
        {
            admission
                .writers_by_workspace
                .remove(&handle.scope.workspace_key);
        }
        self.lifecycle
            .lock()
            .map_err(|_| WorkspaceError::InvalidPath)?
            .insert(handle.worktree_path.clone(), WorktreeLifecycle::Released);
        Ok(ReleaseReceipt {
            worktree_path: handle.worktree_path.clone(),
            branch_retained: false,
            already_released: false,
        })
    }
}

impl<R: GitCommandRunner> WorktreeManifestProvider for GitWorktreeProvider<R> {
    fn persist_manifest(
        &self,
        handle: &WorktreeHandle,
        identity: &WorktreeManifestIdentity,
    ) -> Result<(), WorkspaceError> {
        identity.validate()?;
        if identity.provider_kind != handle.provider_kind
            || identity.handle_id != handle.handle_id
            || identity.work_package_id != handle.work_package_id
            || identity.attempt_id != handle.attempt_id
            || identity.lease_epoch != handle.lease_epoch
        {
            return Err(WorkspaceError::PermissionDrift);
        }
        let record = self.manifest_record(handle, identity)?;
        let path = self.manifest_path(identity)?;
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return Err(WorkspaceError::ProviderUnavailable);
            }
            let bytes = fs::read(&path).map_err(|_| WorkspaceError::ProviderUnavailable)?;
            let existing: PersistedWorktreeManifest =
                serde_json::from_slice(&bytes).map_err(|_| WorkspaceError::ProviderUnavailable)?;
            if existing != record {
                return Err(WorkspaceError::ProviderUnavailable);
            }
            return Ok(());
        }
        let payload =
            serde_json::to_vec(&record).map_err(|_| WorkspaceError::ProviderUnavailable)?;
        let temp = self.manifest_root.join(format!(
            ".{}.tmp_{}",
            identity.manifest_locator,
            Uuid::new_v4().simple()
        ));
        if temp.parent() != Some(self.manifest_root.as_path()) {
            return Err(WorkspaceError::ProviderUnavailable);
        }
        let write_result = (|| -> Result<(), WorkspaceError> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|_| WorkspaceError::ProviderUnavailable)?;
            file.write_all(&payload)
                .map_err(|_| WorkspaceError::ProviderUnavailable)?;
            file.sync_all()
                .map_err(|_| WorkspaceError::ProviderUnavailable)?;
            fs::rename(&temp, &path).map_err(|_| WorkspaceError::ProviderUnavailable)
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temp);
            if let Ok(bytes) = fs::read(&path) {
                if serde_json::from_slice::<PersistedWorktreeManifest>(&bytes)
                    .ok()
                    .is_some_and(|existing| existing == record)
                {
                    return Ok(());
                }
            }
        }
        write_result
    }

    fn resolve_manifest(
        &self,
        expected: &WorktreeManifestIdentity,
    ) -> Result<Option<VerifiedWorktreeHandle>, WorkspaceError> {
        expected.validate()?;
        let Some(record) = self.read_manifest(expected)? else {
            return Ok(None);
        };
        let handle = self.verify_manifest_record(expected, &record)?;
        let lifecycle = match record.state.as_str() {
            "prepared" => WorktreeLifecycle::Prepared,
            "captured" => WorktreeLifecycle::Captured,
            _ => return Err(WorkspaceError::ProviderUnavailable),
        };
        self.lifecycle
            .lock()
            .map_err(|_| WorkspaceError::ProviderUnavailable)?
            .insert(handle.worktree_path.clone(), lifecycle);
        Ok(Some(VerifiedWorktreeHandle {
            identity: expected.clone(),
            handle,
        }))
    }

    fn capture_verified(
        &self,
        verified: &VerifiedWorktreeHandle,
        access: &WorktreeAccess,
        request: &ReviewCaptureRequest,
    ) -> Result<ReviewArtifact, WorkspaceError> {
        verified.identity.validate()?;
        if verified.identity.handle_id != verified.handle.handle_id
            || verified.identity.work_package_id != verified.handle.work_package_id
            || verified.identity.attempt_id != verified.handle.attempt_id
            || verified.identity.lease_epoch != verified.handle.lease_epoch
        {
            return Err(WorkspaceError::PermissionDrift);
        }
        // Re-read and verify the provider manifest before allowing capture.
        // This keeps a stale/restarted handle from becoming authority merely
        // because it still exists in a caller's memory.
        let Some(current) = self.resolve_manifest(&verified.identity)? else {
            return Err(WorkspaceError::WorktreeNotFound);
        };
        if current.identity != verified.identity {
            return Err(WorkspaceError::PermissionDrift);
        }
        self.capture(&current.handle, access, request)
    }

    fn canonical_workspace_verified(
        &self,
        verified: &VerifiedWorktreeHandle,
    ) -> Result<PathBuf, WorkspaceError> {
        verified.identity.validate()?;
        let Some(current) = self.resolve_manifest(&verified.identity)? else {
            return Err(WorkspaceError::WorktreeNotFound);
        };
        if current.identity != verified.identity {
            return Err(WorkspaceError::PermissionDrift);
        }
        Ok(current.handle.canonical_workspace)
    }

    fn release_verified(
        &self,
        verified: &VerifiedWorktreeHandle,
    ) -> Result<ReleaseReceipt, WorkspaceError> {
        verified.identity.validate()?;
        if verified.identity.handle_id != verified.handle.handle_id
            || verified.identity.attempt_id != verified.handle.attempt_id
            || verified.identity.lease_epoch != verified.handle.lease_epoch
        {
            return Err(WorkspaceError::PermissionDrift);
        }
        if self
            .lifecycle_of(&verified.handle.worktree_path)
            .ok()
            .is_some_and(|state| state == WorktreeLifecycle::Released)
        {
            return Ok(ReleaseReceipt {
                worktree_path: verified.handle.worktree_path.clone(),
                branch_retained: false,
                already_released: true,
            });
        }
        let Some(record) = self.read_manifest(&verified.identity)? else {
            let worktree_exists = verified.handle.worktree_path.exists();
            let branch_exists = self.branch_present(&verified.handle)?;
            if !worktree_exists && !branch_exists {
                return Ok(ReleaseReceipt {
                    worktree_path: verified.handle.worktree_path.clone(),
                    branch_retained: false,
                    already_released: true,
                });
            }
            return Err(WorkspaceError::ProviderUnavailable);
        };
        let handle = self.verify_manifest_record(&verified.identity, &record)?;
        let receipt = self.cleanup_verified_handle(&handle)?;
        let path = self.manifest_path(&verified.identity)?;
        match fs::remove_file(path) {
            Ok(()) => Ok(receipt),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(receipt),
            Err(_) => Err(WorkspaceError::ProviderUnavailable),
        }
    }
}

impl<R: GitCommandRunner> WorktreeFileProvider for GitWorktreeProvider<R> {
    fn open_file_access(
        &self,
        identity: &WorktreeManifestIdentity,
        capability: WorktreeCapability,
    ) -> Result<WorktreeAccess, WorkspaceError> {
        let Some(verified) = self.resolve_manifest(identity)? else {
            return Err(WorkspaceError::WorktreeNotFound);
        };
        if matches!(capability.operation, WorktreeOperation::Write)
            && capability.write_roots.is_empty()
        {
            return Err(WorkspaceError::AccessDenied);
        }
        let access = self.grant_access(&verified.handle, capability)?;
        Ok(WorktreeAccess {
            handle_id: access.handle_id,
            capability: access.capability,
            identity: Some(identity.clone()),
        })
    }

    fn read_file(
        &self,
        access: &WorktreeAccess,
        relative_path: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, WorkspaceError> {
        let (handle, checked) =
            self.resolve_file_access(access, relative_path, WorktreeOperation::Read)?;
        let path = checked.ok_or(WorkspaceError::PathNotAllowed)?;
        self.authorize(&handle, access, WorktreeOperation::Read)?;
        let metadata = fs::symlink_metadata(&path).map_err(|_| WorkspaceError::WorktreeNotFound)?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(WorkspaceError::PathNotAllowed);
        }
        let limit = max_bytes.min(4 * 1024 * 1024);
        let bytes = fs::read(&path).map_err(|_| WorkspaceError::ProviderUnavailable)?;
        if bytes.len() > limit {
            return Err(WorkspaceError::InvalidRequest);
        }
        Ok(bytes)
    }

    fn write_candidate(
        &self,
        access: &WorktreeAccess,
        relative_path: &str,
        contents: &[u8],
        max_bytes: usize,
    ) -> Result<(), WorkspaceError> {
        let (handle, checked) =
            self.resolve_file_access(access, relative_path, WorktreeOperation::Write)?;
        let path = checked.ok_or(WorkspaceError::PathNotAllowed)?;
        self.authorize(&handle, access, WorktreeOperation::Write)?;
        let limit = max_bytes.min(4 * 1024 * 1024);
        if contents.len() > limit {
            return Err(WorkspaceError::InvalidRequest);
        }
        let parent = path.parent().ok_or(WorkspaceError::PathNotAllowed)?;
        fs::create_dir_all(parent).map_err(|_| WorkspaceError::ProviderUnavailable)?;
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return Err(WorkspaceError::PathNotAllowed);
            }
        }
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .map_err(|_| WorkspaceError::ProviderUnavailable)?;
        file.write_all(contents)
            .map_err(|_| WorkspaceError::ProviderUnavailable)?;
        file.sync_all()
            .map_err(|_| WorkspaceError::ProviderUnavailable)
    }
}

impl<R: GitCommandRunner> GitWorktreeProvider<R> {
    fn resolve_file_access(
        &self,
        access: &WorktreeAccess,
        relative_path: &str,
        operation: WorktreeOperation,
    ) -> Result<(WorktreeHandle, Option<PathBuf>), WorkspaceError> {
        if !safe_relative(relative_path)
            || relative_path.contains(['\0', '\r', '\n', ':'])
            || !path_in_roots(
                relative_path,
                match operation {
                    WorktreeOperation::Read => &access.capability.read_roots,
                    WorktreeOperation::Write => &access.capability.write_roots,
                },
            )
        {
            return Err(WorkspaceError::PathNotAllowed);
        }
        let identity = access
            .identity
            .as_ref()
            .ok_or(WorkspaceError::ProviderUnavailable)?;
        let verified = self
            .resolve_manifest(identity)?
            .ok_or(WorkspaceError::WorktreeNotFound)?;
        if verified.handle.handle_id != access.handle_id {
            return Err(WorkspaceError::PermissionDrift);
        }
        let root = fs::canonicalize(&verified.handle.worktree_path)
            .map_err(|_| WorkspaceError::WorktreeNotFound)?;
        let candidate = root.join(relative_path);
        let checked = if candidate.exists() {
            fs::canonicalize(&candidate).map_err(|_| WorkspaceError::PathNotAllowed)?
        } else {
            if operation == WorktreeOperation::Read {
                return Err(WorkspaceError::WorktreeNotFound);
            }
            let parent = candidate.parent().ok_or(WorkspaceError::PathNotAllowed)?;
            // Check the nearest existing ancestor before creating anything so
            // a symlinked directory cannot redirect creation outside the
            // provider-owned worktree.
            let mut ancestor = parent;
            while !ancestor.exists() {
                ancestor = ancestor.parent().ok_or(WorkspaceError::PathNotAllowed)?;
            }
            let metadata =
                fs::symlink_metadata(ancestor).map_err(|_| WorkspaceError::PathNotAllowed)?;
            if metadata.file_type().is_symlink() {
                return Err(WorkspaceError::PathNotAllowed);
            }
            let ancestor =
                fs::canonicalize(ancestor).map_err(|_| WorkspaceError::PathNotAllowed)?;
            if !within(&ancestor, &root) {
                return Err(WorkspaceError::PathNotAllowed);
            }
            fs::create_dir_all(parent).map_err(|_| WorkspaceError::ProviderUnavailable)?;
            let parent = fs::canonicalize(parent).map_err(|_| WorkspaceError::PathNotAllowed)?;
            if !within(&parent, &root) {
                return Err(WorkspaceError::PathNotAllowed);
            }
            parent.join(
                candidate
                    .file_name()
                    .ok_or(WorkspaceError::PathNotAllowed)?,
            )
        };
        if !within(&checked, &root) {
            return Err(WorkspaceError::PathNotAllowed);
        }
        Ok((verified.handle, Some(checked)))
    }
}

fn validate_workspace_request(request: &WorkspaceRequest) -> Result<(), WorkspaceError> {
    if !valid_identity(&request.work_package_id) || !valid_identity(&request.attempt_id) {
        return Err(WorkspaceError::InvalidIdentity);
    }
    if request.scope.session_id.is_empty() || request.scope.workspace_key.is_empty() {
        return Err(WorkspaceError::InvalidRequest);
    }
    if !valid_revision(&request.base_revision) {
        return Err(WorkspaceError::InvalidRevision);
    }
    Ok(())
}

fn validate_review_evidence(review: &ReviewEvidence) -> Result<(), ReviewArtifactError> {
    if !valid_identity(&review.verifier_attempt_id)
        || !valid_commit(&review.reviewed_commit_sha)
        || !valid_digest(&review.reviewed_diff_hash)
        || review.reviewed_at <= 0
        || review.evidence_refs.len() > 32
        || review.evidence_refs.iter().any(|r| !safe_ref(r))
    {
        return Err(ReviewArtifactError::Invalid);
    }
    Ok(())
}

fn validate_artifact(artifact: &ReviewArtifact) -> Result<(), ReviewArtifactError> {
    if artifact.schema_version != WORKSPACE_ISOLATION_SCHEMA_VERSION
        || !valid_identity(&artifact.id)
        || !valid_identity(&artifact.work_package_id)
        || !valid_identity(&artifact.implementation_attempt_id)
        || !valid_revision(&artifact.base_revision)
        || !valid_commit(&artifact.base_commit)
        || !valid_commit(&artifact.commit_sha)
        || !valid_digest(&artifact.diff_hash)
        || !safe_ref(&artifact.audit_ref)
        || artifact.scope_digest.trim().is_empty()
        || artifact
            .changed_paths
            .iter()
            .any(|path| !safe_relative(path))
        || artifact
            .allowed_relative_paths
            .iter()
            .any(|path| !safe_relative(path))
        || artifact
            .changed_paths
            .iter()
            .any(|path| !artifact.allowed_relative_paths.contains(path))
    {
        return Err(ReviewArtifactError::Invalid);
    }
    if let Some(review) = artifact.review.as_ref() {
        validate_review_evidence(review)?;
    }
    Ok(())
}

fn parse_paths(bytes: &[u8]) -> Result<Vec<String>, WorkspaceError> {
    let mut paths = Vec::new();
    for item in bytes
        .split(|byte| *byte == 0)
        .filter(|item| !item.is_empty())
    {
        let value = String::from_utf8(item.to_vec())
            .map_err(|_| WorkspaceError::ArtifactInvalid)?
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        if value.is_empty() {
            continue;
        }
        if !safe_relative(&value) {
            return Err(WorkspaceError::ArtifactInvalid);
        }
        paths.push(value);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Parse `git status --porcelain=v1 -z`.  The status prefix is deliberately
/// discarded; only the path is admitted to the explicit Main-Agent allowlist.
/// Rename/copy records are rejected for v1 rather than guessing which side of
/// the record the worker intended to change.
fn parse_status_paths(bytes: &[u8]) -> Result<Vec<String>, WorkspaceError> {
    let mut paths = Vec::new();
    for item in bytes
        .split(|byte| *byte == 0)
        .filter(|item| !item.is_empty())
    {
        if item.len() < 4 || item[2] != b' ' {
            return Err(WorkspaceError::PathNotAllowed);
        }
        let value = String::from_utf8(item[3..].to_vec())
            .map_err(|_| WorkspaceError::PathNotAllowed)?
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        if value.is_empty() || !safe_relative(&value) {
            return Err(WorkspaceError::PathNotAllowed);
        }
        paths.push(value);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn trim_utf8(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes)
        .ok()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}
fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// Workspace keys are trusted, canonicalized scope identifiers.  Unlike row
/// IDs, they intentionally retain a normalized drive/path spelling so they
/// can distinguish two local workspaces. They are never used as a filesystem
/// path by the provider; only `canonical_workspace` is resolved for I/O.
fn valid_workspace_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.contains(['\0', '\r', '\n'])
        && !value.contains("..")
}
fn valid_branch(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.contains("..") && !value.ends_with('/')
}
fn valid_revision(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.chars().any(char::is_whitespace)
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/' | b'^' | b'~')
        })
}

fn validate_capability(
    capability: &WorktreeCapability,
    handle: &WorktreeHandle,
) -> Result<(), WorkspaceError> {
    if capability.collaboration_key != handle.scope.workspace_key
        || capability.lease_epoch != handle.lease_epoch
        || capability.read_roots.len() > 64
        || capability.write_roots.len() > 64
        || (!capability.read_roots.is_empty()
            && capability
                .read_roots
                .iter()
                .any(|path| !safe_relative(path)))
        || capability
            .write_roots
            .iter()
            .any(|path| !safe_relative(path))
    {
        return Err(WorkspaceError::CapabilityInvalid);
    }
    match capability.operation {
        WorktreeOperation::Read if !capability.write_roots.is_empty() => {
            Err(WorkspaceError::CapabilityInvalid)
        }
        WorktreeOperation::Write if capability.write_roots.is_empty() => {
            Err(WorkspaceError::CapabilityInvalid)
        }
        _ => Ok(()),
    }
}

fn capability_digest(capability: &WorktreeCapability) -> String {
    let payload = serde_json::to_vec(capability).unwrap_or_default();
    hex::encode(Sha256::digest(payload))
}

fn worktree_manifest_digest(
    provider_kind: &str,
    handle_id: &str,
    work_package_id: &str,
    attempt_id: &str,
    scope_digest: &str,
    lease_epoch: u64,
    nonce: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"git_worktree_manifest_v1:");
    for value in [
        provider_kind,
        handle_id,
        work_package_id,
        attempt_id,
        scope_digest,
        &lease_epoch.to_string(),
        nonce,
    ] {
        hasher.update(value.as_bytes());
        hasher.update(b":");
    }
    hex::encode(hasher.finalize())
}

fn persisted_manifest_digest(record: &PersistedWorktreeManifest) -> String {
    let mut unsigned = record.clone();
    unsigned.digest.clear();
    let payload = serde_json::to_vec(&unsigned).unwrap_or_default();
    hex::encode(Sha256::digest(payload))
}

/// Git for Windows does not accept the `\\?\` extended-length prefix as a
/// worktree argument.  Providers retain canonical paths for containment
/// checks, but all Git command arguments and its own list output use the
/// ordinary Windows spelling.
fn git_path_string(path: &Path) -> String {
    let raw = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{unc}");
        }
        if let Some(normal) = raw.strip_prefix(r"\\?\") {
            return normal.to_owned();
        }
    }
    raw.into_owned()
}

fn worktree_list_contains(bytes: &[u8], expected_path: &Path, expected_branch: &str) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    // Git for Windows reports worktree locations with forward slashes, while
    // `Path`/the manifest retain the native spelling. This comparison is
    // identity validation, not an authority grant, so normalize only
    // equivalent Windows separators and drive-letter case before matching.
    let expected_path = normalize_git_worktree_path(&git_path_string(expected_path));
    let mut current_path = None::<String>;
    let mut current_branch = None::<String>;
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if current_path.as_deref() == Some(expected_path.as_str())
                && current_branch.as_deref() == Some(expected_branch)
            {
                return true;
            }
            current_path = Some(normalize_git_worktree_path(path.trim()));
            current_branch = None;
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            current_branch = Some(branch.trim().to_owned());
        }
    }
    current_path.as_deref() == Some(expected_path.as_str())
        && current_branch.as_deref() == Some(expected_branch)
}

fn normalize_git_worktree_path(path: &str) -> String {
    #[cfg(windows)]
    {
        path.replace('/', "\\").to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        path.to_owned()
    }
}

fn path_in_roots(path: &str, roots: &[String]) -> bool {
    roots.iter().any(|root| {
        root == "."
            || path == root
            || path
                .strip_prefix(root)
                .is_some_and(|suffix| suffix.starts_with('/'))
            || path
                .strip_prefix(root)
                .is_some_and(|suffix| suffix.starts_with('\\'))
    })
}

fn valid_commit(value: &str) -> bool {
    (40..=128).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn valid_scope_digest(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.contains(['/', '\\', '\0', '\r', '\n'])
}
fn safe_ref(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.contains(['\0', '\r', '\n'])
}
fn safe_relative(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
}
fn within(path: &Path, parent: &Path) -> bool {
    path == parent || path.starts_with(parent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::tempdir;

    #[test]
    fn worktree_list_accepts_git_for_windows_path_spelling() {
        let listing = b"worktree C:/Users/example/active/package-attempt\nHEAD abc\nbranch refs/heads/angelbot/package/attempt\n";
        assert!(worktree_list_contains(
            listing,
            Path::new(r"C:\Users\example\active\package-attempt"),
            "angelbot/package/attempt",
        ));
        assert!(valid_workspace_key("d:/projects/desktop/angelbot"));
        assert!(!valid_workspace_key("../escape"));
    }

    #[derive(Default)]
    struct FakeGit {
        calls: Mutex<Vec<Vec<String>>>,
        root: PathBuf,
    }
    impl GitCommandRunner for FakeGit {
        fn run(&self, _: &Path, args: &[String]) -> io::Result<GitCommandOutput> {
            self.calls.lock().unwrap().push(args.to_vec());
            let stdout = match args.first().map(String::as_str) {
                Some("worktree") if args.get(1).map(String::as_str) == Some("list") => {
                    let calls = self.calls.lock().unwrap();
                    let Some(add) = calls.iter().find(|call| {
                        call.first().map(String::as_str) == Some("worktree")
                            && call.get(1).map(String::as_str) == Some("add")
                    }) else {
                        return Ok(GitCommandOutput {
                            status: Some(0),
                            stdout: Vec::new(),
                        });
                    };
                    let branch = add.get(3).cloned().unwrap_or_default();
                    let path = add.get(4).cloned().unwrap_or_default();
                    format!(
                        "worktree {path}\nHEAD abcdefabcdefabcdefabcdefabcdefabcdefabcd\nbranch refs/heads/{branch}\n"
                    )
                    .into_bytes()
                }
                Some("rev-parse") if args.get(1).map(String::as_str) == Some("--show-toplevel") => {
                    self.root.to_string_lossy().as_bytes().to_vec()
                }
                Some("rev-parse") if args.get(1).map(String::as_str) == Some("--verify") => {
                    b"0123456789012345678901234567890123456789\n".to_vec()
                }
                Some("rev-parse") => b"abcdefabcdefabcdefabcdefabcdefabcdefabcd\n".to_vec(),
                Some("status") => b" M src/lib.rs\0".to_vec(),
                Some("diff") if args.iter().any(|arg| arg == "--name-only") => {
                    b"src/lib.rs\0\n".to_vec()
                }
                Some("diff") => b"diff --git a/src/lib.rs b/src/lib.rs\n+change\n".to_vec(),
                _ => Vec::new(),
            };
            Ok(GitCommandOutput {
                status: Some(0),
                stdout,
            })
        }
    }

    fn scope() -> WorkPackageScope {
        WorkPackageScope {
            session_id: "session".into(),
            owner_profile_id: 1,
            workspace_key: "workspace".into(),
        }
    }

    fn request(root: &Path) -> WorkspaceRequest {
        WorkspaceRequest {
            scope: scope(),
            work_package_id: "wp_1".into(),
            attempt_id: "attempt_1".into(),
            canonical_workspace: root.into(),
            base_revision: "HEAD".into(),
            lease_epoch: 1,
        }
    }

    fn writer_capability() -> WorktreeCapability {
        WorktreeCapability {
            read_roots: vec!["src".into()],
            write_roots: vec!["src".into()],
            operation: WorktreeOperation::Write,
            lease_epoch: 1,
            collaboration_key: "workspace".into(),
        }
    }

    fn reviewer_capability() -> WorktreeCapability {
        WorktreeCapability {
            read_roots: vec!["src".into()],
            write_roots: Vec::new(),
            operation: WorktreeOperation::Read,
            lease_epoch: 1,
            collaboration_key: "workspace".into(),
        }
    }

    #[test]
    fn provider_contract_uses_isolated_branch_and_base_commit() {
        let canonical = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let fake = Arc::new(FakeGit {
            root: canonical.path().canonicalize().unwrap(),
            ..Default::default()
        });
        let provider = GitWorktreeProvider::with_runner(storage.path(), fake.clone()).unwrap();
        let handle = provider.prepare(&request(canonical.path())).unwrap();
        assert_eq!(handle.provider_kind, "git_worktree_v1");
        assert_eq!(handle.branch, "angelbot/wp_1/attempt_1");
        assert!(handle
            .worktree_path
            .starts_with(provider.root().join("active")));
        let calls = fake.calls.lock().unwrap();
        assert!(calls.iter().any(|call| call.starts_with(&[
            "worktree".into(),
            "add".into(),
            "-b".into()
        ])));
    }

    #[test]
    fn release_defaults_to_ephemeral_cleanup_and_repeated_release_is_idempotent() {
        let canonical = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let fake = Arc::new(FakeGit {
            root: canonical.path().canonicalize().unwrap(),
            ..Default::default()
        });
        let provider = GitWorktreeProvider::with_runner(storage.path(), fake.clone()).unwrap();
        let handle = provider.prepare(&request(canonical.path())).unwrap();
        fs::create_dir_all(&handle.worktree_path).unwrap();

        let first = provider
            .release(&handle, ReleaseDisposition::default())
            .unwrap();
        assert!(!first.branch_retained);
        assert!(!first.already_released);
        assert_eq!(
            provider.lifecycle_of(&handle.worktree_path).unwrap(),
            WorktreeLifecycle::Released
        );
        let call_count = fake.calls.lock().unwrap().len();

        let second = provider.cleanup(&handle).unwrap();
        assert!(second.already_released);
        assert!(!second.branch_retained);
        assert_eq!(fake.calls.lock().unwrap().len(), call_count);
    }

    #[test]
    fn branch_retention_is_rejected_before_cleanup() {
        let canonical = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let fake = Arc::new(FakeGit {
            root: canonical.path().canonicalize().unwrap(),
            ..Default::default()
        });
        let provider = GitWorktreeProvider::with_runner(storage.path(), fake.clone()).unwrap();
        let handle = provider.prepare(&request(canonical.path())).unwrap();
        fs::create_dir_all(&handle.worktree_path).unwrap();

        assert_eq!(
            provider.release(&handle, ReleaseDisposition::RetainBranch),
            Err(WorkspaceError::RetentionForbidden)
        );
        assert_eq!(
            provider.lifecycle_of(&handle.worktree_path).unwrap(),
            WorktreeLifecycle::Prepared
        );
    }

    #[test]
    fn capture_returns_commit_backed_artifact_and_rejects_self_review() {
        let canonical = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let fake = Arc::new(FakeGit {
            root: canonical.path().canonicalize().unwrap(),
            ..Default::default()
        });
        let provider = GitWorktreeProvider::with_runner(storage.path(), fake).unwrap();
        let handle = provider.prepare(&request(canonical.path())).unwrap();
        fs::create_dir_all(&handle.worktree_path).unwrap();
        let access = provider.grant_access(&handle, writer_capability()).unwrap();
        let artifact = provider
            .capture(
                &handle,
                &access,
                &ReviewCaptureRequest {
                    change_set_id: Some("change_1".into()),
                    allowed_relative_paths: vec!["src/lib.rs".into()],
                    scope_digest: "scope-1".into(),
                    commit_message: "Implement change".into(),
                    created_at: 1,
                },
            )
            .unwrap();
        assert_eq!(artifact.implementation_attempt_id, "attempt_1");
        assert_eq!(artifact.changed_paths, vec!["src/lib.rs"]);
        assert_eq!(artifact.diff_hash.len(), 64);
        assert!(artifact.audit_ref.starts_with("audit://"));
        assert!(!artifact
            .audit_ref
            .contains(&canonical.path().to_string_lossy().to_string()));
        let review = ReviewEvidence {
            verifier_attempt_id: "attempt_1".into(),
            verdict: ReviewVerdict::Passed,
            reviewed_commit_sha: artifact.commit_sha.clone(),
            reviewed_diff_hash: artifact.diff_hash.clone(),
            evidence_refs: vec!["record://review/1".into()],
            reviewed_at: 2,
        };
        assert_eq!(
            artifact.bind_review(review),
            Err(ReviewArtifactError::ReviewAttemptMatchesImplementation)
        );
    }

    #[test]
    fn materialization_requires_change_set_and_independent_passed_review() {
        let artifact = ReviewArtifact {
            schema_version: 1,
            id: "review_1".into(),
            work_package_id: "wp_1".into(),
            change_set_id: None,
            implementation_attempt_id: "impl_1".into(),
            scope_digest: "scope-1".into(),
            base_revision: "HEAD".into(),
            base_commit: "0123456789012345678901234567890123456789".into(),
            branch: "angelbot/wp_1/impl_1".into(),
            audit_ref: "audit://wp_1/impl_1/abcdefabcdefabcdefabcdefabcdefabcdefabcd".into(),
            commit_sha: "abcdefabcdefabcdefabcdefabcdefabcdefabcd".into(),
            diff_hash: "012345678901234567890123456789012345678901234567890123456789abcd".into(),
            changed_paths: vec!["src/lib.rs".into()],
            allowed_relative_paths: vec!["src/lib.rs".into()],
            created_at: 1,
            review: None,
        };
        assert_eq!(
            artifact.ready_for_materialization(),
            Err(ReviewArtifactError::MissingChangeSet)
        );
    }

    #[test]
    fn capability_epoch_and_roots_are_main_agent_bound_and_cannot_drift() {
        let canonical = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let fake = Arc::new(FakeGit {
            root: canonical.path().canonicalize().unwrap(),
            ..Default::default()
        });
        let provider = GitWorktreeProvider::with_runner(storage.path(), fake).unwrap();
        let handle = provider.prepare(&request(canonical.path())).unwrap();

        let mut wrong_epoch = writer_capability();
        wrong_epoch.lease_epoch = 2;
        assert_eq!(
            provider.grant_access(&handle, wrong_epoch),
            Err(WorkspaceError::LeaseEpochMismatch)
        );
        provider.grant_access(&handle, writer_capability()).unwrap();

        let mut widened = writer_capability();
        widened.write_roots.push("docs".into());
        assert_eq!(
            provider.grant_access(&handle, widened),
            Err(WorkspaceError::PermissionDrift)
        );
    }

    #[test]
    fn candidate_writers_are_serial_but_readonly_reviewers_can_parallel() {
        let canonical = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let fake = Arc::new(FakeGit {
            root: canonical.path().canonicalize().unwrap(),
            ..Default::default()
        });
        let provider = GitWorktreeProvider::with_runner(storage.path(), fake).unwrap();
        let first = provider.prepare(&request(canonical.path())).unwrap();
        let mut second_request = request(canonical.path());
        second_request.attempt_id = "attempt_2".into();
        let second = provider.prepare(&second_request).unwrap();
        provider.grant_access(&first, writer_capability()).unwrap();
        assert_eq!(
            provider.grant_access(&second, writer_capability()),
            Err(WorkspaceError::CollaborationConflict)
        );
        let reviewer = provider
            .grant_access(&second, reviewer_capability())
            .unwrap();
        assert!(provider
            .authorize(&second, &reviewer, WorktreeOperation::Read)
            .is_ok());
        assert_eq!(
            provider.authorize(&second, &reviewer, WorktreeOperation::Write),
            Err(WorkspaceError::AccessDenied)
        );
    }

    #[test]
    fn forged_access_handle_is_rejected() {
        let canonical = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let fake = Arc::new(FakeGit {
            root: canonical.path().canonicalize().unwrap(),
            ..Default::default()
        });
        let provider = GitWorktreeProvider::with_runner(storage.path(), fake).unwrap();
        let handle = provider.prepare(&request(canonical.path())).unwrap();
        let access = provider
            .grant_access(&handle, reviewer_capability())
            .unwrap();
        let forged = WorktreeAccess {
            handle_id: "worktree_handle_forged".into(),
            capability: access.capability.clone(),
            identity: None,
        };
        assert_eq!(
            provider.authorize(&handle, &forged, WorktreeOperation::Read),
            Err(WorkspaceError::AccessDenied)
        );
    }

    #[test]
    fn worktree_manifest_persists_and_resolves_after_provider_restart() {
        let canonical = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let fake = Arc::new(FakeGit {
            root: canonical.path().canonicalize().unwrap(),
            ..Default::default()
        });
        let provider = GitWorktreeProvider::with_runner(storage.path(), fake.clone()).unwrap();
        let handle = provider.prepare(&request(canonical.path())).unwrap();
        fs::create_dir_all(&handle.worktree_path).unwrap();
        let identity = WorktreeManifestIdentity::for_handle(&handle, "a".repeat(64)).unwrap();
        provider.persist_manifest(&handle, &identity).unwrap();
        assert!(provider
            .manifest_root()
            .read_dir()
            .unwrap()
            .next()
            .is_some());
        let restarted = GitWorktreeProvider::with_runner(storage.path(), fake).unwrap();
        let verified = restarted.resolve_manifest(&identity).unwrap().unwrap();
        assert_eq!(verified.identity(), &identity);
        let receipt = restarted.release_verified(&verified).unwrap();
        assert!(!receipt.already_released);
        assert!(
            restarted
                .release_verified(&verified)
                .unwrap()
                .already_released
        );
    }

    #[test]
    fn worktree_manifest_tamper_or_escape_is_rejected_without_cleanup() {
        let canonical = tempdir().unwrap();
        let storage = tempdir().unwrap();
        let fake = Arc::new(FakeGit {
            root: canonical.path().canonicalize().unwrap(),
            ..Default::default()
        });
        let provider = GitWorktreeProvider::with_runner(storage.path(), fake).unwrap();
        let handle = provider.prepare(&request(canonical.path())).unwrap();
        fs::create_dir_all(&handle.worktree_path).unwrap();
        let identity = WorktreeManifestIdentity::for_handle(&handle, "b".repeat(64)).unwrap();
        provider.persist_manifest(&handle, &identity).unwrap();
        let manifest = provider
            .manifest_root()
            .join(format!("{}.json", identity.manifest_locator));
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        value["worktree_path"] = serde_json::Value::String("..\\escape".into());
        fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(
            provider.resolve_manifest(&identity),
            Err(WorkspaceError::ProviderUnavailable)
        );
        assert!(manifest.exists());
    }
}

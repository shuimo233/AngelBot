//! The sole canonical-workspace writer for reviewed delegated changes.
//!
//! The interface is deliberately small: a trusted scope, a parent confirmation,
//! an immutable review artifact, and an observed base revision.  All path
//! resolution, durable binding checks, clean-tree checks, cherry-pick handling,
//! and idempotent receipts live behind this seam.  No worker, model, or Tauri
//! command receives a canonical path or a Git handle.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rusqlite::{params, Connection, OptionalExtension};

use super::{
    review_artifact_store::{ReviewArtifactStore, ReviewArtifactStoreError},
    shared_db::SharedDb,
    work_package::{ChangeSet, WorkPackageError, WorkPackageRepository, WorkPackageScope},
    workspace_isolation::{
        GitCommandRunner, ReviewArtifact, WorktreeManifestIdentity, WorktreeManifestProvider,
    },
};

const MAX_CHANGED_PATHS: usize = 128;
const MAX_PATH_BYTES: usize = 512;

/// A trusted application-owned resolver.  It maps a durable workspace scope to
/// a canonical checkout; it never accepts a path from a model or child worker.
pub trait CanonicalWorkspaceLocator: Send + Sync {
    fn locate(&self, scope: &WorkPackageScope) -> Result<PathBuf, MaterializationError>;
}

/// A per-change-set locator backed by the temporary worktree's durable
/// manifest.  It deliberately does not query a path from a model, a tool, or
/// a broadly-scoped workspace registry: the exact reviewed worktree identity
/// must resolve again immediately before the canonical Git operation.
pub struct ManifestCanonicalWorkspaceLocator<P> {
    provider: Arc<P>,
    expected_scope: WorkPackageScope,
    identity: WorktreeManifestIdentity,
}

impl<P> ManifestCanonicalWorkspaceLocator<P> {
    pub fn new(
        provider: Arc<P>,
        expected_scope: WorkPackageScope,
        identity: WorktreeManifestIdentity,
    ) -> Self {
        Self {
            provider,
            expected_scope,
            identity,
        }
    }
}

impl<P> CanonicalWorkspaceLocator for ManifestCanonicalWorkspaceLocator<P>
where
    P: WorktreeManifestProvider + Send + Sync + 'static,
{
    fn locate(&self, scope: &WorkPackageScope) -> Result<PathBuf, MaterializationError> {
        if scope != &self.expected_scope {
            return Err(MaterializationError::BindingInvalid);
        }
        let verified = self
            .provider
            .resolve_manifest(&self.identity)
            .map_err(|_| MaterializationError::WorkspaceUnavailable)?
            .ok_or(MaterializationError::WorkspaceUnavailable)?;
        self.provider
            .canonical_workspace_verified(&verified)
            .map_err(|_| MaterializationError::WorkspaceUnavailable)
    }
}

impl<F> CanonicalWorkspaceLocator for F
where
    F: Fn(&WorkPackageScope) -> Result<PathBuf, MaterializationError> + Send + Sync,
{
    fn locate(&self, scope: &WorkPackageScope) -> Result<PathBuf, MaterializationError> {
        self(scope)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializeRequest {
    pub scope: WorkPackageScope,
    pub confirmation_id: String,
    pub artifact: ReviewArtifact,
    pub observed_base_revision: String,
    pub now: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterializationStatus {
    Applying,
    Applied,
    Refused,
    Unknown,
}

impl MaterializationStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Applying => "applying",
            Self::Applied => "applied",
            Self::Refused => "refused",
            Self::Unknown => "unknown",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "applying" => Some(Self::Applying),
            "applied" => Some(Self::Applied),
            "refused" => Some(Self::Refused),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializationReceipt {
    pub id: String,
    pub confirmation_id: String,
    pub change_set_id: String,
    pub work_package_id: String,
    pub scope_digest: String,
    pub artifact_id: String,
    pub base_commit: String,
    pub reviewed_commit_sha: String,
    pub reviewed_diff_hash: String,
    pub status: MaterializationStatus,
    pub error_class: Option<String>,
    pub applied_at: Option<i64>,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum MaterializationError {
    #[error("invalid materialization request")]
    InvalidRequest,
    #[error("review artifact is not ready")]
    ArtifactNotReady,
    #[error("confirmation or change-set binding is invalid")]
    BindingInvalid,
    #[error("materialization receipt is already terminal")]
    AlreadyTerminal,
    #[error("materialization effect is indeterminate")]
    Indeterminate,
    #[error("canonical workspace is unavailable")]
    WorkspaceUnavailable,
    #[error("canonical workspace has drifted")]
    WorkspaceDrift,
    #[error("canonical workspace is dirty")]
    WorkspaceDirty,
    #[error("Git operation failed")]
    GitFailed,
    #[error("database error")]
    Storage,
}

/// Materializer implementation.  The Git runner and locator are adapters at
/// the seam; the state machine and safety checks remain centralized here.
pub struct Materializer<R, L> {
    db: SharedDb,
    git: Arc<R>,
    locator: Arc<L>,
}

impl<R, L> Materializer<R, L>
where
    R: GitCommandRunner + 'static,
    L: CanonicalWorkspaceLocator + 'static,
{
    pub fn new(db: SharedDb, git: Arc<R>, locator: Arc<L>) -> Self {
        Self { db, git, locator }
    }

    pub fn materialize(
        &self,
        request: MaterializeRequest,
    ) -> Result<MaterializationReceipt, MaterializationError> {
        validate_request(&request)?;
        request
            .artifact
            .ready_for_materialization()
            .map_err(|_| MaterializationError::ArtifactNotReady)?;
        let change_set_id = request
            .artifact
            .change_set_id
            .as_deref()
            .ok_or(MaterializationError::ArtifactNotReady)?;

        if let Some(existing) = self.load_receipt(&request.confirmation_id)? {
            return match existing.status {
                MaterializationStatus::Applied => Ok(existing),
                MaterializationStatus::Applying | MaterializationStatus::Unknown => {
                    Err(MaterializationError::Indeterminate)
                }
                MaterializationStatus::Refused => Err(MaterializationError::AlreadyTerminal),
            };
        }

        let change = self.approve_binding(&request, change_set_id)?;
        if change.id != change_set_id
            || change.package_id != request.artifact.work_package_id
            || change.scope_digest != request.artifact.scope_digest
            || change.base_revision != request.observed_base_revision
        {
            return Err(MaterializationError::BindingInvalid);
        }

        let canonical = self
            .locator
            .locate(&request.scope)
            .map_err(|_| MaterializationError::WorkspaceUnavailable)?;
        let canonical = canonicalize_git_root(&canonical)?;
        let head = self.git_output(&canonical, vec!["rev-parse".into(), "HEAD".into()])?;
        let head = text_line(&head).ok_or(MaterializationError::WorkspaceDrift)?;
        if head != request.artifact.base_commit {
            return self.refuse(&request, "base_commit_drift");
        }
        let status = self.git_output(
            &canonical,
            vec![
                "status".into(),
                "--porcelain=v1".into(),
                "--untracked-files=all".into(),
                "-z".into(),
            ],
        )?;
        if !status.is_empty() {
            return self.refuse(&request, "canonical_workspace_dirty");
        }

        let receipt = self.begin_receipt(&request, change_set_id)?;
        let cherry_pick = self.git.run(
            &canonical,
            &[
                "cherry-pick".into(),
                "--no-edit".into(),
                request.artifact.commit_sha.clone(),
            ],
        );
        let Some(output) = cherry_pick.ok() else {
            return self.unknown(&receipt, &request, "cherry_pick_transport");
        };
        if output.status != Some(0) {
            let abort = self
                .git
                .run(&canonical, &["cherry-pick".into(), "--abort".into()]);
            if abort.as_ref().is_err() || abort.as_ref().is_ok_and(|value| value.status != Some(0))
            {
                return self.unknown(&receipt, &request, "cherry_pick_abort_unknown");
            }
            return self.refuse(&request, "cherry_pick_conflict");
        }

        let applied_head = self.git_output(&canonical, vec!["rev-parse".into(), "HEAD".into()])?;
        let Some(applied_head) = text_line(&applied_head) else {
            return self.unknown(&receipt, &request, "post_apply_head_unavailable");
        };
        // `cherry-pick` deliberately creates a new commit, so its SHA must
        // not equal the implementation worktree commit. What matters is that
        // the committed tree is identical to the independently reviewed
        // artifact. A mismatch is an indeterminate effect: never replay it.
        let tree_match = self.git.run(
            &canonical,
            &[
                "diff".into(),
                "--quiet".into(),
                request.artifact.commit_sha.clone(),
                applied_head,
            ],
        );
        if tree_match
            .as_ref()
            .ok()
            .is_none_or(|output| output.status != Some(0))
        {
            return self.unknown(&receipt, &request, "post_apply_tree_mismatch");
        }
        self.update_receipt(
            &receipt.id,
            MaterializationStatus::Applied,
            None,
            request.now,
            Some(request.now),
        )
    }

    /// Restart-safe entry point. The caller supplies only an opaque artifact
    /// id; the durable store reconstructs the exact reviewed commit facts.
    pub fn materialize_stored(
        &self,
        store: &ReviewArtifactStore,
        scope: WorkPackageScope,
        confirmation_id: String,
        artifact_id: &str,
        observed_base_revision: String,
        now: i64,
    ) -> Result<MaterializationReceipt, MaterializationError> {
        let artifact = store.load_ready(artifact_id).map_err(|error| match error {
            ReviewArtifactStoreError::NotFound => MaterializationError::ArtifactNotReady,
            ReviewArtifactStoreError::ReviewInvalid => MaterializationError::ArtifactNotReady,
            _ => MaterializationError::Storage,
        })?;
        self.materialize(MaterializeRequest {
            scope,
            confirmation_id,
            artifact,
            observed_base_revision,
            now,
        })
    }

    fn approve_binding(
        &self,
        request: &MaterializeRequest,
        change_set_id: &str,
    ) -> Result<ChangeSet, MaterializationError> {
        self.db
            .with_conn_mut(|conn| {
                WorkPackageRepository::approved_for_materialization(
                    conn,
                    &request.scope,
                    &request.confirmation_id,
                    &request.observed_base_revision,
                    request.now,
                )
                .map_err(|error| match error {
                    WorkPackageError::BindingInvalid | WorkPackageError::NotFound => {
                        MaterializationError::BindingInvalid
                    }
                    _ => MaterializationError::Storage,
                })
                .and_then(|change| {
                    if change.id == change_set_id {
                        Ok(change)
                    } else {
                        Err(MaterializationError::BindingInvalid)
                    }
                })
            })
            .map_err(|_| MaterializationError::Storage)?
    }

    fn load_receipt(
        &self,
        confirmation_id: &str,
    ) -> Result<Option<MaterializationReceipt>, MaterializationError> {
        self.db
            .with_conn(|conn| load_receipt(conn, confirmation_id))
            .map_err(|_| MaterializationError::Storage)?
            .map_err(|_| MaterializationError::Storage)
    }

    fn begin_receipt(
        &self,
        request: &MaterializeRequest,
        change_set_id: &str,
    ) -> Result<MaterializationReceipt, MaterializationError> {
        let id = format!("mat_{}", uuid::Uuid::new_v4().simple());
        let paths = serde_json::to_string(&request.artifact.changed_paths)
            .map_err(|_| MaterializationError::InvalidRequest)?;
        self.db
            .with_conn_mut(|conn| {
                conn.execute(
                    "INSERT INTO delegation_materialization_receipts
                     (id,confirmation_id,change_set_id,work_package_id,scope_digest,artifact_id,
                      base_commit,reviewed_commit_sha,reviewed_diff_hash,changed_paths_json,status,
                      created_at,updated_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'applying',?11,?11)",
                    params![
                        id,
                        request.confirmation_id,
                        change_set_id,
                        request.artifact.work_package_id,
                        request.artifact.scope_digest,
                        request.artifact.id,
                        request.artifact.base_commit,
                        request.artifact.commit_sha,
                        request.artifact.diff_hash,
                        paths,
                        request.now,
                    ],
                )
                .map_err(|_| MaterializationError::Storage)?;
                load_receipt(conn, &request.confirmation_id)
                    .map_err(|_| MaterializationError::Storage)?
                    .ok_or(MaterializationError::Storage)
            })
            .map_err(|_| MaterializationError::Storage)?
    }

    fn refuse(
        &self,
        request: &MaterializeRequest,
        reason: &str,
    ) -> Result<MaterializationReceipt, MaterializationError> {
        // Refusal before a Git effect is safe to record and invalidates the
        // confirmation so a stale approval cannot be retried silently.
        let receipt = self.begin_receipt(
            request,
            request
                .artifact
                .change_set_id
                .as_deref()
                .ok_or(MaterializationError::ArtifactNotReady)?,
        )?;
        let _ = self.update_receipt(
            &receipt.id,
            MaterializationStatus::Refused,
            Some(reason),
            request.now,
            None,
        )?;
        self.db
            .with_conn_mut(|conn| {
                conn.execute(
                    "UPDATE work_package_confirmations
                     SET status='invalidated',updated_at=?1
                     WHERE id=?2 AND status='approved'",
                    params![request.now, request.confirmation_id],
                )
                .map_err(|_| MaterializationError::Storage)
            })
            .map_err(|_| MaterializationError::Storage)??;
        Err(match reason {
            "canonical_workspace_dirty" => MaterializationError::WorkspaceDirty,
            "base_commit_drift" => MaterializationError::WorkspaceDrift,
            _ => MaterializationError::BindingInvalid,
        })
    }

    fn unknown(
        &self,
        receipt: &MaterializationReceipt,
        _request: &MaterializeRequest,
        reason: &str,
    ) -> Result<MaterializationReceipt, MaterializationError> {
        let _ = self.update_receipt(
            &receipt.id,
            MaterializationStatus::Unknown,
            Some(reason),
            _request.now,
            None,
        )?;
        Err(MaterializationError::Indeterminate)
    }

    fn update_receipt(
        &self,
        id: &str,
        status: MaterializationStatus,
        error_class: Option<&str>,
        updated_at: i64,
        applied_at: Option<i64>,
    ) -> Result<MaterializationReceipt, MaterializationError> {
        self.db
            .with_conn_mut(|conn| {
                conn.execute(
                    "UPDATE delegation_materialization_receipts
                     SET status=?1,error_class=?2,updated_at=?3,applied_at=?4
                     WHERE id=?5 AND status IN ('applying','refused','unknown')",
                    params![status.as_str(), error_class, updated_at, applied_at, id],
                )
                .map_err(|_| MaterializationError::Storage)?;
                load_receipt_by_id(conn, id)
                    .map_err(|_| MaterializationError::Storage)?
                    .ok_or(MaterializationError::Storage)
            })
            .map_err(|_| MaterializationError::Storage)?
    }

    fn git_output(&self, cwd: &Path, args: Vec<String>) -> Result<Vec<u8>, MaterializationError> {
        let output = self
            .git
            .run(cwd, &args)
            .map_err(|_| MaterializationError::GitFailed)?;
        if output.status != Some(0) {
            return Err(MaterializationError::GitFailed);
        }
        Ok(output.stdout)
    }
}

fn validate_request(request: &MaterializeRequest) -> Result<(), MaterializationError> {
    if request.confirmation_id.is_empty()
        || request.confirmation_id.len() > 128
        || request
            .confirmation_id
            .contains(['/', '\\', '\0', '\r', '\n'])
        || request.observed_base_revision.is_empty()
        || request.artifact.base_revision != request.observed_base_revision
        || request.artifact.changed_paths.len() > MAX_CHANGED_PATHS
        || request.artifact.changed_paths.iter().any(|path| {
            path.is_empty()
                || path.len() > MAX_PATH_BYTES
                || path.contains(['\0', '\r', '\n'])
                || path.starts_with('/')
                || path.starts_with('\\')
                || path.split(['/', '\\']).any(|part| part == "..")
        })
    {
        return Err(MaterializationError::InvalidRequest);
    }
    if !valid_commit_text(&request.artifact.base_commit)
        || !valid_commit_text(&request.artifact.commit_sha)
        || request.artifact.diff_hash.is_empty()
    {
        return Err(MaterializationError::InvalidRequest);
    }
    Ok(())
}

fn valid_commit_text(value: &str) -> bool {
    (40..=128).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn canonicalize_git_root(path: &Path) -> Result<PathBuf, MaterializationError> {
    let canonical =
        std::fs::canonicalize(path).map_err(|_| MaterializationError::WorkspaceUnavailable)?;
    if !canonical.is_dir() {
        return Err(MaterializationError::WorkspaceUnavailable);
    }
    Ok(canonical)
}

fn text_line(bytes: &[u8]) -> Option<String> {
    std::str::from_utf8(bytes)
        .ok()?
        .lines()
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn load_receipt(
    conn: &Connection,
    confirmation_id: &str,
) -> Result<Option<MaterializationReceipt>, rusqlite::Error> {
    conn.query_row(
        "SELECT id,confirmation_id,change_set_id,work_package_id,scope_digest,artifact_id,
                base_commit,reviewed_commit_sha,reviewed_diff_hash,status,error_class,applied_at
         FROM delegation_materialization_receipts WHERE confirmation_id=?1",
        [confirmation_id],
        map_receipt,
    )
    .optional()
}

fn load_receipt_by_id(
    conn: &Connection,
    id: &str,
) -> Result<Option<MaterializationReceipt>, rusqlite::Error> {
    conn.query_row(
        "SELECT id,confirmation_id,change_set_id,work_package_id,scope_digest,artifact_id,
                base_commit,reviewed_commit_sha,reviewed_diff_hash,status,error_class,applied_at
         FROM delegation_materialization_receipts WHERE id=?1",
        [id],
        map_receipt,
    )
    .optional()
}

fn map_receipt(row: &rusqlite::Row<'_>) -> Result<MaterializationReceipt, rusqlite::Error> {
    let status: String = row.get(9)?;
    Ok(MaterializationReceipt {
        id: row.get(0)?,
        confirmation_id: row.get(1)?,
        change_set_id: row.get(2)?,
        work_package_id: row.get(3)?,
        scope_digest: row.get(4)?,
        artifact_id: row.get(5)?,
        base_commit: row.get(6)?,
        reviewed_commit_sha: row.get(7)?,
        reviewed_diff_hash: row.get(8)?,
        status: MaterializationStatus::from_str(&status)
            .ok_or_else(|| rusqlite::Error::InvalidQuery)?,
        error_class: row.get(10)?,
        applied_at: row.get(11)?,
    })
}

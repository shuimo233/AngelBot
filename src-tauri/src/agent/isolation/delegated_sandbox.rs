//! Filesystem lifecycle for a single delegated-agent sandbox.
//!
//! A sandbox is never the user's workspace. It is an app-managed direct child
//! of this module's configured root and is bound to one delegation attempt by
//! an unguessable manifest nonce. This is deliberately a standalone primitive:
//! it does not dispatch agents, grant capabilities, or apply candidate changes.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const MANIFEST_FILE: &str = ".angelbot-delegated-sandbox.json";
const ACTIVE_DIR: &str = "active";
const QUARANTINE_DIR: &str = "quarantine";

#[derive(Debug, Clone)]
pub struct DelegatedSandboxLifecycle {
    root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxRef {
    pub delegation_id: String,
    pub attempt_id: String,
    pub nonce: String,
    pub path: PathBuf,
}

/// Provider-owned manifest tuple used for restart-safe cleanup.  It contains
/// no filesystem path; the lifecycle resolves the tuple against its own
/// managed root and returns an opaque verified handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxManifestIdentity {
    pub delegation_id: String,
    pub attempt_id: String,
    pub scope_digest: String,
    pub lease_epoch: u64,
    pub manifest_locator: String,
    pub manifest_nonce: String,
    pub manifest_digest: String,
}

impl SandboxManifestIdentity {
    pub fn for_bound(
        delegation_id: impl Into<String>,
        attempt_id: impl Into<String>,
        scope_digest: impl Into<String>,
        lease_epoch: u64,
        manifest_nonce: impl Into<String>,
    ) -> Result<Self, SandboxError> {
        let delegation_id = delegation_id.into();
        let attempt_id = attempt_id.into();
        let scope_digest = scope_digest.into();
        let manifest_nonce = manifest_nonce.into();
        validate_identity(&delegation_id)?;
        validate_identity(&attempt_id)?;
        if scope_digest.is_empty()
            || scope_digest.len() > 256
            || scope_digest.contains(['/', '\\', '\0', '\r', '\n'])
            || lease_epoch == 0
            || !valid_nonce(&manifest_nonce)
        {
            return Err(SandboxError::InvalidIdentity);
        }
        let manifest_locator = format!("manifest_{manifest_nonce}");
        let manifest_digest = sandbox_manifest_digest(
            &delegation_id,
            &attempt_id,
            &scope_digest,
            lease_epoch,
            &manifest_nonce,
        );
        Ok(Self {
            delegation_id,
            attempt_id,
            scope_digest,
            lease_epoch,
            manifest_locator,
            manifest_nonce,
            manifest_digest,
        })
    }

    /// Construct a tuple loaded from durable storage.  The provider compares
    /// the digest with its own manifest before returning a verified handle.
    pub fn new(
        delegation_id: impl Into<String>,
        attempt_id: impl Into<String>,
        scope_digest: impl Into<String>,
        lease_epoch: u64,
        manifest_locator: impl Into<String>,
        manifest_nonce: impl Into<String>,
        manifest_digest: impl Into<String>,
    ) -> Result<Self, SandboxError> {
        let identity = Self {
            delegation_id: delegation_id.into(),
            attempt_id: attempt_id.into(),
            scope_digest: scope_digest.into(),
            lease_epoch,
            manifest_locator: manifest_locator.into(),
            manifest_nonce: manifest_nonce.into(),
            manifest_digest: manifest_digest.into(),
        };
        validate_identity(&identity.delegation_id)?;
        validate_identity(&identity.attempt_id)?;
        if identity.scope_digest.is_empty()
            || identity.scope_digest.len() > 256
            || identity
                .scope_digest
                .contains(['/', '\\', '\0', '\r', '\n'])
            || identity.lease_epoch == 0
            || !valid_nonce(&identity.manifest_nonce)
            || identity.manifest_locator != format!("manifest_{}", identity.manifest_nonce)
            || identity.manifest_digest.is_empty()
            || identity.manifest_digest.len() > 256
            || identity
                .manifest_digest
                .contains(['/', '\\', '\0', '\r', '\n'])
        {
            return Err(SandboxError::InvalidIdentity);
        }
        Ok(identity)
    }
}

/// A manifest-validated sandbox capability.  The provider-owned `SandboxRef`
/// (and therefore its path) is intentionally private to this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSandboxRef {
    identity: SandboxManifestIdentity,
    sandbox: SandboxRef,
    state: SandboxState,
}

impl VerifiedSandboxRef {
    pub fn identity(&self) -> &SandboxManifestIdentity {
        &self.identity
    }

    pub fn state(&self) -> SandboxState {
        self.state
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateArtifact {
    /// Path relative to this sandbox. This is only a proposal for the main
    /// agent; this module never copies it into the real workspace.
    pub relative_path: PathBuf,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxState {
    Active,
    Sealed,
    Revoked,
    Quarantined,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxError {
    InvalidIdentity,
    InvalidCandidatePath,
    OutsideManagedRoot(PathBuf),
    ManifestMissing,
    ManifestInvalid,
    ManifestDoesNotMatch,
    InvalidState {
        expected: SandboxState,
        actual: SandboxState,
    },
    Io(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupOutcome {
    Cleaned,
    /// A caller may persist this and retry it later. Background scheduling is
    /// intentionally outside this primitive.
    Retry(CleanupRetry),
    Refused(SandboxError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupRetry {
    pub kind: CleanupRetryKind,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupRetryKind {
    FileLockedOrPermissionDenied,
    TransientIo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    version: u8,
    delegation_id: String,
    attempt_id: String,
    nonce: String,
    state: SandboxState,
    #[serde(default)]
    scope_digest: Option<String>,
    #[serde(default)]
    lease_epoch: Option<u64>,
    #[serde(default)]
    manifest_locator: Option<String>,
    #[serde(default)]
    manifest_digest: Option<String>,
}

impl DelegatedSandboxLifecycle {
    /// `root` is normally an app-data directory selected by the main agent.
    /// It is created once and canonicalized before any attempt is allocated.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, SandboxError> {
        fs::create_dir_all(root.as_ref()).map_err(io_error)?;
        let root = fs::canonicalize(root.as_ref()).map_err(io_error)?;
        fs::create_dir_all(root.join(ACTIVE_DIR)).map_err(io_error)?;
        fs::create_dir_all(root.join(QUARANTINE_DIR)).map_err(io_error)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn create(
        &self,
        delegation_id: &str,
        attempt_id: &str,
    ) -> Result<SandboxRef, SandboxError> {
        validate_identity(delegation_id)?;
        validate_identity(attempt_id)?;
        let nonce = Uuid::new_v4().to_string();
        let leaf = format!("{attempt_id}-{nonce}");
        let path = self.root.join(ACTIVE_DIR).join(leaf);
        fs::create_dir(&path).map_err(io_error)?;
        let manifest = Manifest {
            version: 1,
            delegation_id: delegation_id.into(),
            attempt_id: attempt_id.into(),
            nonce: nonce.clone(),
            state: SandboxState::Active,
            scope_digest: None,
            lease_epoch: None,
            manifest_locator: None,
            manifest_digest: None,
        };
        if let Err(error) = write_manifest(&path, &manifest) {
            let _ = fs::remove_dir(&path);
            return Err(error);
        }
        Ok(SandboxRef {
            delegation_id: delegation_id.into(),
            attempt_id: attempt_id.into(),
            nonce,
            path,
        })
    }

    /// Allocate a sandbox with a provider-owned manifest tuple.  The returned
    /// handle is opaque; callers cannot extract or supply its path.
    pub fn create_bound(
        &self,
        delegation_id: &str,
        attempt_id: &str,
        scope_digest: &str,
        lease_epoch: u64,
    ) -> Result<VerifiedSandboxRef, SandboxError> {
        validate_identity(delegation_id)?;
        validate_identity(attempt_id)?;
        if scope_digest.is_empty() || lease_epoch == 0 {
            return Err(SandboxError::InvalidIdentity);
        }
        let nonce = Uuid::new_v4().to_string();
        let identity = SandboxManifestIdentity::for_bound(
            delegation_id,
            attempt_id,
            scope_digest,
            lease_epoch,
            &nonce,
        )?;
        let leaf = format!("{attempt_id}-{nonce}");
        let path = self.root.join(ACTIVE_DIR).join(leaf);
        fs::create_dir(&path).map_err(io_error)?;
        let manifest = Manifest {
            version: 1,
            delegation_id: delegation_id.into(),
            attempt_id: attempt_id.into(),
            nonce,
            state: SandboxState::Active,
            scope_digest: Some(identity.scope_digest.clone()),
            lease_epoch: Some(identity.lease_epoch),
            manifest_locator: Some(identity.manifest_locator.clone()),
            manifest_digest: Some(identity.manifest_digest.clone()),
        };
        if let Err(error) = write_manifest(&path, &manifest) {
            let _ = fs::remove_dir(&path);
            return Err(error);
        }
        let sandbox = SandboxRef {
            delegation_id: delegation_id.into(),
            attempt_id: attempt_id.into(),
            nonce: identity.manifest_nonce.clone(),
            path,
        };
        Ok(VerifiedSandboxRef {
            identity,
            sandbox,
            state: SandboxState::Active,
        })
    }

    /// Resolve only a direct child whose complete provider-owned manifest
    /// matches the supplied identity tuple. Unknown or forged residuals are
    /// ignored and therefore retained rather than guessed or deleted.
    pub fn resolve_manifest(
        &self,
        expected: &SandboxManifestIdentity,
    ) -> Result<Option<VerifiedSandboxRef>, SandboxError> {
        let expected_digest = sandbox_manifest_digest(
            &expected.delegation_id,
            &expected.attempt_id,
            &expected.scope_digest,
            expected.lease_epoch,
            &expected.manifest_nonce,
        );
        if expected.manifest_digest != expected_digest {
            return Ok(None);
        }
        let expected_leaf = format!("{}-{}", expected.attempt_id, expected.manifest_nonce);
        for container in [ACTIVE_DIR, QUARANTINE_DIR] {
            let directory = self.root.join(container);
            let canonical_parent = fs::canonicalize(&directory).map_err(io_error)?;
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_symlink() || !file_type.is_dir() {
                    continue;
                }
                if path.file_name().and_then(|name| name.to_str()) != Some(expected_leaf.as_str()) {
                    continue;
                }
                let Ok(manifest) = read_manifest(&path) else {
                    continue;
                };
                let state_allowed = if container == ACTIVE_DIR {
                    matches!(
                        manifest.state,
                        SandboxState::Active | SandboxState::Sealed | SandboxState::Revoked
                    )
                } else {
                    manifest.state == SandboxState::Quarantined
                };
                if manifest.version != 1
                    || !state_allowed
                    || manifest.delegation_id != expected.delegation_id
                    || manifest.attempt_id != expected.attempt_id
                    || manifest.nonce != expected.manifest_nonce
                    || manifest.scope_digest.as_deref() != Some(expected.scope_digest.as_str())
                    || manifest.lease_epoch != Some(expected.lease_epoch)
                    || manifest.manifest_locator.as_deref()
                        != Some(expected.manifest_locator.as_str())
                    || manifest.manifest_digest.as_deref()
                        != Some(expected.manifest_digest.as_str())
                {
                    continue;
                }
                let canonical = match fs::canonicalize(&path) {
                    Ok(path) if path.parent() == Some(canonical_parent.as_path()) => path,
                    _ => continue,
                };
                return Ok(Some(VerifiedSandboxRef {
                    identity: expected.clone(),
                    sandbox: SandboxRef {
                        delegation_id: manifest.delegation_id,
                        attempt_id: manifest.attempt_id,
                        nonce: manifest.nonce,
                        path: canonical,
                    },
                    state: manifest.state,
                }));
            }
        }
        Ok(None)
    }

    pub fn seal_verified(
        &self,
        verified: &VerifiedSandboxRef,
    ) -> Result<VerifiedSandboxRef, SandboxError> {
        let Some(current) = self.resolve_manifest(&verified.identity)? else {
            return Err(SandboxError::ManifestDoesNotMatch);
        };
        if current.state == SandboxState::Sealed {
            return Ok(current);
        }
        if current.state != SandboxState::Active {
            return Err(state_error(SandboxState::Active, current.state));
        }
        self.seal(&current.sandbox)?;
        Ok(VerifiedSandboxRef {
            state: SandboxState::Sealed,
            ..current
        })
    }

    pub fn revoke_verified(
        &self,
        verified: &VerifiedSandboxRef,
    ) -> Result<VerifiedSandboxRef, SandboxError> {
        let Some(current) = self.resolve_manifest(&verified.identity)? else {
            return Err(SandboxError::ManifestDoesNotMatch);
        };
        if current.state == SandboxState::Revoked || current.state == SandboxState::Quarantined {
            return Ok(current);
        }
        self.revoke(&current.sandbox)?;
        Ok(VerifiedSandboxRef {
            state: SandboxState::Revoked,
            ..current
        })
    }

    pub fn quarantine_verified(
        &self,
        verified: &VerifiedSandboxRef,
    ) -> Result<VerifiedSandboxRef, SandboxError> {
        let Some(current) = self.resolve_manifest(&verified.identity)? else {
            return Err(SandboxError::ManifestDoesNotMatch);
        };
        if current.state == SandboxState::Quarantined {
            return Ok(current);
        }
        if current.state != SandboxState::Revoked {
            return Err(state_error(SandboxState::Revoked, current.state));
        }
        let quarantined = self.quarantine_unaccepted(&current.sandbox)?;
        Ok(VerifiedSandboxRef {
            state: SandboxState::Quarantined,
            sandbox: quarantined,
            ..current
        })
    }

    pub fn seal(&self, sandbox: &SandboxRef) -> Result<(), SandboxError> {
        let mut manifest = self.validate(sandbox, ACTIVE_DIR)?;
        if manifest.state != SandboxState::Active {
            return Err(state_error(SandboxState::Active, manifest.state));
        }
        manifest.state = SandboxState::Sealed;
        write_manifest(&sandbox.path, &manifest)?;
        set_tree_readonly(&sandbox.path, true).map_err(io_error)
    }

    /// Revocation is explicit in the durable marker even when cleanup later
    /// fails. We temporarily make the sealed tree writable only to update it.
    pub fn revoke(&self, sandbox: &SandboxRef) -> Result<(), SandboxError> {
        let mut manifest = self.validate(sandbox, ACTIVE_DIR)?;
        if manifest.state != SandboxState::Sealed && manifest.state != SandboxState::Active {
            return Err(SandboxError::InvalidState {
                expected: SandboxState::Sealed,
                actual: manifest.state,
            });
        }
        set_tree_readonly(&sandbox.path, false).map_err(io_error)?;
        manifest.state = SandboxState::Revoked;
        write_manifest(&sandbox.path, &manifest)?;
        set_tree_readonly(&sandbox.path, true).map_err(io_error)
    }

    /// Unaccepted content moves to quarantine. Accepted artifacts remain only
    /// descriptors and must be materialized by a future, confirmed main-agent
    /// flow outside this module.
    pub fn quarantine_unaccepted(&self, sandbox: &SandboxRef) -> Result<SandboxRef, SandboxError> {
        let mut manifest = self.validate(sandbox, ACTIVE_DIR)?;
        if manifest.state != SandboxState::Revoked {
            return Err(state_error(SandboxState::Revoked, manifest.state));
        }
        set_tree_readonly(&sandbox.path, false).map_err(io_error)?;
        manifest.state = SandboxState::Quarantined;
        write_manifest(&sandbox.path, &manifest)?;
        let target = self.root.join(QUARANTINE_DIR).join(sandbox_leaf(sandbox));
        fs::rename(&sandbox.path, &target).map_err(io_error)?;
        set_tree_readonly(&target, true).map_err(io_error)?;
        Ok(SandboxRef {
            path: target,
            ..sandbox.clone()
        })
    }

    pub fn candidate_artifact(
        &self,
        sandbox: &SandboxRef,
        relative_path: impl AsRef<Path>,
        description: impl Into<String>,
    ) -> Result<CandidateArtifact, SandboxError> {
        self.validate(sandbox, ACTIVE_DIR)
            .or_else(|_| self.validate(sandbox, QUARANTINE_DIR))?;
        let relative_path = safe_relative(relative_path.as_ref())?;
        Ok(CandidateArtifact {
            relative_path,
            description: description.into(),
        })
    }

    /// Returns only direct, manifest-validated quarantine children. Symlinks,
    /// forged markers, and nested directories are ignored rather than removed.
    pub fn eligible_quarantine(&self) -> Vec<SandboxRef> {
        let dir = self.root.join(QUARANTINE_DIR);
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                if entry.file_type().ok()?.is_symlink() || !entry.file_type().ok()?.is_dir() {
                    return None;
                }
                let manifest = read_manifest(&path).ok()?;
                if manifest.state != SandboxState::Quarantined
                    || !valid_identity(&manifest.delegation_id)
                    || !valid_identity(&manifest.attempt_id)
                    || !valid_nonce(&manifest.nonce)
                {
                    return None;
                }
                let sandbox = SandboxRef {
                    delegation_id: manifest.delegation_id,
                    attempt_id: manifest.attempt_id,
                    nonce: manifest.nonce,
                    path,
                };
                (sandbox.path.file_name()?.to_string_lossy() == sandbox_leaf(&sandbox))
                    .then_some(sandbox)
            })
            .collect()
    }

    pub fn cleanup_quarantine(&self, sandbox: &SandboxRef) -> CleanupOutcome {
        let manifest = match self.validate(sandbox, QUARANTINE_DIR) {
            Ok(manifest) if manifest.state == SandboxState::Quarantined => manifest,
            Ok(manifest) => {
                return CleanupOutcome::Refused(state_error(
                    SandboxState::Quarantined,
                    manifest.state,
                ))
            }
            Err(error) => return CleanupOutcome::Refused(error),
        };
        let _ = manifest; // keeps the state validation visibly adjacent to deletion.
        match set_tree_readonly(&sandbox.path, false)
            .and_then(|_| fs::remove_dir_all(&sandbox.path))
        {
            Ok(()) => CleanupOutcome::Cleaned,
            Err(error) => CleanupOutcome::Retry(retry_from_io(error)),
        }
    }

    fn validate(&self, sandbox: &SandboxRef, container: &str) -> Result<Manifest, SandboxError> {
        let expected_parent = self.root.join(container);
        let canonical_parent = fs::canonicalize(&expected_parent).map_err(io_error)?;
        let canonical_path =
            fs::canonicalize(&sandbox.path).map_err(|_| SandboxError::ManifestMissing)?;
        // A caller-supplied nonce may be forged, so first establish only that
        // this is a direct canonical child; manifest binding below reports the
        // nonce mismatch without treating a valid owned directory as an escape.
        if canonical_path.parent() != Some(canonical_parent.as_path()) {
            return Err(SandboxError::OutsideManagedRoot(sandbox.path.clone()));
        }
        let manifest = read_manifest(&canonical_path)?;
        if manifest.version != 1
            || manifest.delegation_id != sandbox.delegation_id
            || manifest.attempt_id != sandbox.attempt_id
            || manifest.nonce != sandbox.nonce
        {
            return Err(SandboxError::ManifestDoesNotMatch);
        }
        Ok(manifest)
    }
}

fn sandbox_leaf(s: &SandboxRef) -> String {
    format!("{}-{}", s.attempt_id, s.nonce)
}
fn sandbox_manifest_digest(
    delegation_id: &str,
    attempt_id: &str,
    scope_digest: &str,
    lease_epoch: u64,
    nonce: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"delegated_sandbox_manifest_v2:");
    for value in [
        delegation_id,
        attempt_id,
        scope_digest,
        &lease_epoch.to_string(),
        nonce,
    ] {
        hasher.update(value.as_bytes());
        hasher.update(b":");
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
}
fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn validate_identity(value: &str) -> Result<(), SandboxError> {
    if valid_identity(value) {
        Ok(())
    } else {
        Err(SandboxError::InvalidIdentity)
    }
}
fn valid_nonce(value: &str) -> bool {
    Uuid::parse_str(value).is_ok()
}
fn safe_relative(path: &Path) -> Result<PathBuf, SandboxError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        Err(SandboxError::InvalidCandidatePath)
    } else {
        Ok(path.to_path_buf())
    }
}
fn state_error(expected: SandboxState, actual: SandboxState) -> SandboxError {
    SandboxError::InvalidState { expected, actual }
}
fn io_error(error: io::Error) -> SandboxError {
    SandboxError::Io(error.to_string())
}
fn retry_from_io(error: io::Error) -> CleanupRetry {
    let kind = if error.kind() == io::ErrorKind::PermissionDenied {
        // Windows reports a sharing violation as PermissionDenied. The same
        // category is conservatively retriable on other platforms too.
        CleanupRetryKind::FileLockedOrPermissionDenied
    } else {
        CleanupRetryKind::TransientIo
    };
    CleanupRetry {
        kind,
        detail: error.to_string(),
    }
}
fn read_manifest(path: &Path) -> Result<Manifest, SandboxError> {
    let bytes = fs::read(path.join(MANIFEST_FILE)).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            SandboxError::ManifestMissing
        } else {
            io_error(e)
        }
    })?;
    serde_json::from_slice(&bytes).map_err(|_| SandboxError::ManifestInvalid)
}
fn write_manifest(path: &Path, manifest: &Manifest) -> Result<(), SandboxError> {
    let bytes = serde_json::to_vec(manifest).map_err(|_| SandboxError::ManifestInvalid)?;
    let marker = path.join(MANIFEST_FILE);
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(marker)
        .map_err(io_error)?;
    file.write_all(&bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)
}
fn set_tree_readonly(root: &Path, readonly: bool) -> io::Result<()> {
    let mut paths = vec![root.to_path_buf()];
    let mut index = 0;
    while index < paths.len() {
        let path = paths[index].clone();
        index += 1;
        let ty = fs::symlink_metadata(&path)?.file_type();
        if ty.is_symlink() {
            continue;
        }
        if ty.is_dir() {
            for entry in fs::read_dir(&path)? {
                paths.push(entry?.path());
            }
        }
        let mut permissions = fs::metadata(&path)?.permissions();
        permissions.set_readonly(readonly);
        fs::set_permissions(&path, permissions)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn lifecycle() -> (tempfile::TempDir, DelegatedSandboxLifecycle) {
        let temp = tempdir().unwrap();
        let lifecycle = DelegatedSandboxLifecycle::new(temp.path().join("sandboxes")).unwrap();
        (temp, lifecycle)
    }
    fn create(l: &DelegatedSandboxLifecycle) -> SandboxRef {
        l.create("delegation_1", "attempt_1").unwrap()
    }

    #[test]
    fn creates_binds_seals_revokes_and_quarantines_an_attempt() {
        let (_temp, lifecycle) = lifecycle();
        let sandbox = create(&lifecycle);
        assert!(sandbox.path.starts_with(lifecycle.root()));
        lifecycle.seal(&sandbox).unwrap();
        lifecycle.revoke(&sandbox).unwrap();
        let quarantined = lifecycle.quarantine_unaccepted(&sandbox).unwrap();
        assert_eq!(lifecycle.eligible_quarantine(), vec![quarantined.clone()]);
        assert_eq!(
            lifecycle.cleanup_quarantine(&quarantined),
            CleanupOutcome::Cleaned
        );
        assert!(!quarantined.path.exists());
    }

    #[test]
    fn refuses_forged_missing_and_escaped_manifests() {
        let (temp, lifecycle) = lifecycle();
        let sandbox = create(&lifecycle);
        let forged = SandboxRef {
            nonce: Uuid::new_v4().to_string(),
            ..sandbox.clone()
        };
        assert_eq!(
            lifecycle.seal(&forged),
            Err(SandboxError::ManifestDoesNotMatch)
        );
        fs::remove_file(sandbox.path.join(MANIFEST_FILE)).unwrap();
        assert_eq!(lifecycle.seal(&sandbox), Err(SandboxError::ManifestMissing));
        let external = temp.path().join("external");
        fs::create_dir(&external).unwrap();
        let escaped = SandboxRef {
            path: external,
            ..sandbox
        };
        assert!(matches!(
            lifecycle.seal(&escaped),
            Err(SandboxError::OutsideManagedRoot(_))
        ));
    }

    #[test]
    fn candidate_paths_reject_traversal_and_never_materialize() {
        let (_temp, lifecycle) = lifecycle();
        let sandbox = create(&lifecycle);
        assert!(matches!(
            lifecycle.candidate_artifact(&sandbox, "../escape", "x"),
            Err(SandboxError::InvalidCandidatePath)
        ));
        assert!(matches!(
            lifecycle.candidate_artifact(&sandbox, Path::new("/absolute"), "x"),
            Err(SandboxError::InvalidCandidatePath)
        ));
        let candidate = lifecycle
            .candidate_artifact(&sandbox, "output/report.md", "report")
            .unwrap();
        assert_eq!(candidate.relative_path, PathBuf::from("output/report.md"));
    }

    #[test]
    fn eligibility_ignores_forged_and_nested_quarantine_entries() {
        let (_temp, lifecycle) = lifecycle();
        let valid = create(&lifecycle);
        lifecycle.seal(&valid).unwrap();
        lifecycle.revoke(&valid).unwrap();
        let valid = lifecycle.quarantine_unaccepted(&valid).unwrap();
        let forged = lifecycle.root().join(QUARANTINE_DIR).join("forged");
        fs::create_dir(&forged).unwrap();
        fs::write(forged.join(MANIFEST_FILE), b"{}").unwrap();
        // Quarantine discovery deliberately scans direct children only.
        set_tree_readonly(&valid.path, false).unwrap();
        fs::create_dir(valid.path.join("nested")).unwrap();
        set_tree_readonly(&valid.path, true).unwrap();
        assert_eq!(lifecycle.eligible_quarantine(), vec![valid]);
    }

    #[test]
    fn cleanup_refuses_active_or_forged_paths() {
        let (_temp, lifecycle) = lifecycle();
        let active = create(&lifecycle);
        assert!(matches!(
            lifecycle.cleanup_quarantine(&active),
            CleanupOutcome::Refused(SandboxError::OutsideManagedRoot(_))
        ));
        let fake = SandboxRef {
            path: lifecycle.root().join(QUARANTINE_DIR).join("nope"),
            ..active
        };
        assert!(matches!(
            lifecycle.cleanup_quarantine(&fake),
            CleanupOutcome::Refused(SandboxError::ManifestMissing)
        ));
    }

    #[test]
    fn io_cleanup_failures_have_retry_representation() {
        let retry = retry_from_io(io::Error::from(io::ErrorKind::PermissionDenied));
        assert!(matches!(
            CleanupOutcome::Retry(retry),
            CleanupOutcome::Retry(CleanupRetry {
                kind: CleanupRetryKind::FileLockedOrPermissionDenied,
                ..
            })
        ));
        assert_eq!(
            retry_from_io(io::Error::from(io::ErrorKind::Interrupted)).kind,
            CleanupRetryKind::TransientIo
        );
    }

    #[test]
    fn bound_manifest_resolves_only_with_complete_identity_tuple() {
        let (_temp, lifecycle) = lifecycle();
        let handle = lifecycle
            .create_bound("delegation_1", "attempt_1", "scope_digest", 7)
            .unwrap();
        let resolved = lifecycle
            .resolve_manifest(handle.identity())
            .unwrap()
            .unwrap();
        assert_eq!(resolved.identity(), handle.identity());
        assert_eq!(resolved.state(), SandboxState::Active);

        let forged = SandboxManifestIdentity::for_bound(
            "delegation_1",
            "attempt_1",
            "scope_digest",
            8,
            handle.identity().manifest_nonce.clone(),
        )
        .unwrap();
        assert!(lifecycle.resolve_manifest(&forged).unwrap().is_none());
    }

    #[test]
    fn verified_transitions_are_idempotent_and_restart_resolvable() {
        let (temp, lifecycle) = lifecycle();
        let handle = lifecycle
            .create_bound("delegation_1", "attempt_1", "scope_digest", 1)
            .unwrap();
        let sealed = lifecycle.seal_verified(&handle).unwrap();
        let sealed_again = lifecycle.seal_verified(&sealed).unwrap();
        assert_eq!(sealed_again.state(), SandboxState::Sealed);
        let revoked = lifecycle.revoke_verified(&sealed_again).unwrap();
        let quarantined = lifecycle.quarantine_verified(&revoked).unwrap();
        let restarted = DelegatedSandboxLifecycle::new(temp.path().join("sandboxes")).unwrap();
        let recovered = restarted
            .resolve_manifest(quarantined.identity())
            .unwrap()
            .unwrap();
        assert_eq!(recovered.state(), SandboxState::Quarantined);
        assert_eq!(
            restarted.quarantine_verified(&recovered).unwrap(),
            recovered
        );
    }

    #[test]
    fn unbound_or_unknown_residuals_are_retained() {
        let (_temp, lifecycle) = lifecycle();
        let legacy = create(&lifecycle);
        let unknown = SandboxManifestIdentity::for_bound(
            "delegation_1",
            "attempt_1",
            "scope_digest",
            1,
            Uuid::new_v4().to_string(),
        )
        .unwrap();
        assert!(lifecycle.resolve_manifest(&unknown).unwrap().is_none());
        assert!(legacy.path.exists());
    }
}

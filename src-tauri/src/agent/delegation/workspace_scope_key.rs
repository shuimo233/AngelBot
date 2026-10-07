//! Canonical identity for a filesystem-backed delegated workspace.
//!
//! Network approvals, work packages, and foreground issuance must compare the
//! same stable workspace key. This small utility keeps canonicalization,
//! filesystem-root identity, and Windows path normalization in one place
//! instead of letting each caller accidentally create a slightly different
//! scope.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceScopeKeyError {
    Unavailable,
    NotDirectory,
    IdentityUnavailable,
}

/// Canonicalize one trusted workspace root and derive its durable scope key.
///
/// The returned path stays canonical for filesystem checks, while the key uses
/// a versioned filesystem-root incarnation. The normalized path prevents
/// cross-directory reuse; the filesystem-assigned directory identity prevents
/// a replacement directory at the same path from inheriting old authority. If
/// the root's incarnation cannot be read, network issuance fails closed rather
/// than falling back to a path-only identity.
pub(crate) fn canonical_workspace_scope_key(
    work_dir: &Path,
) -> Result<(PathBuf, String), WorkspaceScopeKeyError> {
    let canonical =
        std::fs::canonicalize(work_dir).map_err(|_| WorkspaceScopeKeyError::Unavailable)?;
    if !canonical.is_dir() {
        return Err(WorkspaceScopeKeyError::NotDirectory);
    }

    let root_identity = directory_instance_identity(&canonical)?;

    let mut normalized_path =
        crate::commands::file::display_work_dir(&canonical).replace('\\', "/");
    #[cfg(windows)]
    {
        normalized_path = normalized_path.to_ascii_lowercase();
    }
    let workspace_key = v2_scope_key(&normalized_path, &root_identity)?;
    Ok((canonical, workspace_key))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectoryInstanceIdentity(String);

impl DirectoryInstanceIdentity {
    fn from_parts(volume_serial: u64, file_id: [u8; 16]) -> Self {
        Self(format!("{volume_serial:016x}:{}", hex::encode(file_id)))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

fn v2_scope_key(
    normalized_path: &str,
    root_identity: &DirectoryInstanceIdentity,
) -> Result<String, WorkspaceScopeKeyError> {
    if normalized_path.trim().is_empty()
        || normalized_path.len() > 455
        || normalized_path.contains(['\0', '\r', '\n'])
    {
        return Err(WorkspaceScopeKeyError::IdentityUnavailable);
    }
    let workspace_key = format!("v2:{normalized_path}:fid:{}", root_identity.as_str());
    (workspace_key.len() <= 512)
        .then_some(workspace_key)
        .ok_or(WorkspaceScopeKeyError::IdentityUnavailable)
}

/// Returns a filesystem-assigned identity for the directory currently rooted
/// at `canonical`. On Windows `FileIdInfo` is deliberately used instead of
/// creation time: callers can alter directory timestamps, whereas the volume
/// serial plus filesystem file ID is assigned by the filesystem.
#[cfg(windows)]
fn directory_instance_identity(
    canonical: &Path,
) -> Result<DirectoryInstanceIdentity, WorkspaceScopeKeyError> {
    use std::{
        fs::OpenOptions,
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandleEx, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO,
    };

    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(canonical)
        .map_err(|_| WorkspaceScopeKeyError::IdentityUnavailable)?;
    let mut info = FILE_ID_INFO::default();
    // SAFETY: `directory` owns a valid handle, `info` has the exact documented
    // buffer size, and it stays writable for this synchronous query.
    let loaded = unsafe {
        GetFileInformationByHandleEx(
            directory.as_raw_handle(),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if loaded == 0 || info.FileId.Identifier == [0; 16] {
        return Err(WorkspaceScopeKeyError::IdentityUnavailable);
    }

    Ok(DirectoryInstanceIdentity::from_parts(
        info.VolumeSerialNumber,
        info.FileId.Identifier,
    ))
}

#[cfg(unix)]
fn directory_instance_identity(
    canonical: &Path,
) -> Result<DirectoryInstanceIdentity, WorkspaceScopeKeyError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::metadata(canonical).map_err(|_| WorkspaceScopeKeyError::Unavailable)?;
    if !metadata.is_dir() {
        return Err(WorkspaceScopeKeyError::NotDirectory);
    }
    if metadata.ino() == 0 {
        return Err(WorkspaceScopeKeyError::IdentityUnavailable);
    }
    let mut file_id = [0; 16];
    file_id[..8].copy_from_slice(&metadata.ino().to_le_bytes());
    Ok(DirectoryInstanceIdentity::from_parts(
        metadata.dev(),
        file_id,
    ))
}

#[cfg(not(any(windows, unix)))]
fn directory_instance_identity(
    _canonical: &Path,
) -> Result<DirectoryInstanceIdentity, WorkspaceScopeKeyError> {
    Err(WorkspaceScopeKeyError::IdentityUnavailable)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::{
        agent::{
            delegated_network_scope::{
                ApprovedNetworkScopeRepository, ConfirmedNetworkScope, NetworkScopeApprovalError,
                NetworkScopeApprovalProvenance,
            },
            worker_policy::NetworkAction,
        },
        db::migrate,
    };

    #[test]
    fn canonical_scope_key_is_versioned_and_uses_the_foreground_path_normalization() {
        let directory = tempfile::tempdir().unwrap();
        let (canonical, key) = canonical_workspace_scope_key(directory.path()).unwrap();

        assert_eq!(canonical, std::fs::canonicalize(directory.path()).unwrap());
        let mut expected = crate::commands::file::display_work_dir(&canonical).replace('\\', "/");
        #[cfg(windows)]
        {
            expected = expected.to_ascii_lowercase();
        }
        assert!(key.starts_with(&format!("v2:{expected}:fid:")));
    }

    #[test]
    fn distinct_filesystem_directory_ids_produce_distinct_incarnation_keys() {
        let original = v2_scope_key(
            "d:/projects/example",
            &DirectoryInstanceIdentity::from_parts(1, [1; 16]),
        )
        .unwrap();
        let replacement = v2_scope_key(
            "d:/projects/example",
            &DirectoryInstanceIdentity::from_parts(1, [2; 16]),
        )
        .unwrap();

        assert_ne!(original, replacement);
        assert_ne!(original, "d:/projects/example");
    }

    #[cfg(windows)]
    #[test]
    fn recreating_a_directory_at_the_same_path_does_not_reuse_its_scope_key() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("workspace");
        std::fs::create_dir(&root).unwrap();
        let (_, original) = canonical_workspace_scope_key(&root).unwrap();

        std::fs::remove_dir(&root).unwrap();
        std::fs::create_dir(&root).unwrap();
        let (_, replacement) = canonical_workspace_scope_key(&root).unwrap();

        assert_ne!(original, replacement);
    }

    #[test]
    fn an_approval_for_a_prior_directory_incarnation_cannot_cover_its_replacement() {
        let original = v2_scope_key(
            "d:/projects/example",
            &DirectoryInstanceIdentity::from_parts(1, [1; 16]),
        )
        .unwrap();
        let replacement = v2_scope_key(
            "d:/projects/example",
            &DirectoryInstanceIdentity::from_parts(1, [2; 16]),
        )
        .unwrap();
        let mut connection = rusqlite::Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        let confirmed = ConfirmedNetworkScope::new(
            NetworkScopeApprovalProvenance {
                owner_profile_id: 1,
                workspace_key: original,
                session_id: "session-a".into(),
                message_id: "message-a".into(),
                parent_run_id: "run-a".into(),
                tool_call_id: "call-a".into(),
            },
            "network_scope_a".into(),
            "sha256:scope-a".into(),
            BTreeSet::from([NetworkAction::Search]),
            BTreeSet::from(["docs.example.com".into()]),
        )
        .unwrap();
        let transaction = connection.transaction().unwrap();
        confirmed.persist_in_tx(&transaction, 1).unwrap();
        transaction.commit().unwrap();

        assert_eq!(
            ApprovedNetworkScopeRepository::find_covering(
                &connection,
                1,
                &replacement,
                &BTreeSet::from([NetworkAction::Search]),
                &BTreeSet::from(["docs.example.com".into()]),
            ),
            Err(NetworkScopeApprovalError::NotFound)
        );
    }
}

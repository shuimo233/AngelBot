//! Bounded file payload for the existing backup, with additive no-clobber restore.
//! Knows only an explicit owned root, never sessions, credentials or user folders.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_FILES: usize = 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BackupFile {
    path: String,
    content_base64: String,
}

fn validate_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.len() > 1024
        || path.split('/').count() > 32
        || path
            .chars()
            .any(|c| c.is_control() || "\\:<>\"|?*".contains(c))
        || path.split('/').any(|part| {
            let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
            part.is_empty()
                || part == "."
                || part == ".."
                || part.ends_with([' ', '.'])
                || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || ["COM", "LPT"].iter().any(|prefix| {
                    stem.strip_prefix(prefix).is_some_and(|n| {
                        matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
                    })
                })
        })
    {
        return Err("备份文件路径无效。".to_owned());
    }
    Ok(())
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err("单个结果文件超过备份上限 16 MiB；请先另存该文件。".to_owned());
    }
    Ok(bytes)
}

pub(crate) fn snapshot(root: &Path) -> Result<Vec<BackupFile>, String> {
    snapshot_excluding(root, None)
}

/// Only a host-verified direct child may be excluded. Never infer ownership
/// from names/extensions; callers without an ownership record include all.
pub(crate) fn snapshot_excluding(
    root: &Path,
    excluded: Option<&Path>,
) -> Result<Vec<BackupFile>, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    if excluded.is_some_and(|path| path.parent() != Some(root.as_path())) {
        return Err("备份临时区必须是结果根目录的直接子目录。".into());
    }
    let mut pending = vec![root.clone()];
    let mut files = Vec::new();
    let mut total = 0;
    let mut entries = 0;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
            entries += 1;
            if entries > 4096 {
                return Err("结果区条目超过备份上限 4096 个。".to_owned());
            }
            let path = entry.map_err(|e| e.to_string())?.path();
            let relative = path
                .strip_prefix(&root)
                .map_err(|e| e.to_string())?
                .to_str()
                .ok_or_else(|| "结果文件名称不是 UTF-8。".to_owned())?
                .replace('\\', "/");
            validate_path(&relative)?;
            let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if metadata.file_type().is_symlink()
                || path.canonicalize().map_err(|e| e.to_string())? != path
            {
                return Err("结果区含链接或重定向目录，备份已停止，不读取链接目标。".to_owned());
            }
            if metadata.is_dir() {
                if excluded == Some(path.as_path()) {
                    continue;
                }
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                return Err("结果区包含非普通文件。".to_owned());
            }
            if files.len() >= MAX_FILES {
                return Err("结果文件超过备份上限 1024 个。".to_owned());
            }
            let bytes = read_bounded(&path)?;
            total += bytes.len();
            if total > MAX_TOTAL_BYTES {
                return Err("结果文件合计超过备份上限 64 MiB；未生成不完整备份。".to_owned());
            }
            files.push(BackupFile {
                path: relative,
                content_base64: BASE64.encode(bytes),
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Owns only files/directories this attempt created. A normal database error
/// rolls them back; neither merge nor replace can delete/overwrite existing
/// results. This is not a cross-filesystem/process-crash transaction.
pub(crate) struct Restore {
    files: Vec<(PathBuf, [u8; 32])>,
    directories: Vec<PathBuf>,
    committed: bool,
}

impl Restore {
    pub(crate) fn install(root: &Path, files: &[BackupFile]) -> Result<Self, String> {
        if files.len() > MAX_FILES {
            return Err("备份结果文件超过 1024 个。".to_owned());
        }
        let mut names = HashSet::new();
        let mut total = 0;
        let mut decoded = Vec::new();
        for file in files {
            validate_path(&file.path)?;
            if !names.insert(file.path.to_lowercase()) {
                return Err("备份结果文件路径重复。".to_owned());
            }
            if file.content_base64.len() > MAX_FILE_BYTES.div_ceil(3) * 4 {
                return Err("备份结果文件超过 16 MiB。".to_owned());
            }
            let bytes = BASE64
                .decode(&file.content_base64)
                .map_err(|_| "备份结果文件编码无效。")?;
            total += bytes.len();
            if bytes.len() > MAX_FILE_BYTES || total > MAX_TOTAL_BYTES {
                return Err("备份结果文件超过大小上限。".to_owned());
            }
            decoded.push((&file.path, bytes));
        }
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let mut restore = Self {
            files: Vec::new(),
            directories: Vec::new(),
            committed: false,
        };
        for (relative, bytes) in decoded {
            let mut parent = root.clone();
            let parts: Vec<_> = relative.split('/').collect();
            for part in &parts[..parts.len() - 1] {
                parent.push(part);
                if !parent.try_exists().map_err(|e| e.to_string())? {
                    std::fs::create_dir(&parent).map_err(|e| e.to_string())?;
                    restore.directories.push(parent.clone());
                }
                if parent.canonicalize().map_err(|e| e.to_string())? != parent {
                    return Err("恢复目标含重定向目录。".to_owned());
                }
            }
            let destination = parent.join(parts.last().unwrap());
            if destination.try_exists().map_err(|e| e.to_string())? {
                if destination.canonicalize().map_err(|e| e.to_string())? != destination
                    || !destination.is_file()
                    || read_bounded(&destination)? != bytes
                {
                    return Err(format!(
                        "结果文件冲突，未覆盖：{relative}。请先重命名或另存现有文件。"
                    ));
                }
                continue;
            }
            let mut temporary =
                tempfile::NamedTempFile::new_in(&parent).map_err(|e| e.to_string())?;
            temporary
                .write_all(&bytes)
                .and_then(|()| temporary.as_file().sync_all())
                .map_err(|e| e.to_string())?;
            temporary
                .persist_noclobber(&destination)
                .map_err(|e| e.to_string())?;
            restore
                .files
                .push((destination, Sha256::digest(&bytes).into()));
        }
        Ok(restore)
    }

    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for Restore {
    fn drop(&mut self) {
        if !self.committed {
            for (path, digest) in self.files.iter().rev() {
                // Another writer's later edit is no longer ours to delete.
                if path.canonicalize().ok().as_ref() != Some(path)
                    || read_bounded(path)
                        .ok()
                        .is_none_or(|bytes| <[u8; 32]>::from(Sha256::digest(bytes)) != *digest)
                {
                    eprintln!(
                        "[Import] Result changed during restore; preserving {}",
                        path.display()
                    );
                    continue;
                }
                if let Err(e) = std::fs::remove_file(path) {
                    eprintln!(
                        "[Import] Unable to roll back new result {}: {e}",
                        path.display()
                    );
                }
            }
            for path in self.directories.iter().rev() {
                if let Err(e) = std::fs::remove_dir(path) {
                    eprintln!(
                        "[Import] Unable to roll back new directory {}: {e}",
                        path.display()
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_files_round_trip_and_restore_is_additive_bounded_and_rollback_safe() {
        let source = tempfile::tempdir().unwrap();
        std::fs::create_dir(source.path().join("交付")).unwrap();
        std::fs::write(
            source.path().join("交付/清单.csv"),
            "事项,状态\n采购,待办\n",
        )
        .unwrap();
        std::fs::write(source.path().join("download.bin"), [0, 255, 1, 128]).unwrap();
        let files = snapshot(source.path()).unwrap();
        let destination = tempfile::tempdir().unwrap();
        Restore::install(destination.path(), &files)
            .unwrap()
            .commit();
        assert_eq!(
            std::fs::read(destination.path().join("download.bin")).unwrap(),
            [0, 255, 1, 128]
        );
        assert_eq!(
            std::fs::read_to_string(destination.path().join("交付/清单.csv")).unwrap(),
            "事项,状态\n采购,待办\n"
        );
        Restore::install(destination.path(), &files)
            .unwrap()
            .commit();
        std::fs::write(destination.path().join("download.bin"), b"newer").unwrap();
        assert!(Restore::install(destination.path(), &files).is_err());
        assert_eq!(
            std::fs::read(destination.path().join("download.bin")).unwrap(),
            b"newer"
        );
        let rollback = tempfile::tempdir().unwrap();
        drop(Restore::install(rollback.path(), &files).unwrap());
        assert_eq!(std::fs::read_dir(rollback.path()).unwrap().count(), 0);
        for path in [
            "../escape.txt",
            "C:/secret",
            "/absolute",
            "dir\\file",
            "a:stream",
            "a/../b",
            "CON.txt",
            "a.",
            "a/",
            "a\n.txt",
        ] {
            let invalid = [BackupFile {
                path: path.into(),
                content_base64: BASE64.encode("no"),
            }];
            assert!(
                Restore::install(rollback.path(), &invalid).is_err(),
                "{path}"
            );
        }
        let duplicate = ["a.txt", "A.txt"].map(|path| BackupFile {
            path: path.into(),
            content_base64: BASE64.encode("no"),
        });
        assert!(Restore::install(rollback.path(), &duplicate).is_err());
        let too_large = [BackupFile {
            path: "large".into(),
            content_base64: "A".repeat(MAX_FILE_BYTES.div_ceil(3) * 4 + 1),
        }];
        assert!(Restore::install(rollback.path(), &too_large).is_err());
        assert_eq!(std::fs::read_dir(rollback.path()).unwrap().count(), 0);
    }
}

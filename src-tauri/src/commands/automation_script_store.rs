//! Immutable-at-import copies, separate from the Main Agent's writable results.
use rusqlite::Connection;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const DIRECTORY: &str = "automation-scripts";
const MAX_SCRIPT_BYTES: u64 = 16 * 1024 * 1024;

pub(super) fn root(conn: &Connection) -> Result<PathBuf, String> {
    crate::commands::file::app_owned_directory(conn, DIRECTORY)
}

pub(super) struct Snapshot {
    directory: tempfile::TempDir,
    pub path: PathBuf,
}

impl Snapshot {
    pub fn prepare(conn: &Connection, source: &str) -> Result<Self, String> {
        let source = Path::new(source.trim());
        if !source.is_absolute() {
            return Err("请选择脚本文件或填写完整绝对路径。".into());
        }
        let source = fs::canonicalize(source).map_err(|e| format!("无法读取源脚本：{e}"))?;
        if !fs::metadata(&source).map_err(|e| e.to_string())?.is_file() {
            return Err("脚本路径不是普通文件。".into());
        }
        // Validate filenames and redirected links with the shared file guard.
        crate::commands::file::resolve_in_existing_root(
            source.parent().ok_or("源脚本缺少目录")?,
            &source,
            true,
        )
        .map_err(|e| e.to_string())?;
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Do not copy a script while another process can edit or replace it.
            options.share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ);
        }
        let mut source_file = options
            .open(&source)
            .map_err(|e| format!("无法打开源脚本：{e}"))?;
        let metadata = source_file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() {
            return Err("脚本路径不是普通文件。".into());
        }
        if metadata.len() > MAX_SCRIPT_BYTES {
            return Err("脚本不能超过16 MiB；仅复制脚本，不复制整个目录。".into());
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut source_file)
            .take(MAX_SCRIPT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_SCRIPT_BYTES {
            return Err("源脚本读取时超出16 MiB，未导入。".into());
        }
        let directory = tempfile::Builder::new()
            .prefix("script-")
            .tempdir_in(root(conn)?)
            .map_err(|e| format!("无法建立脚本托管目录：{e}"))?;
        let path = directory
            .path()
            .join(source.file_name().ok_or("源脚本缺少文件名")?);
        let mut target = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|e| format!("无法复制脚本：{e}"))?;
        target
            .write_all(&bytes)
            .and_then(|_| target.sync_all())
            .map_err(|e| format!("无法保存脚本复制件：{e}"))?;
        Ok(Self { directory, path })
    }

    /// Only keep files after the definition is durable. Errors drop our copy.
    pub fn commit(self) {
        let _ = self.directory.keep();
    }
}

pub(super) fn remove_unreferenced(conn: &Connection, script: &str) -> Result<(), String> {
    let referenced: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM automations WHERE script_path=?1 COLLATE NOCASE)",
            [script],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if referenced {
        return Ok(());
    }
    let path = Path::new(script);
    let Some(directory) = path.parent() else {
        return Ok(());
    };
    let Some(database) = conn.path().filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    let expected = Path::new(database)
        .parent()
        .ok_or("托管脚本目录不可用")?
        .canonicalize()
        .map_err(|e| e.to_string())?
        .join(DIRECTORY);
    let expected = PathBuf::from(crate::commands::file::display_work_dir(&expected));
    // Never clean a legacy/user script or a directory selected as working_dir.
    if directory.parent() != Some(expected.as_path())
        || !directory
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("script-"))
    {
        return Ok(());
    }
    if !directory.try_exists().map_err(|e| e.to_string())? {
        return Ok(());
    }
    let root = root(conn)?;
    if directory.canonicalize().map_err(|e| e.to_string())?
        != root.join(directory.file_name().ok_or("托管目录名称无效")?)
    {
        return Err("任务已删除，但托管脚本目录已被重定向，未清理文件。".into());
    }
    crate::commands::file::resolve_in_existing_root(&root, directory, true)
        .map_err(|e| e.to_string())?;
    // Remove only the known copy, never sweep unexpected files or source directories.
    if path.try_exists().map_err(|e| e.to_string())? {
        fs::remove_file(path).map_err(|e| format!("任务已删除，但托管脚本复制件未清理：{e}"))?;
    }
    fs::remove_dir(directory)
        .map_err(|e| format!("任务已删除，托管目录仍有其它文件或无法清理：{e}"))
}

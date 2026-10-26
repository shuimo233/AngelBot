//! File service commands with sandboxing.
//!
//! Workspace sessions use only their project root or host-owned Daily results.
//! Legacy sessions retain their work_dir, global setting and Documents fallback.
//! Missing or unknown workspace sessions fail closed instead of inheriting it.

use crate::document_text::{DocumentFormat, MAX_DOCUMENT_BYTES};
use rusqlite::OptionalExtension;
use std::io::Read;
use std::path::Path;
use tauri::command;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkDirEntry {
    pub path: String,
    pub name: String,
    pub is_directory: bool,
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporary: Option<bool>,
}

/// File-system scope associated with a session. Supplemental directories are
/// intentionally read-only: all writes still target the primary work directory.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFileAccess {
    pub work_dir: String,
    pub additional_read_dirs: Vec<String>,
}

#[command]
pub fn get_session_file_access(
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
) -> Result<SessionFileAccess, String> {
    get_session_file_access_impl(&state, &session_id)
}

pub fn get_session_file_access_impl(
    state: &crate::AppState,
    session_id: &str,
) -> Result<SessionFileAccess, String> {
    let work_dir = resolve_work_dir(state, Some(session_id))?;
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let raw: String = conn
        .query_row(
            "SELECT additional_read_dirs FROM sessions WHERE id = ?1",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    let additional_read_dirs = serde_json::from_str::<Vec<String>>(&raw)
        .map_err(|_| "Invalid stored supplemental read directories".to_string())?;
    Ok(SessionFileAccess {
        work_dir: display_work_dir(&work_dir),
        additional_read_dirs,
    })
}

#[command]
pub fn set_session_read_dirs(
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
    directories: Vec<String>,
) -> Result<(), String> {
    set_session_read_dirs_impl(&state, &session_id, &directories)
}

pub fn set_session_read_dirs_impl(
    state: &crate::AppState,
    session_id: &str,
    directories: &[String],
) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    require_legacy_file_configuration(&conn, session_id)?;
    drop(conn);

    let mut normalized = Vec::new();
    for directory in directories {
        let path = std::fs::canonicalize(directory.trim())
            .map_err(|e| format!("Supplemental read directory is unavailable: {}", e))?;
        if !path.is_dir() {
            return Err("Supplemental read paths must be directories".to_string());
        }
        let display = display_work_dir(&path);
        if !normalized.contains(&display) {
            normalized.push(display);
        }
    }
    let encoded = serde_json::to_string(&normalized).map_err(|e| e.to_string())?;
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let updated = conn
        .execute(
            "UPDATE sessions SET additional_read_dirs = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![encoded, chrono::Utc::now().timestamp(), session_id],
        )
        .map_err(|e| e.to_string())?;
    if updated == 0 {
        return Err("Session not found".to_string());
    }
    Ok(())
}

/// Returns the effective work directory for a session.
#[command]
pub fn get_work_dir(
    state: tauri::State<'_, crate::AppState>,
    session_id: Option<String>,
) -> Result<String, String> {
    let work_dir = resolve_work_dir(&state, session_id.as_deref())?;
    Ok(display_work_dir(&work_dir))
}

/// Convert a filesystem path to the form users expect to see.
///
/// On Windows, `std::fs::canonicalize` returns an extended-length path such
/// as `\\?\D:\Projects\...`.  That prefix is needed by some Win32 APIs but is
/// an implementation detail, not part of the directory the user selected.
/// Keep the canonical path internally for sandbox checks while removing the
/// prefix at the command boundary used by the desktop UI.
pub(crate) fn display_work_dir(path: &Path) -> String {
    let path = path.to_string_lossy();

    #[cfg(windows)]
    {
        if let Some(unc_path) = path.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{}", unc_path);
        }
        if let Some(normal_path) = path.strip_prefix(r"\\?\") {
            return normal_path.to_string();
        }
    }

    path.into_owned()
}

/// Sets the session-level work directory override.
#[command]
pub fn set_work_dir(
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
    work_dir: String,
) -> Result<(), String> {
    set_work_dir_impl(&state, &session_id, &work_dir)
}

/// Set a session work-directory override after validating and normalizing it.
/// Kept separate from the Tauri wrapper so development HTTP uses the exact
/// same contract as production IPC.
pub fn set_work_dir_impl(
    state: &crate::AppState,
    session_id: &str,
    work_dir: &str,
) -> Result<(), String> {
    let normalized_work_dir = if work_dir.trim().is_empty() {
        String::new()
    } else {
        let path = std::fs::canonicalize(work_dir.trim())
            .map_err(|e| format!("工作目录不可访问: {}", e))?;
        if !path.is_dir() {
            return Err("工作目录必须是一个目录".to_string());
        }
        path.to_string_lossy().to_string()
    };
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    require_legacy_file_configuration(&conn, session_id)?;
    let now = chrono::Utc::now().timestamp();
    let updated = conn
        .execute(
            "UPDATE sessions SET work_dir = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![normalized_work_dir, now, session_id],
        )
        .map_err(|e| e.to_string())?;
    if updated == 0 {
        return Err("会话不存在，无法设置工作目录".to_string());
    }
    // A directory chosen for a session is also the user's most recent working
    // directory. Future sessions without an explicit override resolve through
    // this setting, so AngelBot resumes where the user last worked.
    if !normalized_work_dir.is_empty() {
        remember_work_dir(&conn, &normalized_work_dir, now)?;
    }
    Ok(())
}

/// Project and Personal workspaces own their file capability.  A session is
/// only an internal conversation branch there, so it must never be able to
/// retarget its own directory or add cross-workspace read roots.
///
/// The legacy, unowned-session path remains solely for migration and local
/// development compatibility; user-facing workspace flows never create it.
fn require_legacy_file_configuration(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<(), String> {
    let project_id: Option<Option<String>> = conn
        .query_row(
            "SELECT project_id FROM sessions WHERE id = ?1",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;

    match project_id {
        None => Err("会话不存在，无法修改文件范围".to_owned()),
        Some(Some(_)) => {
            Err("工作区的目录与文件范围由工作区创建时确定，不能由内部会话修改。".to_owned())
        }
        Some(None) => Ok(()),
    }
}

fn remember_work_dir(conn: &rusqlite::Connection, work_dir: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES ('work_directory', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        rusqlite::params![work_dir, now],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

#[derive(Debug, thiserror::Error)]
pub enum FileReadError {
    #[error("work directory is not set")]
    MissingWorkDir,
    #[error("path escapes the work directory")]
    EscapesWorkDir,
    #[error("unsupported Windows file name or alternate data stream")]
    InvalidPath,
    #[error("read failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Validate `target` is within `work_dir` and return the final path.
pub fn resolve_and_validate(
    work_dir: &Path,
    target: &Path,
    require_exists: bool,
) -> Result<std::path::PathBuf, FileReadError> {
    resolve_with_root_policy(work_dir, target, require_exists, true)
}

/// Read-only observations must not bootstrap or resurrect a removed root.
pub(crate) fn resolve_in_existing_root(
    work_dir: &Path,
    target: &Path,
    require_exists: bool,
) -> Result<std::path::PathBuf, FileReadError> {
    resolve_with_root_policy(work_dir, target, require_exists, false)
}

fn resolve_with_root_policy(
    work_dir: &Path,
    target: &Path,
    require_exists: bool,
    ensure_root: bool,
) -> Result<std::path::PathBuf, FileReadError> {
    for component in target.components() {
        if let std::path::Component::Normal(name) = component {
            let name = name.to_string_lossy();
            let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
            if name
                .chars()
                .any(|c| c.is_control() || ":<>\"|?*".contains(c))
                || name.ends_with([' ', '.'])
                || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || ["COM", "LPT"].iter().any(|prefix| {
                    stem.strip_prefix(prefix).is_some_and(|n| {
                        matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
                    })
                })
            {
                return Err(FileReadError::InvalidPath);
            }
        }
        #[cfg(windows)]
        if matches!(component, std::path::Component::Prefix(prefix) if matches!(prefix.kind(), std::path::Prefix::DeviceNS(_)))
        {
            return Err(FileReadError::InvalidPath);
        }
    }
    if ensure_root {
        std::fs::create_dir_all(work_dir).map_err(FileReadError::Io)?;
    }
    let work_canonical = std::fs::canonicalize(work_dir).map_err(FileReadError::Io)?;

    // Build the target path
    let joined = if target.is_absolute() {
        target.to_path_buf()
    } else {
        work_dir.join(target)
    };

    // Canonicalize every existing target before the sandbox comparison. This
    // applies to writes as well as reads: otherwise writing to an existing
    // symlink inside the work directory could follow the link outside it.
    if require_exists {
        if !joined.try_exists().map_err(FileReadError::Io)? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("file not found: {}", joined.display()),
            )
            .into());
        }
        let target_canonical = std::fs::canonicalize(&joined).map_err(FileReadError::Io)?;
        if !target_canonical.starts_with(&work_canonical) {
            return Err(FileReadError::EscapesWorkDir);
        }
        return Ok(target_canonical);
    }

    // An existing entry may be a dangling link. Never treat it as a fresh
    // writable path merely because following it reports NotFound.
    let entry_exists = match std::fs::symlink_metadata(&joined) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(FileReadError::Io(error)),
    };
    if entry_exists {
        let target_canonical = std::fs::canonicalize(&joined).map_err(FileReadError::Io)?;
        if !target_canonical.starts_with(&work_canonical) {
            return Err(FileReadError::EscapesWorkDir);
        }
        return Ok(target_canonical);
    }

    // For new writes, the parent directory must exist and remain inside the
    // work directory. Canonicalizing it also rejects symlinked parents.
    if let Some(parent) = joined.parent() {
        if !parent.try_exists().map_err(FileReadError::Io)? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("parent directory not found: {}", parent.display()),
            )
            .into());
        }
        let parent_canonical = std::fs::canonicalize(parent).map_err(FileReadError::Io)?;
        if !parent_canonical.starts_with(&work_canonical) {
            return Err(FileReadError::EscapesWorkDir);
        }
    }

    Ok(joined)
}

/// Resolve the effective work directory.
/// Workspace-owned sessions resolve only their workspace root. Legacy sessions
/// retain the old fallback while data is being migrated. Personal result files
/// are not a project root and never grant delegated workers project authority.
pub(crate) fn resolve_delegation_workspace_root(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    let workspace: Option<(Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT p.kind, CASE WHEN p.directory_confirmed=1 THEN p.path END, s.work_dir FROM sessions s LEFT JOIN projects p ON p.id = s.project_id WHERE s.id = ?1",
            rusqlite::params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;

    match workspace {
        Some((Some(kind), _, _)) if kind == "personal" => Ok(None),
        Some((Some(kind), root, _)) if kind == "project" => {
            let root = root.filter(|path| !path.is_empty()).ok_or_else(|| {
                "项目目录尚未确认，请在设置的数据备份页批量确认恢复目录。".to_owned()
            })?;
            let root = std::path::PathBuf::from(root);
            root.exists()
                .then_some(Some(root))
                .ok_or_else(|| "项目目录已不可访问。".to_owned())
        }
        Some((Some(_), _, _)) => Err("未知工作区类型不具有委派权限。".to_owned()),
        Some((None, _, work_dir)) => {
            let work_dir = work_dir
                .filter(|path| !path.is_empty())
                .ok_or_else(|| "旧会话缺少可用于委派的工作目录。".to_owned())?;
            let work_dir = std::path::PathBuf::from(work_dir);
            work_dir
                .exists()
                .then_some(Some(work_dir))
                .ok_or_else(|| "旧会话工作目录已不可访问。".to_owned())
        }
        None => Err("会话不存在，无法建立委派范围。".to_owned()),
    }
}

pub fn resolve_work_dir(
    state: &crate::AppState,
    session_id: Option<&str>,
) -> Result<std::path::PathBuf, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    resolve_work_dir_from_connection(&conn, session_id)
}

/// The database location, not model input or a configurable work_dir, owns this
/// durable directory. In-memory tests must supply a real temporary database to
/// exercise files; never fall back to a user's application data in tests.
pub(crate) fn personal_files_root(
    conn: &rusqlite::Connection,
) -> Result<std::path::PathBuf, String> {
    let database = conn
        .path()
        .filter(|path| !path.is_empty())
        .ok_or_else(|| "日常结果区需要持久化数据库。".to_owned())?;
    let parent = std::fs::canonicalize(
        Path::new(database)
            .parent()
            .ok_or_else(|| "日常结果区位置不可用。".to_owned())?,
    )
    .map_err(|e| e.to_string())?;
    let root = parent.join("results");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let canonical = std::fs::canonicalize(&root).map_err(|e| e.to_string())?;
    if canonical != root {
        return Err("日常结果区不能重定向到其它目录。".to_owned());
    }
    Ok(canonical)
}

fn resolve_work_dir_from_connection(
    conn: &rusqlite::Connection,
    session_id: Option<&str>,
) -> Result<std::path::PathBuf, String> {
    // 1. Workspace-owned sessions have a non-negotiable filesystem boundary.
    if let Some(sid) = session_id {
        let workspace: Option<(Option<String>, Option<String>, Option<String>)> = conn
            .query_row(
                "SELECT p.kind, CASE WHEN p.directory_confirmed=1 THEN p.path END, s.work_dir FROM sessions s LEFT JOIN projects p ON p.id = s.project_id WHERE s.id = ?1",
                rusqlite::params![sid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional().map_err(|error| error.to_string())?;

        if workspace.is_none() {
            return Err("会话不存在，无法建立文件范围。".to_owned());
        }

        if let Some((Some(kind), root, _)) = workspace.as_ref() {
            if kind == "personal" {
                return personal_files_root(conn);
            }
            if kind == "project" {
                let root = root
                    .as_ref()
                    .filter(|path| !path.is_empty())
                    .ok_or_else(|| {
                        "项目目录尚未确认，请在设置的数据备份页批量确认恢复目录。".to_owned()
                    })?;
                let root = std::path::PathBuf::from(root);
                return root
                    .exists()
                    .then_some(root)
                    .ok_or_else(|| "项目目录已不可访问。".to_owned());
            }
            // Unknown kinds are fail-closed rather than inheriting a global root.
            return Err("未知工作区类型不具有文件访问权限。".to_owned());
        }

        let session_dir = workspace
            .and_then(|(_, _, work_dir)| work_dir)
            .filter(|path| !path.is_empty());

        if let Some(dir) = session_dir {
            let p = std::path::PathBuf::from(&dir);
            if p.exists() {
                return Ok(p);
            }
            // Sessions created by older Windows builds may contain the
            // extended-length representation (`\\?\C:\…`) returned by
            // `canonicalize`. Reinterpret it as the ordinary user-facing path
            // before falling back to the global directory.
            let display_path = std::path::PathBuf::from(display_work_dir(&p));
            if display_path.exists() {
                return Ok(display_path);
            }
        }
    }

    // 2. Check global setting
    let global_dir: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'work_directory'",
            [],
            |r| r.get(0),
        )
        .ok()
        .filter(|s: &String| !s.is_empty());

    if let Some(dir) = global_dir {
        let p = std::path::PathBuf::from(&dir);
        if p.exists() {
            return Ok(p);
        }
    }

    // 3. Fallback to Documents
    dirs::document_dir()
        .ok_or_else(|| "Work directory not configured and Documents folder not found".to_string())
}

/// Core read function constrained to an explicit work directory.
///
/// Absolute paths are accepted only when they resolve inside the workspace.
/// This keeps direct UI, dev HTTP, and Main-Agent reads on the same canonical
/// traversal and symlink-safe boundary. Office files return an explicitly
/// labelled bounded text snapshot, never the binary package or rendered view.
pub fn read_file_core(work_dir: &Path, path: &str) -> Result<String, String> {
    read_file_snapshot_core(work_dir, path).map(|snapshot| snapshot.content)
}

/// Agent-facing provenance from the same authorized read as the text. The
/// document itself may contain historical paths; they are not this binding.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileTextSnapshot {
    resolved_path: String,
    content: String,
}

pub(crate) fn read_file_snapshot_core(
    work_dir: &Path,
    path: &str,
) -> Result<FileTextSnapshot, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("read_file path cannot be empty".to_string());
    }
    let resolved = resolve_and_validate(work_dir, Path::new(trimmed), true)
        .map_err(|error| error.to_string())?;
    let content = if let Some(format) = DocumentFormat::from_path(Path::new(trimmed))
        .or_else(|| DocumentFormat::from_path(&resolved))
    {
        let mut bytes = Vec::new();
        std::fs::File::open(&resolved)
            .map_err(|error| error.to_string())?
            .take((MAX_DOCUMENT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        format.extract(&bytes)?
    } else {
        std::fs::read_to_string(&resolved).map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidData {
                "该文件不是 UTF-8 文本，暂不支持文字预览。原文件保留，可另存后用对应应用打开。"
                    .to_owned()
            } else {
                error.to_string()
            }
        })?
    };
    Ok(FileTextSnapshot {
        resolved_path: display_work_dir(&resolved),
        content,
    })
}

/// Lists one directory within the session work directory. Entries are relative
/// to that directory, so callers never need to expose arbitrary host paths.
pub fn list_work_dir_core(work_dir: &Path, path: &str) -> Result<Vec<WorkDirEntry>, String> {
    let work_root = std::fs::canonicalize(work_dir).map_err(|e| e.to_string())?;
    let directory =
        resolve_and_validate(work_dir, Path::new(path), true).map_err(|e| e.to_string())?;
    if !directory.is_dir() {
        return Err("path is not a directory".to_string());
    }

    let mut entries = std::fs::read_dir(&directory)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            let absolute = entry.path();
            let relative = absolute
                .strip_prefix(&work_root)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            Some(WorkDirEntry {
                path: relative,
                name: entry.file_name().to_string_lossy().into_owned(),
                is_directory: metadata.is_dir(),
                size: (!metadata.is_dir()).then_some(metadata.len()),
                temporary: None,
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        right
            .is_directory
            .cmp(&left.is_directory)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(entries)
}

/// Core write function — accepts an explicit work directory.
///
/// Issue #106: write stays sandboxed to the work directory. If the user asks
/// for a path outside it, we refuse with an actionable error pointing at the
/// `set_work_dir` / new-session escape hatch instead of silently writing
/// elsewhere.
pub fn write_file_core(work_dir: &Path, path: &str, content: &str) -> Result<(), String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("write_file path cannot be empty".to_string());
    }
    // Detect an obvious absolute-outside-work_dir target before delegating, so
    // we can emit a guidance message instead of the bare `EscapesWorkDir` text.
    let target = Path::new(trimmed);
    if target.is_absolute() {
        let joined = target.to_path_buf();
        // Canonicalize both sides when possible to compare like the sandbox does.
        let canonical_target = std::fs::canonicalize(&joined).ok();
        let canonical_work = std::fs::canonicalize(work_dir).ok();
        let outside = match (canonical_target, canonical_work) {
            (Some(t), Some(w)) => !t.starts_with(&w),
            // If we can't canonicalize either side, fall through to the
            // generic resolver below which will surface the precise failure.
            _ => true,
        };
        if outside {
            return Err(format!(
                "write_file 仅允许写入工作目录（当前: {}）。如需写入 {}，请先把工作目录切换到该文件所在目录（设置 → 文件访问，或新建一个 session）。",
                display_work_dir(work_dir),
                joined.display(),
            ));
        }
    }
    let resolved =
        resolve_and_validate(work_dir, Path::new(trimmed), false).map_err(|e| e.to_string())?;
    if DocumentFormat::from_path(Path::new(trimmed)).is_some()
        || DocumentFormat::from_path(&resolved).is_some()
    {
        return Err("DOCX/XLSX 只能读取文字快照，不能用文本写入覆盖 Office 文件；请将结果保存为 Markdown、TXT 或 CSV。".into());
    }
    std::fs::write(&resolved, content).map_err(|e| e.to_string())
}

/// Create a directory inside the work directory. The parent must already
/// exist, so every path component is validated before it can be created.
pub fn create_directory_core(work_dir: &Path, path: &str) -> Result<(), String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("directory path cannot be empty".to_string());
    }
    let resolved =
        resolve_and_validate(work_dir, Path::new(path), false).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&resolved).map_err(|e| e.to_string())
}

/// Copy a file or move/rename one file or directory inside the workspace.
/// Existing destinations are never overwritten, which keeps every successful
/// move reversible by swapping `source` and `destination` in a later call.
pub fn organize_workspace_item_core(
    work_dir: &Path,
    operation: &str,
    source: &str,
    destination: &str,
) -> Result<(), String> {
    let source = source.trim();
    let destination = destination.trim();
    if source.is_empty() || destination.is_empty() {
        return Err("source and destination cannot be empty".to_string());
    }
    if !matches!(operation, "copy" | "move") {
        return Err("operation must be copy or move".to_string());
    }

    let source_candidate = if Path::new(source).is_absolute() {
        Path::new(source).to_path_buf()
    } else {
        work_dir.join(source)
    };
    let source_path =
        resolve_and_validate(work_dir, Path::new(source), true).map_err(|e| e.to_string())?;
    let source_metadata = std::fs::symlink_metadata(&source_candidate).map_err(|error| {
        format!(
            "source item is unavailable ({}): {error}",
            source_candidate.display()
        )
    })?;
    if source_metadata.file_type().is_symlink() {
        return Err("symbolic links and junction-like file links cannot be organized".to_string());
    }
    let work_root = std::fs::canonicalize(work_dir).map_err(|e| e.to_string())?;
    if source_path == work_root {
        return Err("the workspace root itself cannot be moved or copied".to_string());
    }
    let destination_path =
        resolve_and_validate(work_dir, Path::new(destination), false).map_err(|e| e.to_string())?;
    if destination_path.try_exists().map_err(|e| e.to_string())? {
        return Err("destination already exists; AngelBot will not overwrite it".to_string());
    }
    if source_path == destination_path {
        return Err("source and destination refer to the same item".to_string());
    }
    if source_path.is_dir() && destination_path.starts_with(&source_path) {
        return Err("a directory cannot be moved inside itself".to_string());
    }

    match operation {
        "copy" if source_path.is_dir() => {
            return Err(
                "copy currently supports files only; directories can be moved or renamed"
                    .to_string(),
            )
        }
        "copy" => {
            let parent = destination_path.parent().ok_or("复制目标位置无效")?;
            let mut temporary =
                tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
            let mut source =
                std::fs::File::open(&source_path).map_err(|error| error.to_string())?;
            std::io::copy(&mut source, &mut temporary).map_err(|error| error.to_string())?;
            temporary
                .as_file()
                .sync_all()
                .map_err(|error| error.to_string())?;
            temporary
                .persist_noclobber(&destination_path)
                .map_err(|error| error.to_string())?;
            if !source_path.is_file() || !destination_path.is_file() {
                return Err("post-call verification failed after copying the file".to_string());
            }
        }
        "move" => {
            std::fs::rename(&source_path, &destination_path).map_err(|e| e.to_string())?;
            if source_path.try_exists().map_err(|e| e.to_string())?
                || !destination_path.try_exists().map_err(|e| e.to_string())?
            {
                return Err("post-call verification failed after moving the item".to_string());
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

#[command]
pub fn read_file(state: tauri::State<'_, crate::AppState>, path: String) -> Result<String, String> {
    let work_dir = resolve_work_dir(&state, None)?;
    read_file_core(&work_dir, &path)
}

/// Read a text file using the active session's work-directory boundary.
#[command]
pub fn read_session_file(
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
    path: String,
) -> Result<String, String> {
    let work_dir = resolve_work_dir(&state, Some(&session_id))?;
    read_file_core(&work_dir, &path)
}

/// Save a user-edited text file inside the session work directory.
/// This is deliberately separate from Agent tools: the user initiated the
/// mutation in the file panel, while the same work-directory sandbox applies.
#[command]
pub fn write_session_file(
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
    path: String,
    content: String,
) -> Result<(), String> {
    let work_dir = resolve_work_dir(&state, Some(&session_id))?;
    write_file_core(&work_dir, &path, &content)
}

/// Browse the active session's work directory without granting write access.
#[command]
pub fn list_work_dir(
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
    path: String,
) -> Result<Vec<WorkDirEntry>, String> {
    list_work_dir_impl(&state, &session_id, &path)
}

pub(crate) fn list_work_dir_impl(
    state: &crate::AppState,
    session_id: &str,
    path: &str,
) -> Result<Vec<WorkDirEntry>, String> {
    let work_dir = resolve_work_dir(state, Some(session_id))?;
    let mut entries = list_work_dir_core(&work_dir, path)?;
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    let personal: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sessions s JOIN projects p ON p.id=s.project_id WHERE s.id=?1 AND p.kind='personal')", [session_id], |row| row.get(0)).map_err(|error| error.to_string())?;
    if personal {
        if let Some(owned) = crate::browser_storage::owned_namespace(&conn)? {
            for entry in &mut entries {
                entry.temporary = Some(work_dir.join(&entry.path).starts_with(&owned));
            }
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn personal_files_are_durable_without_granting_project_or_delegation_scope() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("data.db");
        let conn = rusqlite::Connection::open(&database).unwrap();
        crate::db::migrate(&conn).unwrap();
        let root = resolve_work_dir_from_connection(&conn, Some("personal-main")).unwrap();
        assert_eq!(
            root,
            directory.path().canonicalize().unwrap().join("results")
        );
        write_file_core(&root, "清单.csv", "事项,状态\n采购,待办\n").unwrap();
        assert_eq!(list_work_dir_core(&root, "").unwrap()[0].name, "清单.csv");
        assert!(resolve_delegation_workspace_root(&conn, "personal-main")
            .unwrap()
            .is_none());
        assert!(read_file_core(&root, "../data.db").is_err());
        assert!(write_file_core(&root, "../outside.txt", "no").is_err());
        for path in [
            "NUL",
            "CON.txt",
            "record.csv:stream",
            "alias.txt.",
            "bad\n.txt",
        ] {
            assert!(matches!(
                resolve_and_validate(&root, Path::new(path), false),
                Err(FileReadError::InvalidPath)
            ));
        }
        assert!(resolve_work_dir_from_connection(&conn, Some("missing")).is_err());
        drop(conn);
        let reopened = rusqlite::Connection::open(database).unwrap();
        assert_eq!(
            read_file_core(
                &resolve_work_dir_from_connection(&reopened, Some("personal-main")).unwrap(),
                "清单.csv"
            )
            .unwrap(),
            "事项,状态\n采购,待办\n"
        );
    }

    #[test]
    fn project_workspace_keeps_its_delegation_root() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        let root = tempdir().unwrap();
        let root_path = root.path().to_string_lossy().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, kind, updated_at, active_session_id)
             VALUES ('project-1', 'Project', ?1, 1, 'project', 1, NULL)",
            rusqlite::params![root_path],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, updated_at, context_version, work_dir, project_id)
             VALUES ('project-main', '', 1, 1, 0, ?1, 'project-1')",
            rusqlite::params![root_path],
        )
        .unwrap();
        conn.execute(
            "UPDATE projects SET active_session_id = 'project-main' WHERE id = 'project-1'",
            [],
        )
        .unwrap();

        let resolved = resolve_delegation_workspace_root(&conn, "project-main")
            .unwrap()
            .expect("project workspace must keep delegation available");

        assert_eq!(resolved, root.path());
    }

    #[test]
    fn read_existing_file_inside_work_dir() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "ok").unwrap();
        let resolved = resolve_and_validate(dir.path(), Path::new("notes.txt"), true).unwrap();
        assert_eq!(std::fs::read_to_string(resolved).unwrap(), "ok");
    }

    #[test]
    fn write_new_file_inside_work_dir() {
        let dir = tempdir().unwrap();
        let result = resolve_and_validate(dir.path(), Path::new("new_file.txt"), false);
        assert!(result.is_ok());
        let path = result.unwrap();
        std::fs::write(&path, "hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn create_directory_stays_inside_work_dir() {
        let dir = tempdir().unwrap();
        create_directory_core(dir.path(), "src").unwrap();
        assert!(dir.path().join("src").is_dir());
        assert!(create_directory_core(dir.path(), "../outside").is_err());
    }

    #[test]
    fn workspace_item_move_is_reversible_and_never_overwrites() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("before.txt"), "content").unwrap();

        organize_workspace_item_core(dir.path(), "move", "before.txt", "after.txt").unwrap();
        assert!(!dir.path().join("before.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("after.txt")).unwrap(),
            "content"
        );

        organize_workspace_item_core(dir.path(), "move", "after.txt", "before.txt").unwrap();
        std::fs::write(dir.path().join("occupied.txt"), "keep").unwrap();
        let error = organize_workspace_item_core(dir.path(), "move", "before.txt", "occupied.txt")
            .unwrap_err();
        assert!(error.contains("not overwrite"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("occupied.txt")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn workspace_item_copy_stays_inside_workspace_and_rejects_directories() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        std::fs::write(dir.path().join("source.txt"), "copy me").unwrap();
        std::fs::create_dir(dir.path().join("folder")).unwrap();

        organize_workspace_item_core(dir.path(), "copy", "source.txt", "copy.txt").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("copy.txt")).unwrap(),
            "copy me"
        );
        assert!(organize_workspace_item_core(
            dir.path(),
            "copy",
            "source.txt",
            outside.path().join("escaped.txt").to_str().unwrap(),
        )
        .is_err());
        assert!(
            organize_workspace_item_core(dir.path(), "copy", "folder", "folder-copy")
                .unwrap_err()
                .contains("files only")
        );
    }

    #[test]
    fn write_existing_file_inside_work_dir_returns_canonical_path() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "before").unwrap();

        let resolved = resolve_and_validate(dir.path(), Path::new("notes.txt"), false).unwrap();
        assert_eq!(resolved, std::fs::canonicalize(&file).unwrap());
    }

    #[test]
    fn lists_directories_before_files_with_relative_paths() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ok").unwrap();

        let entries = list_work_dir_core(dir.path(), "").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "docs");
        assert!(entries[0].is_directory);
        assert_eq!(entries[1].path, "notes.txt");
        assert_eq!(entries[1].size, Some(2));
    }

    #[test]
    fn remembers_a_selected_session_directory_as_the_global_default() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL);",
        ).unwrap();
        let selected = "D:/Projects/last-workspace";
        remember_work_dir(&conn, selected, 1).unwrap();
        let remembered: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'work_directory'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remembered, selected);
    }

    #[test]
    fn workspace_sessions_cannot_retarget_or_expand_file_scope() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, project_id TEXT);\
             INSERT INTO sessions (id, project_id) VALUES ('project-session', 'project-1');\
             INSERT INTO sessions (id, project_id) VALUES ('legacy-session', NULL);",
        )
        .unwrap();

        let error = require_legacy_file_configuration(&conn, "project-session").unwrap_err();
        assert!(error.contains("工作区的目录与文件范围"));
        assert!(require_legacy_file_configuration(&conn, "legacy-session").is_ok());
    }

    #[test]
    fn rejects_parent_traversal() {
        let dir = tempdir().unwrap();
        let err = resolve_and_validate(dir.path(), Path::new("../secret.txt"), false).unwrap_err();
        assert!(matches!(err, FileReadError::EscapesWorkDir));
    }

    #[test]
    #[cfg(any(windows, unix))]
    fn rejects_dangling_directory_link_as_new_write_target() {
        let fixture = tempdir().unwrap();
        let root = fixture.path().join("workspace");
        let outside = fixture.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let link = root.join("redirect");
        crate::tests::create_test_directory_link(&link, &outside);
        std::fs::remove_dir(&outside).unwrap();
        assert!(std::fs::symlink_metadata(&link).is_ok());
        assert!(!link.try_exists().unwrap());
        let result = resolve_and_validate(&root, Path::new("redirect"), false);
        #[cfg(windows)]
        std::fs::remove_dir(&link).unwrap();
        #[cfg(unix)]
        std::fs::remove_file(&link).unwrap();
        assert!(
            result.is_err(),
            "an existing dangling link is not a fresh writable path"
        );
        assert!(!outside.exists());
    }

    #[test]
    fn rejects_absolute_path_outside_work_dir() {
        let dir = tempdir().unwrap();
        let outside = dir.path().parent().unwrap().join("secret.txt");
        let err = resolve_and_validate(dir.path(), &outside, false).unwrap_err();
        assert!(matches!(err, FileReadError::EscapesWorkDir));
    }

    #[test]
    fn read_file_rejects_absolute_path_outside_work_dir() {
        let dir = tempdir().unwrap();
        let outside_dir = tempdir().unwrap();
        let outside_file = outside_dir.path().join("notes.txt");
        std::fs::write(&outside_file, "hello-from-elsewhere").unwrap();

        let error = read_file_core(dir.path(), outside_file.to_str().unwrap()).unwrap_err();
        assert!(error.contains("path escapes the work directory"));
    }

    #[test]
    fn read_file_accepts_absolute_path_inside_work_dir() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "ok").unwrap();

        let content = read_file_core(dir.path(), file.to_str().unwrap()).unwrap();
        assert_eq!(content, "ok");
    }

    #[test]
    fn read_file_resolves_relative_paths_against_work_dir() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ok").unwrap();
        let content = read_file_core(dir.path(), "notes.txt").unwrap();
        assert_eq!(content, "ok");
        let binary = [0, 255, 1, 128];
        std::fs::write(dir.path().join("download.bin"), binary).unwrap();
        assert_eq!(
            read_file_core(dir.path(), "download.bin").unwrap_err(),
            "该文件不是 UTF-8 文本，暂不支持文字预览。原文件保留，可另存后用对应应用打开。"
        );
        assert_eq!(
            std::fs::read(dir.path().join("download.bin")).unwrap(),
            binary
        );
    }

    #[test]
    fn read_file_returns_actionable_error_when_path_missing() {
        let dir = tempdir().unwrap();
        let err = read_file_core(dir.path(), "does-not-exist.txt").unwrap_err();
        assert!(err.contains("file not found"));
    }

    // Issue #106: write_file keeps the sandbox and explicitly tells the user
    // to switch work directories when the target is outside.
    #[test]
    fn write_file_outside_work_dir_suggests_switching_work_dir() {
        let work_dir = tempdir().unwrap();
        let outside_dir = tempdir().unwrap();
        let outside_target = outside_dir.path().join("foo.txt");
        let err = write_file_core(work_dir.path(), outside_target.to_str().unwrap(), "hello")
            .unwrap_err();
        assert!(err.contains("write_file 仅允许写入工作目录"));
        assert!(err.contains("设置"));
        // The write must NOT have happened.
        assert!(!outside_target.exists());
    }

    #[test]
    fn write_file_inside_work_dir_succeeds() {
        let work_dir = tempdir().unwrap();
        write_file_core(work_dir.path(), "inside.txt", "ok").unwrap();
        let written = work_dir.path().join("inside.txt");
        assert!(written.exists());
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "ok");
    }

    #[test]
    fn office_reads_are_bounded_and_text_writes_cannot_destroy_packages() {
        let dir = tempdir().unwrap();
        for name in ["notes.DOCX", "expenses.xlsx"] {
            let path = dir.path().join(name);
            let bytes = b"original Office package";
            std::fs::write(&path, bytes).unwrap();
            assert!(read_file_core(dir.path(), name).is_err());
            assert!(write_file_core(dir.path(), name, "replacement")
                .unwrap_err()
                .contains("不能用文本写入"));
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            std::fs::File::create(&path)
                .unwrap()
                .set_len((MAX_DOCUMENT_BYTES + 1) as u64)
                .unwrap();
            assert!(read_file_core(dir.path(), name)
                .unwrap_err()
                .contains("4 MiB"));
        }
        assert!(write_file_core(dir.path(), "new.docx", "not an Office package").is_err());
        assert!(!dir.path().join("new.docx").exists());
    }

    #[test]
    #[cfg(windows)]
    fn display_work_dir_hides_windows_extended_path_prefix() {
        assert_eq!(
            display_work_dir(Path::new(r"\\?\D:\Projects\desktop\AngelBot")),
            r"D:\Projects\desktop\AngelBot"
        );
        assert_eq!(
            display_work_dir(Path::new(r"\\?\UNC\server\share\project")),
            r"\\server\share\project"
        );
    }
}

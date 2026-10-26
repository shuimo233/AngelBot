//! Explicit, preview-bound removal of old browser scratch files. No scheduler,
//! recursive deletion, arbitrary paths, or ownership inferred from extensions.
use crate::AppState;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const RETENTION: i64 = 7 * 24 * 60 * 60;
const PREVIEW_LIFETIME: i64 = 300;
const BATCH: usize = 1024;
const MAX_ENTRIES: usize = 50_000;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CleanupRequest {
    pub cutoff: i64,
    pub fingerprint: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupPreview {
    pub cutoff: i64,
    pub fingerprint: String,
    pub file_count: usize,
    pub bytes: u64,
    pub eligible_files: usize,
    pub files: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupResult {
    pub deleted_files: usize,
    pub deleted_bytes: u64,
    pub remaining_files: usize,
    pub partial: bool,
}

#[derive(Serialize)]
struct Record {
    path: String,
    signature: (u64, u64, u64, bool), // size, modified, created, readonly
}

struct Inventory {
    preview: CleanupPreview,
    files: Vec<(PathBuf, File, u64)>,
    _guards: Vec<File>,
}

fn nanos(time: SystemTime) -> Result<u64, String> {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .try_into()
        .map_err(|_| "临时文件时间超出安全范围。".into())
}

fn signature(metadata: &std::fs::Metadata) -> Result<(u64, u64, u64, bool), String> {
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("临时区含非普通文件，未清理。".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("临时区含重定向文件，未清理。".into());
        }
    }
    Ok((
        metadata.len(),
        nanos(metadata.modified().map_err(|error| error.to_string())?)?,
        nanos(metadata.created().unwrap_or(UNIX_EPOCH))?,
        metadata.permissions().readonly(),
    ))
}

fn directory_guard(path: &Path) -> Result<File, String> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        };
        let file = File::options()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|_| "临时目录正在使用或不可访问，未清理。")?;
        super::validate_directory(path)?;
        return Ok(file);
    }
    #[cfg(not(windows))]
    {
        File::open(path).map_err(|error| error.to_string())
    }
}

fn selected_file(path: &Path) -> Result<File, String> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_FLAG_OPEN_REPARSE_POINT};
        // Exclusive access rejects existing readers/writers and prevents a
        // selected leaf from being modified or renamed before deletion.
        return File::options()
            .read(true)
            .access_mode(0x80000000 | DELETE)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|_| {
                "有旧临时文件正在使用、只读或不可访问；未清理，请关闭占用后重新查看。".into()
            });
    }
    #[cfg(not(windows))]
    {
        File::open(path).map_err(|error| error.to_string())
    }
}

fn file_identity(file: &File) -> Result<(u64, [u8; 16]), String> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO,
        };
        let mut identity = FILE_ID_INFO::default();
        // SAFETY: File owns a valid handle; the buffer matches FileIdInfo and
        // remains initialized and live through the synchronous call.
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                &mut identity as *mut _ as *mut _,
                std::mem::size_of::<FILE_ID_INFO>() as u32,
            )
        } == 0
        {
            return Err("无法核对临时文件身份，未清理。".into());
        }
        return Ok((identity.VolumeSerialNumber, identity.FileId.Identifier));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        let mut identity = [0; 16];
        identity[..8].copy_from_slice(&metadata.ino().to_le_bytes());
        Ok((metadata.dev(), identity))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = file;
        Err("当前系统不支持核对临时文件身份。".into())
    }
}

fn inventory(conn: &Connection, cutoff: i64) -> Result<Inventory, String> {
    let cutoff_nanos = u64::try_from(cutoff)
        .ok()
        .and_then(|value| value.checked_mul(1_000_000_000))
        .ok_or("清理截止时间超出安全范围。")?;
    let root = super::owned_namespace(conn)?;
    let mut records = Vec::new();
    let mut guards = Vec::new();
    let mut targets = Vec::new();
    if let Some(root) = &root {
        // Keep the profile, results and namespace names stable on Windows.
        for parent in [
            root.parent().and_then(Path::parent),
            root.parent(),
            Some(root.as_path()),
        ]
        .into_iter()
        .flatten()
        {
            guards.push(directory_guard(parent)?);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            };
            let marker = File::options()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(root.join(super::MARKER))
                .map_err(|_| "临时区归属标记正在变更，未清理。")?;
            signature(&marker.metadata().map_err(|error| error.to_string())?)?;
            guards.push(marker);
        }
        let mut pending = vec![root.clone()];
        let mut entries = 0;
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
                entries += 1;
                if entries > MAX_ENTRIES {
                    return Err("临时区超过本次检查上限 50000 项，请先在文件工作台整理。".into());
                }
                let path = entry.map_err(|error| error.to_string())?.path();
                if path == root.join(super::MARKER) {
                    continue;
                }
                let metadata =
                    std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
                if metadata.file_type().is_symlink()
                    || path.canonicalize().map_err(|error| error.to_string())? != path
                {
                    return Err("临时区含链接或重定向路径，未清理。".into());
                }
                if metadata.is_dir() {
                    guards.push(directory_guard(&path)?);
                    pending.push(path);
                    continue;
                }
                let relative = path
                    .strip_prefix(root)
                    .map_err(|error| error.to_string())?
                    .to_str()
                    .ok_or("临时文件名无法安全显示。")?
                    .replace('\\', "/");
                records.push(Record {
                    path: relative,
                    signature: signature(&metadata)?,
                });
            }
        }
        // Marker validation is separate from file-name classification and is
        // repeated after locking the directory chain.
        if super::owned_namespace(conn)?.as_ref() != Some(root) {
            return Err("浏览器临时区已变化。".into());
        }
        records.sort_by(|left, right| left.path.cmp(&right.path));
        for record in &records {
            let latest = record.signature.1.max(record.signature.2);
            if latest <= cutoff_nanos && !record.signature.3 {
                targets.push(record);
            }
        }
    }
    let eligible_files = targets.len();
    let mut files = Vec::new();
    let mut names = Vec::new();
    let mut bytes = 0u64;
    let mut identities = Vec::new();
    if let Some(root) = &root {
        for record in targets.into_iter().take(BATCH) {
            let path = root.join(&record.path);
            let file = selected_file(&path)?;
            if signature(&file.metadata().map_err(|error| error.to_string())?)? != record.signature
            {
                return Err("临时文件在检查期间已变化，请重新查看；未清理。".into());
            }
            bytes = bytes
                .checked_add(record.signature.0)
                .ok_or("临时文件大小超出范围。")?;
            names.push(record.path.clone());
            identities.push(file_identity(&file)?);
            files.push((path, file, record.signature.0));
        }
    }
    let fingerprint = hex::encode(Sha256::digest(
        serde_json::to_vec(&(
            root.as_ref().map(|path| path.to_string_lossy()),
            cutoff,
            &records,
            &identities,
        ))
        .map_err(|error| error.to_string())?,
    ));
    Ok(Inventory {
        preview: CleanupPreview {
            cutoff,
            fingerprint,
            file_count: files.len(),
            bytes,
            eligible_files,
            files: names,
        },
        files,
        _guards: guards,
    })
}

fn guarded<T>(
    state: &AppState,
    operation: impl FnOnce(&Connection) -> Result<T, String>,
) -> Result<T, String> {
    state.steering.with_idle(|| state.mcp_manager.try_with_policy_gate(|| {
        let conn = state.db.lock().map_err(|error| error.to_string())?;
        crate::commands::settings::require_data_idle(&conn)?;
        let pending: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM task_runs WHERE resumable=1 OR confirmation_state!='none' OR status='awaiting_confirmation')", [], |row| row.get(0)).map_err(|error| error.to_string())?;
        if pending { return Err("仍有可接续或待确认的任务，请先处理后再清理临时文件。".into()); }
        if let Some(namespace) = super::relative_namespace(&conn)? {
            let mut query = conn.prepare("SELECT id FROM mcp_servers WHERE instr(lower(COALESCE(args,'')),lower(?1))>0").map_err(|error| error.to_string())?;
            let ids = query.query_map([namespace], |row| row.get::<_, String>(0)).map_err(|error| error.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|error| error.to_string())?;
            state.mcp_manager.require_servers_stopped(&ids)?;
        }
        operation(&conn)
    }))
}

pub(crate) fn preview(state: &AppState) -> Result<CleanupPreview, String> {
    preview_at(state, chrono::Utc::now().timestamp())
}

fn preview_at(state: &AppState, now: i64) -> Result<CleanupPreview, String> {
    let cutoff = now
        .checked_sub(RETENTION)
        .filter(|cutoff| *cutoff >= 0)
        .ok_or("系统时间不可用于清理。")?;
    guarded(state, |conn| Ok(inventory(conn, cutoff)?.preview))
}

pub(crate) fn execute(state: &AppState, request: CleanupRequest) -> Result<CleanupResult, String> {
    execute_at(state, request, chrono::Utc::now().timestamp())
}

fn execute_at(
    state: &AppState,
    request: CleanupRequest,
    now: i64,
) -> Result<CleanupResult, String> {
    let valid = request.cutoff >= 0
        && request.fingerprint.len() == 64
        && request
            .fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        && request
            .cutoff
            .checked_add(RETENTION)
            .and_then(|created| now.checked_sub(created))
            .is_some_and(|age| (0..=PREVIEW_LIFETIME).contains(&age));
    if !valid {
        return Err("清理预览已过期，请重新查看。".into());
    }
    #[cfg(not(windows))]
    {
        let _ = (state, request);
        Err("当前只支持在 Windows 清理浏览器临时文件。".into())
    }
    #[cfg(windows)]
    guarded(state, |conn| {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
        };
        let mut inventory = inventory(conn, request.cutoff)?;
        if inventory.preview.fingerprint != request.fingerprint {
            return Err("临时文件清单已变化，请重新查看后确认；未清理。".into());
        }
        let mut deleted_files = 0;
        let mut deleted_bytes = 0;
        let mut partial = false;
        for (path, file, size) in inventory.files.drain(..) {
            let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
            // SAFETY: this is the exclusively held, no-reparse handle checked
            // against the reviewed inventory; all parent directory guards live.
            let deleted = unsafe {
                SetFileInformationByHandle(
                    file.as_raw_handle(),
                    FileDispositionInfo,
                    &disposition as *const _ as *const _,
                    std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
                )
            } != 0;
            drop(file);
            if !deleted || path.try_exists().unwrap_or(true) {
                partial = true;
                break;
            }
            deleted_files += 1;
            deleted_bytes += size;
        }
        Ok(CleanupResult {
            deleted_files,
            deleted_bytes,
            remaining_files: inventory.preview.eligible_files - deleted_files,
            partial,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture() -> (tempfile::TempDir, AppState, PathBuf, i64) {
        let directory = tempfile::tempdir().unwrap();
        let conn = crate::db::init(&directory.path().join("data.db")).unwrap();
        crate::db::migrate(&conn).unwrap();
        let output = super::super::prepare_output(&conn, "browser").unwrap();
        std::fs::write(output.join("old.log"), "old cache").unwrap();
        let now = chrono::Utc::now().timestamp() + RETENTION + 86400;
        let recent = output.join("recent.yml");
        std::fs::write(&recent, "recent cache").unwrap();
        File::options()
            .write(true)
            .open(&recent)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(UNIX_EPOCH + std::time::Duration::from_secs(now as u64)),
            )
            .unwrap();
        let state = crate::tests::test_app_state();
        *state.db.lock().unwrap() = conn;
        (directory, state, output, now)
    }

    fn request(preview: &CleanupPreview) -> CleanupRequest {
        CleanupRequest {
            cutoff: preview.cutoff,
            fingerprint: preview.fingerprint.clone(),
        }
    }

    #[test]
    fn previews_are_explicit_bounded_and_protect_pending_work_and_new_files() {
        let (_directory, state, output, now) = fixture();
        assert_eq!(preview(&state).unwrap().file_count, 0);
        let boundary = output.join("boundary.log");
        std::fs::write(&boundary, "just newer than cutoff").unwrap();
        File::options()
            .write(true)
            .open(&boundary)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(
                UNIX_EPOCH
                    + std::time::Duration::from_secs((now - RETENTION) as u64)
                    + std::time::Duration::from_millis(500),
            ))
            .unwrap();
        let first = preview_at(&state, now).unwrap();
        assert_eq!(first.file_count, 1);
        assert!(first.files[0].ends_with("/old.log"));
        assert!(!first.files.iter().any(|path| path.ends_with("recent.yml")));
        assert!(!first
            .files
            .iter()
            .any(|path| path.ends_with("boundary.log")));
        // Preview releases all handles; it never retains a lock while the user
        // is deciding. New content changes the next fingerprint.
        std::fs::write(output.join("old.log"), "changed cache").unwrap();
        let changed = preview_at(&state, now).unwrap();
        assert_ne!(first.fingerprint, changed.fingerprint);
        assert!(execute_at(&state, request(&first), now + PREVIEW_LIFETIME + 1).is_err());
        assert!(execute_at(
            &state,
            CleanupRequest {
                cutoff: now,
                fingerprint: first.fingerprint.clone()
            },
            now
        )
        .is_err());
        for cutoff in [i64::MIN, -1, i64::MAX] {
            assert!(execute_at(
                &state,
                CleanupRequest {
                    cutoff,
                    fingerprint: first.fingerprint.clone()
                },
                now
            )
            .is_err());
        }
        assert!(execute_at(
            &state,
            CleanupRequest {
                cutoff: first.cutoff,
                fingerprint: "invalid".into()
            },
            now
        )
        .is_err());
        {
            let conn = state.db.lock().unwrap();
            conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES('cleanup-msg','personal-main','user','fixture',1)", []).unwrap();
            conn.execute("INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,confirmation_state,resumable,created_at,updated_at) VALUES('cleanup-run','personal-main','cleanup-msg','fixture','awaiting_confirmation','[]','pending',1,1,1)", []).unwrap();
        }
        assert!(preview_at(&state, now).unwrap_err().contains("待确认"));
        state.db.lock().unwrap().execute("UPDATE task_runs SET status='stopped',confirmation_state='none',resumable=0 WHERE id='cleanup-run'", []).unwrap();
        assert!(preview_at(&state, now).is_ok());
        for index in 0..BATCH + 1 {
            std::fs::write(output.join(format!("batch-{index:04}.txt")), "x").unwrap();
        }
        let batch = preview_at(&state, now).unwrap();
        assert_eq!(batch.file_count, BATCH);
        assert_eq!(batch.eligible_files, BATCH + 2);
        assert_eq!(batch.files.len(), BATCH);
        assert_eq!(
            std::fs::read_to_string(output.join("old.log")).unwrap(),
            "changed cache"
        );
        #[cfg(not(windows))]
        assert!(execute_at(&state, request(&batch), now)
            .unwrap_err()
            .contains("Windows"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_cleanup_uses_reviewed_handles_and_preserves_external_and_recent_files() {
        let (directory, state, output, now) = fixture();
        let root = crate::commands::file::personal_files_root(&state.db.lock().unwrap()).unwrap();
        std::fs::write(root.join("formal.log"), "formal result").unwrap();
        std::fs::write(directory.path().join("outside.yml"), "outside").unwrap();
        let script = directory.path().join("mcp-cleanup.cjs");
        std::fs::write(&script, r#"require('readline').createInterface({input:process.stdin}).on('line',line=>{const r=JSON.parse(line);if(r.method==='initialize')process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:r.id,result:{protocolVersion:'2024-11-05',capabilities:{tools:{}},serverInfo:{name:'fixture',version:'1'}}})+'\n')});"#).unwrap();
        let managed_args = serde_json::to_string(&[
            script.to_string_lossy().to_string(),
            "--output-dir".into(),
            output.to_string_lossy().to_string(),
        ])
        .unwrap();
        let other_args = serde_json::to_string(&[script.to_string_lossy()]).unwrap();
        let namespace = super::super::relative_namespace(&state.db.lock().unwrap())
            .unwrap()
            .unwrap();
        let case_variant = managed_args.replace(&namespace, &namespace.to_ascii_uppercase());
        state.db.lock().unwrap().execute("INSERT INTO mcp_servers(id,name,command,args,enabled,created_at) VALUES('browser','fixture','node',?1,1,1)", [&case_variant]).unwrap();
        state
            .mcp_manager
            .start("browser", "fixture", "node", &managed_args, "")
            .unwrap();
        assert!(preview_at(&state, now).unwrap_err().contains("停止"));
        assert!(output.join("old.log").exists());
        state.mcp_manager.stop("browser").unwrap();
        state
            .mcp_manager
            .start("unrelated", "fixture", "node", &other_args, "")
            .unwrap();
        let first = preview_at(&state, now).unwrap();
        {
            let conn = state.db.lock().unwrap();
            let locked = inventory(&conn, first.cutoff).unwrap();
            assert!(std::fs::write(output.join("old.log"), "racing writer").is_err());
            assert!(std::fs::rename(&output, output.with_file_name("moved")).is_err());
            let namespace = super::super::owned_namespace(&conn).unwrap().unwrap();
            assert!(std::fs::write(namespace.join(super::super::MARKER), "changed").is_err());
            drop(locked);
        }
        std::fs::write(output.join("new.txt"), "new file").unwrap();
        assert!(execute_at(&state, request(&first), now)
            .unwrap_err()
            .contains("清单已变化"));
        assert!(output.join("old.log").exists());

        // Replace a reviewed leaf while preserving its size and both timestamps.
        // Metadata alone must not authorize deletion of a different file object.
        let before_replace = preview_at(&state, now).unwrap();
        let old = output.join("old.log");
        let metadata = std::fs::metadata(&old).unwrap();
        let old_signature = signature(&metadata).unwrap();
        let old_identity = file_identity(&File::open(&old).unwrap()).unwrap();
        std::fs::remove_file(&old).unwrap();
        std::fs::write(&old, "new cache").unwrap();
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::{Foundation::FILETIME, Storage::FileSystem::SetFileTime};
            let time = |value: SystemTime| {
                let ticks = (value.duration_since(UNIX_EPOCH).unwrap().as_nanos() / 100
                    + 116444736000000000) as u64;
                FILETIME {
                    dwLowDateTime: ticks as u32,
                    dwHighDateTime: (ticks >> 32) as u32,
                }
            };
            let file = File::options().write(true).open(&old).unwrap();
            let created = time(metadata.created().unwrap());
            let modified = time(metadata.modified().unwrap());
            // SAFETY: all handles and FILETIME values belong to this fixture.
            assert_ne!(
                unsafe { SetFileTime(file.as_raw_handle(), &created, std::ptr::null(), &modified) },
                0
            );
        }
        assert_eq!(
            signature(&std::fs::metadata(&old).unwrap()).unwrap(),
            old_signature
        );
        assert_ne!(
            file_identity(&File::open(&old).unwrap()).unwrap(),
            old_identity
        );
        assert!(execute_at(&state, request(&before_replace), now)
            .unwrap_err()
            .contains("清单已变化"));
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "new cache");

        // Reuse the task-owned, hidden directory-link fixture.
        let link = output.join("redirect");
        crate::tests::create_test_directory_link(&link, directory.path());
        assert!(preview_at(&state, now).is_err());
        assert_eq!(
            std::fs::read_to_string(directory.path().join("outside.yml")).unwrap(),
            "outside"
        );
        std::fs::remove_dir(&link).unwrap(); // Removes this junction, not its target.
        let preview = preview_at(&state, now).unwrap();
        let result = execute_at(&state, request(&preview), now).unwrap();
        assert_eq!(result.deleted_files, 2);
        assert!(!result.partial);
        assert!(!output.join("old.log").exists());
        assert!(!output.join("new.txt").exists());
        assert_eq!(
            std::fs::read_to_string(output.join("recent.yml")).unwrap(),
            "recent cache"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("formal.log")).unwrap(),
            "formal result"
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("outside.yml")).unwrap(),
            "outside"
        );
        assert!(execute_at(&state, request(&preview), now).is_err());
        assert_eq!(
            state.mcp_manager.get_status("unrelated").unwrap().status,
            "running"
        );
        state.mcp_manager.stop("unrelated").unwrap();
        crate::commands::settings::clear_all_user_data_impl(&state).unwrap();
        assert!(super::super::owned_namespace(&state.db.lock().unwrap())
            .unwrap()
            .is_some());
    }
}

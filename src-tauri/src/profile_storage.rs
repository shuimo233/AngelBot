//! Owned profile cutover before database recovery, model access or scheduling.
//! Program files and user projects are never migration inputs. The journal is
//! filesystem transaction metadata, not a second task/recovery authority.

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

pub(crate) const DIRECTORY: &str = "com.angelbot.desktop";
const JOURNAL: &str = ".storage-migration-v1";
const NAMES: &[&str] = &[
    "data.db",
    "data.db-wal",
    "data.db-shm",
    "results",
    "skills",
    "delegated",
    "chatgpt-plan.credentials",
    "model-config",
];

#[derive(Debug)]
struct StorageFailure(String);
impl std::fmt::Display for StorageFailure {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(&self.0)
    }
}
impl std::error::Error for StorageFailure {}
fn failure(message: &str) -> io::Error {
    io::Error::other(StorageFailure(message.to_owned()))
}
pub(crate) fn explanation(error: &io::Error) -> Option<&str> {
    error
        .get_ref()?
        .downcast_ref::<StorageFailure>()
        .map(|error| error.0.as_str())
}

#[derive(Clone)]
pub(crate) struct Paths {
    pub legacy: PathBuf,
    pub current: PathBuf,
    pub config: PathBuf,
}

impl Paths {
    pub fn system() -> io::Result<Self> {
        let local =
            dirs::data_local_dir().ok_or_else(|| failure("本地资料目录不可用，未启动。"))?;
        let config = dirs::config_dir().ok_or_else(|| failure("模型配置目录不可用，未启动。"))?;
        Ok(Self {
            legacy: local.join("angelbot"),
            current: local.join(DIRECTORY),
            config: config.join("AngelBot").join("app_config.json"),
        })
    }
    fn journal(&self) -> PathBuf {
        self.current.join(JOURNAL)
    }
    fn source(&self, name: &str) -> PathBuf {
        if name == "model-config" {
            self.config.clone()
        } else {
            self.legacy.join(name)
        }
    }
    fn staged(&self, name: &str) -> PathBuf {
        self.journal().join("staged").join(name)
    }
    fn backup(&self, name: &str) -> PathBuf {
        let root = self.journal().join("previous").join(name);
        if name == "model-config" {
            root.join("app_config.json")
        } else {
            root
        }
    }
    fn has_legacy(&self) -> bool {
        NAMES.iter().any(|name| self.source(name).exists())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    source_hash: String,
    target_hash: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u8,
    phase: String,
    entries: Vec<Entry>,
}

fn read_journal(paths: &Paths) -> io::Result<Option<Journal>> {
    if paths.journal().exists() && !regular(&paths.journal())?.is_dir() {
        return Err(failure("资料迁移记录目录无效。"));
    }
    let file = paths.journal().join("state.json");
    if !file.exists() {
        return Ok(None);
    }
    regular(&file)?;
    let mut bytes = Vec::new();
    File::open(file)?
        .take(32 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 32 * 1024 {
        return Err(failure("资料迁移记录过大，保留现场。"));
    }
    let state: Journal =
        serde_json::from_slice(&bytes).map_err(|_| failure("资料迁移记录损坏，保留现场。"))?;
    let mut names = std::collections::HashSet::new();
    if state.version != 1
        || ![
            "copying",
            "prepared",
            "complete",
            "rolling_back",
            "rolled_back",
        ]
        .contains(&state.phase.as_str())
        || state.entries.iter().any(|entry| {
            !NAMES.contains(&entry.name.as_str())
                || !names.insert(&entry.name)
                || entry.source_hash.len() != 64
                || (!entry.target_hash.is_empty() && entry.target_hash.len() != 64)
        })
    {
        return Err(failure("资料迁移记录不匹配，未修改目录。"));
    }
    Ok(Some(state))
}

fn save_journal(paths: &Paths, state: &Journal) -> io::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(paths.journal())?;
    serde_json::to_writer(&mut file, state).map_err(io::Error::other)?;
    file.as_file().sync_all()?;
    file.persist(paths.journal().join("state.json"))
        .map_err(|error| error.error)?;
    Ok(())
}

fn regular(path: &Path) -> io::Result<fs::Metadata> {
    let meta = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(failure("资料包含重定向目录，未迁移。"));
        }
    }
    if meta.file_type().is_symlink() || (!meta.is_file() && !meta.is_dir()) {
        return Err(failure("资料包含不支持的文件类型，未迁移。"));
    }
    Ok(meta)
}

fn fingerprint(path: &Path) -> io::Result<String> {
    fn visit(path: &Path, digest: &mut Sha256, depth: usize) -> io::Result<()> {
        if depth > 128 {
            return Err(failure("资料目录层级过深，未迁移。"));
        }
        let metadata = regular(path)?;
        if metadata.is_file() {
            digest.update(b"file\0");
            digest.update(metadata.len().to_le_bytes());
            let mut file = File::open(path)?;
            let mut buffer = [0; 64 * 1024];
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                digest.update(&buffer[..count]);
            }
        } else {
            digest.update(b"directory\0");
            let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let name = entry.file_name();
                let name = name
                    .to_str()
                    .ok_or_else(|| failure("资料文件名无法迁移。"))?;
                digest.update((name.len() as u64).to_le_bytes());
                digest.update(name.as_bytes());
                visit(&entry.path(), digest, depth + 1)?;
            }
            digest.update(b"end\0");
        }
        Ok(())
    }
    let mut digest = Sha256::new();
    visit(path, &mut digest, 0)?;
    Ok(hex::encode(digest.finalize()))
}

fn copy_owned(source: &Path, target: &Path, reject_worktree: bool, depth: usize) -> io::Result<()> {
    if depth > 128 {
        return Err(failure("资料目录层级过深，未迁移。"));
    }
    if regular(source)?.is_dir() {
        fs::create_dir(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_owned(
                &entry.path(),
                &target.join(entry.file_name()),
                reject_worktree,
                depth + 1,
            )?;
        }
    } else {
        if reject_worktree && source.file_name().is_some_and(|name| name == ".git") {
            return Err(failure(
                "仍有 Git worktree 路径引用，请先整理子代理现场后迁移。",
            ));
        }
        let mut input = File::open(source)?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?;
        io::copy(&mut input, &mut output)?;
        output.sync_all()?;
    }
    Ok(())
}

// Unlike fs::rename, this must never replace a destination created after the
// preflight. Profile cutover is currently a Windows-only product capability.
fn move_owned(source: &Path, target: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let source: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let target: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                source.as_ptr(),
                target.as_ptr(),
                0,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (source, target);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows profile cutover required",
        ))
    }
}

fn access_file(root: &Path) -> io::Result<File> {
    if !root.is_absolute() {
        return Err(failure("资料目录必须是绝对路径。"));
    }
    fs::create_dir_all(root)?;
    regular(root)?;
    let path = root.join("profile-access.lock");
    if path.exists() {
        regular(&path)?;
    }
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

pub(crate) fn borrow(root: &Path) -> io::Result<File> {
    let guard = access_file(root)?;
    FileExt::try_lock_shared(&guard)
        .map_err(|_| failure("资料正在迁移，请完成恢复后再读取模型或设置。"))?;
    Ok(guard)
}

fn borrow_saved_for(paths: &Paths) -> io::Result<(PathBuf, File)> {
    let root = saved_root_for(paths);
    let guard = borrow(&root)?;
    if let Some(state) = read_journal(paths)? {
        if !["complete", "rolled_back"].contains(&state.phase.as_str())
            || (state.phase == "complete" && paths.has_legacy())
        {
            return Err(failure("资料迁移尚未完成或存在分叉，请先启动普通版处理。"));
        }
    } else if paths.has_legacy() && NAMES.iter().any(|name| paths.current.join(name).exists()) {
        return Err(failure("新旧资料均存在，不能猜测应读取哪个模型或配置。"));
    }
    if saved_root_for(paths) != root {
        return Err(failure("资料目录已切换，请重新读取设置。"));
    }
    Ok((root, guard))
}

pub(crate) fn borrow_saved() -> io::Result<(PathBuf, File)> {
    borrow_saved_for(&Paths::system()?)
}

pub(crate) fn borrow_config() -> io::Result<(PathBuf, File)> {
    let paths = Paths::system()?;
    let (_, guard) = borrow_saved_for(&paths)?;
    Ok((config_directory_for(&paths), guard))
}

/// Read-only owner locator used by native acceptance before a normal upgrade.
/// Automated E2E never uses it to obtain credentials.
pub(crate) fn saved_root() -> PathBuf {
    let paths = Paths::system().expect("Windows local profile directories must be available");
    saved_root_for(&paths)
}

fn saved_root_for(paths: &Paths) -> PathBuf {
    if let Ok(Some(state)) = read_journal(&paths) {
        if state.phase == "rolled_back" {
            return paths.legacy.clone();
        }
        return paths.current.clone();
    }
    if paths.journal().join("state.json").exists() {
        return paths.current.clone();
    }
    if paths.has_legacy() {
        paths.legacy.clone()
    } else {
        paths.current.clone()
    }
}

pub(crate) fn config_directory() -> PathBuf {
    let paths = Paths::system().expect("Windows local profile directories must be available");
    config_directory_for(&paths)
}

fn config_directory_for(paths: &Paths) -> PathBuf {
    if saved_root_for(paths) == paths.legacy {
        paths
            .config
            .parent()
            .expect("configuration parent")
            .to_owned()
    } else {
        paths.current.join("model-config")
    }
}

pub(crate) struct Lease {
    pub root: PathBuf,
    pub migrated: bool,
    pub legacy_recovery: bool,
    _guards: Vec<File>,
}
impl Lease {
    pub fn isolated(root: PathBuf) -> io::Result<Self> {
        if !root.is_absolute() {
            return Err(failure("隔离验收需要明确的绝对资料目录。"));
        }
        let guard = crate::runtime_instance::acquire(&root)?;
        Ok(Self {
            root,
            migrated: false,
            legacy_recovery: false,
            _guards: vec![guard],
        })
    }
}

fn lock_profiles(paths: &Paths) -> io::Result<Vec<File>> {
    if !paths.current.is_absolute() || !paths.legacy.is_absolute() || paths.current == paths.legacy
    {
        return Err(failure("资料目录无效。"));
    }
    for root in [&paths.legacy, &paths.current] {
        fs::create_dir_all(root)?;
        if !regular(root)?.is_dir() {
            return Err(failure("资料根不是目录。"));
        }
    }
    let legacy = paths.legacy.canonicalize()?;
    let current = paths.current.canonicalize()?;
    if legacy.starts_with(&current) || current.starts_with(&legacy) {
        return Err(failure("新旧资料根不能互相包含。"));
    }
    let mut guards = vec![
        crate::runtime_instance::acquire(&paths.legacy)?,
        crate::runtime_instance::acquire(&paths.current)?,
    ];
    for root in [&paths.legacy, &paths.current] {
        let guard = access_file(root)?;
        guard
            .try_lock_exclusive()
            .map_err(|_| failure("资料正被模型或设置读取，未迁移；请结束隔离验收后重试。"))?;
        guards.push(guard);
    }
    for root in [&paths.legacy, &paths.current] {
        let path = root.join("chatgpt-plan.lock");
        if path.exists() {
            regular(&path)?;
        }
        let guard = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        guard
            .try_lock_exclusive()
            .map_err(|_| failure("ChatGPT 正在授权或续期，未迁移资料。"))?;
        guards.push(guard);
    }
    Ok(guards)
}

fn validate_database(paths: &Paths) -> io::Result<()> {
    let database = paths.staged("data.db");
    if !database.exists() {
        return Ok(());
    }
    let conn = crate::db::init(&database).map_err(io::Error::other)?;
    crate::db::migrate(&conn).map_err(io::Error::other)?;
    let check: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(io::Error::other)?;
    if check != "ok" {
        return Err(failure("原资料数据库检查失败，未迁移。"));
    }
    let retained: i64 = conn
        .query_row(
            "SELECT count(*) FROM delegation_resource_bindings WHERE cleanup_status <> 'cleaned'",
            [],
            |row| row.get(0),
        )
        .map_err(io::Error::other)?;
    let materializing: i64 = conn.query_row("SELECT count(*) FROM delegation_materialization_receipts WHERE status IN ('applying','unknown')", [], |row| row.get(0)).map_err(io::Error::other)?;
    if retained != 0 || materializing != 0 {
        return Err(failure(
            "仍有需保留的子代理或落盘现场，请先处理后再迁移资料。",
        ));
    }
    // Only the host-provided Playwright result-directory field is rebased.
    // Existing trigger invalidates its cached contract; reconnect is explicit.
    let rows = {
        let mut query = conn
            .prepare("SELECT id,command,COALESCE(args,'') FROM mcp_servers")
            .map_err(io::Error::other)?;
        let rows = query
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(io::Error::other)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(io::Error::other)?;
        rows
    };
    for (id, command, args) in rows {
        let Ok(mut args) = serde_json::from_str::<Vec<String>>(&args) else {
            continue;
        };
        let command = Path::new(&command)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let is_old_path = |arg: &String| {
            let path = Path::new(arg);
            path.is_absolute()
                && (path.starts_with(&paths.legacy)
                    || path
                        .canonicalize()
                        .ok()
                        .zip(paths.legacy.canonicalize().ok())
                        .is_some_and(|(path, root)| path.starts_with(root)))
        };
        let mentions_old = args.iter().any(&is_old_path);
        if !["npx", "npx.cmd"].contains(&command.to_ascii_lowercase().as_str())
            || !args.iter().any(|arg| arg == "@playwright/mcp@0.0.83")
        {
            if mentions_old {
                conn.execute("UPDATE mcp_servers SET enabled=0 WHERE id=?1", [&id])
                    .map_err(io::Error::other)?;
            }
            continue;
        }
        let positions: Vec<_> = args
            .iter()
            .enumerate()
            .filter(|(_, arg)| *arg == "--output-dir")
            .map(|(index, _)| index)
            .collect();
        if positions.len() != 1 {
            if mentions_old {
                conn.execute("UPDATE mcp_servers SET enabled=0 WHERE id=?1", [&id])
                    .map_err(io::Error::other)?;
            }
            continue;
        }
        let index = positions[0] + 1;
        let runtime = crate::browser_storage::relative_namespace(&conn)
            .map_err(io::Error::other)?
            .map(|namespace| crate::browser_storage::relative_output(&namespace, &id));
        let relative = if args
            .get(index)
            .is_some_and(|arg| same_path(Path::new(arg), &paths.legacy.join("results")))
        {
            Some(PathBuf::from("results"))
        } else {
            runtime
                .map(|runtime| PathBuf::from("results").join(runtime))
                .filter(|relative| {
                    args.get(index)
                        .is_some_and(|arg| same_path(Path::new(arg), &paths.legacy.join(relative)))
                })
        };
        if let Some(relative) = relative {
            args[index] = paths.current.join(relative).to_string_lossy().into_owned();
            conn.execute(
                "UPDATE mcp_servers SET args=?1 WHERE id=?2",
                rusqlite::params![serde_json::to_string(&args).map_err(io::Error::other)?, id],
            )
            .map_err(io::Error::other)?;
            if args.iter().any(&is_old_path) {
                conn.execute("UPDATE mcp_servers SET enabled=0 WHERE id=?1", [&id])
                    .map_err(io::Error::other)?;
            }
        } else if mentions_old {
            conn.execute("UPDATE mcp_servers SET enabled=0 WHERE id=?1", [&id])
                .map_err(io::Error::other)?;
        }
    }
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .map_err(io::Error::other)?;
    Ok(())
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => {
            #[cfg(windows)]
            {
                left.to_string_lossy()
                    .replace('/', "\\")
                    .trim_end_matches('\\')
                    .eq_ignore_ascii_case(
                        right
                            .to_string_lossy()
                            .replace('/', "\\")
                            .trim_end_matches('\\'),
                    )
            }
            #[cfg(not(windows))]
            {
                left == right
            }
        }
    }
}

fn lease(paths: &Paths, mut guards: Vec<File>, migrated: bool, legacy_recovery: bool) -> Lease {
    // Renewal locks cover cutover only. Holding them for the desktop lifetime
    // would deadlock the normal credential adapter's next model request.
    guards.truncate(2);
    Lease {
        root: if legacy_recovery {
            paths.legacy.clone()
        } else {
            paths.current.clone()
        },
        migrated,
        legacy_recovery,
        _guards: guards,
    }
}

fn rollback_preflight(paths: &Paths, state: &Journal) -> io::Result<()> {
    // Preflight every path before restoring anything. Changed/new data is never
    // overwritten by either interrupted recovery or an explicit rollback.
    for entry in &state.entries {
        let backup = paths.backup(&entry.name);
        let target = paths.current.join(&entry.name);
        if backup.exists() {
            let source = paths.source(&entry.name);
            if fingerprint(&backup)? != entry.source_hash
                || (source.exists()
                    && (entry.name != "model-config" || fingerprint(&source)? != entry.source_hash))
            {
                return Err(failure("旧资料或备份发生变化，保留迁移现场。"));
            }
            if target.exists()
                && (entry.target_hash.is_empty() || fingerprint(&target)? != entry.target_hash)
            {
                return Err(failure("新资料已改变，不自动回滚或覆盖，请先备份。"));
            }
            if target.exists() && paths.staged(&entry.name).exists() {
                return Err(failure("迁移暂存目录冲突，保留现场。"));
            }
        } else if state.phase == "complete" {
            return Err(failure("迁移备份不完整，未回滚。"));
        } else if !paths.source(&entry.name).exists() || target.exists() {
            return Err(failure("资料恢复位置不完整或存在冲突，保留现场。"));
        }
    }
    Ok(())
}

// Only the legacy configuration file can cross volumes (redirected Roaming).
// Persist a complete, synced file without replacing an existing destination.
fn copy_config(source: &Path, target: &Path) -> io::Result<()> {
    if !regular(source)?.is_file() {
        return Err(failure("模型配置不是普通文件，未迁移。"));
    }
    let parent = target.parent().ok_or_else(|| failure("配置位置无效"))?;
    fs::create_dir_all(parent)?;
    regular(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    io::copy(&mut File::open(source)?, &mut file)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(target)
        .map_err(|error| error.error)?;
    Ok(())
}

fn rollback_pending(
    paths: &Paths,
    state: &mut Journal,
    checkpoint: &mut impl FnMut(&str) -> io::Result<()>,
) -> io::Result<()> {
    rollback_preflight(paths, state)?;
    state.phase = "rolling_back".into();
    save_journal(paths, state)?;
    for entry in state.entries.iter().rev() {
        let backup = paths.backup(&entry.name);
        if !backup.exists() {
            continue;
        }
        let target = paths.current.join(&entry.name);
        if target.exists() {
            let staged = paths.staged(&entry.name);
            move_owned(&target, &staged)?;
            checkpoint("rollback-uninstalled")?;
        }
        let source = paths.source(&entry.name);
        fs::create_dir_all(source.parent().ok_or_else(|| failure("旧目录不可用"))?)?;
        if !source.exists() {
            if entry.name == "model-config" {
                copy_config(&backup, &source)?;
            } else {
                move_owned(&backup, &source)?;
            }
        }
        checkpoint("rollback-restored")?;
    }
    state.phase = "rolled_back".into();
    save_journal(paths, state)?;
    Ok(())
}

pub(crate) fn open(paths: &Paths, retry: bool, rollback: bool) -> io::Result<Lease> {
    open_observed(paths, retry, rollback, |_| Ok(()))
}

fn open_observed(
    paths: &Paths,
    retry: bool,
    rollback: bool,
    mut checkpoint: impl FnMut(&str) -> io::Result<()>,
) -> io::Result<Lease> {
    if retry && rollback {
        return Err(failure("不能同时选择迁移与回滚。"));
    }
    let guards = lock_profiles(paths)?;
    let journal_root = paths.journal();
    if journal_root.exists() {
        regular(&journal_root)?;
    }
    if let Some(mut state) = read_journal(paths)? {
        if rollback || state.phase == "rolling_back" {
            if state.phase == "rolled_back" {
                return Ok(lease(paths, guards, false, true));
            }
            rollback_pending(paths, &mut state, &mut checkpoint)?;
            return Ok(lease(paths, guards, false, true));
        }
        if state.phase == "complete" {
            if paths.has_legacy() {
                return Err(failure(
                    "旧版重新写入了资料，未合并两个版本；请先检查目录。",
                ));
            }
            return Ok(lease(paths, guards, false, false));
        }
        if state.phase == "rolled_back" && !retry {
            return Ok(lease(paths, guards, false, true));
        }
        if state.phase != "rolled_back" {
            rollback_pending(paths, &mut state, &mut checkpoint)?;
        }
        // Keep previous payloads, including any manually changed files.
        for name in ["staged", "previous"] {
            let previous = journal_root.join(name);
            if previous.exists() {
                regular(&previous)?;
                move_owned(
                    &previous,
                    &journal_root.join(format!("previous-{name}-{}", uuid::Uuid::new_v4())),
                )?;
            }
        }
    } else if rollback {
        return Err(failure("没有可回滚的资料迁移。"));
    }
    if !paths.has_legacy() {
        return Ok(lease(paths, guards, false, false));
    }
    for name in NAMES {
        if paths.current.join(name).exists() {
            return Err(failure("新旧资料均存在，未自动合并或覆盖。"));
        }
    }
    fs::create_dir_all(journal_root.join("staged"))?;
    fs::create_dir_all(journal_root.join("previous"))?;
    let entries = NAMES
        .iter()
        .filter(|name| paths.source(name).exists())
        .map(|name| {
            Ok(Entry {
                name: (*name).into(),
                source_hash: fingerprint(&paths.source(name))?,
                target_hash: String::new(),
            })
        })
        .collect::<io::Result<Vec<_>>>()?;
    let mut state = Journal {
        version: 1,
        phase: "copying".into(),
        entries,
    };
    save_journal(paths, &state)?;
    let attempt = (|| {
        for entry in &state.entries {
            let staged = paths.staged(&entry.name);
            if entry.name == "model-config" {
                fs::create_dir(&staged)?;
                regular(
                    paths
                        .config
                        .parent()
                        .ok_or_else(|| failure("模型配置目录不可用"))?,
                )?;
                copy_owned(&paths.config, &staged.join("app_config.json"), false, 0)?;
            } else {
                copy_owned(
                    &paths.source(&entry.name),
                    &staged,
                    ["skills", "delegated"].contains(&entry.name.as_str()),
                    0,
                )?;
            }
            if fingerprint(&paths.source(&entry.name))? != entry.source_hash {
                return Err(failure("迁移期间原资料发生变化，未切换目录。"));
            }
        }
        validate_database(paths)?;
        // SQLite checkpoint incorporated the captured WAL. Archive the source
        // sidecars for rollback, but never install a stale WAL beside the new DB.
        for name in ["data.db-wal", "data.db-shm"] {
            let staged = paths.staged(name);
            if staged.exists() {
                regular(&staged)?;
                fs::remove_file(staged)?;
            }
        }
        let credential = paths.staged("chatgpt-plan.credentials");
        if credential.exists() {
            let mut bytes = Vec::new();
            File::open(&credential)?
                .take((crate::llm::chatgpt_auth::MAX_CREDENTIAL_FILE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            let bytes = crate::llm::chatgpt_auth::relocate_credential_blob(
                &bytes,
                &paths.source("chatgpt-plan.credentials"),
            )
            .map_err(io::Error::other)?;
            let mut file = File::create(&credential)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        for entry in &mut state.entries {
            if !["data.db-wal", "data.db-shm"].contains(&entry.name.as_str()) {
                entry.target_hash = fingerprint(&paths.staged(&entry.name))?;
            }
        }
        state.phase = "prepared".into();
        save_journal(paths, &state)?;
        checkpoint("prepared")?;
        for entry in &state.entries {
            if fingerprint(&paths.source(&entry.name))? != entry.source_hash {
                return Err(failure("原资料已改变，未采用过期迁移快照。"));
            }
            let backup = paths.backup(&entry.name);
            if backup.exists() {
                return Err(failure("迁移备份已存在，未覆盖。"));
            }
            fs::create_dir_all(backup.parent().ok_or_else(|| failure("迁移备份位置无效"))?)?;
            let source = paths.source(&entry.name);
            if entry.name == "model-config" {
                copy_config(&source, &backup)?;
                checkpoint("config-backed-up")?;
                if fingerprint(&backup)? != entry.source_hash
                    || fingerprint(&source)? != entry.source_hash
                {
                    return Err(failure("模型配置备份与原件不一致，未移除原件。"));
                }
                fs::remove_file(&source)?;
            } else {
                move_owned(&source, &backup)?;
            }
            checkpoint("retired")?;
            if !entry.target_hash.is_empty() {
                move_owned(&paths.staged(&entry.name), &paths.current.join(&entry.name))?;
            }
            checkpoint("installed")?;
        }
        state.phase = "complete".into();
        save_journal(paths, &state)?;
        Ok(())
    })();
    if let Err(error) = attempt {
        rollback_pending(paths, &mut state, &mut checkpoint)?;
        return Err(error);
    }
    Ok(lease(paths, guards, true, false))
}

#[cfg(test)]
#[path = "profile_storage_tests.rs"]
mod tests;

//! One owned MAA GUI queue, using MAA's existing local remote-control protocol.
//! No scheduler, game actions, task configuration, or detached helper lives here.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const CONFIG_LIMIT: u64 = 4 * 1024 * 1024;
const RECORD_LIMIT: usize = 256 * 1024;
const REMOTE_KEYS: [&str; 4] = [
    "RemoteControlGetTaskEndpointUri",
    "RemoteControlReportStatusUri",
    "RemoteControlUserIdentity",
    "RemoteControlDeviceIdentity",
];

pub struct MaaRunResult {
    pub status: &'static str,
    pub summary: String,
    pub output: String,
    pub exit_code: Option<i32>,
}

fn result(status: &'static str, summary: impl Into<String>) -> MaaRunResult {
    MaaRunResult {
        status,
        summary: summary.into(),
        output: String::new(),
        exit_code: None,
    }
}

struct Install {
    exe: PathBuf,
    root: PathBuf,
    config: PathBuf,
    journal: PathBuf,
}

impl Install {
    fn open(executable: &str) -> Result<Self, String> {
        let exe = fs::canonicalize(executable).map_err(|_| "找不到 MAA 程序")?;
        if !exe.is_file()
            || !exe
                .file_name()
                .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("MAA.exe"))
        {
            return Err("请选择 MAA 安装目录中的 MAA.exe".into());
        }
        let root = exe.parent().ok_or("MAA 安装路径无效")?.to_path_buf();
        let config_dir = fs::canonicalize(root.join("config"))
            .map_err(|_| "MAA 尚未生成配置，请先手动打开并配置一次")?;
        if config_dir != root.join("config") {
            return Err("MAA 配置目录不能指向安装目录之外".into());
        }
        let config = fs::canonicalize(config_dir.join("gui.new.json"))
            .map_err(|_| "需要包含 gui.new.json 的新版 MAA 配置")?;
        if config != config_dir.join("gui.new.json") {
            return Err("MAA 配置文件不能指向其他文件".into());
        }
        Ok(Self {
            exe,
            root,
            config,
            journal: config_dir.join("angelbot-routine.pending.json"),
        })
    }

    fn read_config(&self) -> Result<(Vec<u8>, Value), String> {
        let bytes = read_bounded(&self.config)?;
        let config = serde_json::from_slice(&bytes).map_err(|_| "MAA 配置不是有效 JSON")?;
        Ok((bytes, config))
    }
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|_| "无法读取 MAA 配置或恢复记录")?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(CONFIG_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取 MAA 配置或恢复记录")?;
    check_config_size(&bytes)?;
    Ok(bytes)
}

fn check_config_size(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() as u64 > CONFIG_LIMIT {
        return Err("MAA 配置或恢复记录超过安全大小限制".into());
    }
    Ok(())
}

fn profile_name(config: &Value) -> Result<&str, String> {
    config["Current"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "MAA 没有有效的当前配置".into())
}

fn remote<'a>(config: &'a Value, profile: &str) -> Result<&'a Map<String, Value>, String> {
    config["Configurations"][profile]["Gui"]["RemoteControl"]
        .as_object()
        .ok_or_else(|| "MAA 当前配置缺少官方远控设置".into())
}

fn remote_mut<'a>(
    config: &'a mut Value,
    profile: &str,
) -> Result<&'a mut Map<String, Value>, String> {
    config
        .get_mut("Configurations")
        .and_then(|v| v.get_mut(profile))
        .and_then(|v| v.get_mut("Gui"))
        .and_then(|v| v.get_mut("RemoteControl"))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "MAA 配置已改变，保留恢复记录以供检查".into())
}

fn validate_config(config: &Value) -> Result<(), String> {
    let profile = profile_name(config)?;
    let gui = &config["Configurations"][profile]["Gui"];
    remote(config, profile)?;
    if gui["StartUpSettings"]["RunDirectly"].as_bool() != Some(false) {
        return Err("请先关闭 MAA 的启动后直接运行，避免重复执行；AngelBot 不会修改该设置".into());
    }
    if ["StartEmulator", "RestartEmulatorWhenAdbFailed"]
        .iter()
        .any(|key| gui["StartUpSettings"][*key].as_bool() == Some(true))
    {
        return Err("请关闭 MAA 的自动启动/连接失败重启模拟器，由本次流程明确控制启动；AngelBot 不会修改该设置".into());
    }
    if config["Gui"]["MinimizeToTray"].as_bool() == Some(true) {
        return Err("请先关闭 MAA 的关闭时最小化到托盘，让本次运行能够正常退出".into());
    }
    validate_post_actions(gui.get("PostActions"))?;
    let timers = config["Timers"]["List"]
        .as_array()
        .ok_or("MAA 定时设置格式不受支持")?;
    if timers
        .iter()
        .any(|timer| timer["IsEnabled"].as_bool() != Some(false))
    {
        return Err("请先关闭 MAA 原生定时，避免与 AngelBot 的按需执行冲突".into());
    }
    if !config["Configurations"][profile]["TaskQueue"].is_array() {
        return Err("MAA 当前任务队列格式不受支持".into());
    }
    if !config["Configurations"][profile]["TaskQueue"]
        .as_array()
        .unwrap()
        .iter()
        .any(|task| task["IsEnable"].as_bool() == Some(true))
    {
        return Err("MAA 当前没有勾选任务，请先在 MAA 中选择日常任务".into());
    }
    Ok(())
}

fn validate_post_actions(value: Option<&Value>) -> Result<(), String> {
    let safe = match value {
        None => true, // MAA's default is None.
        Some(Value::String(flags)) => {
            !flags.is_empty()
                && flags.split(',').all(|flag| {
                    matches!(
                        flag.trim(),
                        "None"
                            | "ExitArknights"
                            | "BackToAndroidHome"
                            | "ExitSelf"
                            | "IfNoOtherMaa"
                            | "SkipOnError"
                    )
                })
        }
        Some(Value::Number(flags)) => flags.as_u64().is_some_and(|flags| flags & !283 == 0),
        _ => false,
    };
    if !safe {
        return Err("MAA 结束动作包含关闭模拟器、系统电源操作或不受支持的动作；请先在 MAA 中检查，AngelBot 不会修改该配置".into());
    }
    Ok(())
}

/// Read-only validation. A pending recovery record is handled only at execution.
pub fn validate_install(executable: &str) -> Result<(), String> {
    let install = Install::open(executable)?;
    validate_config(&install.read_config()?.1)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u8,
    profile: String,
    original_config: Value,
    patched: [String; 4],
}

fn replace_config(path: &Path, expected: &[u8], value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| "无法序列化 MAA 配置")?;
    check_config_size(&bytes)?;
    let mut replacement = tempfile::NamedTempFile::new_in(path.parent().ok_or("配置路径无效")?)
        .map_err(|_| "无法创建 MAA 配置临时文件")?;
    replacement
        .write_all(&bytes)
        .and_then(|_| replacement.as_file().sync_all())
        .map_err(|_| "无法写入 MAA 配置临时文件")?;
    if read_bounded(path)? != expected {
        return Err("MAA 配置同时被修改，已停止覆盖".into());
    }
    replacement
        .persist(path)
        .map_err(|_| "无法原子更新 MAA 配置")?;
    if read_bounded(path)? != bytes {
        return Err("MAA 配置更新未能回读确认".into());
    }
    Ok(())
}

fn patch_remote(
    install: &Install,
    config: &Value,
    original_bytes: &[u8],
    patched: [String; 4],
) -> Result<(), String> {
    let profile = profile_name(config)?.to_string();
    let journal = Journal {
        version: 1,
        profile: profile.clone(),
        original_config: config.clone(),
        patched: patched.clone(),
    };
    let bytes = serde_json::to_vec_pretty(&journal).map_err(|_| "无法生成 MAA 恢复记录")?;
    check_config_size(&bytes)?;
    let mut updated = config.clone();
    let values = remote_mut(&mut updated, &profile)?;
    for (key, value) in REMOTE_KEYS.iter().zip(patched) {
        values.insert((*key).into(), Value::String(value));
    }
    check_config_size(&serde_json::to_vec_pretty(&updated).map_err(|_| "无法序列化 MAA 配置")?)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&install.journal)
        .map_err(|_| "存在未恢复的 MAA 运行记录，已停止启动")?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "无法保存 MAA 修改前备份，已停止启动")?;
    replace_config(&install.config, original_bytes, &updated)
}

/// Restore only the four temporary fields. Never restore a whole old profile.
fn restore_remote(install: &Install) -> Result<(), String> {
    if !install
        .journal
        .try_exists()
        .map_err(|_| "无法检查 MAA 恢复记录")?
    {
        return Ok(());
    }
    if fs::symlink_metadata(&install.journal)
        .map_err(|_| "无法检查恢复记录")?
        .file_type()
        .is_symlink()
    {
        return Err("MAA 恢复记录不能是链接".into());
    }
    let journal: Journal = serde_json::from_slice(&read_bounded(&install.journal)?)
        .map_err(|_| "MAA 恢复记录不完整，需要人工检查，未覆盖配置")?;
    if journal.version != 1 {
        return Err("MAA 恢复记录版本不受支持".into());
    }
    let original = remote(&journal.original_config, &journal.profile)?;
    let (bytes, mut current) = install.read_config()?;
    let values = remote(&current, &journal.profile)?;
    let unchanged = REMOTE_KEYS
        .iter()
        .all(|key| values.get(*key) == original.get(*key));
    let ours = REMOTE_KEYS
        .iter()
        .zip(&journal.patched)
        .all(|(key, value)| values.get(*key).and_then(Value::as_str) == Some(value.as_str()));
    if !unchanged && !ours {
        return Err("MAA 远控设置已被其他操作改变，保留备份；未覆盖你的修改".into());
    }
    if ours {
        let values = remote_mut(&mut current, &journal.profile)?;
        for key in REMOTE_KEYS {
            match original.get(key) {
                Some(value) => {
                    values.insert(key.into(), value.clone());
                }
                None => {
                    values.remove(key);
                }
            }
        }
        replace_config(&install.config, &bytes, &current)?;
    }
    fs::remove_file(&install.journal).map_err(|_| "MAA 配置已恢复，但恢复记录未能清理".to_string())
}

#[derive(Default)]
struct QueueEvidence {
    ready: bool,
    delivered: bool,
    reported: bool,
    exited: bool,
    teardown_stop: bool,
    starts: HashSet<i64>,
    failed: HashSet<i64>,
    finished: Option<HashSet<i64>>,
    device: Option<String>,
    uncertain: bool,
}

impl QueueEvidence {
    fn callback(&mut self, kind: &str, value: &Value) {
        if !self.delivered {
            if kind == "TaskChainStart" {
                self.uncertain = true;
            }
            return;
        }
        if kind == "Error" {
            self.uncertain = true;
        }
        if kind == "TaskChainStopped" {
            let matching_finished = self.finished.as_ref().is_some_and(|ids| {
                self.starts == *ids
                    && !ids.is_empty()
                    && value["taskid"].as_i64().is_some_and(|id| ids.contains(&id))
                    && self.device.as_deref() == value["uuid"].as_str()
            });
            if matching_finished {
                self.teardown_stop = true;
            } else {
                self.uncertain = true;
            }
        }
        if kind == "TaskChainStart" {
            if self.finished.is_some() {
                self.uncertain = true;
            }
            let Some(id) = value["taskid"].as_i64() else {
                self.uncertain = true;
                return;
            };
            let Some(device) = value["uuid"].as_str() else {
                self.uncertain = true;
                return;
            };
            if self
                .device
                .as_deref()
                .is_some_and(|existing| existing != device)
            {
                self.uncertain = true;
            }
            self.device = Some(device.to_string());
            self.starts.insert(id);
        }
        if kind == "TaskChainError" {
            if let Some(id) = value["taskid"].as_i64() {
                self.failed.insert(id);
            } else {
                self.uncertain = true;
            }
        }
        if kind == "AllTasksCompleted" {
            if self.finished.is_some() || self.device.as_deref() != value["uuid"].as_str() {
                self.uncertain = true;
            }
            self.finished = value["finished_tasks"]
                .as_array()
                .and_then(|ids| ids.iter().map(Value::as_i64).collect());
            if self.finished.is_none() {
                self.uncertain = true;
            }
        }
    }

    fn complete(&self) -> bool {
        !self.uncertain
            && ((self.reported && !self.teardown_stop) || self.exited)
            && !self.starts.is_empty()
            && self.finished.as_ref().is_some_and(|finished| {
                !finished.is_empty() && self.starts == *finished && self.failed.is_subset(finished)
            })
    }

    fn outcome(&self) -> MaaRunResult {
        if !self.complete() {
            return result(
                "attention_required",
                "MAA 执行结果未知，已暂停自动重试；请先检查任务实际状态",
            );
        }
        let mut outcome = if self.failed.is_empty() {
            result(
                "completed",
                "MAA 当前队列已结束；这不代表所有游戏目标都已完成",
            )
        } else {
            result("failed", "MAA 队列已结束，部分任务失败；未自动重跑")
        };
        outcome.output = format!(
            "开始 {} 个任务；队列结束 {} 个任务；失败 {} 个任务。",
            self.starts.len(),
            self.finished.as_ref().map_or(0, HashSet::len),
            self.failed.len()
        );
        outcome
    }

    fn interrupted(
        &self,
        error: String,
        cancellation_observed: bool,
        owned_started: bool,
    ) -> MaaRunResult {
        result(
            // A started interactive MAA can have user-triggered work which
            // has not flushed to logs. Absence of callbacks proves no safety.
            if self.delivered || self.uncertain || (cancellation_observed && owned_started) {
                "attention_required"
            } else if cancellation_observed {
                "cancelled"
            } else {
                "failed"
            },
            error,
        )
    }
}

fn control_reply(
    identity: (&str, &str, &str),
    path: &str,
    value: &Value,
    evidence: &mut QueueEvidence,
) -> (u16, Value) {
    let (user, device, run) = identity;
    if value["user"].as_str() != Some(user) || value["device"].as_str() != Some(device) {
        return (403, json!({"error":"rejected"}));
    }
    match path {
        "/tasks" if evidence.ready && !evidence.delivered => {
            // At-most-once even when the response is lost.
            evidence.delivered = true;
            (200, json!({"tasks":[{"id":run,"type":"LinkStart"}]}))
        }
        "/tasks" => (200, json!({"tasks":[]})),
        "/report" => {
            if evidence.delivered && value["task"].as_str() == Some(run) {
                if value["status"].as_str() == Some("SUCCESS")
                    && evidence.finished.is_some()
                    && !evidence.starts.is_empty()
                {
                    evidence.reported = true;
                } else {
                    // A premature remote success cannot prove that end scripts
                    // and the GUI interruption guard have released afterwards.
                    evidence.uncertain = true;
                }
            }
            (200, json!({"accepted":true}))
        }
        _ => (404, json!({"error":"rejected"})),
    }
}

/// Bound partial records; JSON can contain escaped braces and span log lines.
#[derive(Default)]
struct LogParser {
    pending: Vec<u8>,
}

impl LogParser {
    fn feed(&mut self, bytes: &[u8], evidence: &mut QueueEvidence, gui: bool) {
        self.pending.extend_from_slice(bytes);
        loop {
            let Some(end) = self.pending.iter().position(|b| *b == b'\n') else {
                break;
            };
            let line = String::from_utf8_lossy(&self.pending[..end]);
            if gui {
                if line.contains("[RunningState]") && line.contains("Init: false to true") {
                    evidence.ready = true;
                }
                if !evidence.delivered
                    && line.contains("[RunningState]")
                    && (line.contains("BeginRun:") || line.contains("Idle: true to false"))
                {
                    evidence.uncertain = true;
                }
                self.pending.drain(..=end);
                continue;
            }
            let Some(marker) = line.find("Assistant::append_callback | ") else {
                self.pending.drain(..=end);
                continue;
            };
            let payload = marker + "Assistant::append_callback | ".len();
            let Some(relative_json) = line[payload..].find('{') else {
                self.pending.drain(..=end);
                continue;
            };
            let json_start = payload + relative_json;
            let kind = line[payload..json_start].trim().to_string();
            match json_end(&self.pending[json_start..]) {
                Some(length) => {
                    match serde_json::from_slice::<Value>(
                        &self.pending[json_start..json_start + length],
                    ) {
                        Ok(value) => evidence.callback(&kind, &value),
                        Err(_) => evidence.uncertain = true,
                    }
                    self.pending.drain(..json_start + length);
                }
                None => break,
            }
        }
        if self.pending.len() > RECORD_LIMIT {
            self.pending.clear();
            evidence.uncertain = true;
        }
    }
}

fn json_end(bytes: &[u8]) -> Option<usize> {
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for (index, byte) in bytes.iter().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'{' | b'[' => depth += 1,
                b'}' | b']' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(index + 1);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

#[cfg(not(windows))]
pub fn run_once(_: &str, _: bool, _: &str, _: Duration, _: &dyn Fn() -> bool) -> MaaRunResult {
    result("failed", "MAA 按需执行目前只支持 Windows")
}

#[cfg(windows)]
pub fn run_once(
    executable: &str,
    launch_emulator: bool,
    run_id: &str,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> MaaRunResult {
    native::run(
        executable,
        launch_emulator,
        run_id,
        timeout,
        cancelled,
        false,
    )
}

pub fn cleanup_install(executable: &str) -> Result<(), String> {
    let pending = Path::new(executable)
        .parent()
        .ok_or("MAA 路径无效")?
        .join("config/angelbot-routine.pending.json");
    if !pending.try_exists().map_err(|_| "无法检查 MAA 恢复记录")? {
        return Ok(());
    }
    let install = Install::open(executable)?;
    #[cfg(windows)]
    {
        let _lock = native::InstallationLock::acquire(&install.exe)?;
        if native::app_running(&install.exe)? {
            return Err("MAA 仍在运行，不会改写其配置或关闭用户实例".into());
        }
    }
    restore_remote(&install)
}

#[cfg(windows)]
mod native {
    use super::*;
    use crate::child_process_tree::ProcessTree;
    use sha2::{Digest, Sha256};
    use std::io;
    use std::net::{TcpListener, TcpStream};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::Instant;
    use windows_sys::Win32::Foundation::{LPARAM, WAIT_ABANDONED, WAIT_OBJECT_0};
    use windows_sys::Win32::System::ProcessStatus::K32EnumProcesses;
    use windows_sys::Win32::System::Threading::{
        CreateMutexW, OpenProcess, QueryFullProcessImageNameW, ReleaseMutex, WaitForSingleObject,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
    };

    pub(super) struct InstallationLock(OwnedHandle);
    impl InstallationLock {
        pub(super) fn acquire(exe: &Path) -> Result<Self, String> {
            let key = hex::encode(Sha256::digest(
                exe.to_string_lossy().to_lowercase().as_bytes(),
            ));
            let name: Vec<u16> = format!("Local\\AngelBot.Maa.{key}")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let raw = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
            if raw.is_null() {
                return Err("无法建立 MAA 安装互斥锁".into());
            }
            let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
            match unsafe { WaitForSingleObject(raw, 0) } {
                WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Self(handle)),
                _ => Err("这个 MAA 安装已有执行或清理操作".into()),
            }
        }
    }
    impl Drop for InstallationLock {
        fn drop(&mut self) {
            unsafe {
                ReleaseMutex(self.0.as_raw_handle());
            }
        }
    }

    fn matching_pids(exe: &Path) -> Result<Vec<u32>, String> {
        let mut matching = Vec::new();
        let mut pids = vec![0u32; 16384];
        let mut needed = 0;
        let size = (pids.len() * std::mem::size_of::<u32>()) as u32;
        if unsafe { K32EnumProcesses(pids.as_mut_ptr(), size, &mut needed) } == 0 || needed >= size
        {
            return Err("无法完整检查已有辅助程序，已停止启动".into());
        }
        for pid in pids
            .into_iter()
            .take(needed as usize / 4)
            .filter(|p| *p != 0)
        {
            let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            if raw.is_null() {
                continue;
            }
            let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
            let mut buffer = vec![0u16; 32768];
            let mut length = buffer.len() as u32;
            if unsafe {
                QueryFullProcessImageNameW(
                    handle.as_raw_handle(),
                    0,
                    buffer.as_mut_ptr(),
                    &mut length,
                )
            } == 0
            {
                continue;
            }
            let path = PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize]));
            let same = fs::canonicalize(&path).ok().as_ref() == Some(&exe.to_path_buf());
            // MAA's GUI single-instance gate is shared across installations.
            let maa = exe
                .file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("MAA.exe"))
                && path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("MAA.exe"));
            if same || maa {
                matching.push(pid);
            }
        }
        Ok(matching)
    }

    pub(super) fn app_running(exe: &Path) -> Result<bool, String> {
        Ok(!matching_pids(exe)?.is_empty())
    }

    struct LogTail {
        path: PathBuf,
        file: Option<File>,
        position: u64,
        identity: Option<(u64, u64)>,
        parser: LogParser,
    }
    impl LogTail {
        fn identity(file: &File) -> io::Result<(u64, u64)> {
            use windows_sys::Win32::Storage::FileSystem::{
                GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
            };
            let mut info = BY_HANDLE_FILE_INFORMATION::default();
            if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok((
                info.dwVolumeSerialNumber as u64,
                ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
            ))
        }
        fn new(path: PathBuf) -> Result<Self, String> {
            let mut file = match File::open(&path) {
                Ok(file) => Some(file),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(_) => return Err("无法建立 MAA 日志起点".into()),
            };
            let position = match &mut file {
                Some(file) => file
                    .seek(SeekFrom::End(0))
                    .map_err(|_| "无法定位 MAA 日志起点")?,
                None => 0,
            };
            let identity = file
                .as_ref()
                .map(Self::identity)
                .transpose()
                .map_err(|_| "无法识别 MAA 日志文件")?;
            Ok(Self {
                path,
                file,
                position,
                identity,
                parser: LogParser::default(),
            })
        }
        fn drain(&mut self, evidence: &mut QueueEvidence, gui: bool) -> Result<(), String> {
            if let Some(file) = &mut self.file {
                if file.metadata().map_err(|_| "无法读取 MAA 日志状态")?.len() < self.position
                {
                    return Err("MAA 日志被截断，无法确认执行结果".into());
                }
                let mut bytes = Vec::new();
                Read::by_ref(file)
                    .take(1024 * 1024)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "无法读取 MAA 日志")?;
                self.position += bytes.len() as u64;
                self.parser.feed(&bytes, evidence, gui);
            }
            match File::open(&self.path) {
                Ok(file) => {
                    let identity = Self::identity(&file).map_err(|_| "无法识别 MAA 日志文件")?;
                    if Some(identity) != self.identity {
                        // Keep the old handle until fully drained. If it is still
                        // growing faster than we can read, do not lose evidence.
                        if self.file.as_ref().is_some_and(|f| {
                            f.metadata().map_or(true, |m| m.len() != self.position)
                        }) {
                            return Err("MAA 日志轮转时存在未读内容，执行结果需要检查".into());
                        }
                        if !self.parser.pending.is_empty() {
                            evidence.uncertain = true;
                        }
                        self.file = Some(file);
                        self.identity = Some(identity);
                        self.position = 0;
                        self.parser = LogParser::default();
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(_) => return Err("无法读取当前 MAA 日志文件".into()),
            }
            Ok(())
        }
    }

    struct Bridge {
        server: TcpListener,
        user: String,
        device: String,
        run: String,
    }
    impl Bridge {
        fn new(run: &str) -> Result<Self, String> {
            if uuid::Uuid::parse_str(run).is_err() {
                return Err("MAA 运行标识无效".into());
            }
            let server =
                TcpListener::bind("127.0.0.1:0").map_err(|_| "无法建立 MAA 本机控制端点")?;
            server
                .set_nonblocking(true)
                .map_err(|_| "无法配置 MAA 本机端点")?;
            Ok(Self {
                server,
                user: uuid::Uuid::new_v4().to_string(),
                device: uuid::Uuid::new_v4().to_string(),
                run: run.into(),
            })
        }
        fn values(&self) -> Result<[String; 4], String> {
            let port = self
                .server
                .local_addr()
                .map_err(|_| "MAA 本机端点地址无效")?
                .port();
            Ok([
                format!("http://127.0.0.1:{port}/tasks"),
                format!("http://127.0.0.1:{port}/report"),
                self.user.clone(),
                self.device.clone(),
            ])
        }
        fn poll(
            &self,
            evidence: &mut QueueEvidence,
            before_reply: &mut dyn FnMut(&mut QueueEvidence) -> Result<(), String>,
            probe: bool,
        ) -> Result<bool, String> {
            let (mut request, addr) = match self.server.accept() {
                Ok(value) => value,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(100));
                    return Ok(false);
                }
                Err(_) => return Err("MAA 本机控制连接异常".into()),
            };
            if !addr.ip().is_loopback() {
                return Ok(false);
            }
            let (path, value) = match read_request(&mut request) {
                Ok(value) => value,
                Err(_) => {
                    let _ = respond(&mut request, 400, &json!({"error":"rejected"}));
                    return Ok(false);
                }
            };
            if value["user"].as_str() != Some(&self.user)
                || value["device"].as_str() != Some(&self.device)
            {
                let _ = respond(&mut request, 403, &json!({"error":"rejected"}));
                return Ok(false);
            }
            before_reply(evidence)?;
            let (status, response) = if probe {
                (200, json!({"tasks":[]}))
            } else {
                control_reply(
                    (&self.user, &self.device, &self.run),
                    &path,
                    &value,
                    evidence,
                )
            };
            let _ = respond(&mut request, status, &response);
            Ok(path == "/tasks")
        }
    }

    // One small JSON POST per connection. Byte/time bounds keep unfinished
    // local requests from stalling cancellation and owned-process cleanup.
    fn read_request(stream: &mut TcpStream) -> Result<(String, Value), String> {
        stream
            .set_read_timeout(Some(Duration::from_millis(200)))
            .map_err(|_| "无法限制本机请求等待")?;
        stream
            .set_write_timeout(Some(Duration::from_millis(200)))
            .map_err(|_| "无法限制本机请求响应")?;
        let deadline = Instant::now() + Duration::from_millis(800);
        let mut bytes = Vec::new();
        let (path, body_start, length) = loop {
            if Instant::now() >= deadline || bytes.len() > 16384 {
                return Err("本机请求过大或超时".into());
            }
            if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                let header = std::str::from_utf8(&bytes[..end]).map_err(|_| "本机请求头无效")?;
                let (path, length) = parse_header(header)?;
                break (path, end + 4, length);
            }
            let mut buffer = [0u8; 2048];
            let count = stream.read(&mut buffer).map_err(|_| "本机请求读取失败")?;
            if count == 0 {
                return Err("本机请求不完整".into());
            }
            bytes.extend_from_slice(&buffer[..count]);
        };
        while bytes.len() < body_start + length {
            if Instant::now() >= deadline {
                return Err("本机请求超时".into());
            }
            let mut buffer = [0u8; 2048];
            let count = stream.read(&mut buffer).map_err(|_| "本机请求读取失败")?;
            if count == 0 {
                return Err("本机请求不完整".into());
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        let value = serde_json::from_slice(&bytes[body_start..body_start + length])
            .map_err(|_| "本机 JSON 请求无效")?;
        Ok((path, value))
    }

    fn parse_header(header: &str) -> Result<(String, usize), String> {
        if header.len() > 8192 {
            return Err("请求头过大".into());
        }
        let mut lines = header.split("\r\n");
        let request: Vec<_> = lines.next().unwrap_or("").split_whitespace().collect();
        if request.len() != 3
            || request[0] != "POST"
            || request[2] != "HTTP/1.1"
            || !matches!(request[1], "/tasks" | "/report")
        {
            return Err("请求方法或路径无效".into());
        }
        let mut length = None;
        for line in lines {
            let (key, value) = line.split_once(':').ok_or("请求头无效")?;
            if key.eq_ignore_ascii_case("Transfer-Encoding") {
                return Err("不支持分块请求".into());
            }
            if key.eq_ignore_ascii_case("Content-Length") {
                if length.is_some() {
                    return Err("重复长度头".into());
                }
                length = Some(value.trim().parse::<usize>().map_err(|_| "请求长度无效")?);
            }
        }
        let length = length
            .filter(|length| *length > 0 && *length <= 8192)
            .ok_or("请求长度无效")?;
        Ok((request[1].into(), length))
    }

    fn respond(stream: &mut TcpStream, code: u16, value: &Value) -> io::Result<()> {
        let body = value.to_string();
        write!(stream, "HTTP/1.1 {code} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
    }

    fn powershell(script: &str, path: &Path) -> Result<Value, String> {
        let system = std::env::var_os("SystemRoot").ok_or("缺少 Windows 系统路径")?;
        let mut command = Command::new(
            PathBuf::from(system).join("System32/WindowsPowerShell/v1.0/powershell.exe"),
        );
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        crate::child_process_env::apply_minimal_child_environment(&mut command);
        command.env("ANGELBOT_MAA_INSPECT_PATH", path);
        let mut process =
            ProcessTree::spawn(&mut command).map_err(|_| "无法读取辅助程序启动信息")?;
        let stdout = process.take_stdout().ok_or("无法读取启动信息")?;
        let reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.take(32769).read_to_end(&mut bytes);
            bytes
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let success = loop {
            if let Some(status) = process.try_wait().map_err(|_| "辅助程序信息读取失败")?
            {
                break status.success();
            }
            if Instant::now() >= deadline {
                break false;
            }
            thread::sleep(Duration::from_millis(20));
        };
        let _ = process.terminate_and_wait();
        drop(process);
        let bytes = reader.join().map_err(|_| "辅助程序信息读取线程失败")?;
        if !success || bytes.len() > 32768 {
            return Err("无法读取辅助程序启动信息".into());
        }
        serde_json::from_slice(&bytes).map_err(|_| "辅助程序启动信息无效".into())
    }

    fn validate_version(install: &Install) -> Result<(), String> {
        let version = powershell("[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); (Get-Item -LiteralPath $env:ANGELBOT_MAA_INSPECT_PATH).VersionInfo.ProductVersion | ConvertTo-Json -Compress", &install.exe)?;
        if !version
            .as_str()
            .is_some_and(|v| v.trim_start_matches('v').starts_with("6.18."))
        {
            return Err("当前按需控制已核验 MAA 6.18；其他版本请先做兼容性验收".into());
        }
        Ok(())
    }

    fn start_emulator(config: &Value) -> Result<Option<ProcessTree>, String> {
        let settings = &config["Configurations"][profile_name(config)?]["Gui"]["StartUpSettings"];
        let mut path = PathBuf::from(
            settings["EmulatorPath"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("MAA 尚未配置模拟器启动路径")?,
        );
        let mut args = settings["EmulatorAddCommand"]
            .as_str()
            .unwrap_or("")
            .to_string();
        if path
            .extension()
            .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("lnk"))
        {
            let link = powershell("[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $l=(New-Object -ComObject WScript.Shell).CreateShortcut($env:ANGELBOT_MAA_INSPECT_PATH); @{target=$l.TargetPath;arguments=$l.Arguments} | ConvertTo-Json -Compress", &path)?;
            path = PathBuf::from(link["target"].as_str().ok_or("模拟器快捷方式缺少目标")?);
            // Preserve both the existing shortcut arguments and MAA additions.
            args = format!("{} {}", link["arguments"].as_str().unwrap_or(""), args)
                .trim()
                .into();
        }
        let path = fs::canonicalize(path).map_err(|_| "MAA 配置的模拟器不存在")?;
        if !path
            .extension()
            .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("exe"))
        {
            return Err("模拟器启动目标必须是可执行程序".into());
        }
        if app_running(&path)? {
            return Ok(None);
        }
        let mut command = Command::new(&path);
        command
            .current_dir(path.parent().ok_or("模拟器路径无效")?)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        crate::child_process_env::apply_minimal_child_environment(&mut command);
        // This is the target executable's Windows argument string, not a shell.
        if !args.is_empty() {
            command.raw_arg(args);
        }
        ProcessTree::spawn(&mut command)
            .map(Some)
            .map_err(|_| "无法启动 MAA 配置的模拟器".to_string())
    }

    unsafe extern "system" fn close_owned_window(
        window: windows_sys::Win32::Foundation::HWND,
        pid: LPARAM,
    ) -> i32 {
        let mut owner = 0;
        unsafe {
            GetWindowThreadProcessId(window, &mut owner);
        }
        if owner == pid as u32 {
            unsafe {
                PostMessageW(window, WM_CLOSE, 0, 0);
            }
        }
        1
    }

    fn close_owned(process: &mut ProcessTree) -> Result<(), String> {
        if process
            .try_wait()
            .map_err(|_| "无法检查 MAA 退出状态")?
            .is_none()
        {
            unsafe {
                EnumWindows(Some(close_owned_window), process.id() as LPARAM);
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                if process
                    .try_wait()
                    .map_err(|_| "无法检查 MAA 退出状态")?
                    .is_some()
                {
                    return Ok(());
                }
                thread::sleep(Duration::from_millis(50));
            }
            process
                .terminate_and_wait()
                .map_err(|_| "无法停止本次启动的 MAA 进程树")?;
            return Err("MAA 未正常关闭，已回收本次启动的进程；结果需要检查".into());
        }
        Ok(())
    }

    pub(super) fn run(
        executable: &str,
        launch_emulator: bool,
        run_id: &str,
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
        probe: bool,
    ) -> MaaRunResult {
        let started = Instant::now();
        let timeout = timeout.clamp(Duration::from_secs(10), Duration::from_secs(3600));
        let deadline = started + timeout;
        let setup = || -> Result<(Install, InstallationLock), MaaRunResult> {
            let install = Install::open(executable).map_err(|error| result("failed", error))?;
            let lock = InstallationLock::acquire(&install.exe)
                .map_err(|error| result("skipped", error))?;
            if app_running(&install.exe).map_err(|error| result("attention_required", error))? {
                return Err(result(
                    "skipped",
                    "MAA 已打开，本次跳过；不会接管或关闭现有实例",
                ));
            }
            restore_remote(&install).map_err(|error| result("attention_required", error))?;
            validate_version(&install).map_err(|error| result("failed", error))?;
            Ok((install, lock))
        };
        let (install, _lock) = match setup() {
            Ok(v) => v,
            Err(error) => return error,
        };
        let mut owned: Option<ProcessTree> = None;
        let mut emulator: Option<ProcessTree> = None;
        let mut evidence = QueueEvidence::default();
        let cancellation_observed = std::cell::Cell::new(false);
        let is_cancelled = || {
            let stopped = cancelled();
            if stopped {
                cancellation_observed.set(true);
            }
            stopped
        };
        let operation = || -> Result<(), String> {
            if is_cancelled() {
                return Err("本次 MAA 已请求停止，未派发任务".into());
            }
            if Instant::now() >= deadline {
                return Err("MAA 已超过运行时限，未启动任务".into());
            }
            let (bytes, config) = install.read_config()?;
            validate_config(&config)?;
            let bridge = Bridge::new(run_id)?;
            let mut gui = LogTail::new(install.root.join("debug/gui.log"))?;
            let mut core = LogTail::new(install.root.join("debug/asst.log"))?;
            patch_remote(&install, &config, &bytes, bridge.values()?)?;
            if launch_emulator {
                emulator = start_emulator(&config)?;
                let wait = config["Configurations"][profile_name(&config)?]["Gui"]
                    ["StartUpSettings"]["EmulatorWaitSeconds"]
                    .as_u64()
                    .unwrap_or(60)
                    .min(300);
                let until = Instant::now() + Duration::from_secs(wait);
                while emulator.is_some() && Instant::now() < until {
                    if is_cancelled() {
                        return Err("本次 MAA 已请求停止，已停止启动流程".into());
                    }
                    if Instant::now() >= deadline {
                        return Err("模拟器启动等待超过本次运行时限，已停止启动流程".into());
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
            if is_cancelled() {
                return Err("本次 MAA 已请求停止，未启动 MAA".into());
            }
            if Instant::now() >= deadline || app_running(&install.exe)? {
                return Err("MAA 启动前权限或实例状态改变，已停止启动".into());
            }
            let mut command = Command::new(&install.exe);
            command
                .current_dir(&install.root)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            crate::child_process_env::apply_minimal_child_environment(&mut command);
            owned = Some(ProcessTree::spawn(&mut command).map_err(|_| "无法启动 MAA")?);
            let gui_started = Instant::now();
            loop {
                if is_cancelled() {
                    return Err(
                        "本次 MAA 已请求停止；已启动实例的操作结果需要检查，不会自动重试".into(),
                    );
                }
                gui.drain(&mut evidence, true)?;
                core.drain(&mut evidence, false)?;
                if evidence.uncertain {
                    return Err("MAA 队列被停止或证据不完整，结果需要检查".into());
                }
                if evidence.complete() {
                    return Ok(());
                }
                if let Some(status) = owned
                    .as_mut()
                    .unwrap()
                    .try_wait()
                    .map_err(|_| "无法检查 MAA 进程")?
                {
                    // Drain flushed callbacks once more after exit.
                    gui.drain(&mut evidence, true)?;
                    core.drain(&mut evidence, false)?;
                    // Existing MAA ExitSelf actions may shut down before the
                    // remote client can report. A verified natural exit plus
                    // full Core completion is also terminal, never exit alone.
                    evidence.exited = status.success();
                    return if evidence.complete() {
                        Ok(())
                    } else {
                        Err("MAA 提前退出，任务结果未知".into())
                    };
                }
                if Instant::now() >= deadline {
                    return Err("MAA 超过运行时限，结果未知；不会自动重试".into());
                }
                if !evidence.ready && gui_started.elapsed() > Duration::from_secs(120) {
                    return Err("MAA 未能完成初始化，未派发任务".into());
                }
                if evidence.ready
                    && !evidence.delivered
                    && matching_pids(&install.exe)? != vec![owned.as_ref().unwrap().id()]
                {
                    return Err("MAA 实例所有权改变，未派发任务，也不会接管其他实例".into());
                }
                let connected = bridge.poll(
                    &mut evidence,
                    &mut |current| {
                        gui.drain(current, true)?;
                        core.drain(current, false)?;
                        if current.uncertain {
                            return Err("派发前发现手动任务或日志证据缺失，未重复派发".into());
                        }
                        if current.ready
                            && !current.delivered
                            && matching_pids(&install.exe)? != vec![owned.as_ref().unwrap().id()]
                        {
                            return Err("MAA 实例所有权改变，未派发任务".into());
                        }
                        if is_cancelled() {
                            return Err(
                                "本次 MAA 已请求停止，未继续派发队列；已启动实例的结果需要检查"
                                    .into(),
                            );
                        }
                        if Instant::now() >= deadline {
                            return Err("MAA 派发前授权或运行时限已变化".into());
                        }
                        Ok(())
                    },
                    probe,
                )?;
                if probe && connected && evidence.ready {
                    return Ok(());
                }
            }
        };
        let mut operation = operation;
        let execution = operation();
        let mut outcome = match execution {
            Ok(()) if probe => result(
                "completed",
                "MAA 本机初始化与控制连接已核验；未派发游戏任务",
            ),
            Ok(()) => evidence.outcome(),
            Err(error) => evidence.interrupted(error, cancellation_observed.get(), owned.is_some()),
        };
        if let Some(process) = &mut owned {
            if let Err(error) = close_owned(process) {
                outcome = result("attention_required", error);
            }
            outcome.exit_code = process.try_wait().ok().flatten().and_then(|s| s.code());
        }
        // Closing jobs also reclaims descendants started by this run; never an
        // emulator which was already present before we began.
        drop(owned);
        if let Some(process) = &mut emulator {
            if process.terminate_and_wait().is_err() {
                outcome = result(
                    "attention_required",
                    "本次启动的模拟器未能确认停止，需要检查",
                );
            }
        }
        drop(emulator);
        match app_running(&install.exe).and_then(|running| {
            if running {
                Err("MAA 仍在运行，未覆盖配置；恢复记录已保留".into())
            } else {
                restore_remote(&install)
            }
        }) {
            Ok(()) => {}
            Err(error) => outcome = result("attention_required", error),
        }
        outcome
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn local_post_headers_are_strict_and_bounded() {
            assert_eq!(
                parse_header("POST /tasks HTTP/1.1\r\nContent-Length: 20").unwrap(),
                ("/tasks".into(), 20)
            );
            for header in [
                "GET /tasks HTTP/1.1\r\nContent-Length: 20",
                "POST /other HTTP/1.1\r\nContent-Length: 20",
                "POST /tasks HTTP/1.1\r\nContent-Length: 8193",
                "POST /tasks HTTP/1.1\r\nContent-Length: 20\r\ncontent-length: 20",
                "POST /tasks HTTP/1.1\r\nTransfer-Encoding: chunked\r\nContent-Length: 20",
            ] {
                assert!(parse_header(header).is_err());
            }
        }

        #[test]
        #[ignore = "manual installed-MAA preflight; never run in offline full"]
        fn installed_maa_preflight_without_game_tasks() {
            let executable = std::env::var("ANGELBOT_MAA_PREFLIGHT_PATH")
                .expect("explicit installed-MAA path required");
            let install = Install::open(&executable).unwrap();
            assert!(
                !app_running(&install.exe).unwrap(),
                "existing MAA is never adopted"
            );
            let original = install.read_config().unwrap().1;
            let outcome = run(
                &executable,
                false,
                &uuid::Uuid::new_v4().to_string(),
                Duration::from_secs(40),
                &|| false,
                true,
            );
            let restored = install.read_config().unwrap().1;
            assert!(!app_running(&install.exe).unwrap(), "owned MAA must exit");
            assert!(
                !install.journal.exists(),
                "temporary control fields must be restored"
            );
            for key in ["Current", "Timers"] {
                assert_eq!(original[key], restored[key]);
            }
            let profile = profile_name(&original).unwrap();
            for key in ["TaskQueue", "Gui"] {
                assert_eq!(
                    original["Configurations"][profile][key],
                    restored["Configurations"][profile][key]
                );
            }
            assert_eq!(outcome.status, "completed", "{}", outcome.summary);
            assert!(!outcome.summary.contains("队列已结束"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Install, Value) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("config")).unwrap();
        fs::write(dir.path().join("MAA.exe"), b"fixture").unwrap();
        let config = json!({"Current":"daily","Configurations":{"daily":{"TaskQueue":[{"IsEnable":true}],"Gui":{"RemoteControl":{},"StartUpSettings":{"RunDirectly":false}}}},"Timers":{"List":[{"IsEnabled":false}]},"keep":"original"});
        fs::write(
            dir.path().join("config/gui.new.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        let install = Install::open(dir.path().join("MAA.exe").to_str().unwrap()).unwrap();
        (dir, install, config)
    }

    #[test]
    fn static_validation_is_read_only_and_rejects_competing_auto_run() {
        let (_dir, install, mut config) = fixture();
        let before = read_bounded(&install.config).unwrap();
        validate_install(install.exe.to_str().unwrap()).unwrap();
        assert_eq!(before, read_bounded(&install.config).unwrap());
        config["Timers"]["List"][0]["IsEnabled"] = json!(true);
        assert!(validate_config(&config).is_err());
        for dangerous in [
            json!(4),
            json!(32),
            json!(64),
            json!(128),
            json!(512),
            json!("ExitEmulator"),
            json!("ExitSelf, Shutdown"),
            json!(null),
        ] {
            assert!(validate_post_actions(Some(&dangerous)).is_err());
        }
        for safe in [
            json!(0),
            json!(283),
            json!("None"),
            json!("ExitSelf, SkipOnError"),
        ] {
            validate_post_actions(Some(&safe)).unwrap();
        }
        config["Timers"]["List"][0]["IsEnabled"] = json!(false);
        config["Configurations"]["daily"]["Gui"]["StartUpSettings"]["RunDirectly"] = json!(true);
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn restore_only_owned_fields_and_preserve_user_edits_and_original_remote() {
        let (_dir, install, mut config) = fixture();
        config["Configurations"]["daily"]["Gui"]["RemoteControl"][REMOTE_KEYS[0]] =
            json!("https://existing.invalid/task");
        replace_config(
            &install.config,
            &read_bounded(&install.config).unwrap(),
            &config,
        )
        .unwrap();
        patch_remote(
            &install,
            &config,
            &read_bounded(&install.config).unwrap(),
            [
                "task".into(),
                "report".into(),
                "user".into(),
                "device".into(),
            ],
        )
        .unwrap();
        let (bytes, mut changed) = install.read_config().unwrap();
        changed["keep"] = json!("user edit");
        changed["Configurations"]["daily"]["TaskQueue"] = json!([{"user":"task"}]);
        replace_config(&install.config, &bytes, &changed).unwrap();
        restore_remote(&install).unwrap();
        let restored = install.read_config().unwrap().1;
        assert_eq!(restored["keep"], "user edit");
        assert_eq!(
            restored["Configurations"]["daily"]["TaskQueue"],
            changed["Configurations"]["daily"]["TaskQueue"]
        );
        assert_eq!(
            remote(&restored, "daily").unwrap(),
            remote(&config, "daily").unwrap()
        );
        assert!(!install.journal.exists());
        restore_remote(&install).unwrap();
    }

    #[test]
    fn recovery_never_overwrites_other_remote_changes_and_handles_unpatched_journal() {
        let (_dir, install, config) = fixture();
        let bytes = read_bounded(&install.config).unwrap();
        patch_remote(
            &install,
            &config,
            &bytes,
            [
                "task".into(),
                "report".into(),
                "user".into(),
                "device".into(),
            ],
        )
        .unwrap();
        let (bytes, mut modified) = install.read_config().unwrap();
        modified["Configurations"]["daily"]["Gui"]["RemoteControl"][REMOTE_KEYS[1]] =
            json!("user change");
        replace_config(&install.config, &bytes, &modified).unwrap();
        assert!(restore_remote(&install).is_err());
        assert!(install.journal.exists());
        assert_eq!(install.read_config().unwrap().1, modified);
        replace_config(
            &install.config,
            &read_bounded(&install.config).unwrap(),
            &config,
        )
        .unwrap();
        restore_remote(&install).unwrap();
        assert!(!install.journal.exists());

        for divisor in [8, 6] {
            let (_dir, install, mut config) = fixture();
            config["padding"] = json!(vec![0_u8; CONFIG_LIMIT as usize / divisor]);
            let original = serde_json::to_vec(&config).unwrap();
            assert!(original.len() as u64 <= CONFIG_LIMIT);
            validate_config(&config).unwrap();
            fs::write(&install.config, &original).unwrap();
            let patched = [
                "task".into(),
                "report".into(),
                "user".into(),
                "device".into(),
            ];
            let journal = Journal {
                version: 1,
                profile: "daily".into(),
                original_config: config.clone(),
                patched: patched.clone(),
            };
            assert!(serde_json::to_vec_pretty(&journal).unwrap().len() as u64 > CONFIG_LIMIT);
            let mut updated = config.clone();
            for (key, value) in REMOTE_KEYS.iter().zip(patched.clone()) {
                remote_mut(&mut updated, "daily")
                    .unwrap()
                    .insert((*key).into(), Value::String(value));
            }
            assert_eq!(
                serde_json::to_vec_pretty(&updated).unwrap().len() as u64 > CONFIG_LIMIT,
                divisor == 6,
                "cover both journal-only overflow and overflowing replacement"
            );
            assert_eq!(
                patch_remote(&install, &config, &original, patched).unwrap_err(),
                "MAA 配置或恢复记录超过安全大小限制"
            );
            assert_eq!(fs::read(&install.config).unwrap(), original);
            assert!(!install.journal.exists());
            assert_eq!(
                fs::read_dir(install.config.parent().unwrap())
                    .unwrap()
                    .count(),
                1
            );
            restore_remote(&install).unwrap();

            if divisor == 6 {
                assert_eq!(
                    replace_config(&install.config, &original, &updated).unwrap_err(),
                    "MAA 配置或恢复记录超过安全大小限制"
                );
                assert_eq!(fs::read(&install.config).unwrap(), original);
                assert!(!install.journal.exists());
                assert_eq!(
                    fs::read_dir(install.config.parent().unwrap())
                        .unwrap()
                        .count(),
                    1
                );
            }
        }

        let (_dir, install, _) = fixture();
        let mut config = json!({"Current":"d","Configurations":{"d":{"TaskQueue":[{"IsEnable":true}],"Gui":{"RemoteControl":{},"StartUpSettings":{"RunDirectly":false}}}},"Timers":{"List":[]},"padding":""});
        let patched = [
            "task".into(),
            "report".into(),
            "user".into(),
            "device".into(),
        ];
        let mut updated = config.clone();
        for (key, value) in REMOTE_KEYS.iter().zip(patched.clone()) {
            remote_mut(&mut updated, "d")
                .unwrap()
                .insert((*key).into(), Value::String(value));
        }
        let padding =
            CONFIG_LIMIT as usize + 1 - serde_json::to_vec_pretty(&updated).unwrap().len();
        config["padding"] = json!("x".repeat(padding));
        updated["padding"] = config["padding"].clone();
        let journal = Journal {
            version: 1,
            profile: "d".into(),
            original_config: config.clone(),
            patched: patched.clone(),
        };
        assert!(serde_json::to_vec_pretty(&journal).unwrap().len() as u64 <= CONFIG_LIMIT);
        assert!(serde_json::to_vec_pretty(&updated).unwrap().len() as u64 > CONFIG_LIMIT);
        let original = serde_json::to_vec(&config).unwrap();
        assert!(original.len() as u64 <= CONFIG_LIMIT);
        validate_config(&config).unwrap();
        fs::write(&install.config, &original).unwrap();
        assert_eq!(
            patch_remote(&install, &config, &original, patched).unwrap_err(),
            "MAA 配置或恢复记录超过安全大小限制"
        );
        assert_eq!(fs::read(&install.config).unwrap(), original);
        assert!(!install.journal.exists());
        assert_eq!(
            fs::read_dir(install.config.parent().unwrap())
                .unwrap()
                .count(),
            1
        );
    }

    fn record(kind: &str, body: Value) -> Vec<u8> {
        format!(
            "[fixture] Assistant::append_callback | {kind} {}\n",
            serde_json::to_string_pretty(&body).unwrap()
        )
        .into_bytes()
    }
    fn finish(evidence: &mut QueueEvidence, parser: &mut LogParser) {
        parser.feed(
            &record(
                "AllTasksCompleted",
                json!({"finished_tasks":[1],"uuid":"device"}),
            ),
            evidence,
            false,
        );
        evidence.reported = true;
    }

    #[test]
    fn multiline_callbacks_require_start_completion_and_report_not_just_success() {
        let mut evidence = QueueEvidence {
            delivered: true,
            ..Default::default()
        };
        let mut parser = LogParser::default();
        let bytes = record(
            "TaskChainStart",
            json!({"taskid":1,"uuid":"device","text":"escaped \\\" { brace"}),
        );
        for part in bytes.chunks(7) {
            parser.feed(part, &mut evidence, false);
        }
        assert_eq!(evidence.starts.len(), 1);
        finish(&mut evidence, &mut parser);
        assert!(evidence.complete());
        assert_eq!(evidence.outcome().status, "completed");
        let report_only = QueueEvidence {
            delivered: true,
            reported: true,
            ..Default::default()
        };
        assert!(!report_only.complete());
    }

    #[test]
    fn failed_finished_ids_are_not_success_and_stop_or_missing_ids_are_unknown() {
        for (delivered, uncertain, observed, started, expected) in [
            (false, false, false, false, "failed"),
            (false, false, false, true, "failed"),
            (false, false, true, false, "cancelled"),
            (false, false, true, true, "attention_required"),
            (true, false, true, true, "attention_required"),
            (false, true, true, false, "attention_required"),
            (true, true, false, false, "attention_required"),
        ] {
            let evidence = QueueEvidence {
                delivered,
                uncertain,
                ..Default::default()
            };
            assert_eq!(
                evidence
                    .interrupted("fixture interruption".into(), observed, started)
                    .status,
                expected
            );
        }
        let mut evidence = QueueEvidence {
            delivered: true,
            ..Default::default()
        };
        let mut parser = LogParser::default();
        parser.feed(
            &record("TaskChainStart", json!({"taskid":1,"uuid":"device"})),
            &mut evidence,
            false,
        );
        parser.feed(
            &record("TaskChainError", json!({"taskid":1,"uuid":"device"})),
            &mut evidence,
            false,
        );
        finish(&mut evidence, &mut parser);
        assert_eq!(evidence.outcome().status, "failed");
        parser.feed(
            &record("TaskChainStopped", json!({"taskid":1})),
            &mut evidence,
            false,
        );
        assert_eq!(evidence.outcome().status, "attention_required");
        let mut missing = QueueEvidence {
            delivered: true,
            reported: true,
            ..Default::default()
        };
        missing.callback("TaskChainStart", &json!({"taskid":1,"uuid":"device"}));
        missing.callback(
            "AllTasksCompleted",
            &json!({"finished_tasks":[1,2],"uuid":"device"}),
        );
        assert!(!missing.complete());
    }

    #[test]
    fn old_callbacks_and_unbounded_records_do_not_prove_a_new_run() {
        let mut evidence = QueueEvidence::default();
        let mut parser = LogParser::default();
        parser.feed(
            &record("TaskChainStart", json!({"taskid":1,"uuid":"device"})),
            &mut evidence,
            false,
        );
        assert!(evidence.starts.is_empty());
        assert!(
            evidence.uncertain,
            "a manual pre-dispatch start cannot be ignored"
        );
        parser.feed(&vec![b'x'; RECORD_LIMIT + 1], &mut evidence, false);
        assert!(evidence.uncertain);
        assert!(parser.pending.is_empty());
        let mut gui = LogParser::default();
        gui.feed(
            b"[RunningState] Init: false to true (called from Init)\n",
            &mut evidence,
            true,
        );
        assert!(evidence.ready);
    }

    #[test]
    fn remote_protocol_is_authenticated_at_most_once_and_requires_terminal_evidence() {
        let mut evidence = QueueEvidence::default();
        let identity = ("user", "device", "run");
        let poll = json!({"user":"user","device":"device"});
        assert_eq!(
            control_reply(identity, "/tasks", &poll, &mut evidence).1["tasks"],
            json!([])
        );
        evidence.ready = true;
        assert_eq!(
            control_reply(
                identity,
                "/tasks",
                &json!({"user":"other","device":"device"}),
                &mut evidence
            )
            .0,
            403
        );
        assert!(!evidence.delivered);
        assert_eq!(
            control_reply(identity, "/tasks", &poll, &mut evidence).1["tasks"][0]["type"],
            "LinkStart"
        );
        assert_eq!(
            control_reply(identity, "/tasks", &poll, &mut evidence).1["tasks"],
            json!([])
        );
        let report = json!({"user":"user","device":"device","task":"run","status":"SUCCESS"});
        control_reply(identity, "/report", &report, &mut evidence);
        assert!(
            evidence.uncertain,
            "premature remote SUCCESS cannot close a future queue"
        );
        assert!(!evidence.complete());
        let mut complete = QueueEvidence {
            delivered: true,
            ..Default::default()
        };
        complete.callback("TaskChainStart", &json!({"taskid":1,"uuid":"device"}));
        complete.callback(
            "AllTasksCompleted",
            &json!({"finished_tasks":[1],"uuid":"device"}),
        );
        control_reply(
            identity,
            "/report",
            &json!({"user":"user","device":"device","task":"different","status":"SUCCESS"}),
            &mut complete,
        );
        assert!(!complete.complete());
        control_reply(identity, "/report", &report, &mut complete);
        assert!(complete.complete());
        complete.reported = false;
        complete.exited = true;
        assert!(
            complete.complete(),
            "verified natural ExitSelf with Core evidence is terminal"
        );
        complete.exited = false;
        complete.reported = true;
        complete.callback("TaskChainStopped", &json!({"taskid":1,"uuid":"device"}));
        assert!(
            !complete.complete(),
            "teardown stop requires verified natural exit, not remote success alone"
        );
        complete.exited = true;
        assert!(complete.complete());
        complete.callback("TaskChainStart", &json!({"taskid":1,"uuid":"device"}));
        assert!(
            !complete.complete(),
            "a later queue cannot borrow old completion evidence"
        );
    }
}

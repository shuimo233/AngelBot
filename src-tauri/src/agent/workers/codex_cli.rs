//! Runs the installed Codex CLI within a prepared candidate worktree.

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::agent::tool::ToolCancellation;
use crate::child_process_env::apply_minimal_child_environment;
use crate::child_process_tree::ProcessTree;

const MAX_EVENT_BYTES: u64 = 1024 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct CodexResult {
    pub(super) commands_finished: usize,
    pub(super) commands_failed: usize,
    completed: bool,
    failed: bool,
    file_change_reported: bool,
    file_change_completed: bool,
    pub(super) file_change_failed: bool,
    pub(super) reported_sandbox_restriction: bool,
    #[cfg(test)]
    synthetic_test_receipt: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(super) enum CodexError {
    #[error("Codex CLI could not be started")]
    Spawn,
    #[error("Codex CLI process or event stream could not be verified")]
    Unverified,
    #[error("Codex CLI reported a failed turn")]
    Failed,
    #[error("Codex CLI run cancelled")]
    Cancelled,
    #[error("Codex CLI run reached its time limit after {0:?}")]
    TimedOut(CodexPhase),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CodexPhase {
    Launch,
    Session,
    Inference,
    Action,
}

impl CodexPhase {
    fn read(signal: &AtomicU8) -> Self {
        match signal.load(Ordering::Relaxed) {
            1 => Self::Session,
            2 => Self::Inference,
            3 => Self::Action,
            _ => Self::Launch,
        }
    }
}

pub(super) fn installed_executable() -> Option<PathBuf> {
    let name = if cfg!(windows) { "codex.exe" } else { "codex" };
    std::env::var_os("PATH").and_then(|path| executable_on_path(&path, name))
}

fn executable_on_path(path: &OsStr, name: &str) -> Option<PathBuf> {
    std::env::split_paths(path)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[derive(Debug, PartialEq, Eq)]
struct CliProxy {
    url: String,
    bypass: String,
}

/// Only reuse the currently configured static, local HTTP proxy. Arbitrary
/// endpoints, credentials, PAC and WPAD are deliberately outside this adapter.
fn static_loopback_proxy(server: &str, bypass: &str) -> Option<CliProxy> {
    let endpoint = if server.contains('=') {
        server.split(';').find_map(|entry| {
            let (scheme, endpoint) = entry.split_once('=')?;
            scheme
                .trim()
                .eq_ignore_ascii_case("https")
                .then_some(endpoint.trim())
        })?
    } else {
        server.trim()
    };
    let url = if endpoint.contains("://") {
        endpoint.to_owned()
    } else {
        format!("http://{endpoint}")
    };
    let url = reqwest::Url::parse(&url).ok()?;
    if url.scheme() != "http"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || url.port().is_none()
    {
        return None;
    }
    let host = url.host_str()?.trim_matches(['[', ']']);
    if !host.eq_ignore_ascii_case("localhost")
        && !host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|host| host.is_loopback())
    {
        return None;
    }
    // Map the supported subset of WinHTTP bypass entries to NO_PROXY. If a
    // rule cannot be represented, do not silently send it through a proxy.
    let mut entries = vec!["localhost".to_string(), "127.0.0.1".into(), "::1".into()];
    for entry in bypass
        .split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        if entry.eq_ignore_ascii_case("<local>") {
            continue;
        }
        if let Some(prefix) = entry.strip_suffix(".*") {
            let octets = prefix
                .split('.')
                .map(str::parse::<u8>)
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            if !(1..=3).contains(&octets.len()) {
                return None;
            }
            let mut addr = [0; 4];
            addr[..octets.len()].copy_from_slice(&octets);
            entries.push(format!(
                "{}/{}",
                std::net::Ipv4Addr::from(addr),
                octets.len() * 8
            ));
            continue;
        }
        let entry = entry.strip_prefix("*.").map_or(entry, |_| &entry[1..]);
        if entry.trim_start_matches('.').is_empty()
            || !entry
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | ':'))
        {
            return None;
        }
        entries.push(entry.into());
    }
    Some(CliProxy {
        url: url.to_string(),
        bypass: entries.join(","),
    })
}

#[cfg(windows)]
fn desktop_proxy() -> Option<CliProxy> {
    use windows_sys::Win32::{
        Foundation::GlobalFree,
        Networking::WinHttp::{
            WinHttpGetIEProxyConfigForCurrentUser, WINHTTP_CURRENT_USER_IE_PROXY_CONFIG,
        },
    };
    // SAFETY: WinHTTP fills the struct and transfers ownership of each
    // non-null UTF-16 allocation to this caller, even on an early return.
    struct ProxyConfig(WINHTTP_CURRENT_USER_IE_PROXY_CONFIG);
    impl Drop for ProxyConfig {
        fn drop(&mut self) {
            for value in [
                self.0.lpszProxy,
                self.0.lpszProxyBypass,
                self.0.lpszAutoConfigUrl,
            ] {
                if !value.is_null() {
                    unsafe { GlobalFree(value.cast()) };
                }
            }
        }
    }
    fn copied(value: *const u16) -> Option<String> {
        if value.is_null() {
            return Some(String::new());
        }
        let mut units = Vec::new();
        for index in 0..4096 {
            // WinHTTP returns a NUL-terminated string. Stop at the NUL or
            // reject exceptionally large settings rather than copying them.
            let unit = unsafe { *value.add(index) };
            if unit == 0 {
                return String::from_utf16(&units).ok();
            }
            units.push(unit);
        }
        None
    }
    let mut config = ProxyConfig(unsafe { std::mem::zeroed() });
    if unsafe { WinHttpGetIEProxyConfigForCurrentUser(&mut config.0) } == 0
        || config.0.fAutoDetect != 0
        || !config.0.lpszAutoConfigUrl.is_null()
    {
        return None;
    }
    static_loopback_proxy(
        &copied(config.0.lpszProxy)?,
        &copied(config.0.lpszProxyBypass)?,
    )
}

#[cfg(not(windows))]
fn desktop_proxy() -> Option<CliProxy> {
    None
}

#[cfg(windows)]
fn is_system_drive(value: &OsStr) -> bool {
    matches!(value.to_str().map(str::as_bytes), Some([letter, b':']) if letter.is_ascii_alphabetic())
}

fn configured_command(executable: &Path, candidate: &Path, proxy: Option<CliProxy>) -> Command {
    let mut command = Command::new(executable);
    apply_minimal_child_environment(&mut command);
    // MXC must discover the Windows volume even under an intentionally
    // minimal child environment. Do not inherit any other host settings.
    #[cfg(windows)]
    if let Some(drive) = std::env::var_os("SystemDrive").filter(|drive| is_system_drive(drive)) {
        command.env("SystemDrive", drive);
    }
    if let Some(proxy) = proxy {
        command
            .env("HTTPS_PROXY", proxy.url)
            .env("NO_PROXY", proxy.bypass);
    }
    command.current_dir(candidate).args([
        "-c",
        "sandbox_workspace_write.network_access=false",
        "-c",
        "shell_environment_policy.exclude=[\"*proxy*\"]",
        "-c",
        "web_search=\"disabled\"",
    ]);
    // Ignoring user config must not quietly leave native tools unavailable,
    // or reuse a legacy Windows mode with weaker network isolation. Require
    // MXC explicitly; unsupported machines do not fall back to another mode.
    #[cfg(windows)]
    command.args(["-c", "windows.sandbox=\"mxc\""]);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

fn command(executable: &Path, candidate: &Path, proxy: Option<CliProxy>) -> Command {
    let mut command = configured_command(executable, candidate, proxy);
    command.args([
        "--ask-for-approval",
        "never",
        "exec",
        "--json",
        "--ephemeral",
        "--ignore-user-config",
        "--sandbox",
        "workspace-write",
        "-",
    ]);
    command
}

/// Keep stdout processing bounded and keep the process tree owned until
/// termination. Only structured lifecycle and command status leave the runner.
pub(super) fn run(
    executable: &Path,
    candidate: &Path,
    prompt: String,
    cancelled: &ToolCancellation,
    timeout: Duration,
    heartbeat: impl FnMut() -> bool,
) -> Result<CodexResult, CodexError> {
    run_command(
        command(executable, candidate, desktop_proxy()),
        prompt,
        cancelled,
        timeout,
        heartbeat,
    )
}

fn run_command(
    mut command: Command,
    prompt: String,
    cancelled: &ToolCancellation,
    timeout: Duration,
    mut heartbeat: impl FnMut() -> bool,
) -> Result<CodexResult, CodexError> {
    if cancelled.is_cancelled() {
        return Err(CodexError::Cancelled);
    }
    let mut child = ProcessTree::spawn_with_outcome(&mut command).map_err(|error| match error {
        crate::child_process_tree::ProcessTreeSpawnError::NotStarted(_) => CodexError::Spawn,
        #[cfg(windows)]
        crate::child_process_tree::ProcessTreeSpawnError::Started(_) => CodexError::Unverified,
    })?;
    let mut stdin = child.take_stdin().expect("Codex stdin is piped");
    let stdout = child.take_stdout().expect("Codex stdout is piped");
    let input = thread::spawn(move || stdin.write_all(prompt.as_bytes()));
    let phase = Arc::new(AtomicU8::new(0));
    let events_phase = phase.clone();
    let output = thread::spawn(move || collect_events(BufReader::new(stdout), &events_phase));
    let start = Instant::now();
    let mut next_heartbeat = start + HEARTBEAT_INTERVAL;
    let status = loop {
        let now = Instant::now();
        let stop = if cancelled.is_cancelled() {
            Some(CodexError::Cancelled)
        } else if now.duration_since(start) >= timeout {
            Some(CodexError::TimedOut(CodexPhase::read(&phase)))
        } else if now >= next_heartbeat {
            next_heartbeat = now + HEARTBEAT_INTERVAL;
            (!heartbeat()).then_some(CodexError::Cancelled)
        } else {
            None
        };
        if let Some(error) = stop {
            break child
                .terminate_and_wait()
                .map_err(|_| CodexError::Unverified)
                .and(Err(error));
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(_) => {
                let _ = child.terminate_and_wait();
                break Err(CodexError::Unverified);
            }
        }
    };
    // On Windows, closing the job kills descendants holding either pipe.
    drop(child);
    let status = status?;
    let written = input.join().is_ok_and(|result| result.is_ok());
    let result = output
        .join()
        .map_err(|_| CodexError::Unverified)?
        .map_err(|_| CodexError::Unverified)?;
    if !status.success() || result.failed {
        return Err(CodexError::Failed);
    }
    if !written || !result.completed {
        return Err(CodexError::Unverified);
    }
    Ok(result)
}

fn collect_events(mut stream: impl BufRead, phase: &AtomicU8) -> std::io::Result<CodexResult> {
    let mut result = CodexResult::default();
    let mut line = Vec::new();
    loop {
        line.clear();
        if (&mut stream)
            .take(MAX_EVENT_BYTES)
            .read_until(b'\n', &mut line)?
            == 0
        {
            return Ok(result);
        }
        if line.len() as u64 == MAX_EVENT_BYTES && line.last() != Some(&b'\n') {
            stream.skip_until(b'\n')?;
            continue;
        }
        let Ok(event) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        match event["type"].as_str() {
            Some("thread.started") => {
                phase.fetch_max(1, Ordering::Relaxed);
            }
            Some("turn.started") => {
                phase.fetch_max(2, Ordering::Relaxed);
            }
            Some("item.started" | "item.completed") if event["item"]["type"] == "file_change" => {
                phase.fetch_max(3, Ordering::Relaxed);
                result.file_change_reported = true;
                if event["type"] == "item.completed" {
                    result.file_change_completed |= event["item"]["status"] == "completed";
                    result.file_change_failed |= event["item"]["status"] == "failed";
                }
            }
            Some("item.completed") if event["item"]["type"] == "agent_message" => {
                let text = event["item"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                result.reported_sandbox_restriction |= [
                    "read-only",
                    "permission denied",
                    "sandbox",
                    "write access",
                    "写入权限",
                ]
                .iter()
                .any(|label| text.contains(label));
            }
            Some("item.started") if event["item"]["type"] == "command_execution" => {
                phase.fetch_max(3, Ordering::Relaxed);
            }
            Some("turn.completed") => result.completed = true,
            Some("turn.failed") => result.failed = true,
            Some("item.completed") if event["item"]["type"] == "command_execution" => {
                let item = &event["item"];
                match (item["status"].as_str(), item["exit_code"].as_i64()) {
                    (Some("completed"), Some(0)) => result.commands_finished += 1,
                    (Some("failed"), _) | (_, Some(1..)) | (_, Some(..=-1)) => {
                        result.commands_finished += 1;
                        result.commands_failed += 1;
                    }
                    _ => {}
                }
                #[cfg(test)]
                if item["exit_code"].as_i64() == Some(0)
                    && item["command"]
                        .as_str()
                        .is_some_and(|s| s.contains("verify-fixture.cmd"))
                    && item["aggregated_output"].as_str().is_some_and(|s| {
                        s.lines()
                            .any(|line| line.trim() == "ANGELBOT_SYNTHETIC_TEST_OK")
                    })
                {
                    result.synthetic_test_receipt = true;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    #[ignore = "explicit real Codex acceptance; uses the existing ChatGPT login on a synthetic repository"]
    fn real_codex_writes_one_synthetic_candidate_only() {
        synthetic_candidate_acceptance(false);
    }

    #[test]
    #[cfg(windows)]
    #[ignore = "local installed Codex MXC, no model or network; explicit Windows acceptance"]
    fn codex_same_process_tree_mxc_writes_and_verifies_candidate_without_model() {
        synthetic_candidate_acceptance(true);
    }

    #[cfg(windows)]
    fn synthetic_candidate_acceptance(native_only: bool) {
        use crate::agent::{
            work_package::WorkPackageScope,
            workspace_isolation::{
                GitWorktreeProvider, ReviewCaptureRequest, WorkspaceProvider, WorkspaceRequest,
                WorktreeCapability, WorktreeFileProvider, WorktreeManifestIdentity,
                WorktreeManifestProvider, WorktreeOperation,
            },
        };
        let codex =
            installed_executable().expect("Codex CLI must be installed for this acceptance");
        let repo = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        fn git(cwd: &Path, args: &[&str]) {
            let output = Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "synthetic Git fixture could not be prepared"
            );
        }
        git(repo.path(), &["init", "-q"]);
        git(repo.path(), &["config", "user.name", "Fixture"]);
        git(
            repo.path(),
            &["config", "user.email", "fixture@example.invalid"],
        );
        std::fs::write(repo.path().join("fixture.txt"), "before\n").unwrap();
        std::fs::write(repo.path().join("verify-fixture.cmd"), "@echo off\r\nfindstr /x /l ANGELBOT_CODEX_CANDIDATE fixture.txt >nul || exit /b 1\r\necho ANGELBOT_SYNTHETIC_TEST_OK\r\n").unwrap();
        if native_only {
            std::fs::write(repo.path().join("native-probe.cmd"), "@echo off\r\necho ANGELBOT_CODEX_CANDIDATE>fixture.txt\r\nif errorlevel 1 exit /b 1\r\ncall verify-fixture.cmd\r\nexit /b %errorlevel%\r\n").unwrap();
        }
        git(repo.path(), &["add", "--", "."]);
        git(repo.path(), &["commit", "-qm", "synthetic base"]);
        let scope = WorkPackageScope {
            session_id: "synthetic".into(),
            owner_profile_id: 1,
            workspace_key: "synthetic".into(),
        };
        let provider =
            GitWorktreeProvider::new(storage.path(), storage.path().join("manifests")).unwrap();
        let handle = provider
            .prepare(&WorkspaceRequest {
                scope,
                work_package_id: "codex_fixture".into(),
                attempt_id: "attempt_fixture".into(),
                canonical_workspace: repo.path().into(),
                base_revision: "HEAD".into(),
                lease_epoch: 1,
            })
            .unwrap();
        let identity = WorktreeManifestIdentity::for_handle(&handle, "a".repeat(64)).unwrap();
        provider.persist_manifest(&handle, &identity).unwrap();
        let capability = WorktreeCapability {
            read_roots: vec![".".into()],
            write_roots: vec![".".into()],
            operation: WorktreeOperation::Write,
            lease_epoch: 1,
            collaboration_key: "synthetic".into(),
        };
        let receipt = provider.with_candidate_executor(&identity, capability.clone(), |candidate| {
            if native_only {
                native_mxc_probe(&codex, candidate);
                None
            } else {
                Some(run(&codex, candidate, "Implement one isolated repository fix now using the available tools: replace `before` in the tracked file `fixture.txt` with `ANGELBOT_CODEX_CANDIDATE`, leaving a final newline. Then use the shell to run `cmd.exe /d /c verify-fixture.cmd`; this checked-in test reads the file and exits nonzero unless the change works. Do not just describe the change or claim the test ran. Touch only fixture.txt; do not commit, ask for more permissions, or use the network. This is synthetic data.".into(),
                    &ToolCancellation::new(), Duration::from_secs(90), || true).unwrap())
            }
        }).unwrap();
        let (paths, hash) = provider
            .inspect_executor_candidate(&identity, capability.clone())
            .unwrap();
        assert_eq!(
            paths,
            ["fixture.txt"],
            "the executor supplied no verifiable edit: {receipt:?}"
        );
        let patch = provider.executor_review_patch(&identity, &hash).unwrap();
        assert!(patch.contains("-before") && patch.contains("+ANGELBOT_CODEX_CANDIDATE"));
        let verified = provider.resolve_manifest(&identity).unwrap().unwrap();
        let access = provider.open_file_access(&identity, capability).unwrap();
        let artifact = provider
            .capture_verified(
                &verified,
                &access,
                &ReviewCaptureRequest {
                    change_set_id: None,
                    allowed_relative_paths: paths,
                    scope_digest: "a".repeat(64),
                    commit_message: "Synthetic candidate capture".into(),
                    created_at: 100,
                },
            )
            .unwrap();
        assert_eq!(artifact.diff_hash, hash);
        assert_eq!(
            std::fs::read_to_string(repo.path().join("fixture.txt"))
                .unwrap()
                .trim(),
            "before"
        );
        let status = Command::new("git")
            .current_dir(repo.path())
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(status.status.success() && status.stdout.is_empty());
        provider.cleanup(&handle).unwrap();
        if let Some(receipt) = receipt {
            assert!(receipt.synthetic_test_receipt, "Git edit independently verified; no observed receipt for the actual test command: {receipt:?}");
        }
    }

    #[cfg(windows)]
    fn native_mxc_probe(codex: &Path, candidate: &Path) {
        let mut probe = configured_command(codex, candidate, desktop_proxy());
        probe.args([
            "sandbox",
            "--include-managed-config",
            "--permission-profile",
            ":workspace",
            "--",
            "cmd.exe",
            "/d",
            "/c",
            "native-probe.cmd",
        ]);
        let mut child = ProcessTree::spawn(&mut probe).unwrap();
        let Some(stdout) = child.take_stdout() else {
            panic!("native probe stdout unavailable");
        };
        let read = thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.take(4097).read_to_end(&mut bytes).unwrap();
            bytes
        });
        let deadline = Instant::now() + Duration::from_secs(15);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.terminate_and_wait().unwrap();
                panic!("native MXC command timed out");
            }
            thread::sleep(POLL_INTERVAL);
        };
        drop(child);
        let output = read.join().unwrap();
        let marker_seen = String::from_utf8_lossy(&output)
            .lines()
            .any(|line| line.trim() == "ANGELBOT_SYNTHETIC_TEST_OK");
        assert!(
            status.success() && marker_seen,
            "native MXC process tree: exit={:?}, test_marker={marker_seen}",
            status.code()
        );
    }

    #[test]
    #[cfg(windows)]
    fn distinguishes_process_success_failure_unverified_and_cancellation() {
        fn fixture(script: &str) -> Command {
            let mut cmd = Command::new("powershell.exe");
            cmd.args(["-NoProfile", "-NonInteractive", "-Command", script])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            cmd
        }
        let cases = [
            (
                r#"[void][Console]::In.ReadToEnd(); [Console]::WriteLine('{"type":"turn.completed"}'); exit 0"#,
                None,
            ),
            (
                r#"[void][Console]::In.ReadToEnd(); [Console]::WriteLine('{"type":"turn.completed"}'); exit 3"#,
                Some(CodexError::Failed),
            ),
            (
                r#"[void][Console]::In.ReadToEnd(); [Console]::WriteLine('{"type":"turn.failed"}'); exit 0"#,
                Some(CodexError::Failed),
            ),
            (
                r#"[void][Console]::In.ReadToEnd(); exit 0"#,
                Some(CodexError::Unverified),
            ),
        ];
        for (script, error) in cases {
            assert_eq!(
                run_command(
                    fixture(script),
                    "Synthetic prompt".into(),
                    &ToolCancellation::new(),
                    Duration::from_secs(10),
                    || true
                )
                .err(),
                error
            );
        }
        let cancellation = ToolCancellation::new();
        let signal = cancellation.clone();
        let task = thread::spawn(move || {
            run_command(
                fixture("Start-Sleep -Seconds 30"),
                "Synthetic prompt".into(),
                &signal,
                Duration::from_secs(10),
                || true,
            )
        });
        thread::sleep(Duration::from_millis(500));
        cancellation.cancel();
        assert_eq!(task.join().unwrap(), Err(CodexError::Cancelled));
    }

    #[test]
    fn reports_observed_command_outcomes_without_model_assertions() {
        let stream =
            br#"{"type":"item.completed","item":{"type":"agent_message","text":"All tests passed"}}
{"type":"item.completed","item":{"type":"command_execution","status":"completed","exit_code":0}}
{"type":"item.completed","item":{"type":"command_execution","status":"failed","exit_code":2}}
{"type":"turn.completed"}"#;
        let result = collect_events(&stream[..], &AtomicU8::new(0)).unwrap();
        assert!(result.completed);
        assert_eq!(result.commands_finished, 2);
        assert_eq!(result.commands_failed, 1);
    }

    #[test]
    fn file_change_events_distinguish_attempt_failure_and_completion() {
        let stream =
            br#"{"type":"item.started","item":{"type":"file_change","status":"in_progress"}}
{"type":"item.completed","item":{"type":"file_change","status":"failed"}}
{"type":"turn.completed"}"#;
        let result = collect_events(&stream[..], &AtomicU8::new(0)).unwrap();
        assert!(result.file_change_reported && result.file_change_failed && result.completed);
        assert!(!result.file_change_completed);
        let success =
            br#"{"type":"item.completed","item":{"type":"file_change","status":"completed"}}"#;
        let result = collect_events(&success[..], &AtomicU8::new(0)).unwrap();
        assert!(result.file_change_completed && !result.file_change_failed);
    }

    #[test]
    fn stage_diagnostics_only_retains_the_last_observed_protocol_boundary() {
        let signal = AtomicU8::new(0);
        assert_eq!(CodexPhase::read(&signal), CodexPhase::Launch);
        collect_events(
            &b"{\"type\":\"turn.started\"}\n{\"type\":\"thread.started\"}\n"[..],
            &signal,
        )
        .unwrap();
        assert_eq!(CodexPhase::read(&signal), CodexPhase::Inference);
        collect_events(
            &b"{\"type\":\"item.completed\",\"item\":{\"type\":\"file_change\"}}\n"[..],
            &signal,
        )
        .unwrap();
        assert_eq!(CodexPhase::read(&signal), CodexPhase::Action);
    }

    #[test]
    fn keeps_terminal_events_after_oversized_tool_output() {
        let mut stream = vec![b'x'; MAX_EVENT_BYTES as usize + 5];
        stream.extend_from_slice(b"\n{\"type\":\"turn.failed\"}\n");
        let result = collect_events(&stream[..], &AtomicU8::new(0)).unwrap();
        assert!(result.failed);
        assert!(!result.completed);
    }

    #[test]
    #[cfg(windows)]
    fn codex_bootstrap_accepts_only_a_system_drive_letter() {
        assert!(is_system_drive(OsStr::new("C:")));
        assert!(!is_system_drive(OsStr::new("C:\\")));
        assert!(!is_system_drive(OsStr::new(r"\\server\volume")));
        assert!(!is_system_drive(OsStr::new("")));
        let expected = std::env::var_os("SystemDrive").filter(|drive| is_system_drive(drive));
        let child = command(Path::new("codex"), Path::new("candidate"), None);
        assert_eq!(
            child
                .get_envs()
                .find_map(|(name, value)| (name == "SystemDrive").then_some(value).flatten())
                .map(OsStr::to_owned),
            expected
        );
    }

    #[test]
    fn passes_prompt_via_stdin_with_constrained_cli_flags() {
        let candidate = Path::new("candidate");
        let command = command(Path::new("codex"), candidate, None);
        let arguments: Vec<_> = command.get_args().collect();
        assert_eq!(command.get_current_dir(), Some(candidate));
        assert_eq!(arguments.last(), Some(&OsStr::new("-")));
        assert!(arguments
            .windows(2)
            .any(|window| window == ["--sandbox", "workspace-write"]));
        assert!(arguments.contains(&OsStr::new("--ignore-user-config")));
        #[cfg(windows)]
        assert!(arguments.contains(&OsStr::new("windows.sandbox=\"mxc\"")));
    }

    #[test]
    fn proxy_is_narrowed_to_cli_transport_and_kept_out_of_model_commands() {
        let proxy = static_loopback_proxy(
            "http=127.0.0.1:8888;https=127.0.0.1:9999",
            "<local>;*.example.invalid;192.168.*",
        )
        .unwrap();
        assert_eq!(proxy.url, "http://127.0.0.1:9999/");
        assert!(proxy.bypass.contains(".example.invalid"));
        assert!(proxy.bypass.contains("192.168.0.0/16"));
        let command = command(Path::new("codex"), Path::new("candidate"), Some(proxy));
        assert!(command.get_envs().any(|(name, _)| name == "HTTPS_PROXY"));
        assert!(command
            .get_envs()
            .all(|(name, _)| !name.to_string_lossy().contains("KEY")));
        let args: Vec<_> = command.get_args().collect();
        assert!(args.contains(&OsStr::new(
            "shell_environment_policy.exclude=[\"*proxy*\"]"
        )));
        assert!(args.contains(&OsStr::new("sandbox_workspace_write.network_access=false")));
        for endpoint in [
            "proxy.example.invalid:80",
            "https://localhost:7890",
            "user@localhost:7890",
            "localhost:7890/extra",
        ] {
            assert!(static_loopback_proxy(endpoint, "").is_none());
        }
        assert!(static_loopback_proxy("localhost:7890", "unhandled*pattern").is_none());
    }
}

use super::{McpProcessManager, McpTool};
use std::fs;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const RESPONSIVENESS_DEADLINE: Duration = Duration::from_secs(4);

struct HeldPolicyRead {
    release: Option<mpsc::Sender<()>>,
    handle: Option<JoinHandle<Result<(), String>>>,
}

impl HeldPolicyRead {
    fn on(manager: Arc<McpProcessManager>, server_id: &'static str) -> Self {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            manager.with_server_policy_read(server_id, || {
                entered_tx.send(()).map_err(|error| error.to_string())?;
                release_rx.recv().map_err(|error| error.to_string())
            })
        });
        entered_rx
            .recv_timeout(RESPONSIVENESS_DEADLINE)
            .expect("server policy read did not enter");
        Self {
            release: Some(release_tx),
            handle: Some(handle),
        }
    }

    fn release(mut self) {
        self.release.take().unwrap().send(()).unwrap();
        self.handle.take().unwrap().join().unwrap().unwrap();
    }
}

impl Drop for HeldPolicyRead {
    fn drop(&mut self) {
        // Dropping the sender releases a held read even if an assertion fails.
        self.release.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn spawn_policy_action(
    manager: Arc<McpProcessManager>,
    action: impl FnOnce(&McpProcessManager) -> Result<(), String> + Send + 'static,
) -> (
    mpsc::Receiver<()>,
    mpsc::Receiver<Result<(), String>>,
    JoinHandle<()>,
) {
    let (started_tx, started_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let _ = started_tx.send(());
        let _ = result_tx.send(action(&manager));
    });
    (started_rx, result_rx, handle)
}

#[test]
fn one_server_policy_read_allows_another_servers_read_and_config_write() {
    let manager = Arc::new(McpProcessManager::new());
    let held_read = HeldPolicyRead::on(Arc::clone(&manager), "service-a");
    let (read_started, read_result, read_handle) =
        spawn_policy_action(Arc::clone(&manager), |manager| {
            manager.with_server_policy_read("service-b", || Ok(()))
        });
    let (write_started, write_result, write_handle) =
        spawn_policy_action(Arc::clone(&manager), |manager| {
            manager.with_server_policy_gate("service-b", || Ok(()))
        });
    read_started.recv_timeout(RESPONSIVENESS_DEADLINE).unwrap();
    write_started.recv_timeout(RESPONSIVENESS_DEADLINE).unwrap();

    let read_before_release = read_result.recv_timeout(RESPONSIVENESS_DEADLINE);
    let write_before_release = write_result.recv_timeout(RESPONSIVENESS_DEADLINE);
    held_read.release();
    read_handle.join().unwrap();
    write_handle.join().unwrap();

    read_before_release
        .expect("service B read was blocked by service A read")
        .unwrap();
    write_before_release
        .expect("service B config write was blocked by service A read")
        .unwrap();
}

#[test]
fn same_server_and_global_policy_writes_wait_for_an_active_read() {
    let manager = Arc::new(McpProcessManager::new());
    let held_read = HeldPolicyRead::on(Arc::clone(&manager), "service-a");
    let (same_started, same_result, same_handle) =
        spawn_policy_action(Arc::clone(&manager), |manager| {
            manager.with_server_policy_gate("service-a", || Ok(()))
        });
    let (global_started, global_result, global_handle) =
        spawn_policy_action(Arc::clone(&manager), |manager| {
            manager.with_policy_gate(|| Ok(()))
        });
    same_started.recv_timeout(RESPONSIVENESS_DEADLINE).unwrap();
    global_started
        .recv_timeout(RESPONSIVENESS_DEADLINE)
        .unwrap();

    let same_before_release = same_result.recv_timeout(Duration::from_millis(150));
    let global_before_release = global_result.recv_timeout(Duration::from_millis(150));
    held_read.release();
    let same_after_release = same_result.recv_timeout(RESPONSIVENESS_DEADLINE);
    let global_after_release = global_result.recv_timeout(RESPONSIVENESS_DEADLINE);
    same_handle.join().unwrap();
    global_handle.join().unwrap();

    assert!(
        matches!(same_before_release, Err(mpsc::RecvTimeoutError::Timeout)),
        "same-service config write ran before the active read completed: {same_before_release:?}"
    );
    assert!(
        matches!(global_before_release, Err(mpsc::RecvTimeoutError::Timeout)),
        "global import/clear write ran before the active read completed: {global_before_release:?}"
    );
    same_after_release.unwrap().unwrap();
    global_after_release.unwrap().unwrap();
}

#[test]
fn same_server_lifecycle_gate_preempts_a_read_while_config_write_waits() {
    let manager = Arc::new(McpProcessManager::new());
    let held_read = HeldPolicyRead::on(Arc::clone(&manager), "service-a");
    let (lifecycle_started, lifecycle_result, lifecycle_handle) =
        spawn_policy_action(Arc::clone(&manager), |manager| {
            manager.with_server_lifecycle_gate("service-a", || Ok(()))
        });
    let (config_started, config_result, config_handle) =
        spawn_policy_action(Arc::clone(&manager), |manager| {
            manager.with_server_policy_gate("service-a", || Ok(()))
        });
    lifecycle_started
        .recv_timeout(RESPONSIVENESS_DEADLINE)
        .unwrap();
    config_started
        .recv_timeout(RESPONSIVENESS_DEADLINE)
        .unwrap();

    let lifecycle_before_release = lifecycle_result.recv_timeout(RESPONSIVENESS_DEADLINE);
    let config_before_release = config_result.recv_timeout(Duration::from_millis(150));
    held_read.release();
    let config_after_release = config_result.recv_timeout(RESPONSIVENESS_DEADLINE);
    lifecycle_handle.join().unwrap();
    config_handle.join().unwrap();

    lifecycle_before_release
        .expect("same-service lifecycle gate was blocked by active read")
        .unwrap();
    assert!(
        matches!(config_before_release, Err(mpsc::RecvTimeoutError::Timeout)),
        "same-service config write ran before the active read completed: {config_before_release:?}"
    );
    config_after_release.unwrap().unwrap();
}

struct StalledServerFixture {
    _dir: tempfile::TempDir,
    script: PathBuf,
    observed: PathBuf,
    release: PathBuf,
    manager: Arc<McpProcessManager>,
}

impl StalledServerFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mcp-concurrency.cjs");
        let observed = dir.path().join("request-observed");
        let release = dir.path().join("release-request");
        fs::write(
            &script,
            r#"
const fs = require('fs');
const readline = require('readline');
const [mode, observed, release] = process.argv.slice(2);

function respond(id, result) {
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id, result }) + '\n');
}

function afterRelease(reply) {
  fs.writeFileSync(observed, 'seen');
  const poll = setInterval(() => {
    if (fs.existsSync(release)) {
      clearInterval(poll);
      reply();
    }
  }, 10);
}

readline.createInterface({ input: process.stdin }).on('line', line => {
  const request = JSON.parse(line);
  if (request.method === 'initialize') {
    const reply = () => respond(request.id, {
      protocolVersion: '2024-11-05', capabilities: { tools: {} },
      serverInfo: { name: 'concurrency-fixture', version: '1.0.0' }
    });
    if (mode === 'stalled-init') afterRelease(reply);
    else reply();
  } else if (request.method === 'tools/list') {
    const reply = () => respond(request.id, {
      tools: [{ name: 'echo', description: 'Fixture', inputSchema: { type: 'object' } }]
    });
    if (mode === 'stalled') afterRelease(reply);
    else reply();
  } else if (request.method === 'tools/call') {
    respond(request.id, { content: [{ type: 'text', text: 'fixture called' }] });
  }
});
"#,
        )
        .unwrap();
        Self {
            _dir: dir,
            script,
            observed,
            release,
            manager: Arc::new(McpProcessManager::new()),
        }
    }

    fn args(&self, mode: &str) -> String {
        serde_json::json!([
            self.script.to_str().unwrap(),
            mode,
            self.observed.to_str().unwrap(),
            self.release.to_str().unwrap()
        ])
        .to_string()
    }

    fn start_stalled(&self) {
        self.manager
            .start(
                "stalled",
                "Stalled fixture",
                "node",
                &self.args("stalled"),
                "",
            )
            .unwrap();
    }

    fn refresh_stalled(&self) -> (JoinHandle<()>, mpsc::Receiver<Result<Vec<McpTool>, String>>) {
        let manager = Arc::clone(&self.manager);
        let (result_tx, result_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let _ = result_tx.send(manager.refresh_tools("stalled"));
        });
        (handle, result_rx)
    }

    fn wait_until_request_is_in_flight(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.observed.exists() {
            assert!(
                Instant::now() < deadline,
                "fixture never received the stalled tools/list request"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn release_request(&self) {
        fs::write(&self.release, "release").unwrap();
    }
}

impl Drop for StalledServerFixture {
    fn drop(&mut self) {
        // Also unblock the fixture if an assertion fails before explicit cleanup.
        let _ = fs::write(&self.release, "release");
        self.manager.stop_all();
    }
}

#[test]
fn stalled_server_discovery_does_not_block_another_server() {
    let fixture = StalledServerFixture::new();
    fixture.start_stalled();
    let (stalled_handle, stalled_rx) = fixture.refresh_stalled();
    fixture.wait_until_request_is_in_flight();

    let manager = Arc::clone(&fixture.manager);
    let fast_args = fixture.args("fast");
    let (fast_tx, fast_rx) = mpsc::channel();
    let fast_handle = thread::spawn(move || {
        let result = (|| {
            manager.start("fast", "Fast fixture", "node", &fast_args, "")?;
            let tools = manager.refresh_tools("fast")?;
            let echo = tools
                .iter()
                .find(|tool| tool.name == "echo")
                .ok_or_else(|| "fast fixture did not advertise echo".to_string())?;
            manager.call_tool_if_current_snapshot(
                "fast",
                "echo",
                &echo.snapshot_for("fast"),
                &serde_json::json!({}),
                || Ok(()),
            )
        })();
        let _ = fast_tx.send(result);
    });

    let fast_before_release = fast_rx.recv_timeout(RESPONSIVENESS_DEADLINE);
    fixture.release_request();
    let stalled_result = stalled_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let _ = fast_rx.recv_timeout(Duration::from_secs(5));
    stalled_handle.join().unwrap();
    fast_handle.join().unwrap();

    assert_eq!(stalled_result.unwrap()[0].name, "echo");
    assert_eq!(
        fast_before_release
            .expect("fast server was blocked by stalled server I/O")
            .unwrap(),
        "fixture called"
    );
}

#[test]
fn status_remains_responsive_during_its_servers_stalled_request() {
    let fixture = StalledServerFixture::new();
    fixture.start_stalled();
    let (stalled_handle, stalled_rx) = fixture.refresh_stalled();
    fixture.wait_until_request_is_in_flight();

    let manager = Arc::clone(&fixture.manager);
    let (status_tx, status_rx) = mpsc::channel();
    let status_handle = thread::spawn(move || {
        let _ = status_tx.send(manager.get_status("stalled"));
    });
    let status_before_release = status_rx.recv_timeout(RESPONSIVENESS_DEADLINE);
    fixture.release_request();
    let _ = stalled_rx.recv_timeout(Duration::from_secs(5));
    let _ = status_rx.recv_timeout(Duration::from_secs(5));
    stalled_handle.join().unwrap();
    status_handle.join().unwrap();

    assert_eq!(
        status_before_release
            .expect("status was blocked by its server's stalled I/O")
            .unwrap()
            .status,
        "running"
    );
}

#[test]
fn stop_preempts_its_servers_stalled_request() {
    let fixture = StalledServerFixture::new();
    fixture.start_stalled();
    let (stalled_handle, stalled_rx) = fixture.refresh_stalled();
    fixture.wait_until_request_is_in_flight();

    let manager = Arc::clone(&fixture.manager);
    let (stop_tx, stop_rx) = mpsc::channel();
    let stop_handle = thread::spawn(move || {
        let _ = stop_tx.send(manager.stop("stalled"));
    });
    let stop_before_release = stop_rx.recv_timeout(RESPONSIVENESS_DEADLINE);
    fixture.release_request();
    let stalled_result = stalled_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let _ = stop_rx.recv_timeout(Duration::from_secs(5));
    stalled_handle.join().unwrap();
    stop_handle.join().unwrap();

    stop_before_release
        .expect("stop was blocked by its server's stalled I/O")
        .unwrap();
    assert!(
        stalled_result.is_err(),
        "stop must interrupt the pending request"
    );
    assert_eq!(
        fixture.manager.get_status("stalled").unwrap().status,
        "stopped"
    );
}

#[test]
fn stop_cancels_a_silent_first_connection_without_resurrecting_it() {
    let fixture = StalledServerFixture::new();
    let manager = Arc::clone(&fixture.manager);
    let args = fixture.args("stalled-init");
    let (start_tx, start_rx) = mpsc::channel();
    let start_handle = thread::spawn(move || {
        let result = manager
            .with_server_policy_gate("cancel-init", || {
                manager.begin_start("cancel-init", "Silent fixture", "node", &args, "")
            })
            .and_then(|pending| manager.finish_start(pending));
        let _ = start_tx.send(result);
    });
    fixture.wait_until_request_is_in_flight();
    assert_eq!(
        fixture.manager.get_status("cancel-init").unwrap().status,
        "starting"
    );

    let manager = Arc::clone(&fixture.manager);
    let (stop_tx, stop_rx) = mpsc::channel();
    let stop_handle = thread::spawn(move || {
        let result =
            manager.with_server_lifecycle_gate("cancel-init", || manager.stop("cancel-init"));
        let _ = stop_tx.send(result);
    });
    let stop_before_release = stop_rx.recv_timeout(RESPONSIVENESS_DEADLINE);
    fixture.release_request();
    let start_result = start_rx.recv_timeout(RESPONSIVENESS_DEADLINE);
    start_handle.join().unwrap();
    stop_handle.join().unwrap();

    stop_before_release
        .expect("stop waited for the silent initialize timeout")
        .unwrap();
    let start_error = start_result.unwrap().unwrap_err();
    assert!(start_error.contains("stopped"), "{start_error}");
    assert!(!start_error.contains("Server exited"), "{start_error}");
    assert_eq!(
        fixture.manager.get_status("cancel-init").unwrap().status,
        "stopped"
    );
}

#[test]
fn stale_initialize_failure_cannot_remove_a_replacement_under_the_same_id() {
    let fixture = StalledServerFixture::new();
    let manager = Arc::clone(&fixture.manager);
    let stalled_args = fixture.args("stalled-init");
    let (start_tx, start_rx) = mpsc::channel();
    let start_handle = thread::spawn(move || {
        let _ = start_tx.send(manager.start("reused", "Old fixture", "node", &stalled_args, ""));
    });
    fixture.wait_until_request_is_in_flight();

    // Hold only the old child lock: stop removes it from the registry, then
    // waits to kill it. The replacement can initialize before the old start
    // thread observes either its late reply or EOF.
    let old_entry = fixture.manager.process_entry("reused").unwrap();
    let old_child = old_entry.child.lock().unwrap();
    let manager = Arc::clone(&fixture.manager);
    let (stop_tx, stop_rx) = mpsc::channel();
    let stop_handle = thread::spawn(move || {
        let _ = stop_tx.send(manager.stop("reused"));
    });
    let deadline = Instant::now() + RESPONSIVENESS_DEADLINE;
    while fixture
        .manager
        .managed_server_ids()
        .unwrap()
        .iter()
        .any(|id| id == "reused")
    {
        assert!(
            Instant::now() < deadline,
            "stop did not remove the old entry"
        );
        thread::sleep(Duration::from_millis(10));
    }

    fixture
        .manager
        .start("reused", "Replacement", "node", &fixture.args("fast"), "")
        .unwrap();
    fixture.release_request();
    drop(old_child);
    let old_start = start_rx.recv_timeout(RESPONSIVENESS_DEADLINE).unwrap();
    let stopped_old = stop_rx.recv_timeout(RESPONSIVENESS_DEADLINE).unwrap();
    start_handle.join().unwrap();
    stop_handle.join().unwrap();

    assert!(old_start.is_err(), "stopped initialization cannot succeed");
    stopped_old.unwrap();
    assert_eq!(
        fixture.manager.get_status("reused").unwrap().status,
        "running"
    );
    let tools = fixture.manager.refresh_tools("reused").unwrap();
    let output = fixture
        .manager
        .call_tool_if_current_snapshot(
            "reused",
            "echo",
            &tools[0].snapshot_for("reused"),
            &serde_json::json!({}),
            || Ok(()),
        )
        .unwrap();
    assert_eq!(output, "fixture called");
}

#[test]
fn stop_all_rejects_later_starts_before_spawning_a_child() {
    let manager = McpProcessManager::new();
    manager.stop_all();
    let error = manager
        .start(
            "after-shutdown",
            "After shutdown",
            "not-a-real-mcp-command",
            "",
            "",
        )
        .unwrap_err();
    assert!(error.contains("shutting down"), "unexpected error: {error}");
    assert!(manager.managed_server_ids().unwrap().is_empty());
}

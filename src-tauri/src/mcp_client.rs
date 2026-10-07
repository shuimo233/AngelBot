//! MCP client process manager — spawn, communicate with, and manage MCP servers.
//!
//! Implements the Model Context Protocol (MCP) stdio transport:
//! - Spawns child processes from stored server configs
//! - JSON-RPC initialization handshake (initialize → initialized)
//! - tools/list for discovering server capabilities
//! - Bounded requests, process cleanup, and error reporting

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant};

use crate::child_process_tree::ProcessTree;

const MAX_MCP_LINE_BYTES: usize = 4 * 1024 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(windows)]
const NPX_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(90);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
const TOOL_CALL_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_MCP_TOOL_IMAGES: usize = 2;
use crate::llm::MAX_TOOL_IMAGE_TOTAL_BYTES as MAX_MCP_TOOL_IMAGE_BYTES;
const MAX_MCP_STRUCTURED_DEPTH: usize = 16;
const MAX_MCP_STRUCTURED_NODES: usize = 4096;

/// Image payloads stay on the internal model path, never the String/IPC path.
#[derive(Debug)]
pub(crate) struct McpToolOutput {
    pub text: String,
    pub images: Vec<crate::llm::ToolImage>,
}

/// Information about a running MCP server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub server_id: String,
    pub status: String, // "stopped", "starting", "running", "error"
    pub error: Option<String>,
    pub tools_count: Option<usize>,
}

/// JSON-RPC response payload
#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: Option<String>,
    id: Option<u64>,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

/// A rejected request is safe to reconsider; a dispatched call without a
/// trustworthy receipt must not be replayed as an ordinary tool failure.
#[derive(Debug)]
pub(crate) enum McpToolCallError {
    Known(String),
    ResultUnknown,
}

impl std::fmt::Display for McpToolCallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Known(message) => formatter.write_str(message),
            Self::ResultUnknown => formatter.write_str(
                "MCP 工具请求已派发但未收到可靠回执，外部操作结果未知；不要自动重试，请先检查目标应用。",
            ),
        }
    }
}

impl From<String> for McpToolCallError {
    fn from(message: String) -> Self {
        Self::Known(message)
    }
}

/// A running MCP server entry
struct McpProcessEntry {
    child: Mutex<ProcessTree>,
    server_name: String,
    handshake_timeout: Duration,
    ready: AtomicBool,
    /// Stop closes admission before terminating the tree. A request holding an
    /// old Arc must not enqueue a new side effect after that point.
    send_open: Mutex<bool>,
    io: Mutex<McpProcessIo>,
}

/// One server's ordered JSON-RPC exchange. Waiting for its response must not
/// hold the process table or another server's I/O lock.
struct McpProcessIo {
    next_request_id: u64,
    writes: SyncSender<String>,
    responses: Receiver<Result<String, String>>,
}

/// A child already admitted under the server policy/lifecycle gate. The
/// handshake may wait outside that gate so Stop can cancel the first connect.
pub(crate) struct McpPendingStart {
    server_id: String,
    entry: Arc<McpProcessEntry>,
}

struct McpServerGate {
    policy: RwLock<()>,
    lifecycle: Mutex<()>,
}

/// Manages all MCP server processes
pub struct McpProcessManager {
    /// Global imports/clear wait for all active calls. Ordinary per-server
    /// configuration changes only wait for that server's calls.
    policy_gate: RwLock<()>,
    server_policy_gates: Mutex<HashMap<String, Weak<McpServerGate>>>,
    shutting_down: AtomicBool,
    processes: Mutex<HashMap<String, Arc<McpProcessEntry>>>,
    /// Cached tool lists per server
    tool_lists: Mutex<HashMap<String, Vec<McpTool>>>,
}

/// Represents a tool discovered from an MCP server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// Stable contract captured when a Main-Agent tool is presented for approval.
///
/// The revision deliberately binds the server, remote tool name, and canonical
/// input schema. Descriptions are explanatory text, not executable authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct McpToolSnapshot {
    pub(crate) revision: String,
}

impl McpTool {
    pub(crate) fn snapshot_for(&self, server_id: &str) -> McpToolSnapshot {
        let canonical_schema = canonical_json(&self.input_schema);
        let mut hasher = Sha256::new();
        hasher.update(b"angelbot:mcp-tool-contract:v1\0");
        hasher.update(server_id.as_bytes());
        hasher.update(b"\0");
        hasher.update(self.name.as_bytes());
        hasher.update(b"\0");
        hasher.update(canonical_schema.as_bytes());
        McpToolSnapshot {
            revision: format!("{:x}", hasher.finalize()),
        }
    }
}

fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(entries) => {
            let mut keys = entries.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let body = keys
                .into_iter()
                .map(|key| {
                    let encoded_key =
                        serde_json::to_string(key).expect("JSON object keys serialize");
                    format!("{encoded_key}:{}", canonical_json(&entries[key]))
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
        serde_json::Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        primitive => primitive.to_string(),
    }
}

impl Default for McpProcessManager {
    fn default() -> Self {
        Self::new()
    }
}

impl McpProcessManager {
    pub fn new() -> Self {
        Self {
            policy_gate: RwLock::new(()),
            server_policy_gates: Mutex::new(HashMap::new()),
            shutting_down: AtomicBool::new(false),
            processes: Mutex::new(HashMap::new()),
            tool_lists: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_policy_gate<T>(
        &self,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let _guard = self
            .policy_gate
            .write()
            .map_err(|error| error.to_string())?;
        action()
    }

    fn server_policy_gate(&self, server_id: &str) -> Result<Arc<McpServerGate>, String> {
        let mut gates = self
            .server_policy_gates
            .lock()
            .map_err(|error| error.to_string())?;
        if let Some(gate) = gates.get(server_id).and_then(Weak::upgrade) {
            return Ok(gate);
        }
        // Weak entries do not retain a lock for every service ever configured.
        gates.retain(|_, gate| gate.strong_count() > 0);
        let gate = Arc::new(McpServerGate {
            policy: RwLock::new(()),
            lifecycle: Mutex::new(()),
        });
        gates.insert(server_id.to_string(), Arc::downgrade(&gate));
        Ok(gate)
    }

    /// A config edit or launch excludes calls to the same service, while calls
    /// to unrelated services continue. A global import still excludes both.
    pub fn with_server_policy_gate<T>(
        &self,
        server_id: &str,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let _global = self.policy_gate.read().map_err(|error| error.to_string())?;
        let gate = self.server_policy_gate(server_id)?;
        let _server = gate.policy.write().map_err(|error| error.to_string())?;
        let _lifecycle = gate.lifecycle.lock().map_err(|error| error.to_string())?;
        action()
    }

    /// Stop can cancel an in-flight request without waiting for its response.
    /// Start and config edits still serialize with Stop through `lifecycle`.
    pub fn with_server_lifecycle_gate<T>(
        &self,
        server_id: &str,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let _global = self.policy_gate.read().map_err(|error| error.to_string())?;
        let gate = self.server_policy_gate(server_id)?;
        let _lifecycle = gate.lifecycle.lock().map_err(|error| error.to_string())?;
        action()
    }

    fn with_server_policy_read<T>(
        &self,
        server_id: &str,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let _global = self.policy_gate.read().map_err(|error| error.to_string())?;
        let gate = self.server_policy_gate(server_id)?;
        let _server = gate.policy.read().map_err(|error| error.to_string())?;
        action()
    }

    /// Start an MCP server by its config ID.
    pub fn start(
        &self,
        server_id: &str,
        name: &str,
        command: &str,
        args: &str,
        env_str: &str,
    ) -> Result<(), String> {
        let pending = self.begin_start(server_id, name, command, args, env_str)?;
        self.finish_start(pending)
    }

    /// Spawn and register the child while configuration and Stop are excluded.
    /// The caller must hold the server policy gate until this returns.
    pub(crate) fn begin_start(
        &self,
        server_id: &str,
        name: &str,
        command: &str,
        args: &str,
        env_str: &str,
    ) -> Result<McpPendingStart, String> {
        // A child can exit between two settings visits. Reap it here so an
        // explicit retry can start a replacement under the same server ID.
        if self.shutting_down.load(Ordering::Acquire) {
            return Err("MCP process manager is shutting down".to_string());
        }
        let prior = self
            .processes
            .lock()
            .map_err(|error| error.to_string())?
            .get(server_id)
            .cloned();
        if let Some(entry) = prior {
            let status = entry
                .child
                .lock()
                .map_err(|error| error.to_string())?
                .try_wait();
            match status {
                Ok(None) => return Err(format!("Server '{}' is already running", name)),
                Ok(Some(_)) => {
                    Self::terminate_entry(&entry)?;
                    self.remove_if_current(server_id, &entry)?;
                }
                Err(error) => return Err(format!("Failed to inspect MCP server: {error}")),
            }
        }

        let args_vec = parse_command_args(args)?;

        // Parse env vars (format: KEY=VALUE\nKEY2=VALUE2)
        let env_vars: Vec<(&str, &str)> = if env_str.is_empty() {
            Vec::new()
        } else {
            env_str
                .lines()
                .filter_map(|line| line.split_once('='))
                .map(|(k, v)| (k.trim(), v.trim()))
                .collect()
        };

        // Spawn the process
        let executable = resolve_mcp_executable(command);
        let mut cmd = Command::new(executable);
        crate::child_process_env::apply_minimal_child_environment(&mut cmd);
        cmd.args(&args_vec);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        // Unread stderr can fill its pipe and deadlock a healthy server.
        // Server diagnostics are untrusted and are not needed for transport.
        cmd.stderr(Stdio::null());

        // Apply environment variables
        for (key, value) in &env_vars {
            if !key.is_empty() {
                cmd.env(key, value);
            }
        }

        if self.shutting_down.load(Ordering::Acquire) {
            return Err("MCP process manager is shutting down".to_string());
        }
        let mut child = ProcessTree::spawn(&mut cmd).map_err(|e| {
            format!(
                "Failed to start MCP server '{}' (command: {}): {}",
                name, executable, e
            )
        })?;

        let stdin = child.take_stdin().expect("piped MCP stdin");
        let stdout = child.take_stdout().expect("piped MCP stdout");
        let (write_tx, write_rx) = mpsc::sync_channel::<String>(4);
        std::thread::spawn(move || {
            let mut stdin = stdin;
            while let Ok(line) = write_rx.recv() {
                if writeln!(stdin, "{line}")
                    .and_then(|()| stdin.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        let (response_tx, response_rx) = mpsc::sync_channel(8);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_bounded_line(&mut reader) {
                    Ok(Some(line)) => {
                        if response_tx.send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {
                        let _ = response_tx.send(Err("MCP server closed stdout".to_string()));
                        break;
                    }
                    Err(error) => {
                        let _ = response_tx.send(Err(error));
                        break;
                    }
                }
            }
        });

        let entry = Arc::new(McpProcessEntry {
            child: Mutex::new(child),
            server_name: name.to_string(),
            handshake_timeout: handshake_timeout_for_command(command),
            ready: AtomicBool::new(false),
            send_open: Mutex::new(true),
            io: Mutex::new(McpProcessIo {
                next_request_id: 1,
                writes: write_tx,
                responses: response_rx,
            }),
        });
        let mut procs = self.processes.lock().map_err(|error| error.to_string())?;
        if self.shutting_down.load(Ordering::Acquire) || procs.contains_key(server_id) {
            drop(procs);
            let _ = Self::terminate_entry(&entry);
            return Err(if self.shutting_down.load(Ordering::Acquire) {
                "MCP process manager is shutting down".to_string()
            } else {
                format!("Server '{}' is already running", name)
            });
        }
        procs.insert(server_id.to_string(), Arc::clone(&entry));

        // Drop the lock before initialize to avoid holding it
        drop(procs);

        Ok(McpPendingStart {
            server_id: server_id.to_string(),
            entry,
        })
    }

    /// Wait for initialize without the lifecycle gate: Stop, config edits, and
    /// imports can revoke this exact entry while the server is silent.
    pub(crate) fn finish_start(&self, pending: McpPendingStart) -> Result<(), String> {
        let McpPendingStart { server_id, entry } = pending;
        // Perform MCP initialization handshake
        match Self::initialize(&entry) {
            Ok(_) => {
                let procs = self.processes.lock().map_err(|error| error.to_string())?;
                let current = procs
                    .get(&server_id)
                    .is_some_and(|current| Arc::ptr_eq(current, &entry));
                if !current || self.shutting_down.load(Ordering::Acquire) {
                    drop(procs);
                    let _ = Self::terminate_entry(&entry);
                    return Err("MCP server was stopped during initialization".to_string());
                }
                entry.ready.store(true, Ordering::Release);
                eprintln!(
                    "[MCP] Server '{}' initialized successfully",
                    entry.server_name
                );
            }
            Err(e) => {
                eprintln!(
                    "[MCP] Server '{}' initialization failed: {}",
                    entry.server_name, e
                );
                // A half-initialized child must not be presented as a running
                // service. It could otherwise block a visible retry and be
                // accidentally registered on a later foreground turn.
                let _ = self.remove_if_current(&server_id, &entry);
                let _ = Self::terminate_entry(&entry);
                return Err(format!("MCP initialize failed: {e}"));
            }
        }

        Ok(())
    }

    /// Stop a running MCP server by its config ID.
    pub fn stop(&self, server_id: &str) -> Result<(), String> {
        let mut procs = self.processes.lock().map_err(|e| e.to_string())?;
        let entry = procs
            .get(server_id)
            .cloned()
            .ok_or_else(|| format!("Server '{}' is not running", server_id))?;
        Self::close_entry(&entry);
        procs.remove(server_id);
        if let Ok(mut tools) = self.tool_lists.lock() {
            tools.remove(server_id);
        }
        drop(procs);

        // A server that stopped reading stdin must not be able to block Stop.
        // Killing it also releases the dedicated stdin/stdout workers.
        match Self::terminate_entry(&entry) {
            Ok(()) => {
                eprintln!("[MCP] Server '{}' stopped", entry.server_name);
                Ok(())
            }
            Err(error) => {
                // Re-insert the entry so it can be retried
                let mut procs = self.processes.lock().map_err(|e| e.to_string())?;
                if !self.shutting_down.load(Ordering::Acquire) {
                    procs.entry(server_id.to_string()).or_insert(entry);
                }
                Err(format!("Failed to stop MCP server: {error}"))
            }
        }
    }

    fn terminate_entry(entry: &McpProcessEntry) -> Result<(), String> {
        Self::close_entry(entry);
        let mut child = entry.child.lock().map_err(|error| error.to_string())?;
        child
            .terminate_and_wait()
            .map_err(|error| error.to_string())
    }

    fn close_entry(entry: &McpProcessEntry) {
        entry.ready.store(false, Ordering::Release);
        let mut open = entry
            .send_open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *open = false;
    }

    /// Get the status of an MCP server.
    pub fn get_status(&self, server_id: &str) -> Result<McpServerStatus, String> {
        let entry = self
            .processes
            .lock()
            .map_err(|error| error.to_string())?
            .get(server_id)
            .cloned();
        if let Some(entry) = entry {
            let exited = entry
                .child
                .lock()
                .map_err(|error| error.to_string())?
                .try_wait()
                .map_err(|error| format!("Failed to inspect MCP server: {error}"))?
                .is_some();
            if exited {
                Self::terminate_entry(&entry)?;
                self.remove_if_current(server_id, &entry)?;
            }
        }
        let procs = self.processes.lock().map_err(|error| error.to_string())?;
        match procs.get(server_id) {
            Some(entry) => {
                let tools_count = self
                    .tool_lists
                    .lock()
                    .ok()
                    .and_then(|t| t.get(server_id).map(|tools| tools.len()));

                Ok(McpServerStatus {
                    server_id: server_id.to_string(),
                    status: if entry.ready.load(Ordering::Acquire) {
                        "running"
                    } else {
                        "starting"
                    }
                    .to_string(),
                    error: None,
                    tools_count,
                })
            }
            None => Ok(McpServerStatus {
                server_id: server_id.to_string(),
                status: "stopped".to_string(),
                error: None,
                tools_count: None,
            }),
        }
    }

    /// List tools from a running MCP server, using the last discovery when it
    /// is available. Execution must use [`Self::call_tool_if_current_snapshot`]
    /// instead, which bypasses this cache before a side effect.
    pub fn list_tools(&self, server_id: &str) -> Result<Vec<McpTool>, String> {
        if self.get_status(server_id)?.status != "running" {
            return Err(format!("Server '{}' is not running", server_id));
        }
        if let Ok(tools) = self.tool_lists.lock() {
            if let Some(cached) = tools.get(server_id) {
                if !cached.is_empty() {
                    return Ok(cached.clone());
                }
            }
        }

        self.refresh_tools(server_id)
    }

    /// Force a fresh `tools/list` round trip and replace the discovery cache.
    /// This is intentionally separate from `list_tools`: a cached schema is
    /// suitable for presenting a tool but never for authorizing a call.
    pub(crate) fn refresh_tools(&self, server_id: &str) -> Result<Vec<McpTool>, String> {
        self.with_server_policy_read(server_id, || {
            let entry = self.process_entry(server_id)?;
            if !entry.ready.load(Ordering::Acquire) {
                return Err(format!("Server '{}' is still starting", server_id));
            }
            let mut io = entry.io.lock().map_err(|error| error.to_string())?;
            let tools = match Self::list_tools_from_entry(&entry, &mut io) {
                Ok(tools) => tools,
                Err(error) => {
                    self.clear_cached_tools_if_current(server_id, &entry);
                    return Err(error);
                }
            };
            self.replace_cached_tools_if_current(server_id, &entry, &tools);
            Ok(tools)
        })
    }

    /// Call a tool only if the same server still advertises the exact contract
    /// the Main Agent registered. The fresh discovery and call share one
    /// per-server I/O lock, so another local request cannot slip a response between
    /// the version check and `tools/call`.
    pub(crate) fn call_tool_if_current_snapshot<F>(
        &self,
        server_id: &str,
        tool_name: &str,
        expected: &McpToolSnapshot,
        arguments: &serde_json::Value,
        before_call: F,
    ) -> Result<String, McpToolCallError>
    where
        F: FnOnce() -> Result<(), String>,
    {
        self.call_tool_output_if_current_snapshot(
            server_id,
            tool_name,
            expected,
            arguments,
            before_call,
        )
        .map(|output| output.text)
    }

    pub(crate) fn call_tool_output_if_current_snapshot<F>(
        &self,
        server_id: &str,
        tool_name: &str,
        expected: &McpToolSnapshot,
        arguments: &serde_json::Value,
        before_call: F,
    ) -> Result<McpToolOutput, McpToolCallError>
    where
        F: FnOnce() -> Result<(), String>,
    {
        self.with_server_policy_read(server_id, || {
            let entry = self.process_entry(server_id)?;
            if !entry.ready.load(Ordering::Acquire) {
                return Err(format!("Server '{}' is still starting", server_id));
            }
            let mut io = entry.io.lock().map_err(|error| error.to_string())?;
            let tools = match Self::list_tools_from_entry(&entry, &mut io) {
                Ok(tools) => tools,
                Err(error) => {
                    self.clear_cached_tools_if_current(server_id, &entry);
                    return Err(error);
                }
            };
            let result = match tools.iter().find(|tool| tool.name == tool_name) {
                None => Err(McpToolCallError::Known(format!(
                    "MCP 工具 '{}' 已不再由服务 '{}' 提供，未执行。请重新发起该操作。",
                    tool_name, server_id
                ))),
                Some(tool) if tool.snapshot_for(server_id) != *expected => {
                    Err(McpToolCallError::Known(format!(
                        "MCP 工具 '{}' 的定义已在确认后发生变化，未执行。请重新发起并确认该操作。",
                        tool_name
                    )))
                }
                Some(_) => before_call()
                    .map_err(McpToolCallError::Known)
                    .and_then(|()| {
                        Self::call_tool_from_entry(&entry, &mut io, tool_name, arguments)
                    }),
            };
            self.replace_cached_tools_if_current(server_id, &entry, &tools);
            Ok(result)
        })
        .map_err(McpToolCallError::Known)?
    }

    fn list_tools_from_entry(
        entry: &McpProcessEntry,
        io: &mut McpProcessIo,
    ) -> Result<Vec<McpTool>, String> {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "tools/list",
            "id": 2
        });
        let response =
            Self::send_request_to_entry(entry, io, &request).map_err(|error| error.to_string())?;
        if let Some(error) = response.error {
            // The remote message is untrusted and may echo a credential that
            // was explicitly provided to this service. Keep only the code.
            return Err(format!("MCP tools/list error {}", error.code));
        }
        let result = response
            .result
            .ok_or_else(|| "No result in tools/list response".to_string())?;
        let tools = result
            .get("tools")
            .ok_or_else(|| "MCP tools/list response is missing tools".to_string())?;
        serde_json::from_value(tools.clone())
            .map_err(|error| format!("Invalid MCP tool list: {error}"))
    }

    fn replace_cached_tools_if_current(
        &self,
        server_id: &str,
        entry: &Arc<McpProcessEntry>,
        tools: &[McpTool],
    ) {
        if let Ok(processes) = self.processes.lock() {
            if processes
                .get(server_id)
                .is_some_and(|current| Arc::ptr_eq(current, entry))
                && entry.ready.load(Ordering::Acquire)
            {
                if let Ok(mut cache) = self.tool_lists.lock() {
                    cache.insert(server_id.to_string(), tools.to_vec());
                }
            }
        }
    }

    fn clear_cached_tools_if_current(&self, server_id: &str, entry: &Arc<McpProcessEntry>) {
        if let Ok(processes) = self.processes.lock() {
            if processes
                .get(server_id)
                .is_some_and(|current| Arc::ptr_eq(current, entry))
            {
                if let Ok(mut cache) = self.tool_lists.lock() {
                    cache.remove(server_id);
                }
            }
        }
    }

    fn call_tool_from_entry(
        entry: &McpProcessEntry,
        io: &mut McpProcessIo,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<McpToolOutput, McpToolCallError> {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": {
                "name": tool_name,
                "arguments": arguments
            },
            "id": 100
        });
        let response = Self::send_request_to_entry(entry, io, &request)?;
        if let Some(err) = response.error {
            return Err(McpToolCallError::Known(format!(
                "MCP tool error {}: {}",
                err.code, err.message
            )));
        }
        let result = response.result.ok_or(McpToolCallError::ResultUnknown)?;
        Self::parse_tool_output(result)
    }

    fn parse_tool_output(result: serde_json::Value) -> Result<McpToolOutput, McpToolCallError> {
        let tool_failed = result
            .get("isError")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if result
            .get("isError")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(McpToolCallError::ResultUnknown);
        }
        let content = result
            .get("content")
            .and_then(serde_json::Value::as_array)
            .ok_or(McpToolCallError::ResultUnknown)?;
        let mut texts = Vec::new();
        let mut images = Vec::new();
        let mut image_bytes = 0usize;
        let mut omitted_content = 0usize;
        for item in content {
            match item.get("type").and_then(serde_json::Value::as_str) {
                Some("text") => {
                    if let Some(text) = item.get("text").and_then(serde_json::Value::as_str) {
                        texts.push(text.to_string());
                    } else {
                        return Err(McpToolCallError::ResultUnknown);
                    }
                }
                Some("image") if !tool_failed && images.len() < MAX_MCP_TOOL_IMAGES => {
                    let image = item
                        .get("mimeType")
                        .and_then(serde_json::Value::as_str)
                        .zip(item.get("data").and_then(serde_json::Value::as_str))
                        .and_then(|(mime, data)| {
                            crate::llm::ToolImage::from_base64(mime, data).ok()
                        });
                    if let Some(image) = image {
                        if let Some(total) = image_bytes.checked_add(image.byte_len()) {
                            if total <= MAX_MCP_TOOL_IMAGE_BYTES {
                                image_bytes = total;
                                images.push(image);
                                continue;
                            }
                        }
                    }
                    // A trustworthy success receipt with unusable images must
                    // not turn into a retryable failure of the external action.
                    omitted_content += 1;
                }
                None => return Err(McpToolCallError::ResultUnknown),
                _ => omitted_content += 1,
            }
        }
        if let Some(structured) = result.get("structuredContent") {
            let mut remaining_nodes = MAX_MCP_STRUCTURED_NODES;
            texts.push(
                sanitize_mcp_structured_content(structured, 0, &mut remaining_nodes, false)
                    .to_string(),
            );
        }
        if !images.is_empty() {
            texts.push(format!(
                "[MCP 返回了 {} 张经验证的 PNG 图像；文本桥接不展示原始数据。]",
                images.len()
            ));
        }
        if omitted_content > 0 {
            texts.push(format!(
                "[MCP 返回了 {omitted_content} 项非文本内容；因不支持、未通过图像验证或超过图像预算，已安全省略原始数据。]"
            ));
            if !tool_failed {
                texts.push("[工具成功回执已收到，不要为获取已省略的内容而重做已完成操作。]".into());
            }
        }
        let text = texts.join("\n");
        if tool_failed {
            Err(McpToolCallError::Known(format!(
                "MCP tool reported an error: {text}"
            )))
        } else {
            Ok(McpToolOutput { text, images })
        }
    }

    /// Return all server IDs that are currently running.
    pub fn running_servers(&self) -> Vec<String> {
        let entries = match self.processes.lock() {
            Ok(processes) => processes
                .iter()
                .map(|(id, entry)| (id.clone(), Arc::clone(entry)))
                .collect::<Vec<_>>(),
            Err(_) => return Vec::new(),
        };
        let mut running = Vec::new();
        for (server_id, entry) in entries {
            let status = entry
                .child
                .lock()
                .map_err(|error| error.to_string())
                .and_then(|mut child| child.try_wait().map_err(|error| error.to_string()));
            match status {
                Ok(None) if entry.ready.load(Ordering::Acquire) => {
                    if self
                        .processes
                        .lock()
                        .ok()
                        .and_then(|processes| {
                            processes
                                .get(&server_id)
                                .map(|current| Arc::ptr_eq(current, &entry))
                        })
                        .unwrap_or(false)
                    {
                        running.push(server_id);
                    }
                }
                Ok(None) => {}
                Ok(Some(_)) => match Self::terminate_entry(&entry) {
                    Ok(()) => {
                        let _ = self.remove_if_current(&server_id, &entry);
                    }
                    Err(error) => eprintln!(
                        "[MCP] Could not clean up exited server '{}': {error}",
                        entry.server_name
                    ),
                },
                Err(error) => {
                    entry.ready.store(false, Ordering::Release);
                    eprintln!(
                        "[MCP] Could not inspect server '{}': {error}",
                        entry.server_name
                    );
                }
            }
        }
        running
    }

    /// Process IDs for lifecycle cleanup, including a child still handshaking.
    pub fn managed_server_ids(&self) -> Result<Vec<String>, String> {
        self.processes
            .lock()
            .map(|processes| processes.keys().cloned().collect())
            .map_err(|error| error.to_string())
    }

    /// Get cached tool list for a server.
    pub fn get_cached_tools(&self, server_id: &str) -> Vec<McpTool> {
        self.tool_lists
            .lock()
            .map(|t| t.get(server_id).cloned().unwrap_or_default())
            .unwrap_or_default()
    }

    /// Stop all running MCP servers (called on app shutdown).
    pub fn stop_all(&self) {
        self.shutting_down.store(true, Ordering::Release);
        let mut procs = match self.processes.lock() {
            Ok(p) => p,
            Err(_) => return,
        };
        for entry in procs.values() {
            Self::close_entry(entry);
        }
        let entries = procs.drain().map(|(_, entry)| entry).collect::<Vec<_>>();
        if let Ok(mut tools) = self.tool_lists.lock() {
            tools.clear();
        }
        drop(procs);
        for entry in entries {
            eprintln!("[MCP] Shutting down server '{}'...", entry.server_name);
            let _ = Self::terminate_entry(&entry);
        }
    }

    // ─── Private helpers ──────────────────────────────────────────────────────

    fn process_entry(&self, server_id: &str) -> Result<Arc<McpProcessEntry>, String> {
        self.processes
            .lock()
            .map_err(|error| error.to_string())?
            .get(server_id)
            .cloned()
            .ok_or_else(|| format!("Server '{}' not found", server_id))
    }

    fn remove_if_current(
        &self,
        server_id: &str,
        entry: &Arc<McpProcessEntry>,
    ) -> Result<bool, String> {
        let mut processes = self.processes.lock().map_err(|error| error.to_string())?;
        if !processes
            .get(server_id)
            .is_some_and(|current| Arc::ptr_eq(current, entry))
        {
            return Ok(false);
        }
        Self::close_entry(entry);
        processes.remove(server_id);
        self.tool_lists
            .lock()
            .map_err(|error| error.to_string())?
            .remove(server_id);
        Ok(true)
    }

    /// Perform MCP initialization handshake.
    fn initialize(entry: &McpProcessEntry) -> Result<serde_json::Value, String> {
        let init_request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "clientInfo": {
                    "name": "AngelBot",
                    "version": "0.1.0"
                }
            },
            "id": 1
        });

        let mut io = entry.io.lock().map_err(|error| error.to_string())?;
        let response = Self::send_request_to_entry(entry, &mut io, &init_request)
            .map_err(|error| error.to_string())?;

        // Check for JSON-RPC error
        if let Some(err) = response.error {
            return Err(format!("MCP init error {}", err.code));
        }

        let result = response
            .result
            .ok_or_else(|| "No result in initialize response".to_string())?;

        // Send initialized notification (no response expected)
        let initialized = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        });
        Self::send_message_to_entry(entry, &mut io, &initialized)?;

        Ok(result)
    }

    fn send_request_to_entry(
        entry: &McpProcessEntry,
        io: &mut McpProcessIo,
        request: &serde_json::Value,
    ) -> Result<JsonRpcResponse, McpToolCallError> {
        let id = io.next_request_id;
        io.next_request_id = io
            .next_request_id
            .checked_add(1)
            .ok_or_else(|| "MCP request ID exhausted".to_string())?;
        let mut request = request.clone();
        request
            .as_object_mut()
            .ok_or_else(|| "Invalid MCP request".to_string())?
            .insert("id".to_string(), serde_json::json!(id));
        let method = request.get("method").and_then(|value| value.as_str());
        let timeout = match method {
            Some("initialize") => entry.handshake_timeout,
            Some("tools/call") => TOOL_CALL_TIMEOUT,
            _ => DISCOVERY_TIMEOUT,
        };
        let response = Self::send_message_to_entry(entry, io, &request)
            .map_err(McpToolCallError::Known)
            .and_then(|()| {
                Self::read_response_from_io(io, id, timeout).map_err(|error| {
                    if method == Some("tools/call") {
                        McpToolCallError::ResultUnknown
                    } else {
                        McpToolCallError::Known(error)
                    }
                })
            });
        match response {
            Ok(result) => Ok(result),
            Err(error) => {
                // Capture the exit status before our own cleanup kills a
                // still-running server. Otherwise a timeout can be reported
                // misleadingly as a server crash with our termination code.
                let exited =
                    if method == Some("initialize") && !error.to_string().contains("timed out") {
                        let deadline = Instant::now() + Duration::from_millis(150);
                        loop {
                            let status = entry
                                .child
                                .lock()
                                .ok()
                                .and_then(|mut child| child.try_wait().ok().flatten());
                            if status.is_some() || Instant::now() >= deadline {
                                break status;
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    } else {
                        None
                    };
                let stopped_during_initialize = method == Some("initialize")
                    && !*entry
                        .send_open
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                let _ = Self::terminate_entry(entry);
                if stopped_during_initialize {
                    return Err(McpToolCallError::Known(
                        "MCP server was stopped during initialization".to_string(),
                    ));
                }
                match exited {
                    Some(status) => Err(McpToolCallError::Known(format!(
                        "Server exited ({status}); {error}"
                    ))),
                    None => Err(error),
                }
            }
        }
    }

    fn send_message_to_entry(
        entry: &McpProcessEntry,
        io: &mut McpProcessIo,
        message: &serde_json::Value,
    ) -> Result<(), String> {
        let msg_str = serde_json::to_string(message).map_err(|e| e.to_string())?;
        let open = entry.send_open.lock().map_err(|error| error.to_string())?;
        if !*open {
            return Err("MCP server was stopped before the request was sent".to_string());
        }
        io.writes
            .try_send(msg_str)
            .map_err(|error| format!("Failed to queue MCP request: {error}"))
    }

    /// A persistent reader preserves buffered lines. Notifications and late
    /// replies may appear before the response to this request; neither is
    /// authority to approve a tool call.
    fn read_response_from_io(
        io: &mut McpProcessIo,
        expected_id: u64,
        timeout: Duration,
    ) -> Result<JsonRpcResponse, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("MCP server response timed out".to_string());
            }
            let line = io
                .responses
                .recv_timeout(remaining)
                .map_err(|error| match error {
                    mpsc::RecvTimeoutError::Timeout => "MCP server response timed out".to_string(),
                    mpsc::RecvTimeoutError::Disconnected => {
                        "MCP server response stream closed".to_string()
                    }
                })??;
            let response: JsonRpcResponse = serde_json::from_str(line.trim())
                .map_err(|error| format!("Invalid MCP response: {error}"))?;
            if response.jsonrpc.as_deref() != Some("2.0") {
                return Err("MCP server returned invalid JSON-RPC version".to_string());
            }
            match response.id {
                None => continue,                         // Server notification.
                Some(id) if id < expected_id => continue, // Late response to an older request.
                Some(id) if id == expected_id => return Ok(response),
                Some(id) => {
                    return Err(format!(
                        "MCP server returned unexpected response ID {id} (expected {expected_id})"
                    ));
                }
            }
        }
    }
}

/// Preserve business fields without making structuredContent a second image
/// transport. Only known image/binary-resource envelopes lose their payloads.
fn sanitize_mcp_structured_content(
    value: &serde_json::Value,
    depth: usize,
    remaining_nodes: &mut usize,
    resource_context: bool,
) -> serde_json::Value {
    if *remaining_nodes == 0 {
        return serde_json::Value::String("[MCP 结构化内容超过安全处理上限，已省略。]".into());
    }
    *remaining_nodes -= 1;
    if depth > MAX_MCP_STRUCTURED_DEPTH {
        return serde_json::Value::String("[MCP 结构化内容超过安全处理上限，已省略。]".into());
    }
    match value {
        serde_json::Value::Object(fields) => {
            let kind = fields.get("type").and_then(serde_json::Value::as_str);
            let image_payload = kind == Some("image")
                || fields
                    .get("mimeType")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|mime| mime.starts_with("image/"));
            let resource_context = resource_context || kind == Some("resource");
            let binary_resource = resource_context && fields.contains_key("blob");
            let mut sanitized = serde_json::Map::new();
            for (key, child) in fields {
                if (image_payload && matches!(key.as_str(), "data" | "blob"))
                    || (binary_resource && key == "blob")
                    || ((image_payload || resource_context)
                        && key == "uri"
                        && child.as_str().is_some_and(|uri| uri.starts_with("data:")))
                {
                    *remaining_nodes = remaining_nodes.saturating_sub(1);
                    sanitized.insert(
                        key.clone(),
                        serde_json::Value::String("[MCP 原始非文本数据已省略。]".into()),
                    );
                } else {
                    sanitized.insert(
                        key.clone(),
                        sanitize_mcp_structured_content(
                            child,
                            depth + 1,
                            remaining_nodes,
                            resource_context,
                        ),
                    );
                }
                if *remaining_nodes == 0 {
                    sanitized.insert(
                        "_angelbot_omitted".into(),
                        serde_json::Value::String(
                            "MCP 结构化内容超过安全处理上限，已省略。".into(),
                        ),
                    );
                    break;
                }
            }
            serde_json::Value::Object(sanitized)
        }
        serde_json::Value::Array(items) => {
            let mut sanitized = Vec::new();
            for child in items {
                sanitized.push(sanitize_mcp_structured_content(
                    child,
                    depth + 1,
                    remaining_nodes,
                    resource_context,
                ));
                if *remaining_nodes == 0 {
                    sanitized.push(serde_json::Value::String(
                        "[MCP 结构化内容超过安全处理上限，已省略。]".into(),
                    ));
                    break;
                }
            }
            serde_json::Value::Array(sanitized)
        }
        _ => value.clone(),
    }
}

fn resolve_mcp_executable(command: &str) -> &str {
    let configured = command.trim().trim_matches('"');
    // Rust only appends .exe during Windows PATH resolution. Keep older
    // `npx` configurations usable without routing arbitrary commands through
    // a shell or changing explicitly configured paths.
    #[cfg(windows)]
    if configured.eq_ignore_ascii_case("npx") {
        return "npx.cmd";
    }
    configured
}

/// A cold npx install can take longer than a local server start. Only the
/// bare Windows launcher receives this allowance; explicit paths and other
/// commands retain the short handshake deadline.
fn handshake_timeout_for_command(command: &str) -> Duration {
    #[cfg(windows)]
    if resolve_mcp_executable(command).eq_ignore_ascii_case("npx.cmd") {
        return NPX_HANDSHAKE_TIMEOUT;
    }
    #[cfg(not(windows))]
    let _ = command;
    HANDSHAKE_TIMEOUT
}

fn read_bounded_line(reader: &mut impl BufRead) -> Result<Option<String>, String> {
    let mut line = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .map_err(|error| format!("Failed to read MCP stdout: {error}"))?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err("MCP server closed stdout mid-response".to_string())
            };
        }
        let length = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|position| position + 1)
            .unwrap_or(available.len());
        if line.len() + length > MAX_MCP_LINE_BYTES {
            return Err("MCP response exceeds size limit".to_string());
        }
        let complete = available[length - 1] == b'\n';
        line.extend_from_slice(&available[..length]);
        reader.consume(length);
        if complete {
            return String::from_utf8(line)
                .map(Some)
                .map_err(|_| "MCP server sent non-UTF-8 response".to_string());
        }
    }
}

/// Parse the settings field without invoking a shell. Quoted Windows paths
/// stay one argument; JSON arrays handle arguments that themselves need quote
/// characters or other unusual whitespace.
fn parse_command_args(input: &str) -> Result<Vec<String>, String> {
    let trimmed = input.trim();
    if trimmed.starts_with('[') {
        return serde_json::from_str(trimmed).map_err(|error| format!("MCP 参数数组无效: {error}"));
    }
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut started = false;
    for character in input.chars() {
        match character {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            character if character.is_whitespace() && !quoted => {
                if started {
                    arguments.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            character => {
                current.push(character);
                started = true;
            }
        }
    }
    if quoted {
        return Err("MCP 参数中的双引号未闭合".to_string());
    }
    if started {
        arguments.push(current);
    }
    Ok(arguments)
}

#[cfg(test)]
mod concurrency_tests;

#[cfg(all(test, windows))]
mod windows_launcher_tests;

#[cfg(test)]
mod tests {
    use super::{
        handshake_timeout_for_command, parse_command_args, McpProcessManager, McpTool,
        McpToolCallError, HANDSHAKE_TIMEOUT, MAX_MCP_TOOL_IMAGE_BYTES,
    };
    use base64::Engine;
    use std::fs;
    use std::time::Duration;

    fn png_image_content() -> serde_json::Value {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[32, 64, 128, 255]).unwrap();
            writer.finish().unwrap();
        }
        serde_json::json!({
            "type": "image",
            "mimeType": "image/png",
            "data": base64::engine::general_purpose::STANDARD.encode(bytes),
        })
    }

    #[test]
    fn typed_mcp_output_keeps_text_and_bounds_validated_images() {
        let image = png_image_content();
        let output = McpProcessManager::parse_tool_output(serde_json::json!({
            "content": [
                {"type": "text", "text": "captured"},
                image.clone(), image.clone(), image.clone(),
                {"type": "audio", "data": "RAW_AUDIO_CANARY"},
            ],
            "structuredContent": {"data": {"businessAmount": 42}},
        }))
        .unwrap();
        assert_eq!(output.images.len(), 2);
        assert!(
            output
                .images
                .iter()
                .map(|image| image.byte_len())
                .sum::<usize>()
                <= MAX_MCP_TOOL_IMAGE_BYTES
        );
        assert!(output.text.starts_with("captured\n"));
        assert!(output.text.contains("businessAmount"));
        assert!(output.text.contains("2 项非文本内容"));
        assert!(!output.text.contains(image["data"].as_str().unwrap()));
        assert!(!output.text.contains("RAW_AUDIO_CANARY"));
    }

    #[test]
    fn unusable_mcp_images_are_omitted_without_replaying_a_successful_action() {
        let mut wrong_mime = png_image_content();
        wrong_mime["mimeType"] = "image/jpeg".into();
        let output = McpProcessManager::parse_tool_output(serde_json::json!({
            "isError": false,
            "content": [
                {"type": "text", "text": "action completed"},
                {"type": "image", "mimeType": "image/png", "data": "RAW_INVALID_IMAGE_CANARY"},
                {"type": "image", "mimeType": "image/png"},
                wrong_mime,
            ],
        }))
        .unwrap();
        assert!(output.images.is_empty());
        assert!(output.text.contains("action completed"));
        assert!(output.text.contains("3 项非文本内容"));
        assert!(!output.text.contains("RAW_INVALID_IMAGE_CANARY"));
    }

    #[test]
    fn mcp_error_receipts_never_attach_images_or_echo_raw_non_text_payloads() {
        let image = png_image_content();
        let error = McpProcessManager::parse_tool_output(serde_json::json!({
            "isError": true,
            "content": [{"type": "text", "text": "explicit rejection"}, image.clone()],
        }))
        .unwrap_err();
        assert!(matches!(error, McpToolCallError::Known(_)));
        assert!(error.to_string().contains("explicit rejection"));
        assert!(!error.to_string().contains(image["data"].as_str().unwrap()));
    }

    #[test]
    fn malformed_mcp_receipts_are_unknown_without_echoing_payloads() {
        for value in [
            serde_json::Value::Null,
            serde_json::json!({"data": "RAW_RECEIPT_CANARY"}),
            serde_json::json!({"content": "RAW_RECEIPT_CANARY"}),
            serde_json::json!({"content": [], "isError": "RAW_RECEIPT_CANARY"}),
            serde_json::json!({"content": [{"type": "text", "text": 42}]}),
        ] {
            let error = McpProcessManager::parse_tool_output(value).unwrap_err();
            assert!(matches!(error, McpToolCallError::ResultUnknown));
            assert!(!error.to_string().contains("RAW_RECEIPT_CANARY"));
        }
    }

    #[test]
    fn mcp_structured_content_keeps_business_data_but_strips_known_image_envelopes() {
        let output = McpProcessManager::parse_tool_output(serde_json::json!({
            "content": [],
            "structuredContent": {
                "data": {"businessAmount": 42},
                "image": {"type": "image", "mimeType": "image/png", "data": "RAW_IMAGE_CANARY"},
                "resource": {"type": "resource", "resource": {
                    "uri": "data:image/png;base64,RAW_URI_CANARY",
                    "mimeType": "image/png", "blob": "RAW_BLOB_CANARY",
                }},
            },
        }))
        .unwrap();
        let structured: serde_json::Value = serde_json::from_str(&output.text).unwrap();
        assert_eq!(structured["data"]["businessAmount"], 42);
        assert_eq!(structured["image"]["mimeType"], "image/png");
        for canary in ["RAW_IMAGE_CANARY", "RAW_BLOB_CANARY", "RAW_URI_CANARY"] {
            assert!(!output.text.contains(canary));
        }
    }

    #[test]
    fn mcp_structured_content_sanitization_has_depth_and_node_budgets() {
        let mut nested = serde_json::json!({"secret": "RAW_TOO_DEEP_CANARY"});
        for _ in 0..32 {
            nested = serde_json::json!({"nested": nested});
        }
        let output = McpProcessManager::parse_tool_output(serde_json::json!({
            "content": [], "structuredContent": nested,
        }))
        .unwrap();
        assert!(output.text.contains("超过安全处理上限"));
        assert!(!output.text.contains("RAW_TOO_DEEP_CANARY"));
        let output = McpProcessManager::parse_tool_output(serde_json::json!({
            "content": [], "structuredContent": vec![0; 5000],
        }))
        .unwrap();
        let values: Vec<serde_json::Value> = serde_json::from_str(&output.text).unwrap();
        assert!(values.len() <= super::MAX_MCP_STRUCTURED_NODES);
        assert!(output.text.contains("超过安全处理上限"));
    }

    #[test]
    fn legacy_mcp_string_call_discards_images_from_the_shared_typed_call_path() {
        let image = png_image_content();
        let payload = serde_json::json!({
            "content": [{"type": "text", "text": "capture complete"}, image.clone()],
        });
        let script = format!("const output = {payload};\n")
            + r#"
const readline = require('readline');
readline.createInterface({ input: process.stdin }).on('line', line => {
  const request = JSON.parse(line);
  let result;
  if (request.method === 'initialize') {
    result = { protocolVersion: '2024-11-05', capabilities: { tools: {} },
      serverInfo: { name: 'image-fixture', version: '1.0.0' } };
  } else if (request.method === 'tools/list') {
    result = { tools: [{ name: 'capture', inputSchema: { type: 'object' } }] };
  } else if (request.method === 'tools/call') {
    result = output;
  } else { return; }
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result }) + '\n');
});
"#;
        let manager = McpProcessManager::new();
        let arguments = serde_json::to_string(&["-e", &script]).unwrap();
        manager
            .start("image-fixture", "Image fixture", "node", &arguments, "")
            .unwrap();
        let snapshot =
            manager.refresh_tools("image-fixture").unwrap()[0].snapshot_for("image-fixture");
        let typed = manager
            .call_tool_output_if_current_snapshot(
                "image-fixture",
                "capture",
                &snapshot,
                &serde_json::json!({}),
                || Ok(()),
            )
            .unwrap();
        assert_eq!(typed.images.len(), 1);
        let text = manager
            .call_tool_if_current_snapshot(
                "image-fixture",
                "capture",
                &snapshot,
                &serde_json::json!({}),
                || Ok(()),
            )
            .unwrap();
        assert_eq!(text, typed.text);
        assert!(text.contains("capture complete"));
        assert!(!text.contains(image["data"].as_str().unwrap()));
        manager.stop("image-fixture").unwrap();
    }

    #[test]
    fn only_bare_windows_npx_gets_a_longer_initialization_deadline() {
        assert_eq!(HANDSHAKE_TIMEOUT, Duration::from_secs(15));
        for command in [
            "node",
            "npm.cmd",
            r"C:\Program Files\nodejs\npx.cmd",
            r#""C:\Program Files\nodejs\npx.cmd""#,
        ] {
            assert_eq!(handshake_timeout_for_command(command), HANDSHAKE_TIMEOUT);
        }
        #[cfg(windows)]
        for command in ["npx", "npx.cmd", "  NpX  ", r#""npx.cmd""#] {
            assert_eq!(
                handshake_timeout_for_command(command),
                Duration::from_secs(90)
            );
        }
        #[cfg(not(windows))]
        for command in ["npx", "npx.cmd"] {
            assert_eq!(handshake_timeout_for_command(command), HANDSHAKE_TIMEOUT);
        }
    }

    #[test]
    fn tool_snapshot_tracks_contract_not_description_or_object_key_order() {
        let original = McpTool {
            name: "echo".to_string(),
            description: "Echo a message".to_string(),
            input_schema: serde_json::from_str(
                r#"{"type":"object","properties":{"message":{"type":"string"}},"required":["message"]}"#,
            )
            .unwrap(),
        };
        let equivalent = McpTool {
            name: "echo".to_string(),
            description: "Updated explanatory copy".to_string(),
            input_schema: serde_json::from_str(
                r#"{"required":["message"],"properties":{"message":{"type":"string"}},"type":"object"}"#,
            )
            .unwrap(),
        };
        let changed = McpTool {
            name: "echo".to_string(),
            description: "Echo a message".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "message": { "type": "number" } },
                "required": ["message"]
            }),
        };

        assert_eq!(
            original.snapshot_for("mcp-test"),
            equivalent.snapshot_for("mcp-test")
        );
        assert_ne!(
            original.snapshot_for("mcp-test"),
            changed.snapshot_for("mcp-test")
        );
        assert_ne!(
            original.snapshot_for("mcp-test"),
            original.snapshot_for("another-server")
        );
    }

    #[test]
    fn parses_quoted_windows_paths_without_a_shell() {
        assert_eq!(
            parse_command_args(r#"-y "C:\Users\Test User\server.cjs" --mode safe"#).unwrap(),
            vec!["-y", r"C:\Users\Test User\server.cjs", "--mode", "safe"]
        );
        assert_eq!(
            parse_command_args(r#"["--config", "{\"name\":\"A B\"}"]"#).unwrap(),
            vec!["--config", r#"{"name":"A B"}"#]
        );
        assert!(parse_command_args(r#""unclosed"#).is_err());
    }

    #[test]
    fn early_mcp_exit_reports_code_without_waiting_for_handshake_timeout() {
        let manager = McpProcessManager::new();
        let args = serde_json::to_string(&["-e", "process.exit(42)"]).unwrap();
        let error = manager
            .start("early-exit", "Early exit", "node", &args, "")
            .unwrap_err();

        assert!(error.contains("Server exited"), "{error}");
        assert!(error.contains("42"), "{error}");
        assert_eq!(manager.get_status("early-exit").unwrap().status, "stopped");
    }

    #[test]
    fn closing_stdout_while_alive_does_not_report_our_cleanup_as_a_crash() {
        let manager = McpProcessManager::new();
        let args =
            serde_json::to_string(&["-c", "import os, time; os.close(1); time.sleep(30)"]).unwrap();
        let error = manager
            .start("closed-stdout", "Closed stdout", "python", &args, "")
            .unwrap_err();

        assert!(error.contains("closed stdout"), "{error}");
        assert!(!error.contains("Server exited"), "{error}");
        assert_eq!(
            manager.get_status("closed-stdout").unwrap().status,
            "stopped"
        );
    }

    #[test]
    fn mcp_child_receives_only_launch_and_explicit_environment() {
        let script = r#"
const readline = require('readline');
readline.createInterface({ input: process.stdin }).on('line', line => {
  const request = JSON.parse(line);
  let result;
  if (request.method === 'initialize') {
    result = { protocolVersion: '2024-11-05', capabilities: { tools: {} },
      serverInfo: { name: 'env-fixture', version: '1.0.0' } };
  } else if (request.method === 'tools/list') {
    result = { tools: [{ name: 'environment', inputSchema: { type: 'object' } }] };
  } else if (request.method === 'tools/call') {
    result = { content: [{ type: 'text', text: JSON.stringify({
      keys: Object.keys(process.env), explicit: process.env.ANGELBOT_MCP_TEST_EXPLICIT
    }) }] };
  } else { return; }
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result }) + '\n');
});
"#;
        let manager = McpProcessManager::new();
        let args = serde_json::to_string(&["-e", script]).unwrap();
        manager
            .start(
                "env-fixture",
                "Environment fixture",
                "node",
                &args,
                "ANGELBOT_MCP_TEST_EXPLICIT=granted",
            )
            .unwrap();
        let tools = manager.refresh_tools("env-fixture").unwrap();
        let output = manager
            .call_tool_if_current_snapshot(
                "env-fixture",
                "environment",
                &tools[0].snapshot_for("env-fixture"),
                &serde_json::json!({}),
                || Ok(()),
            )
            .unwrap();
        let environment: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(environment["explicit"], "granted");
        #[cfg(windows)]
        let allowed = [
            "PATH",
            "PATHEXT",
            "SYSTEMROOT",
            "WINDIR",
            "COMSPEC",
            "TEMP",
            "TMP",
            "APPDATA",
            "LOCALAPPDATA",
            "USERPROFILE",
            "ANGELBOT_MCP_TEST_EXPLICIT",
        ];
        #[cfg(not(windows))]
        let allowed = [
            "PATH",
            "HOME",
            "TMPDIR",
            "LANG",
            "LC_ALL",
            "ANGELBOT_MCP_TEST_EXPLICIT",
        ];
        for key in environment["keys"].as_array().unwrap() {
            let key = key.as_str().unwrap();
            assert!(
                allowed
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(key)),
                "MCP child inherited unexpected environment key {key}"
            );
        }
        manager.stop("env-fixture").unwrap();
    }

    #[test]
    fn failed_handshake_can_retry_and_shutdown_reaps_discovered_server() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mcp-lifecycle.cjs");
        let mode = dir.path().join("mode.txt");
        fs::write(&mode, "bad").unwrap();
        fs::write(
            &script,
            r#"
const fs = require('fs');
const readline = require('readline');
const modePath = process.argv[2];
const lines = readline.createInterface({ input: process.stdin });
lines.on('line', line => {
  const request = JSON.parse(line);
  if (request.method === 'initialize') {
    if (fs.readFileSync(modePath, 'utf8').trim() === 'bad') {
      process.stdout.write('not-json\n');
    } else {
      process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result: {
        protocolVersion: '2024-11-05', capabilities: { tools: {} },
        serverInfo: { name: 'fixture', version: '1.0.0' }
      } }) + '\n');
    }
  } else if (request.method === 'tools/list') {
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result: {
      tools: [{ name: 'inspect', description: 'Fixture', inputSchema: { type: 'object' } }]
    } }) + '\n');
  } else if (request.method === 'tools/call') {
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result: {
      isError: true, content: [{ type: 'text', text: 'fixture failure' }]
    } }) + '\n');
  }
});
"#,
        )
        .unwrap();
        let args = format!("{} {}", script.display(), mode.display());
        let manager = McpProcessManager::new();

        assert!(manager
            .start("fixture", "Fixture", "node", &args, "")
            .is_err());
        assert!(manager.running_servers().is_empty());
        assert_eq!(manager.get_status("fixture").unwrap().status, "stopped");

        fs::write(&mode, "good").unwrap();
        manager
            .start("fixture", "Fixture", "node", &args, "")
            .unwrap();
        let tools = manager.refresh_tools("fixture").unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(manager.get_status("fixture").unwrap().tools_count, Some(1));
        let error = manager
            .call_tool_if_current_snapshot(
                "fixture",
                "inspect",
                &tools[0].snapshot_for("fixture"),
                &serde_json::json!({}),
                || Ok(()),
            )
            .unwrap_err();
        assert!(error.to_string().contains("fixture failure"));

        manager.stop_all();
        assert!(manager.running_servers().is_empty());
        assert!(manager.get_cached_tools("fixture").is_empty());
        assert_eq!(manager.get_status("fixture").unwrap().status, "stopped");
    }

    #[test]
    fn stdio_reader_preserves_buffered_lines_and_matches_current_request() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mcp-notifications.cjs");
        fs::write(
            &script,
            r#"
const readline = require('readline');
let lists = 0;
readline.createInterface({ input: process.stdin }).on('line', line => {
  const request = JSON.parse(line);
  if (request.method === 'initialize') {
    process.stderr.write('diagnostic'.repeat(100000));
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result: {
      protocolVersion: '2024-11-05', capabilities: { tools: {} },
      serverInfo: { name: 'fixture', version: '1.0.0' }
    } }) + '\n');
  } else if (request.method === 'tools/list') {
    lists++;
    const notice = JSON.stringify({ jsonrpc: '2.0', method: 'notifications/tools/list_changed' });
    const response = JSON.stringify({ jsonrpc: '2.0', id: request.id, result: {
      tools: lists === 3 ? {} : [{ name: 'tool-' + lists, description: 'Fixture', inputSchema: { type: 'object' } }]
    } });
    process.stdout.write(notice + '\n' + response + '\n');
  }
});
"#,
        )
        .unwrap();
        let manager = McpProcessManager::new();
        manager
            .start(
                "fixture",
                "Fixture",
                "node",
                &script.display().to_string(),
                "",
            )
            .unwrap();
        assert_eq!(manager.refresh_tools("fixture").unwrap()[0].name, "tool-1");
        assert_eq!(manager.refresh_tools("fixture").unwrap()[0].name, "tool-2");
        assert!(manager.refresh_tools("fixture").is_err());
        assert!(manager.get_cached_tools("fixture").is_empty());
        manager.stop("fixture").unwrap();
    }

    #[test]
    fn silent_server_response_has_a_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("mcp-silent.cjs");
        fs::write(
            &script,
            r#"
const readline = require('readline');
readline.createInterface({ input: process.stdin }).on('line', line => {
  const request = JSON.parse(line);
  if (request.method === 'initialize') {
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result: {
      protocolVersion: '2024-11-05', capabilities: { tools: {} },
      serverInfo: { name: 'fixture', version: '1.0.0' }
    } }) + '\n');
  }
});
"#,
        )
        .unwrap();
        let manager = McpProcessManager::new();
        manager
            .start(
                "fixture",
                "Fixture",
                "node",
                &script.display().to_string(),
                "",
            )
            .unwrap();
        {
            let entry = manager.process_entry("fixture").unwrap();
            let mut io = entry.io.lock().unwrap();
            let request = serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
            McpProcessManager::send_message_to_entry(&entry, &mut io, &request).unwrap();
            let error =
                McpProcessManager::read_response_from_io(&mut io, 2, Duration::from_millis(100))
                    .unwrap_err();
            assert!(error.contains("timed out"));
        }
        manager.stop("fixture").unwrap();
    }
}

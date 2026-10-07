//! Tool definition and schema types
//!
//! Issue #42: Tool execution modes (sequential, parallel, conditional)
//! Issue #52: Enhanced tool descriptions with parameter details

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// Tool execution modes
///
/// `Sequential` is the default; `Parallel` is set explicitly on a tool via
/// `Tool::with_execution_mode` to opt into concurrent execution within a
/// single tool batch. We deliberately do **not** model `Conditional`
/// (depends_on + predicate) here: that kind of plan-level dependency lives
/// in `PlanStep.depends_on` (the planner module), and mixing the two
/// would put the same idea in two places. Keeping the runner's parallel/
/// sequential split as a flat bool matches §2.1 "no extra graph".
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ToolExecutionMode {
    /// Sequential execution (default, for MCP tools with state dependencies)
    Sequential,
    /// Parallel execution (for independent tools)
    Parallel,
}

impl Default for ToolExecutionMode {
    fn default() -> Self {
        ToolExecutionMode::Sequential
    }
}

/// Tool category for classification and loading strategy.
/// Issue #095: Core tools are always loaded; extension/platform tools are loaded on demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCategory {
    /// Core tools: read, write, edit, execute - always loaded
    Core,
    /// Extension tools: loaded on demand (memory, goal, knowledge, constraint, proactive, skill)
    Extension,
    /// Platform tools: Windows-specific capabilities
    Platform,
}

impl Default for ToolCategory {
    fn default() -> Self {
        ToolCategory::Core
    }
}

/// Represents a callable tool with metadata and handler
#[derive(Clone)]
pub struct Tool {
    /// Unique identifier for the tool (e.g., "read_file")
    pub name: String,
    /// Human-readable display name for UI (e.g., "读取文件"). Falls back to name if None.
    pub label: Option<String>,
    /// Human-readable description of what the tool does
    pub description: String,
    /// JSON Schema for the tool's parameters
    pub parameters: Value,
    /// Whether this tool requires user confirmation before execution
    pub requires_confirmation: bool,
    /// Execution mode for this tool
    pub execution_mode: ToolExecutionMode,
    /// Category for loading strategy (Issue #095)
    pub category: ToolCategory,
    /// Issue #102: Optional post-call verifier. Runs after the handler
    /// returns and before the runner appends the audit step. The verifier
    /// is read-only — it cannot modify the `ToolResult` — but it can
    /// attach `VerificationEvidence` to the trace so the completion gate
    /// can flag claims the disk disagrees with.
    pub post_call_verifier: Option<Arc<dyn super::verifier::PostCallVerifier>>,
    /// Issue #104: Snippet injected into the system prompt when this tool is available.
    /// Examples: "Use read to examine files instead of cat." / "Execute bash commands (ls, grep)."
    /// Unlike `description` (which goes into the function-calling schema), `prompt_snippet`
    /// targets the conversation-level system prompt so the LLM knows *when* to reach for
    /// a tool — not just *what parameters* to pass.
    pub prompt_snippet: Option<String>,
}

impl std::fmt::Debug for Tool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tool")
            .field("name", &self.name)
            .field("label", &self.label)
            .field("description", &self.description)
            .field("parameters", &self.parameters)
            .field("requires_confirmation", &self.requires_confirmation)
            .field("execution_mode", &self.execution_mode)
            .field("category", &self.category)
            .field("post_call_verifier.set", &self.post_call_verifier.is_some())
            .field("prompt_snippet", &self.prompt_snippet)
            .finish()
    }
}

impl Tool {
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: Value) -> Self {
        Self {
            name: name.into(),
            label: None,
            description: description.into(),
            parameters,
            requires_confirmation: false,
            execution_mode: ToolExecutionMode::default(),
            category: ToolCategory::default(),
            post_call_verifier: None,
            prompt_snippet: None,
        }
    }

    /// Get the display name (label if set, otherwise name)
    pub fn display_name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }

    pub fn with_confirmation(mut self, requires: bool) -> Self {
        self.requires_confirmation = requires;
        self
    }

    pub fn with_execution_mode(mut self, mode: ToolExecutionMode) -> Self {
        self.execution_mode = mode;
        self
    }

    /// Set a human-friendly label for UI display. Issue #093
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set tool category for loading strategy. Issue #095
    pub fn with_category(mut self, category: ToolCategory) -> Self {
        self.category = category;
        self
    }

    /// Issue #102: attach a post-call verifier. The verifier inspects the
    /// tool's arguments and result and emits `VerificationEvidence` that
    /// the runner records into the trace. Common implementations are
    /// `FileExistenceVerifier` (write/edit/create) and `FileAbsenceVerifier`
    /// (delete).
    pub fn with_post_call_verifier(
        mut self,
        verifier: Arc<dyn super::verifier::PostCallVerifier>,
    ) -> Self {
        self.post_call_verifier = Some(verifier);
        self
    }

    /// Issue #104: attach a prompt snippet — a short sentence injected into
    /// the system prompt to guide the LLM on *when* to use this tool.
    pub fn with_prompt_snippet(mut self, snippet: impl Into<String>) -> Self {
        self.prompt_snippet = Some(snippet.into());
        self
    }

    /// Convert to OpenAI-compatible function calling format
    pub fn to_function_schema(&self) -> Value {
        build_function_schema(&self.name, &self.description, &self.parameters)
    }

    /// Convert to schema with UI-friendly metadata including label. Issue #093.
    /// The label is embedded in the description for LLM awareness.
    pub fn to_schema_with_label(&self) -> Value {
        let display_name = self.display_name();
        let desc_with_label = if self.label.is_some() {
            format!("[{}] {}", display_name, self.description)
        } else {
            self.description.clone()
        };
        build_function_schema(&self.name, &desc_with_label, &self.parameters)
    }

    /// Convert to enhanced function schema with parameter details (Issue #52).
    /// When `include_param_details` is true, the description appends each
    /// parameter's name and description so the LLM knows exactly what to pass.
    pub fn to_function_schema_enhanced(&self) -> Value {
        let param_details = build_param_details(&self.parameters);
        let enhanced_desc = if param_details.is_empty() {
            self.description.clone()
        } else {
            format!("{}\n\n参数说明:\n{}", self.description, param_details)
        };
        build_function_schema(&self.name, &enhanced_desc, &self.parameters)
    }
}

/// Internal helper for the three public `to_function_schema*` methods.
/// Description is pre-computed by the caller (it differs between the
/// variants); everything else is identical, so this keeps the shape
/// definition in one place.
fn build_function_schema(name: &str, description: &str, parameters: &Value) -> Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters,
        }
    })
}

/// Build a human-readable parameter details string from a JSON Schema.
/// Issue #52: Makes tool descriptions self-documenting for the LLM.
pub fn build_param_details(schema: &Value) -> String {
    let mut lines = Vec::new();
    if let Some(obj) = schema.get("properties").and_then(|p| p.as_object()) {
        for (name, prop) in obj {
            let param_type = prop.get("type").and_then(|t| t.as_str()).unwrap_or("any");
            let description = prop
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("");
            let required = schema
                .get("required")
                .and_then(|r| r.as_array())
                .map(|arr| arr.iter().any(|v| v.as_str() == Some(name)))
                .unwrap_or(false);
            let req_marker = if required { "(必填)" } else { "(可选)" };
            let default = prop
                .get("default")
                .and_then(|d| serde_json::to_string(d).ok())
                .map(|s| format!(" [默认值: {}]", s))
                .unwrap_or_default();
            let enum_vals = prop
                .get("enum")
                .and_then(|e| e.as_array())
                .filter(|arr| !arr.is_empty())
                .map(|arr| {
                    let vals: Vec<_> = arr
                        .iter()
                        .filter_map(|v| serde_json::to_string(v).ok())
                        .collect();
                    format!(" [可选值: {}]", vals.join(", "))
                })
                .unwrap_or_default();
            lines.push(format!(
                "- {name}: {description}（类型: {param_type}）{req_marker}{default}{enum_vals}"
            ));
        }
    }
    lines.join("\n")
}

/// Parse a parameters schema and return a map of param name -> (required, description).
/// Used by the prompt layer to build a tool-use-guidance table (Issue #52).
pub fn parse_param_schema(schema: &Value) -> HashMap<String, (bool, String)> {
    let mut params = HashMap::new();
    if let Some(obj) = schema.get("properties").and_then(|p| p.as_object()) {
        let required = schema
            .get("required")
            .and_then(|r| r.as_array())
            .map(|arr| {
                let mut s = std::collections::HashSet::new();
                for v in arr {
                    if let Some(sv) = v.as_str() {
                        s.insert(sv.to_string());
                    }
                }
                s
            })
            .unwrap_or_default();
        for (name, prop) in obj {
            let desc = prop
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            params.insert(name.clone(), (required.contains(name), desc));
        }
    }
    params
}

/// A tool call request from the LLM with execution metadata
#[derive(Debug, Clone, Deserialize)]
pub struct ToolCall {
    /// The name of the tool to call
    pub name: String,
    /// The arguments to pass to the tool (parsed JSON)
    pub arguments: Value,
    /// The call ID for tracking
    pub id: Option<String>,
    /// Execution mode for this specific call
    #[serde(default)]
    pub execution_mode: ToolExecutionMode,
}

impl ToolCall {
    pub fn new(name: String, arguments: Value) -> Self {
        Self {
            name,
            arguments,
            id: None,
            execution_mode: ToolExecutionMode::default(),
        }
    }

    pub fn with_id(mut self, id: String) -> Self {
        self.id = Some(id);
        self
    }

    pub fn with_execution_mode(mut self, mode: ToolExecutionMode) -> Self {
        self.execution_mode = mode;
        self
    }
}

/// Trait for executable tool handlers
/// Allows dynamic dispatch so tools can be registered at runtime.
/// Maps to Hermes Agent's "ToolExecutor" pattern.
pub trait ToolHandler: Send + Sync {
    /// Execute the tool with the given JSON arguments value.
    /// `work_dir` is the sandboxed directory this tool is allowed to operate in.
    fn execute(&self, arguments: &Value, work_dir: &std::path::Path) -> ToolResult;

    /// Execute with the runtime context for this individual call.
    ///
    /// The legacy `execute` method remains the compatibility boundary for
    /// existing handlers. Handlers that launch cancellable work should override
    /// this method and cooperate with `context.cancellation` and `deadline`.
    fn execute_with_context(
        &self,
        arguments: &Value,
        work_dir: &std::path::Path,
        _context: &ToolExecutionContext,
    ) -> ToolResult {
        self.execute(arguments, work_dir)
    }
    /// Return the tool name this handler is for
    fn name(&self) -> &str;

    /// Issue #093: Preprocess arguments before execution.
    /// Return `Ok(processed_args)` to use the processed arguments,
    /// or `Err(message)` to abort with an error.
    /// Default implementation returns the original arguments unchanged.
    fn prepare_arguments(
        &self,
        arguments: &Value,
        _work_dir: &std::path::Path,
    ) -> Result<Value, String> {
        Ok(arguments.clone())
    }
}

/// Shared, cooperative cancellation signal for one agent run.
///
/// A signal is intentionally one-way: a new run receives a new signal so a
/// cancellation from an earlier run can never be cleared underneath a tool
/// that is still unwinding.
#[derive(Clone, Debug)]
pub struct ToolCancellation {
    cancelled: Arc<AtomicBool>,
    /// Retains the terminal cancelled state for late subscribers and wakes
    /// async operations that are already waiting on the signal.
    state: watch::Sender<bool>,
}

impl Default for ToolCancellation {
    fn default() -> Self {
        let (state, _initial_receiver) = watch::channel(false);
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            state,
        }
    }
}

impl ToolCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        // `send_replace` deliberately retains the value even when no task is
        // currently subscribed. That makes cancellation observed before a
        // future begins waiting just as durable as cancellation observed while
        // it is already in flight.
        if !self.cancelled.swap(true, Ordering::AcqRel) {
            self.state.send_replace(true);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Wait for this one-way signal to be cancelled.
    ///
    /// The retained watch value closes the check/subscribe race: cancellation
    /// can happen before subscription, while subscription is being created, or
    /// after the waiter begins waiting without leaving the waiter stranded.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }

        let mut state = self.state.subscribe();
        if *state.borrow() || self.is_cancelled() {
            return;
        }

        // The sender is held by every clone of ToolCancellation for the run,
        // so a closed channel is not an alternate terminal state. If it ever
        // occurs, the synchronous flag remains the authoritative signal.
        let _ = state.changed().await;
    }
}

/// Progress callback passed to long-running tools so they can emit streaming updates
/// without coupling to the runner's event system. Signature:
/// `(tool_name: &str, progress: &str, percent: Option<f32>)`.
pub type ProgressCallback = Box<dyn Fn(&str, &str, Option<f32>) + Send + Sync>;

// ─── Operations traits (Pi §V pattern) ─────────────────────────────────────────

/// File-system operations abstracted so handlers stay testable and environments
/// can inject remote/mock implementations without changing tool code.
///
/// Each trait has a default implementation wrapping the standard library.
/// Pi calls this "Operations abstraction" — see pi §5 "Operations抽象".
///
/// Default implementations delegate to `std::fs`:
/// - `FileOps::default()` → real filesystem
/// - `CommandOps::default()` → real subprocess via `std::process::Command`
///
/// Override specific methods to inject SSH, Docker, or test doubles.
mod operations {
    use std::path::Path;

    /// Read operations needed by ReadFileHandler.
    ///
    /// Default: real filesystem via `std::fs`.
    /// Override `read` for SSH / Docker / test doubles.
    pub trait FileReadOps: Send + Sync {
        /// Check if `path` exists and is readable. Return Ok(()) or Err(reason).
        fn access(&self, path: &Path) -> std::io::Result<()>;
        /// Read `path` as raw bytes.
        fn read(&self, path: &Path) -> std::io::Result<Vec<u8>>;
    }

    /// Write operations needed by WriteFileHandler and EditFileHandler.
    ///
    /// Default: real filesystem via `std::fs`.
    pub trait FileWriteOps: Send + Sync {
        /// Write `content` (UTF-8 bytes) to `path`. Truncate if exists.
        fn write(&self, path: &Path, content: &[u8]) -> std::io::Result<()>;
        /// Create directory `path` (parent must exist).
        fn mkdir(&self, path: &Path) -> std::io::Result<()>;
    }

    /// Directory listing operations.
    ///
    /// Default: real filesystem via `std::fs`.
    pub trait DirOps: Send + Sync {
        /// List entries in `path`. Return (name, is_dir, size) tuples.
        fn list(&self, path: &Path) -> std::io::Result<Vec<(String, bool, Option<u64>)>>;
    }

    /// Command execution operations needed by RunProjectCommandHandler.
    ///
    /// Default: real subprocess via `std::process::Command`.
    /// Override `exec` for SSH / Docker / test doubles.
    ///
    /// Return semantics mirror `std::process::Command::output()`:
    /// - `Ok((stdout, Some(exit_code)))` — command finished (zero or non-zero)
    /// - `Err` — command could not be started or was killed
    /// Cancellation and timeout are handled by the caller; `exec` blocks until
    /// the process terminates.
    pub trait CommandOps: Send + Sync {
        fn exec(
            &self,
            command: &str,
            args: &[String],
            cwd: &Path,
        ) -> std::io::Result<(String, Option<i32>)>;
    }

    /// Default implementation using `std::process::Command`.
    /// This is what the real handler uses; swap it for a mock in tests.
    #[derive(Default)]
    pub struct DefaultCommandOps;
    impl CommandOps for DefaultCommandOps {
        fn exec(
            &self,
            command: &str,
            args: &[String],
            cwd: &Path,
        ) -> std::io::Result<(String, Option<i32>)> {
            let output = std::process::Command::new(command)
                .args(args)
                .current_dir(cwd)
                .stdin(std::process::Stdio::null())
                .output()?;
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            Ok((stdout, output.status.code()))
        }
    }

    /// Default filesystem operations using `std::fs`.
    #[derive(Default)]
    pub struct DefaultFileReadOps;
    impl FileReadOps for DefaultFileReadOps {
        fn access(&self, path: &Path) -> std::io::Result<()> {
            std::fs::metadata(path).map(|_| ())
        }
        fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
            std::fs::read(path)
        }
    }

    /// Default filesystem write operations using `std::fs`.
    #[derive(Default)]
    pub struct DefaultFileWriteOps;
    impl FileWriteOps for DefaultFileWriteOps {
        fn write(&self, path: &Path, content: &[u8]) -> std::io::Result<()> {
            std::fs::write(path, content)
        }
        fn mkdir(&self, path: &Path) -> std::io::Result<()> {
            std::fs::create_dir(path)
        }
    }

    /// Default directory listing using `std::fs`.
    #[derive(Default)]
    pub struct DefaultDirOps;
    impl DirOps for DefaultDirOps {
        fn list(&self, path: &Path) -> std::io::Result<Vec<(String, bool, Option<u64>)>> {
            let mut entries = std::fs::read_dir(path)?
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let m = e.metadata().ok()?;
                    Some((
                        e.file_name().to_string_lossy().into_owned(),
                        m.is_dir(),
                        if m.is_dir() { None } else { Some(m.len()) },
                    ))
                })
                .collect::<Vec<_>>();
            entries.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
            Ok(entries)
        }
    }
}

pub use operations::{
    CommandOps, DefaultCommandOps, DefaultDirOps, DefaultFileReadOps, DefaultFileWriteOps, DirOps,
    FileReadOps, FileWriteOps,
};

/// Runtime-owned metadata passed to a single tool invocation.
///
/// It is deliberately separate from model-provided arguments. This prevents a
/// tool's cancellation and time budget from being inferred from untrusted LLM
/// input and establishes the contract used by future capability tools.
#[derive(Clone)]
pub struct ToolExecutionContext {
    pub run_id: Option<String>,
    pub call_id: String,
    pub cancellation: ToolCancellation,
    /// Durable, runtime-owned binding for a user-confirmed capability. It is
    /// never derived from model arguments and is only populated by the
    /// confirmation-resume path.
    confirmation_approval: bool,
    expected_definition_revision: Option<String>,
    /// Backend-attested UIA target for a reviewed desktop draft. Never model input.
    desktop_expected_target: Option<crate::desktop_control::DesktopDraftTargetIdentity>,
    deadline: Option<Instant>,
    /// Optional callback for emitting ToolExecutionProgress events from
    /// long-running tools (e.g. run_project_command, search_files).
    /// Arc<Option<...>> avoids generic type complexity — caller wraps in Arc::new(Some(callback)).
    progress_callback: std::sync::Arc<Option<ProgressCallback>>,
    /// Guards against emitting progress after the handler has settled.
    /// Pi calls this `acceptingUpdates`. Once false, emit_progress is a no-op.
    accepting_updates: std::sync::Arc<AtomicBool>,
}

impl ToolExecutionContext {
    pub fn new(call_id: impl Into<String>, cancellation: ToolCancellation) -> Self {
        Self {
            run_id: None,
            call_id: call_id.into(),
            cancellation,
            confirmation_approval: false,
            expected_definition_revision: None,
            desktop_expected_target: None,
            deadline: None,
            progress_callback: std::sync::Arc::new(None),
            accepting_updates: std::sync::Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn with_run_id(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }

    /// Mark this invocation as the execution of a stored user confirmation.
    /// `expected_definition_revision` is opaque to generic tools; capability
    /// handlers decide whether and how it binds their live definition.
    pub fn for_confirmation(mut self, expected_definition_revision: Option<String>) -> Self {
        self.confirmation_approval = true;
        self.expected_definition_revision = expected_definition_revision;
        self
    }

    pub fn is_confirmation_approval(&self) -> bool {
        self.confirmation_approval
    }

    pub fn expected_definition_revision(&self) -> Option<&str> {
        self.expected_definition_revision.as_deref()
    }

    pub fn with_desktop_expected_target(
        mut self,
        target: crate::desktop_control::DesktopDraftTargetIdentity,
    ) -> Self {
        self.desktop_expected_target = Some(target);
        self
    }

    pub fn desktop_expected_target(
        &self,
    ) -> Option<&crate::desktop_control::DesktopDraftTargetIdentity> {
        self.desktop_expected_target.as_ref()
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.deadline = Some(Instant::now() + timeout);
        self
    }

    pub fn with_progress_callback(
        mut self,
        callback: std::sync::Arc<Option<ProgressCallback>>,
    ) -> Self {
        self.progress_callback = callback;
        self
    }

    /// Emit a ToolExecutionProgress event through the callback if one is registered.
    /// Safe to call even when no callback is set.
    /// Ignored after `settle()` has been called (Pi's `acceptingUpdates` guard).
    pub fn emit_progress(&self, tool_name: &str, progress: &str, percent: Option<f32>) {
        if self.accepting_updates.load(Ordering::Acquire) {
            if let Some(cb) = self.progress_callback.as_ref().as_ref() {
                cb(tool_name, progress, percent);
            }
        }
    }

    /// Signal that this tool's execution has settled (returned or panicked).
    /// All subsequent `emit_progress` calls become no-ops.
    pub fn settle(&self) {
        self.accepting_updates.store(false, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    pub fn remaining(&self) -> Option<Duration> {
        self.deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }

    pub fn is_expired(&self) -> bool {
        self.remaining()
            .is_some_and(|remaining| remaining.is_zero())
    }
}

/// Result of executing a tool
#[derive(Debug, Clone, Serialize)]
pub struct ToolResult {
    /// The name of the tool that was executed
    pub name: String,
    /// Whether execution was successful
    pub success: bool,
    /// The result content or error message
    pub content: String,
    /// Issue #082: signals that the runner should stop executing this batch.
    /// Defaults to false for backward compatibility.
    #[serde(default)]
    pub terminate_batch: bool,
    /// Current tool round only; excluded from safe UI/log serialization.
    #[serde(skip)]
    pub images: Vec<crate::llm::ToolImage>,
}

impl ToolResult {
    pub fn success_with_images(
        name: impl Into<String>,
        content: impl Into<String>,
        images: Vec<crate::llm::ToolImage>,
    ) -> Self {
        Self {
            name: name.into(),
            success: true,
            content: content.into(),
            terminate_batch: false,
            images,
        }
    }
    pub fn success(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            success: true,
            content: content.into(),
            terminate_batch: false,
            images: Vec::new(),
        }
    }

    pub fn error(name: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            success: false,
            content: format!("Error: {}", error.into()),
            terminate_batch: false,
            images: Vec::new(),
        }
    }

    /// Issue #082: create a result that signals "stop executing this batch".
    pub fn terminate_batch(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            success: false,
            content: format!("Error: {}", reason.into()),
            terminate_batch: true,
            images: Vec::new(),
        }
    }

    /// A completed side-effect call can have an uncertain outcome when its
    /// acknowledgement is lost. Pause the agent instead of letting another
    /// model turn automatically repeat the action.
    pub fn requires_user_review(&self) -> bool {
        if self.success || !self.terminate_batch {
            return false;
        }
        let content = self
            .content
            .strip_prefix("Error: ")
            .unwrap_or(&self.content);
        let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
            return false;
        };
        value.get("code").and_then(|code| code.as_str()) == Some("RESULT_UNKNOWN")
    }

    /// Format result for LLM consumption — returns a structured string
    /// that makes it easy for the LLM to decide the next step.
    /// Issue #48: Enables LLM self-correction on failure.
    pub fn format_for_llm(&self, include_hint: bool) -> String {
        if self.success {
            format!("Result: {}", self.content)
        } else {
            if include_hint && !self.terminate_batch {
                format!(
                    "Error: {name}: {content}. \
                     The tool call failed. Please analyze the error and decide how to proceed: \
                     (1) Fix the arguments and retry the same tool, \
                     (2) Use a different tool to achieve the same goal, \
                     (3) Explain the problem to the user and ask for clarification.",
                    name = self.name,
                    content = self
                        .content
                        .strip_prefix("Error: ")
                        .unwrap_or(&self.content)
                )
            } else {
                format!("Error: {}: {}", self.name, self.content)
            }
        }
    }
}

/// Built-in tool schemas for file operations
pub mod schemas {
    use serde_json::json;

    pub fn read_file() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "The path to the file to read, relative to the work directory"
                }
            },
            "additionalProperties": false,
            "required": ["path"]
        })
    }

    pub fn write_file() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "The path where the file will be created or overwritten"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "additionalProperties": false,
            "required": ["path", "content"]
        })
    }

    pub fn create_directory() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Directory path relative to the work directory. Its parent must already exist."
                }
            },
            "additionalProperties": false,
            "required": ["path"]
        })
    }

    pub fn list_workspace_items() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "default": "",
                    "description": "Directory path relative to the workspace root. Use an empty string for the root."
                }
            },
            "additionalProperties": false
        })
    }

    pub fn organize_workspace_item() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["copy", "move"],
                    "description": "copy duplicates one file; move relocates or renames a file or directory"
                },
                "source": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Existing item relative to the workspace root"
                },
                "destination": {
                    "type": "string",
                    "minLength": 1,
                    "description": "New path relative to the workspace root. It must not already exist and its parent must exist."
                }
            },
            "additionalProperties": false,
            "required": ["operation", "source", "destination"]
        })
    }

    pub fn run_project_command() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "enum": ["npm", "pnpm", "yarn", "bun", "cargo", "go", "dotnet", "python", "python3", "node", "deno", "uv", "git"],
                    "description": "A supported project tool. It is executed without a shell in the current work directory."
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string", "maxLength": 512 },
                    "maxItems": 32,
                    "default": [],
                    "description": "Arguments for the project tool. Shell syntax is not supported."
                },
                "timeout_seconds": {
                    "type": "integer",
                    "minimum": 5,
                    "maximum": 180,
                    "default": 90,
                    "description": "Maximum time to wait for the command."
                }
            },
            "additionalProperties": false,
            "required": ["command"]
        })
    }

    pub fn edit_file() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "The path to the file to edit, relative to the work directory"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact text to replace. Must match a contiguous section in the file."
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement text. Can be empty to delete old_string."
                }
            },
            "additionalProperties": false,
            "required": ["path", "old_string", "new_string"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tokio::time::timeout;

    #[test]
    fn tool_cancellation_retains_cancel_when_nobody_is_subscribed() {
        let cancellation = ToolCancellation::new();

        cancellation.cancel();

        assert!(cancellation.is_cancelled());
    }

    #[tokio::test]
    async fn tool_cancellation_wait_after_cancel_returns_immediately() {
        let cancellation = ToolCancellation::new();
        cancellation.cancel();

        timeout(Duration::from_secs(1), cancellation.cancelled())
            .await
            .expect("a late waiter must observe retained cancellation");
    }

    #[tokio::test]
    async fn tool_cancellation_wakes_a_waiter_when_cancelled() {
        let cancellation = ToolCancellation::new();
        let waiter_cancellation = cancellation.clone();
        let waiter = tokio::spawn(async move {
            waiter_cancellation.cancelled().await;
        });

        tokio::task::yield_now().await;
        assert!(
            !waiter.is_finished(),
            "uncancelled wait must remain pending"
        );

        cancellation.cancel();

        timeout(Duration::from_secs(1), waiter)
            .await
            .expect("an in-flight waiter must be woken")
            .expect("waiter task must not panic");
    }

    #[tokio::test]
    async fn tool_cancellation_is_idempotent_for_waiters() {
        let cancellation = ToolCancellation::new();
        let waiter_cancellation = cancellation.clone();
        let waiter = tokio::spawn(async move {
            waiter_cancellation.cancelled().await;
        });

        tokio::task::yield_now().await;
        cancellation.cancel();
        cancellation.cancel();

        timeout(Duration::from_secs(1), waiter)
            .await
            .expect("repeated cancellation must not strand a waiter")
            .expect("waiter task must not panic");
        assert!(cancellation.is_cancelled());
    }

    #[test]
    fn test_format_for_llm_success() {
        let result = ToolResult::success("read_file", "file contents here");
        assert_eq!(result.format_for_llm(true), "Result: file contents here");
    }

    #[test]
    fn test_format_for_llm_error_with_hint() {
        let result = ToolResult::error("read_file", "文件不存在");
        let msg = result.format_for_llm(true);
        assert!(msg.starts_with("Error:"));
        assert!(msg.contains("read_file"));
        assert!(msg.contains("文件不存在"));
        assert!(msg.contains("retry") || msg.contains("Fix"));
    }

    #[test]
    fn test_format_for_llm_error_no_hint() {
        let result = ToolResult::error("read_file", "参数解析失败: missing field");
        let msg = result.format_for_llm(false);
        assert!(msg.contains("Error:"));
        assert!(msg.contains("read_file"));
        assert!(!msg.contains("retry"));
    }

    #[test]
    fn terminal_tool_error_does_not_suggest_an_automatic_retry() {
        let result = ToolResult::terminate_batch(
            "prepare_message_draft",
            "RESULT_UNKNOWN: inspect the target field before retrying",
        );
        let message = result.format_for_llm(true);
        assert!(message.contains("RESULT_UNKNOWN"));
        assert!(!message.contains("Fix the arguments and retry"));
    }

    #[test]
    fn only_terminal_structured_unknown_result_requires_user_review() {
        let uncertain = ToolResult::terminate_batch(
            "draft",
            r#"{"code":"RESULT_UNKNOWN","message":"inspect first"}"#,
        );
        assert!(uncertain.requires_user_review());
        assert!(!ToolResult::error("draft", r#"{"code":"RESULT_UNKNOWN"}"#).requires_user_review());
        assert!(!ToolResult::terminate_batch("draft", "plain failure").requires_user_review());
    }

    #[test]
    fn test_build_param_details_with_required_and_optional() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "要读取的文件路径"
                },
                "encoding": {
                    "type": "string",
                    "description": "文件编码",
                    "default": "utf-8"
                }
            },
            "required": ["path"]
        });
        let details = build_param_details(&schema);
        assert!(details.contains("path"));
        assert!(details.contains("必填"));
        assert!(details.contains("encoding"));
        assert!(details.contains("可选"));
        assert!(details.contains("utf-8"));
    }

    #[test]
    fn test_build_param_details_with_enum() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "format": {
                    "type": "string",
                    "description": "输出格式",
                    "enum": ["json", "yaml", "xml"]
                }
            }
        });
        let details = build_param_details(&schema);
        assert!(details.contains("json"));
        assert!(details.contains("yaml"));
        assert!(details.contains("可选值"));
    }

    #[test]
    fn test_to_function_schema_enhanced_includes_params() {
        let tool = Tool::new(
            "test_tool",
            "测试工具",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "arg1": {
                        "type": "string",
                        "description": "第一个参数"
                    }
                },
                "required": ["arg1"]
            }),
        );
        let schema = tool.to_function_schema_enhanced();
        let func = schema.get("function").unwrap();
        assert!(func["description"].as_str().unwrap().contains("第一个参数"));
        assert!(func["description"].as_str().unwrap().contains("参数说明"));
    }

    #[test]
    fn test_parse_param_schema() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "用户名"
                },
                "age": {
                    "type": "integer",
                    "description": "年龄"
                }
            },
            "required": ["name"]
        });
        let params = parse_param_schema(&schema);
        assert_eq!(params.len(), 2);
        let (is_req, desc) = params.get("name").unwrap();
        assert!(*is_req);
        assert_eq!(desc, "用户名");
        let (is_req, _) = params.get("age").unwrap();
        assert!(!*is_req);
    }

    // Issue #093: Tool label and prepareArguments tests
    #[test]
    fn test_tool_label_falls_back_to_name() {
        let tool = Tool::new("read_file", "读取文件", serde_json::json!({}));
        assert_eq!(tool.display_name(), "read_file");
    }

    #[test]
    fn test_tool_with_label_overrides_name() {
        let tool = Tool::new("read_file", "读取文件", serde_json::json!({})).with_label("读取文件");
        assert_eq!(tool.display_name(), "读取文件");
    }

    #[test]
    fn test_to_schema_with_label_includes_label_in_description() {
        let tool = Tool::new("read_file", "读取文件", serde_json::json!({})).with_label("读取文件");
        let schema = tool.to_schema_with_label();
        let desc = schema["function"]["description"].as_str().unwrap();
        assert!(desc.contains("[读取文件]"));
        assert!(desc.contains("读取文件")); // label
        assert!(desc.contains("读取文件")); // description
    }

    #[test]
    fn test_prepare_arguments_default_passes_through() {
        struct TestHandler;
        impl ToolHandler for TestHandler {
            fn execute(&self, _args: &Value, _work_dir: &Path) -> ToolResult {
                ToolResult::success("test", "done")
            }
            fn name(&self) -> &str {
                "test"
            }
        }
        let handler = TestHandler;
        let args = serde_json::json!({"key": "value"});
        let result = handler.prepare_arguments(&args, Path::new("."));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), args);
    }

    #[test]
    fn test_prepare_arguments_can_modify_args() {
        struct ModifyingHandler;
        impl ToolHandler for ModifyingHandler {
            fn execute(&self, args: &Value, _work_dir: &Path) -> ToolResult {
                // Check that args were modified
                if args.get("processed").and_then(|v| v.as_bool()) == Some(true) {
                    ToolResult::success("test", "processed correctly")
                } else {
                    ToolResult::error("test", "args not processed")
                }
            }
            fn name(&self) -> &str {
                "test"
            }
            fn prepare_arguments(&self, args: &Value, _work_dir: &Path) -> Result<Value, String> {
                let mut modified = args.clone();
                modified["processed"] = serde_json::json!(true);
                Ok(modified)
            }
        }
        let handler = ModifyingHandler;
        let args = serde_json::json!({"original": "value"});
        let result = handler.prepare_arguments(&args, Path::new("."));
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["processed"].as_bool(), Some(true));
    }

    #[test]
    fn test_prepare_arguments_can_abort() {
        struct AbortingHandler;
        impl ToolHandler for AbortingHandler {
            fn execute(&self, _args: &Value, _work_dir: &Path) -> ToolResult {
                ToolResult::success("test", "should not reach here")
            }
            fn name(&self) -> &str {
                "test"
            }
            fn prepare_arguments(&self, _args: &Value, _work_dir: &Path) -> Result<Value, String> {
                Err("Invalid arguments: required field missing".into())
            }
        }
        let handler = AbortingHandler;
        let args = serde_json::json!({"key": "value"});
        let result = handler.prepare_arguments(&args, Path::new("."));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("required field missing"));
    }
}

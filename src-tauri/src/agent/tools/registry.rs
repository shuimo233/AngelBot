//! Tool registry — manages all available tools with dynamic dispatch
//!
//! Tools are stored as (Tool, Arc<dyn ToolHandler>) pairs:
//! - `Tool`: metadata/schema (name, description, JSON Schema)
//! - `ToolHandler`: execution logic (via dynamic dispatch, shared via Arc)
//!
//! Issue #47: Registry is wrapped in `Arc<Mutex<HashMap>>` for thread-safe
//! concurrent access during parallel tool execution.

use super::tool::{Tool, ToolCall, ToolCategory, ToolExecutionContext, ToolHandler, ToolResult};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Result of executing a tool
#[derive(Debug, Clone)]
pub struct ToolExecResult {
    pub result: ToolResult,
    pub call_id: String,
    /// Issue #082: signals that the remainder of the tool batch should be skipped.
    pub terminate_batch: bool,
}

/// Global registry for all available tools.
///
/// Wrapped in `Arc<Mutex<HashMap>>` for thread-safe concurrent access (Issue #47).
/// Cloning the registry only clones the Arc pointer (cheap).
#[derive(Clone, Default)]
pub struct ToolRegistry {
    entries: Arc<Mutex<HashMap<String, ToolEntry>>>,
}

struct ToolEntry {
    tool: Tool,
    handler: Arc<dyn ToolHandler>,
}

impl ToolRegistry {
    /// Create an empty registry. Foreground surfaces use this constructor so
    /// installation has one explicit, auditable entry point.
    pub fn empty() -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn new() -> Self {
        let mut registry = Self::empty();
        super::handlers::register_builtin_handlers(&mut registry);
        registry
    }

    /// Register a new tool with its handler.
    pub fn register(&mut self, tool: Tool, handler: Arc<dyn ToolHandler>) {
        let mut guard = self.entries.lock().unwrap();
        guard.insert(tool.name.clone(), ToolEntry { tool, handler });
    }

    /// Issue #095: Register a batch of extension tools.
    pub fn register_extension(&mut self, tools: Vec<(Tool, Arc<dyn ToolHandler>)>) {
        let mut guard = self.entries.lock().unwrap();
        for (tool, handler) in tools {
            guard.insert(tool.name.clone(), ToolEntry { tool, handler });
        }
    }

    /// Get a tool by name (cloned for thread-safety)
    pub fn get(&self, name: &str) -> Option<Tool> {
        let guard = self.entries.lock().unwrap();
        guard.get(name).map(|e| {
            let mut tool = e.tool.clone();
            if is_side_effect_tool(&tool.name) {
                tool.requires_confirmation = true;
            }
            tool
        })
    }

    /// Check if a tool exists
    pub fn contains(&self, name: &str) -> bool {
        let guard = self.entries.lock().unwrap();
        guard.contains_key(name)
    }

    /// Get all tool schemas for LLM function calling
    pub fn get_schemas(&self) -> Vec<Value> {
        let guard = self.entries.lock().unwrap();
        guard
            .values()
            .map(|entry| entry.tool.to_function_schema())
            .collect()
    }

    /// Get all tool schemas, optionally with enhanced parameter descriptions (Issue #52).
    pub fn get_schemas_with_enhanced(&self, enhanced: bool) -> Vec<Value> {
        let guard = self.entries.lock().unwrap();
        guard
            .values()
            .map(|entry| {
                if enhanced {
                    entry.tool.to_function_schema_enhanced()
                } else {
                    entry.tool.to_function_schema()
                }
            })
            .collect()
    }

    /// Issue #095: Get all tools filtered by category.
    pub fn get_by_category(&self, category: ToolCategory) -> Vec<Tool> {
        let guard = self.entries.lock().unwrap();
        guard
            .values()
            .filter(|e| e.tool.category == category)
            .map(|e| {
                let mut tool = e.tool.clone();
                if is_side_effect_tool(&tool.name) {
                    tool.requires_confirmation = true;
                }
                tool
            })
            .collect()
    }

    /// Issue #095: Get all core tools (always loaded).
    pub fn get_core_tools(&self) -> Vec<Tool> {
        self.get_by_category(ToolCategory::Core)
    }

    /// Issue #095: Get all extension tools (loaded on demand).
    pub fn get_extension_tools(&self) -> Vec<Tool> {
        self.get_by_category(ToolCategory::Extension)
    }

    /// Issue #095: Get all platform tools (Windows-specific).
    pub fn get_platform_tools(&self) -> Vec<Tool> {
        self.get_by_category(ToolCategory::Platform)
    }

    /// Issue #104: Collect all non-empty prompt snippets from available tools.
    /// These are injected into the system prompt to guide the LLM on *when*
    /// to use each tool (distinct from the schema which tells it *how*).
    pub fn get_prompt_snippets(&self) -> Vec<(String, String)> {
        let guard = self.entries.lock().unwrap();
        guard
            .values()
            .filter_map(|e| {
                e.tool
                    .prompt_snippet
                    .as_ref()
                    .map(|s| (e.tool.name.clone(), s.clone()))
            })
            .collect()
    }

    /// Get the number of registered tools
    pub fn len(&self) -> usize {
        let guard = self.entries.lock().unwrap();
        guard.len()
    }

    /// Check if registry is empty
    pub fn is_empty(&self) -> bool {
        let guard = self.entries.lock().unwrap();
        guard.is_empty()
    }

    /// Validate the minimum JSON-schema contract before a call is confirmed or
    /// executed. This keeps malformed provider payloads from becoming a
    /// misleading approval request (for example `write_file({})`).
    pub fn validate_arguments(&self, name: &str, arguments: &Value) -> Result<(), String> {
        let guard = self
            .entries
            .lock()
            .map_err(|_| "tool registry is unavailable".to_string())?;
        let entry = guard
            .get(name)
            .ok_or_else(|| format!("tool '{}' was not found", name))?;
        let object = arguments
            .as_object()
            .ok_or_else(|| "arguments must be a JSON object".to_string())?;
        let schema = &entry.tool.parameters;
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut missing = Vec::new();
        for field in required.iter().filter_map(Value::as_str) {
            let valid = object.get(field).is_some_and(|value| !value.is_null());
            if !valid {
                missing.push(field.to_string());
            }
        }
        if !missing.is_empty() {
            return Err(format!(
                "missing required argument(s): {}",
                missing.join(", ")
            ));
        }

        let properties = schema.get("properties").and_then(Value::as_object);
        if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
            if let Some(properties) = properties {
                let unexpected: Vec<_> = object
                    .keys()
                    .filter(|key| !properties.contains_key(*key))
                    .cloned()
                    .collect();
                if !unexpected.is_empty() {
                    return Err(format!("unexpected argument(s): {}", unexpected.join(", ")));
                }
            }
        }

        if let Some(properties) = properties {
            for (name, value) in object {
                let Some(property) = properties.get(name) else {
                    continue;
                };
                if let Some(expected) = property.get("type").and_then(Value::as_str) {
                    let type_matches = match expected {
                        "string" => value.is_string(),
                        "integer" => value.is_i64() || value.is_u64(),
                        "number" => value.is_number(),
                        "boolean" => value.is_boolean(),
                        "array" => value.is_array(),
                        "object" => value.is_object(),
                        _ => true,
                    };
                    if !type_matches {
                        return Err(format!("argument '{}' must be a {}", name, expected));
                    }
                }
                if let Some(allowed) = property.get("enum").and_then(Value::as_array) {
                    if !allowed.contains(value) {
                        return Err(format!(
                            "argument '{}' is not allowed by the tool enum",
                            name
                        ));
                    }
                }
                if let (Some(min), Some(text)) = (
                    property.get("minLength").and_then(Value::as_u64),
                    value.as_str(),
                ) {
                    if text.chars().count() < min as usize {
                        return Err(format!("argument '{}' must not be empty", name));
                    }
                }
            }
        }
        Ok(())
    }

    /// Execute a tool by name with the given arguments, call_id, and work_dir.
    ///
    /// Safe to call from multiple threads concurrently (Issue #47).
    /// The mutex is locked only briefly to look up the handler.
    pub fn execute(
        &self,
        tool_call: &ToolCall,
        call_id: &str,
        work_dir: &std::path::Path,
    ) -> ToolExecResult {
        self.execute_with_context(
            tool_call,
            call_id,
            work_dir,
            &ToolExecutionContext::new(call_id, super::tool::ToolCancellation::new()),
        )
    }

    /// Execute a tool with runtime-owned cancellation and deadline metadata.
    ///
    /// The registry is the common enforcement point so cancelled calls cannot
    /// start a handler merely because a caller forgot to check first.
    pub fn execute_with_context(
        &self,
        tool_call: &ToolCall,
        call_id: &str,
        work_dir: &std::path::Path,
        context: &ToolExecutionContext,
    ) -> ToolExecResult {
        if context.is_cancelled() {
            return ToolExecResult {
                result: ToolResult::error(&tool_call.name, "工具执行已取消"),
                call_id: call_id.to_string(),
                terminate_batch: true,
            };
        }
        if context.is_expired() {
            return ToolExecResult {
                result: ToolResult::error(&tool_call.name, "工具执行截止时间已过"),
                call_id: call_id.to_string(),
                terminate_batch: true,
            };
        }
        if let Err(error) = self.validate_arguments(&tool_call.name, &tool_call.arguments) {
            return ToolExecResult {
                result: ToolResult::error(
                    &tool_call.name,
                    format!("工具参数不合法，未执行: {error}"),
                ),
                call_id: call_id.to_string(),
                terminate_batch: false,
            };
        }
        let handler = {
            let guard = self.entries.lock().unwrap();
            match guard.get(&tool_call.name) {
                Some(entry) => Arc::clone(&entry.handler),
                None => {
                    return ToolExecResult {
                        result: ToolResult::error(
                            &tool_call.name,
                            format!("工具 '{}' 未找到", tool_call.name),
                        ),
                        call_id: call_id.to_string(),
                        terminate_batch: false,
                    };
                }
            }
        };

        // Issue #093: Call prepare_arguments to preprocess args
        let arguments = match handler.prepare_arguments(&tool_call.arguments, work_dir) {
            Ok(args) => args,
            Err(err_msg) => {
                return ToolExecResult {
                    result: ToolResult::error(&tool_call.name, err_msg),
                    call_id: call_id.to_string(),
                    terminate_batch: false,
                };
            }
        };

        // Execute without holding the lock (handler may do I/O / DB work)
        let result = handler.execute_with_context(&arguments, work_dir, context);
        // Issue #082: propagate terminate_batch from handler result
        let terminate_batch_flag = result.terminate_batch;
        ToolExecResult {
            result,
            call_id: call_id.to_string(),
            terminate_batch: terminate_batch_flag,
        }
    }
}

/// Returns true when the given tool is considered a "side effect" for the
/// purposes of guard accounting. Re-exported from `crate::agent::guards`
/// so the runner and the guard system share one source of truth.
pub fn is_side_effect_tool(name: &str) -> bool {
    matches!(
        name,
        "write_file"
            | "edit_file"
            | "create_directory"
            | "organize_workspace_item"
            | "run_project_command"
            | "create_automation"
            | "schedule_reminder"
            | "cancel_reminder"
            | "import_skill_from_github"
            | "open_trusted_app"
            | "open_windows_setting"
            | "reveal_workspace_item"
            | "prepare_message_draft"
            | "set_trusted_app_text"
            | "operate_trusted_app_control"
            | "materialize_delegated_change"
            | "delegate_network_exploration"
    ) || name.starts_with("mcp_")
}

/// Actions whose authority must never be inferred from a broad execution
/// preference. These operations change credentials, retained personal data,
/// install executable guidance, or write into an external application that
/// may sync immediately. They always need an exact, single-use user decision.
pub fn requires_immediate_confirmation(name: &str) -> bool {
    matches!(
        name,
        "delete_memory"
            | "forget_memory"
            | "update_memory"
            | "clear_channel_credential"
            | "save_channel_credential"
            | "import_skill_from_github"
            | "delegate_network_exploration"
            | "prepare_message_draft"
            | "set_trusted_app_text"
            | "operate_trusted_app_control"
    )
}

#[cfg(test)]
mod tests {
    use super::{requires_immediate_confirmation, ToolRegistry};
    use crate::agent::tool::{
        Tool, ToolCall, ToolCancellation, ToolExecutionContext, ToolHandler, ToolResult,
    };
    use serde_json::json;
    use std::path::Path;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct RecordingHandler {
        calls: Arc<AtomicUsize>,
    }

    impl ToolHandler for RecordingHandler {
        fn name(&self) -> &str {
            "recording_tool"
        }

        fn execute(&self, _arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            ToolResult::success("recording_tool", "executed")
        }
    }

    #[test]
    fn protected_actions_always_require_an_exact_decision() {
        for name in [
            "delete_memory",
            "save_channel_credential",
            "import_skill_from_github",
            "delegate_network_exploration",
            "prepare_message_draft",
            "set_trusted_app_text",
            "operate_trusted_app_control",
        ] {
            assert!(requires_immediate_confirmation(name), "{name}");
        }
        assert!(!requires_immediate_confirmation("write_file"));
        assert!(!requires_immediate_confirmation("open_trusted_app"));
        assert!(!requires_immediate_confirmation("mcp_send_email"));
    }

    #[test]
    fn write_file_requires_path_and_content_before_confirmation() {
        let registry = ToolRegistry::new();
        let error = registry
            .validate_arguments("write_file", &json!({}))
            .unwrap_err();
        assert!(error.contains("path"));
        assert!(error.contains("content"));

        assert!(registry
            .validate_arguments(
                "write_file",
                &json!({"path": "page.html", "content": "<main />"}),
            )
            .is_ok());
    }

    #[test]
    fn workspace_organization_is_confirmation_gated_and_schema_validated() {
        let registry = ToolRegistry::new();
        let tool = registry
            .get("organize_workspace_item")
            .expect("workspace organization tool should be registered");
        assert!(tool.requires_confirmation);
        assert!(registry
            .validate_arguments(
                "organize_workspace_item",
                &json!({
                    "operation": "move",
                    "source": "draft.txt",
                    "destination": "archive/draft.txt"
                }),
            )
            .is_ok());
        assert!(registry
            .validate_arguments(
                "organize_workspace_item",
                &json!({ "operation": "delete", "source": "draft.txt", "destination": "archive/draft.txt" }),
            )
            .is_err());
    }

    #[test]
    fn cancelled_context_rejects_a_tool_before_handler_lookup() {
        let registry = ToolRegistry::new();
        let cancellation = ToolCancellation::new();
        cancellation.cancel();
        let call = ToolCall::new("write_file".to_string(), json!({"path": "a.txt"}));

        let result = registry.execute_with_context(
            &call,
            "call-1",
            Path::new("."),
            &ToolExecutionContext::new("call-1", cancellation),
        );

        assert!(!result.result.success);
        assert!(result.result.content.contains("已取消"));
        assert!(result.terminate_batch);
    }

    #[test]
    fn direct_execution_validates_arguments_before_a_handler_can_act() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = ToolRegistry::empty();
        registry.register(
            Tool::new(
                "recording_tool",
                "records a test-only side effect",
                json!({
                    "type": "object",
                    "properties": { "target": { "type": "string", "enum": ["ok"] } },
                    "required": ["target"],
                    "additionalProperties": false
                }),
            ),
            Arc::new(RecordingHandler {
                calls: calls.clone(),
            }),
        );

        for arguments in [
            json!({}),
            json!({"target":"unsupported"}),
            json!({"target":"Ok"}),
        ] {
            let invalid = registry.execute(
                &ToolCall::new("recording_tool".to_string(), arguments),
                "invalid-call",
                Path::new("."),
            );
            assert!(!invalid.result.success);
            assert!(invalid.result.content.contains("参数不合法"));
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }

        let valid = registry.execute(
            &ToolCall::new("recording_tool".to_string(), json!({ "target": "ok" })),
            "valid-call",
            Path::new("."),
        );
        assert!(valid.result.success);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

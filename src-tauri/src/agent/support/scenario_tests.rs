//! Agent toolchain integration tests with realistic scenarios.
//!
//! These tests simulate complete multi-step task flows through the full agent toolchain:
//! Runner → Planner → ToT → Critique → CircuitBreaker
//!
//! Each test represents a concrete user scenario and validates the complete response.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::agent::config::AgentConfig;
use crate::agent::runner::{AgentResponse, AgentRunner};
use crate::agent::{Tool, ToolHandler, ToolResult};
use crate::llm::{LlmProvider, LlmResponse, Message, ToolSchema};

/// Sequence of LLM responses to simulate a multi-turn conversation.
#[derive(Clone)]
struct ScenarioProvider {
    responses: Vec<(Option<Vec<crate::llm::ToolCall>>, Option<String>)>,
    call_idx: Arc<AtomicUsize>,
}

impl ScenarioProvider {
    fn new(responses: Vec<(Option<Vec<crate::llm::ToolCall>>, Option<String>)>) -> Self {
        Self {
            responses,
            call_idx: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Simple text-only responses
    fn text_only(responses: Vec<&'static str>) -> Self {
        Self::new(
            responses
                .into_iter()
                .map(|s| (None, Some(s.to_string())))
                .collect(),
        )
    }

    /// Start with tool calls, then text
    fn with_tools_and_text(
        tool_then_text: Vec<(Option<Vec<crate::llm::ToolCall>>, Option<String>)>,
    ) -> Self {
        Self::new(tool_then_text)
    }
}

#[async_trait::async_trait]
impl LlmProvider for ScenarioProvider {
    fn name(&self) -> &str {
        "scenario"
    }

    async fn chat(
        &self,
        _messages: &[Message],
        _tools: Option<&[ToolSchema]>,
        _thinking: Option<&crate::llm::ThinkingSettings>,
    ) -> Result<LlmResponse, String> {
        let idx = self.call_idx.fetch_add(1, Ordering::SeqCst);
        match self.responses.get(idx) {
            Some((tool_calls, text)) => Ok(match (tool_calls, text) {
                (Some(tc), Some(t)) => LlmResponse::ToolCalls {
                    calls: tc.clone(),
                    text: Some(t.clone()),
                    stop_reason: crate::llm::StopReason::ToolUse,
                },
                (Some(tc), None) => LlmResponse::ToolCalls {
                    calls: tc.clone(),
                    text: None,
                    stop_reason: crate::llm::StopReason::ToolUse,
                },
                (None, Some(t)) => LlmResponse::Text {
                    text: t.clone(),
                    stop_reason: crate::llm::StopReason::EndTurn,
                },
                (None, None) => LlmResponse::Text {
                    text: "...".to_string(),
                    stop_reason: crate::llm::StopReason::EndTurn,
                },
            }),
            None => Ok(LlmResponse::Text {
                text: "No more responses available.".to_string(),
                stop_reason: crate::llm::StopReason::EndTurn,
            }),
        }
    }
}

/// Tool handler that returns configurable results.
struct ScenarioToolHandler {
    name: &'static str,
    results: Vec<ToolResult>,
    call_idx: Arc<AtomicUsize>,
}

impl ScenarioToolHandler {
    fn new(name: &'static str, results: Vec<ToolResult>) -> Self {
        Self {
            name,
            results,
            call_idx: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl ToolHandler for ScenarioToolHandler {
    fn name(&self) -> &str {
        self.name
    }

    fn execute(&self, _args: &serde_json::Value, _work_dir: &std::path::Path) -> ToolResult {
        let idx = self.call_idx.fetch_add(1, Ordering::SeqCst);
        self.results
            .get(idx)
            .cloned()
            .unwrap_or_else(|| ToolResult::success(self.name, "default result"))
    }
}

fn default_config() -> AgentConfig {
    AgentConfig {
        persona_name: "Angel".into(),
        persona_description: "Helpful assistant".into(),
        persona_greeting: "Hello!".into(),
        ..Default::default()
    }
}

fn build_runner_with_tools(
    provider: ScenarioProvider,
    tools: Vec<(Tool, Arc<dyn ToolHandler>)>,
) -> AgentRunner {
    let mut runner = AgentRunner::new(Box::new(provider), default_config());
    for (tool, handler) in tools {
        runner.tool_registry_mut().register(tool, handler);
    }
    runner
}

// =============================================================================
// SCENARIO TESTS
// =============================================================================

#[cfg(test)]
mod scenario_tests {

    use super::*;

    // -------------------------------------------------------------------------
    // Scenario 1: Multi-step File Search
    // "Find all TODO comments in my project"
    // Expected: search_files → read_file → synthesize
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn scenario_multi_step_file_search() {
        let provider = ScenarioProvider::with_tools_and_text(vec![
            // Turn 1: agent calls search_files
            (
                Some(vec![crate::llm::ToolCall {
                    id: "call-1".into(),
                    name: "search_files".into(),
                    arguments: serde_json::json!({
                        "path": ".",
                        "pattern": "TODO"
                    }),
                }]),
                Some("我来找一下项目中的 TODO 注释。".to_string()),
            ),
            // Turn 2: agent calls read_file
            (
                Some(vec![crate::llm::ToolCall {
                    id: "call-2".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({
                        "path": "src/main.rs",
                        "max_lines": 100
                    }),
                }]),
                Some("正在读取找到的文件...".to_string()),
            ),
            // Turn 3: final synthesis text response
            (
                None,
                Some("在 src/main.rs 中找到了 3 个 TODO：\n1. TODO: 优化性能\n2. TODO: 添加测试\n3. TODO: 清理注释".to_string()),
            ),
        ]);

        let tools = vec![
            (
                Tool::new(
                    "search_files",
                    "Search for files matching a pattern",
                    serde_json::json!({
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "pattern": { "type": "string" }
                        },
                        "required": ["path", "pattern"]
                    }),
                ),
                Arc::new(ScenarioToolHandler::new(
                    "search_files",
                    vec![
                        ToolResult::success("search_files", r#"["src/main.rs","src/lib.rs"]"#),
                    ],
                )) as Arc<dyn ToolHandler>,
            ),
            (
                Tool::new(
                    "read_file",
                    "Read file contents",
                    serde_json::json!({
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "max_lines": { "type": "integer" }
                        },
                        "required": ["path"]
                    }),
                ),
                Arc::new(ScenarioToolHandler::new(
                    "read_file",
                    vec![ToolResult::success(
                        "read_file",
                        "// TODO: optimize performance\n// TODO: add tests\n// TODO: cleanup comments",
                    )],
                )) as Arc<dyn ToolHandler>,
            ),
        ];

        let runner = build_runner_with_tools(provider, tools);
        let result = runner
            .run("Find all TODO comments in my project", &[])
            .await;

        match result {
            AgentResponse::TextWithToolCalls { text, steps } => {
                assert_eq!(steps.len(), 2, "Should execute 2 tools");
                assert_eq!(steps[0].tool_name, "search_files");
                assert_eq!(steps[1].tool_name, "read_file");
                assert!(
                    text.contains("TODO"),
                    "Response should contain TODO findings"
                );
                assert!(text.contains("src/main.rs"));
            }
            other => panic!("Expected TextWithToolCalls, got {:?}", other),
        }
    }

    // -------------------------------------------------------------------------
    // Scenario 1b: Multi-file page generation
    // Read two source files in one model turn, write a derived file, then read
    // it back. This exercises the real built-in file handlers and their work
    // directory sandbox rather than a mocked handler.
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn scenario_parallel_reads_then_write_and_verify_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("brief.txt"), "Launch page content").unwrap();
        std::fs::write(dir.path().join("tone.txt"), "Warm and concise").unwrap();

        let provider = ScenarioProvider::with_tools_and_text(vec![
            (
                Some(vec![
                    crate::llm::ToolCall {
                        id: "call-read-brief".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "brief.txt"}),
                    },
                    crate::llm::ToolCall {
                        id: "call-read-tone".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "tone.txt"}),
                    },
                ]),
                None,
            ),
            (
                Some(vec![crate::llm::ToolCall {
                    id: "call-write-page".into(),
                    name: "write_file".into(),
                    arguments: serde_json::json!({
                        "path": "page.html",
                        "content": "<main>Launch page content — Warm and concise</main>",
                    }),
                }]),
                None,
            ),
            (
                Some(vec![crate::llm::ToolCall {
                    id: "call-verify-page".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "page.html"}),
                }]),
                None,
            ),
            (None, Some("页面已生成并验证。".to_string())),
        ]);

        let mut runner = AgentRunner::new(Box::new(provider), default_config())
            .with_execution_permission("workspace_auto".to_string());
        runner.set_work_dir(dir.path().to_path_buf());

        let result = runner
            .run(
                "读取 brief.txt 和 tone.txt，生成 page.html，并读取生成的文件验证。",
                &[],
            )
            .await;

        match result {
            AgentResponse::TextWithToolCalls { text, steps } => {
                assert_eq!(text, "页面已生成并验证。");
                assert_eq!(steps.len(), 4);
                assert_eq!(
                    steps
                        .iter()
                        .map(|step| step.tool_name.as_str())
                        .collect::<Vec<_>>(),
                    vec!["read_file", "read_file", "write_file", "read_file"],
                );
                assert!(steps.iter().all(|step| step.success));
            }
            other => panic!("Expected TextWithToolCalls, got {:?}", other),
        }

        assert_eq!(
            std::fs::read_to_string(dir.path().join("page.html")).unwrap(),
            "<main>Launch page content — Warm and concise</main>",
        );
    }

    // -------------------------------------------------------------------------
    // Scenario 2: Memory Recall
    // "What's my name?" — agent should recall from memory
    // Expected: recall_memories tool → text response with recalled fact
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn scenario_memory_recall() {
        let provider = ScenarioProvider::with_tools_and_text(vec![
            // Turn 1: agent calls recall_memories
            (
                Some(vec![crate::llm::ToolCall {
                    id: "call-recall".into(),
                    name: "recall_memories".into(),
                    arguments: serde_json::json!({
                        "query": "user name"
                    }),
                }]),
                Some("让我回忆一下...".to_string()),
            ),
            // Turn 2: final response using recalled memory
            (None, Some("你叫小明！".to_string())),
        ]);

        let tools = vec![(
            Tool::new(
                "recall_memories",
                "Recall memories relevant to a query",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    },
                    "required": ["query"]
                }),
            ),
            Arc::new(ScenarioToolHandler::new(
                "recall_memories",
                vec![ToolResult::success(
                    "recall_memories",
                    r#"{"memories":[{"content":"user_name: 小明","importance":0.9}]}"#,
                )],
            )) as Arc<dyn ToolHandler>,
        )];

        let runner = build_runner_with_tools(provider, tools);
        let result = runner.run("What's my name?", &[]).await;

        match result {
            AgentResponse::TextWithToolCalls { text, steps } => {
                assert_eq!(steps.len(), 1);
                assert_eq!(steps[0].tool_name, "recall_memories");
                assert!(text.contains("小明"), "Should recall user name from memory");
            }
            other => panic!("Expected TextWithToolCalls, got {:?}", other),
        }
    }

    // -------------------------------------------------------------------------
    // Scenario 3: Tool Error Recovery
    // File read fails → agent retries → succeeds
    // Expected: read fails → retry → success → respond
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn scenario_tool_error_recovery() {
        let call_count = Arc::new(AtomicUsize::new(0));

        let provider = ScenarioProvider::with_tools_and_text(vec![
            // Turn 1: read_file fails
            (
                Some(vec![crate::llm::ToolCall {
                    id: "call-r1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "missing.txt"}),
                }]),
                Some("我来读取这个文件...".to_string()),
            ),
            // Turn 2: read_file retry
            (
                Some(vec![crate::llm::ToolCall {
                    id: "call-r2".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "note.txt"}),
                }]),
                Some("让我重试读取另一个文件...".to_string()),
            ),
            // Turn 3: success
            (None, Some("文件读取成功！内容是：hello world".to_string())),
        ]);

        // First call fails, second succeeds
        let call_count_clone = call_count.clone();
        let tools = vec![(
            Tool::new(
                "read_file",
                "Read a file",
                serde_json::json!({
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }),
            ),
            Arc::new(move |_: &str| -> ToolResult {
                let idx = call_count_clone.fetch_add(1, Ordering::SeqCst);
                if idx == 0 {
                    ToolResult::error("read_file", "File not found: missing.txt")
                } else {
                    ToolResult::success("read_file", "hello world")
                }
            }) as Arc<dyn Fn(&str) -> ToolResult + Send + Sync>,
        )];

        // Build registry with closure handler
        let mut runner = AgentRunner::new(Box::new(provider), default_config());

        struct ClosureHandler<F> {
            f: F,
            name: String,
        }
        impl<F: Fn(&serde_json::Value) -> ToolResult + Send + Sync> ToolHandler for ClosureHandler<F> {
            fn name(&self) -> &str {
                &self.name
            }
            fn execute(&self, args: &serde_json::Value, _work_dir: &std::path::Path) -> ToolResult {
                (self.f)(args)
            }
        }

        let f = move |_args: &serde_json::Value| -> ToolResult {
            let idx = call_count.fetch_add(1, Ordering::SeqCst);
            if idx == 0 {
                ToolResult::error("read_file", "File not found: missing.txt")
            } else {
                ToolResult::success("read_file", "hello world")
            }
        };

        runner.tool_registry_mut().register(
            Tool::new(
                "read_file",
                "Read a file",
                serde_json::json!({
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }),
            ),
            Arc::new(ClosureHandler {
                name: "read_file".to_string(),
                f,
            }) as Arc<dyn ToolHandler>,
        );

        let result = runner.run("Read the file", &[]).await;

        match result {
            AgentResponse::TextWithToolCalls { text, steps } => {
                assert_eq!(steps.len(), 2, "Should have 2 tool calls (failed + retry)");
                assert!(!steps[0].success, "First call should fail");
                assert!(steps[1].success, "Second call should succeed");
                assert!(text.contains("成功") || text.contains("hello"));
            }
            other => panic!("Expected TextWithToolCalls, got {:?}", other),
        }
    }

    // -------------------------------------------------------------------------
    // Scenario 4: Side Effect Confirmation Flow
    // Agent requests confirmation before writing a file
    // Expected: write_file blocked → confirmation required → no execution
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn scenario_write_confirmation_blocks_execution() {
        let execution_count = Arc::new(AtomicUsize::new(0));

        let provider = ScenarioProvider::new(vec![
            // Turn 1: agent tries to write file
            (
                Some(vec![crate::llm::ToolCall {
                    id: "call-w1".into(),
                    name: "write_file".into(),
                    arguments: serde_json::json!({
                        "path": "output.txt",
                        "content": "important data"
                    }),
                }]),
                Some("我将写入文件 output.txt".to_string()),
            ),
            // Turn 2: agent waits for confirmation
            (None, Some("等待你的确认来写入文件。".to_string())),
        ]);

        let exec_count = execution_count.clone();
        struct WriteCounter {
            count: Arc<AtomicUsize>,
        }
        impl ToolHandler for WriteCounter {
            fn name(&self) -> &str {
                "write_file"
            }
            fn execute(
                &self,
                _args: &serde_json::Value,
                _work_dir: &std::path::Path,
            ) -> ToolResult {
                self.count.fetch_add(1, Ordering::SeqCst);
                ToolResult::success("write_file", "File written")
            }
        }

        let mut runner = AgentRunner::new(Box::new(provider), default_config());
        runner.tool_registry_mut().register(
            Tool::new(
                "write_file",
                "Write content to a file",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"]
                }),
            ),
            Arc::new(WriteCounter {
                count: exec_count.clone(),
            }) as Arc<dyn ToolHandler>,
        );

        let result = runner
            .run("Write 'important data' to output.txt", &[])
            .await;

        match result {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                assert_eq!(steps.len(), 1);
                assert!(
                    steps[0].confirmation_required,
                    "write_file should require confirmation"
                );
                assert!(!steps[0].success, "Should not execute without confirmation");
                // Verify the handler was NOT called (execution blocked)
                assert_eq!(
                    exec_count.load(Ordering::SeqCst),
                    0,
                    "Handler should NOT execute without confirmation"
                );
            }
            other => panic!("Expected TextWithToolCalls, got {:?}", other),
        }
    }

    // -------------------------------------------------------------------------
    // Scenario 5: Multi-Turn Conversation Memory
    // User: "记住我喜欢蓝色" → agent confirms
    // User: "我有什么喜好?" → agent recalls
    // Expected: remember_fact stores → recall shows it
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn scenario_conversation_memory_across_turns() {
        // Simulate 2 turns: remember → recall
        let history = vec![("user".to_string(), "记住我喜欢蓝色".to_string())];

        let provider = ScenarioProvider::with_tools_and_text(vec![
            // Turn 1 (with history): remember_fact
            (
                Some(vec![crate::llm::ToolCall {
                    id: "call-remember".into(),
                    name: "remember_fact".into(),
                    arguments: serde_json::json!({
                        "fact": "user prefers blue color",
                        "importance": 0.8
                    }),
                }]),
                Some("好的，我记住了你更喜欢蓝色！".to_string()),
            ),
            // Turn 2 (simulated next turn): recall
            (
                Some(vec![crate::llm::ToolCall {
                    id: "call-recall2".into(),
                    name: "recall_memories".into(),
                    arguments: serde_json::json!({"query": "color preference"}),
                }]),
                Some("让我查一下...".to_string()),
            ),
            (None, Some("你更喜欢蓝色！".to_string())),
        ]);

        let tools = vec![
            (
                Tool::new(
                    "remember_fact",
                    "Remember a fact about the user",
                    serde_json::json!({
                        "type": "object",
                        "properties": {
                            "fact": { "type": "string" },
                            "importance": { "type": "number" }
                        },
                        "required": ["fact"]
                    }),
                ),
                Arc::new(ScenarioToolHandler::new(
                    "remember_fact",
                    vec![ToolResult::success(
                        "remember_fact",
                        "Fact remembered successfully",
                    )],
                )) as Arc<dyn ToolHandler>,
            ),
            (
                Tool::new(
                    "recall_memories",
                    "Recall memories",
                    serde_json::json!({
                        "type": "object",
                        "properties": { "query": { "type": "string" } },
                        "required": ["query"]
                    }),
                ),
                Arc::new(ScenarioToolHandler::new(
                    "recall_memories",
                    vec![ToolResult::success(
                        "recall_memories",
                        r#"{"memories":[{"content":"user prefers blue color","importance":0.8}]}"#,
                    )],
                )) as Arc<dyn ToolHandler>,
            ),
        ];

        let runner = build_runner_with_tools(provider, tools);

        // Simulate the full conversation
        let result = runner.run("What are my preferences?", &history).await;

        match result {
            AgentResponse::TextWithToolCalls { text, steps } => {
                assert_eq!(steps.len(), 2, "Should call both remember and recall");
                assert_eq!(steps[0].tool_name, "remember_fact");
                assert_eq!(steps[1].tool_name, "recall_memories");
                assert!(text.contains("蓝色"), "Should recall blue preference");
            }
            other => panic!("Expected TextWithToolCalls, got {:?}", other),
        }
    }

    // -------------------------------------------------------------------------
    // Scenario 6: Parallel Tool Execution
    // Agent calls multiple independent tools simultaneously
    // Expected: search + web_fetch in same turn → both execute in parallel
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn scenario_parallel_independent_tools() {
        let provider = ScenarioProvider::with_tools_and_text(vec![
            (
                Some(vec![
                    crate::llm::ToolCall {
                        id: "call-p1".into(),
                        name: "search_files".into(),
                        arguments: serde_json::json!({"path": ".", "pattern": "*.md"}),
                    },
                    crate::llm::ToolCall {
                        id: "call-p2".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "README.md"}),
                    },
                ]),
                Some("我同时搜索文件并读取 README".to_string()),
            ),
            (
                None,
                Some("找到 5 个 markdown 文件。README 内容已读取。".to_string()),
            ),
        ]);

        let tools = vec![
            (
                Tool::new(
                    "search_files",
                    "Search for files",
                    serde_json::json!({
                        "type": "object",
                        "properties": { "path": { "type": "string" }, "pattern": { "type": "string" } },
                        "required": ["path", "pattern"]
                    }),
                ),
                Arc::new(ScenarioToolHandler::new(
                    "search_files",
                    vec![ToolResult::success(
                        "search_files",
                        r#"["README.md","docs/a.md"]"#,
                    )],
                )) as Arc<dyn ToolHandler>,
            ),
            (
                Tool::new(
                    "read_file",
                    "Read a file",
                    serde_json::json!({
                        "type": "object",
                        "properties": { "path": { "type": "string" } },
                        "required": ["path"]
                    }),
                ),
                Arc::new(ScenarioToolHandler::new(
                    "read_file",
                    vec![ToolResult::success(
                        "read_file",
                        "# Project\n\nAngelBot is...",
                    )],
                )) as Arc<dyn ToolHandler>,
            ),
        ];

        let runner = build_runner_with_tools(provider, tools);
        let result = runner
            .run("Search for markdown files and read the README", &[])
            .await;

        match result {
            AgentResponse::TextWithToolCalls { text, steps } => {
                assert_eq!(steps.len(), 2, "Should execute 2 independent tools");
                let tool_names: Vec<_> = steps.iter().map(|s| s.tool_name.as_str()).collect();
                assert!(tool_names.contains(&"search_files"));
                assert!(tool_names.contains(&"read_file"));
                assert!(steps.iter().all(|s| s.success), "Both should succeed");
            }
            other => panic!("Expected TextWithToolCalls, got {:?}", other),
        }
    }

    // -------------------------------------------------------------------------
    // Scenario 7: No-Tool Simple Question
    // User asks a simple question → no tools needed → fast text response
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn scenario_simple_text_only_response() {
        let provider = ScenarioProvider::text_only(vec![
            "你好！很高兴见到你。我是 Angel，很乐意帮助你。有什么我可以效劳的吗？",
        ]);
        let runner = build_runner_with_tools(provider, vec![]);

        let result = runner.run("你好！", &[]).await;

        match result {
            AgentResponse::Text(text) => {
                assert!(text.contains("Angel") || text.contains("帮助") || text.contains("你好"));
            }
            other => panic!("Expected plain Text response, got {:?}", other),
        }
    }

    // -------------------------------------------------------------------------
    // Scenario 8: Max Iterations Boundary
    // Task that requires more than MAX_ITERATIONS → should stop cleanly
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn scenario_max_iterations_stops_cleanly() {
        // Provider that always returns tool calls (would loop forever without limit)
        let provider = ScenarioProvider::new(
            (0..20)
                .map(|i| {
                    (
                        Some(vec![crate::llm::ToolCall {
                            id: format!("call-{}", i),
                            name: "search_files".into(),
                            arguments: serde_json::json!({"path": ".", "pattern": "*"}),
                        }]),
                        Some(format!("第 {} 步...", i + 1)),
                    )
                })
                .collect(),
        );

        let tools = vec![(
            Tool::new(
                "search_files",
                "Search files",
                serde_json::json!({
                    "type": "object",
                    "properties": { "path": { "type": "string" }, "pattern": { "type": "string" } },
                    "required": ["path", "pattern"]
                }),
            ),
            Arc::new(ScenarioToolHandler::new(
                "search_files",
                vec![ToolResult::success("search_files", "[]"); 20],
            )) as Arc<dyn ToolHandler>,
        )];

        let runner = build_runner_with_tools(provider, tools);
        let result = runner.run("Count all files recursively", &[]).await;

        // Should complete (not panic) even when hitting iteration limit
        match result {
            AgentResponse::Text(_) | AgentResponse::TextWithToolCalls { .. } => {
                // Should produce some result, not hang
            }
            AgentResponse::Error(e) => {
                panic!("Should not error: {}", e);
            }
        }
    }

    // -------------------------------------------------------------------------
    // Scenario 9: Multiple Tool Calls in Sequence
    // Agent makes multiple sequential tool calls
    // Expected: each step recorded, all succeed
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn scenario_sequential_multi_step_tools() {
        let provider = ScenarioProvider::with_tools_and_text(vec![
            (
                Some(vec![crate::llm::ToolCall {
                    id: "s1".into(),
                    name: "search_files".into(),
                    arguments: serde_json::json!({"path": ".", "pattern": "*.rs"}),
                }]),
                Some("搜索 Rust 文件...".to_string()),
            ),
            (
                Some(vec![crate::llm::ToolCall {
                    id: "s2".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "src/main.rs"}),
                }]),
                Some("读取找到的文件...".to_string()),
            ),
            (
                Some(vec![crate::llm::ToolCall {
                    id: "s3".into(),
                    name: "create_goal".into(),
                    arguments: serde_json::json!({
                        "title": "Review Rust files",
                        "description": "Found 5 files to review"
                    }),
                }]),
                Some("创建任务来跟踪...".to_string()),
            ),
            (
                None,
                Some("完成了！找到 5 个 Rust 文件，并已创建审查任务。".to_string()),
            ),
        ]);

        let tools = vec![
            (
                Tool::new(
                    "search_files",
                    "Search files",
                    serde_json::json!({"type": "object", "properties": {}}),
                ),
                Arc::new(ScenarioToolHandler::new(
                    "search_files",
                    vec![ToolResult::success("search_files", "5 files")],
                )) as Arc<dyn ToolHandler>,
            ),
            (
                Tool::new(
                    "read_file",
                    "Read file",
                    serde_json::json!({"type": "object", "properties": {}}),
                ),
                Arc::new(ScenarioToolHandler::new(
                    "read_file",
                    vec![ToolResult::success("read_file", "content")],
                )) as Arc<dyn ToolHandler>,
            ),
            (
                Tool::new(
                    "create_goal",
                    "Create goal",
                    serde_json::json!({"type": "object", "properties": {}}),
                ),
                Arc::new(ScenarioToolHandler::new(
                    "create_goal",
                    vec![ToolResult::success("create_goal", "Goal created")],
                )) as Arc<dyn ToolHandler>,
            ),
        ];

        let runner = build_runner_with_tools(provider, tools);
        let result = runner
            .run("Find Rust files, read them, and create a review task", &[])
            .await;

        match result {
            AgentResponse::TextWithToolCalls { text, steps } => {
                assert_eq!(steps.len(), 3);
                assert_eq!(steps[0].tool_name, "search_files");
                assert_eq!(steps[1].tool_name, "read_file");
                assert_eq!(steps[2].tool_name, "create_goal");
                assert!(steps.iter().all(|s| s.success), "All steps should succeed");
                assert!(text.contains("5") || text.contains("完成"));
            }
            other => panic!("Expected TextWithToolCalls, got {:?}", other),
        }
    }
}

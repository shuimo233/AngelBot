//! AgentRunner tests with mock LLM provider.
//! Tests: text response, tool calling, max iterations, timeout, error handling.

#[cfg(test)]
mod tests {
    use crate::agent::config::*;
    use crate::agent::runner::*;
    use crate::agent::{Tool, ToolHandler, ToolResult};
    use crate::llm::{LlmProvider, LlmResponse, Message, ToolSchema};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// A mock LLM provider that returns pre-defined responses (must be Clone for the test).
    #[derive(Clone)]
    struct MockProvider {
        responses: Vec<Result<String, String>>,
        call_count: std::sync::Arc<AtomicUsize>,
    }

    impl MockProvider {
        fn new(responses: Vec<Result<String, String>>) -> Self {
            Self {
                responses,
                call_count: std::sync::Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    struct ToolCallProvider {
        call_count: std::sync::Arc<AtomicUsize>,
        tool_name: String,
        arguments: serde_json::Value,
    }

    impl ToolCallProvider {
        fn new() -> Self {
            Self::for_tool(
                "write_file",
                serde_json::json!({
                    "path": "note.txt",
                    "content": "hello"
                }),
            )
        }

        fn for_tool(tool_name: impl Into<String>, arguments: serde_json::Value) -> Self {
            Self {
                call_count: std::sync::Arc::new(AtomicUsize::new(0)),
                tool_name: tool_name.into(),
                arguments,
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            match self.responses.get(idx) {
                Some(Ok(text)) => Ok(LlmResponse::Text {
                    text: text.clone(),
                    stop_reason: crate::llm::StopReason::EndTurn,
                }),
                Some(Err(e)) => Err(e.clone()),
                None => Ok(LlmResponse::Text {
                    text: "mock fallback".into(),
                    stop_reason: crate::llm::StopReason::EndTurn,
                }),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for ToolCallProvider {
        fn name(&self) -> &str {
            "tool-call-mock"
        }
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            if idx > 0 {
                return Ok(LlmResponse::Text {
                    text: "Waiting for your approval.".to_string(),
                    stop_reason: crate::llm::StopReason::EndTurn,
                });
            }
            Ok(LlmResponse::ToolCalls {
                calls: vec![crate::llm::ToolCall {
                    id: format!("call-{}", self.tool_name),
                    name: self.tool_name.clone(),
                    arguments: self.arguments.clone(),
                }],
                text: Some(format!("I need to call {}.", self.tool_name)),
                stop_reason: crate::llm::StopReason::ToolUse,
            })
        }
    }

    struct MixedToolCallProvider {
        call_count: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for MixedToolCallProvider {
        fn name(&self) -> &str {
            "mixed-tool-call-mock"
        }

        async fn chat(
            &self,
            _messages: &[Message],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(LlmResponse::ToolCalls {
                calls: vec![
                    crate::llm::ToolCall {
                        id: "call-read".to_string(),
                        name: "read_file".to_string(),
                        arguments: serde_json::json!({ "path": "AGENTS.md" }),
                    },
                    crate::llm::ToolCall {
                        id: "call-write".to_string(),
                        name: "write_file".to_string(),
                        arguments: serde_json::json!({ "path": "report.md", "content": "report" }),
                    },
                ],
                text: Some("I will read, then request approval for writing.".to_string()),
                stop_reason: crate::llm::StopReason::ToolUse,
            })
        }
    }

    struct CountingHandler {
        name: String,
        calls: Arc<AtomicUsize>,
    }

    impl ToolHandler for CountingHandler {
        fn name(&self) -> &str {
            &self.name
        }

        fn execute(
            &self,
            _arguments: &serde_json::Value,
            _work_dir: &std::path::Path,
        ) -> ToolResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            ToolResult::success(&self.name, "executed")
        }
    }
    fn default_config() -> AgentConfig {
        AgentConfig {
            persona_name: "测试".into(),
            persona_description: "test desc".into(),
            persona_greeting: "hello".into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_agent_text_response() {
        let provider = MockProvider::new(vec![Ok("你好！".into())]);
        let config = default_config();
        let agent = AgentRunner::new(Box::new(provider), config);
        let result = agent.run("hello", &[]).await;
        match result {
            AgentResponse::Text(text) => assert_eq!(text, "你好！"),
            _ => panic!("Expected Text response"),
        }
    }

    #[tokio::test]
    async fn test_agent_error_handling() {
        // A transient provider failure is retried once before surfacing an
        // error to the caller.
        let provider = MockProvider::new(vec![
            Err("API rate limited".into()),
            Err("API rate limited".into()),
        ]);
        let config = default_config();
        let agent = AgentRunner::new(Box::new(provider), config);
        let result = agent.run("hello", &[]).await;
        match result {
            AgentResponse::Error(e) => assert!(e.contains("rate limited")),
            _ => panic!("Expected Error, got {:?}", result),
        }
    }

    #[tokio::test]
    async fn test_agent_with_history() {
        let provider = MockProvider::new(vec![Ok("记得之前聊的。".into())]);
        let config = default_config();
        let agent = AgentRunner::new(Box::new(provider), config);
        let history = vec![
            ("user".into(), "之前的问题".into()),
            ("assistant".into(), "之前的回答".into()),
        ];
        let result = agent.run("继续", &history).await;
        match result {
            AgentResponse::Text(text) => assert!(text.contains("记得")),
            _ => panic!("Expected Text"),
        }
    }

    #[tokio::test]
    async fn test_side_effect_tool_requires_confirmation() {
        let config = default_config();
        let provider = ToolCallProvider::new();
        let call_count = provider.call_count.clone();
        let agent = AgentRunner::new(Box::new(provider), config);
        let result = agent.run("write a note", &[]).await;
        match result {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                assert_eq!(steps.len(), 1);
                assert_eq!(steps[0].tool_name, "write_file");
                assert!(steps[0].confirmation_required);
                assert_eq!(steps[0].confirmation_status.as_deref(), Some("pending"));
                assert!(!steps[0].success);
            }
            other => panic!("Expected confirmation step, got {:?}", other),
        }
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "the runner must not request another model turn before confirmation"
        );
    }

    #[tokio::test]
    async fn update_memory_requires_explicit_confirmation() {
        let config = default_config();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut agent = AgentRunner::new(
            Box::new(ToolCallProvider::for_tool(
                "update_memory",
                serde_json::json!({
                    "memory_id": "memory-1",
                    "new_content": "Do not change this without approval"
                }),
            )),
            config,
        );
        agent.tool_registry_mut().register(
            Tool::new(
                "update_memory",
                "Update memory",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "memory_id": { "type": "string" },
                        "new_content": { "type": "string" }
                    },
                    "required": ["memory_id", "new_content"]
                }),
            ),
            Arc::new(CountingHandler {
                name: "update_memory".to_string(),
                calls: calls.clone(),
            }),
        );

        let result = agent.run("update my memory", &[]).await;
        match result {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                let step = steps
                    .iter()
                    .find(|step| step.tool_name == "update_memory")
                    .expect("the update_memory request should be represented in the audit steps");
                assert!(step.confirmation_required, "unexpected steps: {steps:#?}");
                assert_eq!(step.confirmation_status.as_deref(), Some("pending"));
                assert!(!step.success);
            }
            other => panic!("Expected confirmation step, got {:?}", other),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn model_cannot_self_approve_side_effect_tool() {
        let config = default_config();
        let agent = AgentRunner::new(
            Box::new(ToolCallProvider::for_tool(
                "write_file",
                serde_json::json!({
                    "path": "self-approved.txt",
                    "content": "must not be written",
                    "confirmed": true,
                    "_confirmed": true
                }),
            )),
            config,
        );

        let result = agent.run("write a note", &[]).await;
        match result {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                let rejected = steps
                    .iter()
                    .find(|step| step.tool_name == "write_file")
                    .unwrap();
                assert!(!rejected.confirmation_required);
                assert!(!rejected.success);
                assert!(rejected.output.contains("unexpected argument(s)"));
                assert!(!steps
                    .iter()
                    .any(|step| step.tool_name == "write_file" && step.success));
            }
            other => panic!("Expected rejected tool step, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_mixed_tool_batch_completes_read_only_calls_before_confirmation() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let config = default_config();
        let mut agent = AgentRunner::new(
            Box::new(MixedToolCallProvider {
                call_count: call_count.clone(),
            }),
            config,
        );
        agent.tool_registry_mut().register(
            Tool::new(
                "read_file",
                "Read a file",
                serde_json::json!({ "type": "object" }),
            ),
            Arc::new(CountingHandler {
                name: "read_file".to_string(),
                calls: Arc::new(AtomicUsize::new(0)),
            }),
        );

        let result = agent.run("read then write", &[]).await;
        match result {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                assert_eq!(steps.len(), 2);
                // Characterize the foreground host contract before routing
                // the batch through the shared boundary: safe work still
                // settles while a sibling side effect waits for the user.
                assert!(steps.iter().any(|step| {
                    step.call_id == "call-read"
                        && step.success
                        && !step.confirmation_required
                        && step.confirmation_status.is_none()
                }));
                assert!(steps.iter().any(|step| {
                    step.call_id == "call-write"
                        && !step.success
                        && step.confirmation_required
                        && step.confirmation_status.as_deref() == Some("pending")
                }));
            }
            other => panic!("Expected mixed tool result, got {:?}", other),
        }
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_create_directory_requires_confirmation() {
        let config = default_config();
        let agent = AgentRunner::new(
            Box::new(ToolCallProvider::for_tool(
                "create_directory",
                serde_json::json!({
                    "path": "notes"
                }),
            )),
            config,
        );
        let result = agent.run("create a notes directory", &[]).await;
        match result {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                assert_eq!(steps.len(), 1);
                assert_eq!(steps[0].tool_name, "create_directory");
                assert!(steps[0].confirmation_required);
                assert_eq!(steps[0].confirmation_status.as_deref(), Some("pending"));
                assert!(!steps[0].success);
                assert!(steps[0].output.contains("Confirmation required"));
            }
            other => panic!("Expected confirmation step, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_mcp_tool_requires_confirmation_without_execution() {
        let config = default_config();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut agent = AgentRunner::new(
            Box::new(ToolCallProvider::for_tool(
                "mcp_send_email",
                serde_json::json!({
                    "to": "user@example.com",
                    "body": "hello"
                }),
            )),
            config,
        );
        agent.tool_registry_mut().register(
            Tool::new(
                "mcp_send_email",
                "Send email through MCP",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "to": { "type": "string" },
                        "body": { "type": "string" }
                    },
                    "required": ["to", "body"]
                }),
            )
            .with_confirmation(true),
            Arc::new(CountingHandler {
                name: "mcp_send_email".to_string(),
                calls: calls.clone(),
            }),
        );

        let result = agent.run("send the email", &[]).await;
        match result {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                assert_eq!(steps.len(), 1);
                assert_eq!(steps[0].tool_name, "mcp_send_email");
                assert!(steps[0].confirmation_required);
                assert_eq!(steps[0].confirmation_status.as_deref(), Some("pending"));
                assert_eq!(calls.load(Ordering::SeqCst), 0);
            }
            other => panic!("Expected confirmation step, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_full_access_executes_an_admitted_mcp_tool() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut agent = AgentRunner::new(
            Box::new(ToolCallProvider::for_tool(
                "mcp_send_email",
                serde_json::json!({
                    "to": "user@example.com",
                    "body": "hello"
                }),
            )),
            default_config(),
        )
        .with_execution_permission("full_access".to_string());
        agent.tool_registry_mut().register(
            Tool::new(
                "mcp_send_email",
                "Send email through MCP",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "to": { "type": "string" },
                        "body": { "type": "string" }
                    },
                    "required": ["to", "body"]
                }),
            )
            .with_confirmation(true),
            Arc::new(CountingHandler {
                name: "mcp_send_email".to_string(),
                calls: calls.clone(),
            }),
        );

        let result = agent.run("send the email", &[]).await;
        match result {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                assert_eq!(steps.len(), 1);
                assert!(!steps[0].confirmation_required);
                assert_eq!(steps[0].confirmation_status, None);
                assert!(steps[0].success);
                assert_eq!(calls.load(Ordering::SeqCst), 1);
            }
            other => panic!("Expected confirmation step, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_full_access_does_not_bypass_a_protected_action() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut agent = AgentRunner::new(
            Box::new(ToolCallProvider::for_tool(
                "import_skill_from_github",
                serde_json::json!({
                    "repository_url": "https://github.com/example/skills"
                }),
            )),
            default_config(),
        )
        .with_execution_permission("full_access".to_string());
        agent.tool_registry_mut().register(
            Tool::new(
                "import_skill_from_github",
                "Install reusable guidance from a public GitHub repository",
                serde_json::json!({
                    "type": "object",
                    "properties": { "repository_url": { "type": "string" } },
                    "required": ["repository_url"]
                }),
            )
            .with_confirmation(true),
            Arc::new(CountingHandler {
                name: "import_skill_from_github".to_string(),
                calls: calls.clone(),
            }),
        );

        let result = agent.run("install this skill", &[]).await;
        match result {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                assert_eq!(steps.len(), 1);
                assert!(steps[0].confirmation_required);
                assert_eq!(steps[0].confirmation_status.as_deref(), Some("pending"));
                assert_eq!(calls.load(Ordering::SeqCst), 0);
            }
            other => panic!("Expected protected confirmation step, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_full_access_still_confirms_external_writes_and_unknown_control_risk() {
        for (name, input) in [
            (
                "prepare_message_draft",
                serde_json::json!({"app_id":"chat", "text":"private draft"}),
            ),
            (
                "set_trusted_app_text",
                serde_json::json!({"app_id":"chat", "field_ref":"field", "text":"private draft"}),
            ),
            (
                "operate_trusted_app_control",
                serde_json::json!({"app_id":"chat", "control_ref":"control", "action":"invoke"}),
            ),
            (
                "operate_trusted_app_control",
                serde_json::json!({"app_id":"chat", "control_ref":"control", "action":"select"}),
            ),
            (
                "operate_trusted_app_control",
                serde_json::json!({"app_id":"chat", "control_ref":"control", "action":"expand"}),
            ),
            (
                "operate_trusted_app_control",
                serde_json::json!({"app_id":"chat", "control_ref":"control", "action":"collapse"}),
            ),
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let mut agent = AgentRunner::new(
                Box::new(ToolCallProvider::for_tool(name, input.clone())),
                default_config(),
            )
            .with_execution_permission("full_access".to_string());
            agent.tool_registry_mut().register(
                Tool::new(
                    name,
                    "Perform an external action whose risk cannot be inferred from its UI label",
                    serde_json::json!({
                        "type": "object",
                        "properties": input.as_object().unwrap().keys().map(|key| (key.clone(), serde_json::json!({"type":"string"}))).collect::<serde_json::Map<_,_>>(),
                        "required": input.as_object().unwrap().keys().cloned().collect::<Vec<_>>(),
                        "additionalProperties": false,
                    }),
                )
                .with_confirmation(true),
                Arc::new(CountingHandler {
                    name: name.to_string(),
                    calls: calls.clone(),
                }),
            );

            let result = agent.run("prepare my draft", &[]).await;
            match result {
                AgentResponse::TextWithToolCalls { steps, .. } => {
                    assert_eq!(steps.len(), 1);
                    assert!(steps[0].confirmation_required);
                    assert_eq!(steps[0].confirmation_status.as_deref(), Some("pending"));
                    assert_eq!(calls.load(Ordering::SeqCst), 0);
                }
                other => panic!("Expected draft confirmation step, got {:?}", other),
            }
        }
    }

    #[tokio::test]
    async fn test_workspace_auto_executes_an_admitted_mcp_tool() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut agent = AgentRunner::new(
            Box::new(ToolCallProvider::for_tool(
                "mcp_search_project",
                serde_json::json!({ "query": "architecture" }),
            )),
            default_config(),
        )
        .with_execution_permission("workspace_auto".to_string());
        agent.tool_registry_mut().register(
            Tool::new(
                "mcp_search_project",
                "Search through the workspace-approved MCP service",
                serde_json::json!({
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"]
                }),
            )
            .with_confirmation(true),
            Arc::new(CountingHandler {
                name: "mcp_search_project".to_string(),
                calls: calls.clone(),
            }),
        );

        let result = agent.run("search the project", &[]).await;
        match result {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                assert_eq!(steps.len(), 1);
                assert!(!steps[0].confirmation_required);
                assert!(steps[0].success);
                assert_eq!(calls.load(Ordering::SeqCst), 1);
            }
            other => panic!("Expected tool execution, got {:?}", other),
        }
    }

    #[test]
    fn task_facts_preserve_verification_and_no_progress_evidence() {
        let steps = vec![
            AgentStep {
                call_id: "write-1".to_string(),
                tool_name: "write_file".to_string(),
                arguments: serde_json::json!({"path":"src/lib.rs"}),
                success: true,
                output: "written".to_string(),
                confirmation_required: false,
                confirmation_status: None,
                verification: None,
            },
            AgentStep {
                call_id: "verify-1".to_string(),
                tool_name: "run_project_command".to_string(),
                arguments: serde_json::json!({"command":"cargo test --lib"}),
                success: true,
                output: "passed".to_string(),
                confirmation_required: false,
                confirmation_status: None,
                verification: None,
            },
            AgentStep {
                call_id: "no-progress-1".to_string(),
                tool_name: "agent_loop_no_progress".to_string(),
                arguments: serde_json::json!({"count":2,"signature":"read_file:{}"}),
                success: false,
                output: "No new facts".to_string(),
                confirmation_required: false,
                confirmation_status: None,
                verification: None,
            },
        ];

        let facts = task_facts_from_steps("修改项目并运行测试", &steps);

        assert_eq!(facts.modified_files, vec!["src/lib.rs"]);
        assert_eq!(facts.verification_evidence.len(), 1);
        assert_eq!(facts.verification_evidence[0].command, "cargo test --lib");
        assert_eq!(facts.no_progress_count, 2);
        assert_eq!(
            facts.terminal_reason,
            Some(crate::agent::task_facts::TaskTerminalReason::NeedsAttention)
        );
    }

    // ── Issue #079: Tool-call truncation guard ─────────────────────────────────

    /// Mock provider that returns tool calls with a configurable stop_reason.
    #[derive(Clone)]
    struct TruncationProvider {
        stop_reason: crate::llm::StopReason,
        call_count: Arc<AtomicUsize>,
    }

    impl TruncationProvider {
        fn new(stop_reason: crate::llm::StopReason) -> Self {
            Self {
                stop_reason,
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for TruncationProvider {
        fn name(&self) -> &str {
            "truncation-mock"
        }
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            if idx > 0 {
                // After the first turn the run should have already terminated.
                return Ok(LlmResponse::Text {
                    text: "should not be reached".into(),
                    stop_reason: crate::llm::StopReason::EndTurn,
                });
            }
            Ok(LlmResponse::ToolCalls {
                calls: vec![crate::llm::ToolCall {
                    id: "call-write".to_string(),
                    name: "write_file".to_string(),
                    arguments: serde_json::json!({
                        "path": "truncated.txt",
                        "content": "incomplete"
                    }),
                }],
                text: Some("I need to write this file.".to_string()),
                stop_reason: self.stop_reason,
            })
        }
    }

    #[tokio::test]
    async fn truncation_guard_skips_tool_execution_on_max_tokens() {
        let handler_calls = Arc::new(AtomicUsize::new(0));
        let mut agent = AgentRunner::new(
            Box::new(TruncationProvider::new(crate::llm::StopReason::MaxTokens)),
            default_config(),
        );
        agent.tool_registry_mut().register(
            Tool::new(
                "write_file",
                "Write a file",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"]
                }),
            ),
            Arc::new(CountingHandler {
                name: "write_file".to_string(),
                calls: handler_calls.clone(),
            }),
        );

        let result = agent.run("write a file", &[]).await;

        // No handler should have been called.
        assert_eq!(
            handler_calls.load(Ordering::SeqCst),
            0,
            "Tool handler must not execute when stop_reason is MaxTokens"
        );

        // The run should terminate with a descriptive error.
        let text = match &result {
            AgentResponse::Text(t) => t.clone(),
            AgentResponse::Error(e) => e.clone(),
            AgentResponse::TextWithToolCalls { text, .. } => text.clone(),
        };
        assert!(
            text.contains("token 限制截断"),
            "Error message must mention truncation: {text}"
        );
    }

    #[tokio::test]
    async fn truncation_guard_records_truncated_steps_in_agent_steps() {
        let handler_calls = Arc::new(AtomicUsize::new(0));
        let mut agent = AgentRunner::new(
            Box::new(TruncationProvider::new(crate::llm::StopReason::MaxTokens)),
            default_config(),
        );
        agent.tool_registry_mut().register(
            Tool::new(
                "write_file",
                "Write a file",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"]
                }),
            ),
            Arc::new(CountingHandler {
                name: "write_file".to_string(),
                calls: handler_calls.clone(),
            }),
        );

        let result = agent.run("write a file", &[]).await;

        let steps = match &result {
            AgentResponse::TextWithToolCalls { steps, .. } => steps.clone(),
            AgentResponse::Error(_) => vec![],
            AgentResponse::Text(_) => vec![],
        };

        assert_eq!(
            steps.len(),
            2,
            "One truncation step + one agent_loop synthetic step"
        );
        let step = &steps[0];
        assert_eq!(step.tool_name, "write_file");
        assert!(!step.success, "Step success must be false");
        assert!(
            step.output.contains("截断"),
            "Step output must mention truncation: {}",
            step.output
        );
        assert!(
            !step.confirmation_required,
            "Truncation steps do not require confirmation"
        );
        // Second step is the agent_loop synthetic termination step.
        assert_eq!(steps[1].tool_name, "agent_loop");
        assert!(!steps[1].success);
    }

    #[tokio::test]
    async fn truncation_guard_preserves_end_turn_behavior() {
        // When stop_reason is ToolUse (not MaxTokens), the existing confirmation
        // flow must not change. This is a backward-compatibility anchor.
        let handler_calls = Arc::new(AtomicUsize::new(0));
        let mut agent = AgentRunner::new(
            Box::new(TruncationProvider::new(crate::llm::StopReason::ToolUse)),
            default_config(),
        );
        agent.tool_registry_mut().register(
            Tool::new(
                "write_file",
                "Write a file",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"]
                }),
            ),
            Arc::new(CountingHandler {
                name: "write_file".to_string(),
                calls: handler_calls.clone(),
            }),
        );

        let result = agent.run("write a file", &[]).await;

        // Handler should NOT be called (write_file requires confirmation).
        assert_eq!(
            handler_calls.load(Ordering::SeqCst),
            0,
            "Side-effect tools must still require confirmation"
        );

        // Should be a TextWithToolCalls with confirmation pending.
        match &result {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                assert_eq!(steps.len(), 1);
                assert!(steps[0].confirmation_required);
                assert_eq!(
                    steps[0].confirmation_status.as_deref(),
                    Some("pending"),
                    "ToolUse stop_reason must preserve confirmation-pending behavior"
                );
            }
            other => panic!("Expected TextWithToolCalls, got {:?}", other),
        }
    }

    // ── Issue #081: Soft finish hint injection ──────────────────────────────────

    /// Provider that returns EndTurn on first call — used to verify no hint is
    /// injected when the loop exits before the soft-finish threshold.
    #[derive(Clone)]
    struct EndTurnProvider {
        call_count: Arc<AtomicUsize>,
        messages_per_call: Arc<Mutex<Vec<usize>>>,
    }

    impl EndTurnProvider {
        fn new() -> Self {
            Self {
                call_count: Arc::new(AtomicUsize::new(0)),
                messages_per_call: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for EndTurnProvider {
        fn name(&self) -> &str {
            "end-turn-mock"
        }
        async fn chat(
            &self,
            messages: &[Message],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            let _idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut guard) = self.messages_per_call.lock() {
                guard.push(messages.len());
            }
            Ok(LlmResponse::Text {
                text: "done".into(),
                stop_reason: crate::llm::StopReason::EndTurn,
            })
        }
    }

    /// Provider that returns ToolCalls up to `max_tool_calls` times, then returns
    /// Text (allowing the agent to reach the soft-finish threshold without hitting
    /// the no-progress guard). Used for Issue #081 tests.
    #[derive(Clone)]
    struct ToolCallCountingProvider {
        call_count: Arc<AtomicUsize>,
        messages_per_call: Arc<Mutex<Vec<usize>>>,
        /// Stop returning ToolCalls after this many calls to avoid the
        /// no-progress guard while still reaching the soft-finish threshold.
        max_tool_calls: usize,
    }

    impl ToolCallCountingProvider {
        fn new(max_tool_calls: usize) -> Self {
            Self {
                call_count: Arc::new(AtomicUsize::new(0)),
                messages_per_call: Arc::new(Mutex::new(Vec::new())),
                max_tool_calls,
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for ToolCallCountingProvider {
        fn name(&self) -> &str {
            "tool-call-counting-mock"
        }
        async fn chat(
            &self,
            messages: &[Message],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut guard) = self.messages_per_call.lock() {
                guard.push(messages.len());
            }
            if idx < self.max_tool_calls {
                Ok(LlmResponse::ToolCalls {
                    calls: vec![crate::llm::ToolCall {
                        id: format!("call-{}", idx),
                        name: "read_file".into(),
                        arguments: serde_json::json!({ "path": format!("file{}.txt", idx) }),
                    }],
                    text: Some(format!("Reading (call {})", idx)),
                    stop_reason: crate::llm::StopReason::ToolUse,
                })
            } else {
                Ok(LlmResponse::Text {
                    text: "done".into(),
                    stop_reason: crate::llm::StopReason::EndTurn,
                })
            }
        }
    }

    #[tokio::test]
    async fn soft_finish_hint_not_injected_before_threshold() {
        // The loop exits after iteration 0 (EndTurn response) — well before
        // iteration 8 where SOFT_FINISH_REMAINING=2 would trigger.
        let provider = EndTurnProvider::new();
        let mut agent = AgentRunner::new(Box::new(provider.clone()), default_config());
        agent.tool_registry_mut().register(
            Tool::new(
                "read_file",
                "Read a file",
                serde_json::json!({"type": "object"}),
            ),
            Arc::new(CountingHandler {
                name: "read_file".into(),
                calls: Arc::new(AtomicUsize::new(0)),
            }),
        );

        let result = agent.run("read a file", &[]).await;
        match &result {
            AgentResponse::Text(t) => assert!(t.contains("done")),
            other => panic!("Expected Text, got {:?}", other),
        }
        assert_eq!(
            provider.call_count.load(Ordering::SeqCst),
            1,
            "EndTurn provider should produce exactly one LLM call"
        );
        let msgs = provider.messages_per_call.lock().unwrap();
        let first_msg_count = *msgs.get(0).unwrap_or(&0);
        // build_initial_messages produces: [system_prompt, user_message] = 2 messages.
        // No soft-finish hint should be injected at iteration 0.
        assert_eq!(
            first_msg_count, 2,
            "First call should have 2 messages (system + user); no hint at iteration 0"
        );
    }

    #[tokio::test]
    async fn long_run_completes_without_hard_limit() {
        // With infinite iterations, the runner should continue until EndTurn
        let provider = ToolCallCountingProvider::new(20);
        let mut agent = AgentRunner::new(Box::new(provider.clone()), default_config());
        agent.tool_registry_mut().register(
            Tool::new(
                "read_file",
                "Read a file",
                serde_json::json!({"type": "object"}),
            ),
            Arc::new(CountingHandler {
                name: "read_file".into(),
                calls: Arc::new(AtomicUsize::new(0)),
            }),
        );

        let result = agent.run("keep reading", &[]).await;
        match &result {
            AgentResponse::TextWithToolCalls { .. } | AgentResponse::Error(_) => {}
            other => panic!("Expected terminal response, got {:?}", other),
        }

        let msgs = provider.messages_per_call.lock().unwrap();
        assert!(
            msgs.len() >= 15,
            "Runner should make >=15 LLM calls (no hard limit)"
        );
    }

    // ── Issue #082: terminate_batch soft-termination ───────────────────────────

    struct TerminateBatchHandler {
        call_count: Arc<AtomicUsize>,
    }

    impl TerminateBatchHandler {
        fn new() -> Self {
            Self {
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl ToolHandler for TerminateBatchHandler {
        fn name(&self) -> &str {
            "terminate_batch_tool"
        }
        fn execute(&self, _args: &serde_json::Value, _work_dir: &std::path::Path) -> ToolResult {
            let count = self.call_count.fetch_add(1, Ordering::SeqCst);
            if count == 0 {
                ToolResult::terminate_batch(self.name(), "intentional batch termination")
            } else {
                ToolResult::success(self.name(), "should not reach")
            }
        }
    }

    struct TerminateBatchLLMProvider {
        call_count: Arc<AtomicUsize>,
    }

    impl TerminateBatchLLMProvider {
        fn new() -> Self {
            Self {
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for TerminateBatchLLMProvider {
        fn name(&self) -> &str {
            "terminate-batch-llm"
        }
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            if idx > 0 {
                return Ok(LlmResponse::Text {
                    text: "done".into(),
                    stop_reason: crate::llm::StopReason::EndTurn,
                });
            }
            Ok(LlmResponse::ToolCalls {
                calls: vec![
                    crate::llm::ToolCall {
                        id: "call-1".into(),
                        name: "terminate_batch_tool".into(),
                        arguments: serde_json::json!({}),
                    },
                    crate::llm::ToolCall {
                        id: "call-2".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "x.txt"}),
                    },
                    crate::llm::ToolCall {
                        id: "call-3".into(),
                        name: "write_file".into(),
                        arguments: serde_json::json!({"path": "y.txt", "content": "hi"}),
                    },
                ],
                text: Some("Calling three tools.".into()),
                stop_reason: crate::llm::StopReason::ToolUse,
            })
        }
    }

    #[tokio::test]
    async fn terminate_batch_skips_remaining_tools() {
        let handler_calls = Arc::new(AtomicUsize::new(0));
        let mut agent =
            AgentRunner::new(Box::new(TerminateBatchLLMProvider::new()), default_config());
        agent.tool_registry_mut().register(
            Tool::new(
                "terminate_batch_tool",
                "Test",
                serde_json::json!({"type": "object"}),
            ),
            Arc::new(TerminateBatchHandler::new()),
        );
        agent.tool_registry_mut().register(
            Tool::new(
                "read_file",
                "Read a file",
                serde_json::json!({"type": "object"}),
            ),
            Arc::new(CountingHandler {
                name: "read_file".into(),
                calls: handler_calls.clone(),
            }),
        );
        agent.tool_registry_mut().register(
            Tool::new(
                "write_file",
                "Write a file",
                serde_json::json!({
                    "type": "object",
                    "properties": {"path": {"type": "string"}, "content": {"type": "string"}},
                    "required": ["path", "content"]
                }),
            ),
            Arc::new(CountingHandler {
                name: "write_file".into(),
                calls: Arc::new(AtomicUsize::new(0)),
            }),
        );

        let result = agent.run("run three tools", &[]).await;

        match &result {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                // find terminate_batch_tool step
                let tb_step = steps
                    .iter()
                    .find(|s| s.tool_name == "terminate_batch_tool")
                    .expect("terminate_batch_tool should be in steps");
                assert!(!tb_step.success, "terminate_batch_tool should fail");
                assert!(
                    tb_step.output.contains("batch"),
                    "terminate_batch_tool output should mention batch termination"
                );
                // read_file should be skipped
                let skipped: Vec<_> = steps.iter().filter(|s| s.output.contains("跳过")).collect();
                assert!(
                    skipped.iter().any(|s| s.tool_name == "read_file"),
                    "read_file should be skipped"
                );
            }
            other => panic!("Expected TextWithToolCalls, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn terminate_batch_preserves_default_behavior() {
        // Provider returns 1 tool call then EndTurn — no batch termination.
        let provider = ToolCallCountingProvider::new(1);
        let handler_calls = Arc::new(AtomicUsize::new(0));
        let mut agent = AgentRunner::new(Box::new(provider.clone()), default_config());
        agent.tool_registry_mut().register(
            Tool::new(
                "read_file",
                "Read a file",
                serde_json::json!({"type": "object"}),
            ),
            Arc::new(CountingHandler {
                name: "read_file".into(),
                calls: handler_calls.clone(),
            }),
        );

        let result = agent.run("read", &[]).await;
        assert_eq!(
            handler_calls.load(Ordering::SeqCst),
            1,
            "read_file should have been called once"
        );
        match &result {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                assert!(
                    !steps.iter().any(|s| s.output.contains("跳过")),
                    "No tool should be skipped when terminate_batch is not set"
                );
            }
            other => panic!("Expected TextWithToolCalls, got {:?}", other),
        }
    }

    // ── Issue #096: skip-recording deduplication ─────────────────────────────────

    /// Provider returning one parallel `terminate_batch_tool` plus one
    /// sequential follow-up. Exercises the path where the parallel branch
    /// pre-skip and the sequential branch must not double-record the step.
    #[derive(Clone)]
    struct ParallelTerminateProvider {
        call_count: Arc<AtomicUsize>,
    }

    impl ParallelTerminateProvider {
        fn new() -> Self {
            Self {
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for ParallelTerminateProvider {
        fn name(&self) -> &str {
            "parallel-terminate-mock"
        }
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            if idx > 0 {
                return Ok(LlmResponse::Text {
                    text: "done".into(),
                    stop_reason: crate::llm::StopReason::EndTurn,
                });
            }
            // First call: parallel + sequential mix. Use sequential-mode
            // execution for both so the test exercises the documented
            // mixed-batch code path in #082 + #096.
            Ok(LlmResponse::ToolCalls {
                calls: vec![
                    crate::llm::ToolCall {
                        id: "call-tb".into(),
                        name: "terminate_batch_tool".into(),
                        arguments: serde_json::json!({}),
                    },
                    crate::llm::ToolCall {
                        id: "call-r".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "x.txt"}),
                    },
                ],
                text: Some("two tools".into()),
                stop_reason: crate::llm::StopReason::ToolUse,
            })
        }
    }

    #[tokio::test]
    async fn skip_recording_does_not_duplicate_when_terminate_signals_late() {
        // Regression: #082 + #096 combined. The first tool in the batch
        // signals terminate_batch on a sequential call. The remaining
        // call must produce exactly one skipped step — not two.
        let read_calls = Arc::new(AtomicUsize::new(0));
        let mut agent =
            AgentRunner::new(Box::new(ParallelTerminateProvider::new()), default_config());
        agent.tool_registry_mut().register(
            Tool::new(
                "terminate_batch_tool",
                "Test",
                serde_json::json!({"type": "object"}),
            ),
            Arc::new(TerminateBatchHandler::new()),
        );
        agent.tool_registry_mut().register(
            Tool::new(
                "read_file",
                "Read a file",
                serde_json::json!({"type": "object"}),
            ),
            Arc::new(CountingHandler {
                name: "read_file".into(),
                calls: read_calls.clone(),
            }),
        );

        let result = agent.run("two tools", &[]).await;

        match &result {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                let skipped_reads = steps
                    .iter()
                    .filter(|s| s.tool_name == "read_file" && s.output.contains("跳过"))
                    .count();
                assert_eq!(
                    skipped_reads, 1,
                    "read_file must produce exactly one skipped step, found {}",
                    skipped_reads
                );
                // The skipped handler must never have executed the read.
                assert_eq!(
                    read_calls.load(Ordering::SeqCst),
                    0,
                    "read_file handler must not run after terminate_batch"
                );
            }
            other => panic!("Expected TextWithToolCalls, got {:?}", other),
        }
    }

    // ── Issue #096: Phase 10 guard integration tests ────────────────────────────

    /// Provider that always requests `write_file` and never produces a
    /// natural EndTurn. Used to verify side-effect guards.
    #[derive(Clone)]
    struct SideEffectStormProvider {
        call_count: Arc<AtomicUsize>,
    }

    impl SideEffectStormProvider {
        fn new() -> Self {
            Self {
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for SideEffectStormProvider {
        fn name(&self) -> &str {
            "side-effect-storm"
        }
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(LlmResponse::ToolCalls {
                calls: vec![crate::llm::ToolCall {
                    id: format!("call-write-{}", idx),
                    name: "write_file".into(),
                    arguments: serde_json::json!({
                        "path": format!("note-{}.txt", idx),
                        "content": "hello"
                    }),
                }],
                text: Some(format!("step {}", idx)),
                stop_reason: crate::llm::StopReason::ToolUse,
            })
        }
    }

    /// Records every tool call without requiring confirmation.
    struct NoConfirmWriteHandler {
        calls: Arc<AtomicUsize>,
    }

    impl ToolHandler for NoConfirmWriteHandler {
        fn name(&self) -> &str {
            "write_file"
        }
        fn execute(
            &self,
            _arguments: &serde_json::Value,
            _work_dir: &std::path::Path,
        ) -> ToolResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            ToolResult::success("write_file", "ok")
        }
    }

    /// Mock guard that always terminates. Used to verify runner wires
    /// guards in without depending on real timing or side-effect counting.
    struct AlwaysTerminateGuard;
    impl crate::agent::guards::LoopGuard for AlwaysTerminateGuard {
        fn name(&self) -> &'static str {
            "always_terminate"
        }
        fn evaluate(
            &self,
            _ctx: &crate::agent::guards::GuardContext,
        ) -> crate::agent::guards::GuardVerdict {
            crate::agent::guards::GuardVerdict::Terminate {
                reason: "always".into(),
                branch: crate::agent::lifecycle::TerminalBranchKind::WallClockTimeout,
            }
        }
    }

    /// Mock guard that always pauses for user input. Used to verify
    /// soft pause does not emit AgentEnd.
    struct AlwaysPauseGuard;
    impl crate::agent::guards::LoopGuard for AlwaysPauseGuard {
        fn name(&self) -> &'static str {
            "always_pause"
        }
        fn evaluate(
            &self,
            _ctx: &crate::agent::guards::GuardContext,
        ) -> crate::agent::guards::GuardVerdict {
            crate::agent::guards::GuardVerdict::PauseForUser {
                reason: "always".into(),
            }
        }
    }

    #[tokio::test]
    async fn runner_emits_agent_end_when_hard_guard_terminates() {
        // A hard-terminating guard must drive the runner to a clean close:
        // AgentEnd is emitted and the response carries a synthetic agent_loop
        // step the UI can render as "stopped by safety guard".
        let agent = AgentRunner::new(Box::new(EndTurnProvider::new()), default_config())
            .add_guard(std::sync::Arc::new(AlwaysTerminateGuard));

        let result = agent.run("hello", &[]).await;

        match result {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                assert!(
                    steps.iter().any(|s| s.tool_name == "agent_loop"),
                    "Hard guard termination must record an agent_loop synthetic step"
                );
            }
            other => panic!(
                "Expected TextWithToolCalls after guard termination, got {:?}",
                other
            ),
        }
    }

    #[tokio::test]
    async fn runner_does_not_emit_agent_end_when_soft_guard_pauses() {
        // Soft pause (PauseForUser) must NOT emit AgentEnd — the run stays
        // open so the user can resume by sending a new prompt.
        let agent = AgentRunner::new(Box::new(EndTurnProvider::new()), default_config())
            .add_guard(std::sync::Arc::new(AlwaysPauseGuard));

        let result = agent.run("hello", &[]).await;

        match result {
            AgentResponse::TextWithToolCalls { .. } => {
                // No agent_loop synthetic step: this is a soft pause.
            }
            other => panic!(
                "Expected TextWithToolCalls after soft pause, got {:?}",
                other
            ),
        }
    }

    #[tokio::test]
    async fn side_effect_guard_pauses_after_threshold_in_full_access_mode() {
        // In full_access mode write_file does not require confirmation. The
        // SideEffectAutoPauseGuard (threshold=3) must soft-pause after the
        // 3rd side-effect call, so the 4th iteration is never requested.
        let write_calls = Arc::new(AtomicUsize::new(0));
        let mut agent =
            AgentRunner::new(Box::new(SideEffectStormProvider::new()), default_config());
        agent = agent.with_execution_permission("full_access".into());
        agent.tool_registry_mut().register(
            Tool::new(
                "write_file",
                "Write a file",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "content": {"type": "string"}
                    },
                    "required": ["path", "content"]
                }),
            ),
            Arc::new(NoConfirmWriteHandler {
                calls: write_calls.clone(),
            }),
        );
        agent = agent.add_guard(std::sync::Arc::new(
            crate::agent::guards::SideEffectAutoPauseGuard::new(3),
        ));

        let result = agent.run("write a lot", &[]).await;

        // Three successful writes were executed before the guard paused.
        assert_eq!(
            write_calls.load(Ordering::SeqCst),
            3,
            "SideEffectAutoPause must pause at exactly the threshold"
        );
        // The provider is called once per iteration; we expect 4 (3 tool
        // rounds + 1 where the guard fires before the LLM call).
        match result {
            AgentResponse::TextWithToolCalls { steps, .. } => {
                let writes = steps
                    .iter()
                    .filter(|s| s.tool_name == "write_file" && s.success)
                    .count();
                assert_eq!(writes, 3, "Three successful write steps expected");
            }
            other => panic!(
                "Expected TextWithToolCalls after side-effect pause, got {:?}",
                other
            ),
        }
    }

    // ── Issue #098: default guard registration ────────────────────────────────
    // Phase 10 delivered the LoopGuard *mechanism* (trait, three impls,
    // runner integration) but never wired it into the production path.
    // `AgentRunner::new()` produced an empty guard list, so the only
    // safety wall users hit was a hard `MAX_TOOL_CALLS_PER_TURN` cap.
    // The hard cap has been removed in favour of Pi-style soft-only
    // termination: `WallClockGuard` (wall-clock budget) +
    // `NetProgressGuard` (no-progress iterations) +
    // `SideEffectAutoPauseGuard` (irreversible actions) — all three must
    // be registered by default and they must trip before any silent
    // truncation.

    #[test]
    fn default_runner_includes_three_guards() {
        let provider = MockProvider::new(vec![Ok("done".into())]);
        let agent = AgentRunner::new(Box::new(provider), AgentConfig::default());
        assert_eq!(
            agent.guards().len(),
            3,
            "AgentRunner::new() must register WallClock + NetProgress + SideEffectAutoPause by default"
        );
        let names: Vec<&'static str> = agent.guards().iter().map(|g| g.name()).collect();
        assert!(
            names.contains(&"wall_clock"),
            "Default guards must include wall_clock"
        );
        assert!(
            names.contains(&"net_progress"),
            "Default guards must include net_progress"
        );
        assert!(
            names.contains(&"side_effect_auto_pause"),
            "Default guards must include side_effect_auto_pause"
        );
    }

    #[test]
    fn with_default_guards_off_empties_the_guard_list() {
        let provider = MockProvider::new(vec![Ok("done".into())]);
        let agent =
            AgentRunner::new(Box::new(provider), AgentConfig::default()).with_default_guards_off();
        assert!(
            agent.guards().is_empty(),
            "with_default_guards_off() must clear the default guard list"
        );
    }

    /// Provider that keeps requesting `write_file` (a side-effect tool)
    /// forever, varying the path so each batch is *not* a duplicate of the
    /// previous one. With the default `SideEffectAutoPauseGuard`
    /// (threshold = 8), the runner must soft-pause for user input well
    /// before the wall-clock guard terminates the run.
    #[derive(Clone)]
    struct ForeverWriteProvider {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for ForeverWriteProvider {
        fn name(&self) -> &str {
            "forever-write"
        }
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(LlmResponse::ToolCalls {
                calls: vec![crate::llm::ToolCall {
                    id: format!("call-{}", n),
                    name: "write_file".into(),
                    // Vary the path so each batch is unique — the
                    // runner's "repeated batch" detector must not fire
                    // ahead of the soft guard.
                    arguments: serde_json::json!({
                        "path": format!("note-{}", n),
                        "content": "hi"
                    }),
                }],
                text: Some("again".into()),
                stop_reason: crate::llm::StopReason::ToolUse,
            })
        }
    }

    #[tokio::test]
    async fn default_side_effect_guard_pauses_before_hard_cap() {
        // The default `SideEffectAutoPauseGuard` (threshold = 8) must
        // pause the run after ~8 side-effects. There is no longer a
        // hard `MAX_TOOL_CALLS_PER_TURN` cap — termination is governed
        // by soft guards, with the wall-clock guard as the absolute
        // backstop. This is the production-path proof that #096's
        // mechanism actually fires.
        let provider = ForeverWriteProvider {
            calls: Arc::new(AtomicUsize::new(0)),
        };

        let mut agent = AgentRunner::new(Box::new(provider), AgentConfig::default())
            .with_execution_permission("full_access".into());
        agent.tool_registry_mut().register(
            Tool::new("write_file", "Write", serde_json::json!({"type": "object"})),
            Arc::new(NoConfirmWriteHandler {
                calls: Arc::new(AtomicUsize::new(0)),
            }),
        );
        // Note: no `.with_default_guards_off()` — we explicitly want the defaults.

        let result = agent.run("write forever", &[]).await;
        // The default guard should pause (PauseForUser) rather than the
        // wall-clock backstop terminating. A paused run surfaces as
        // `TextWithToolCalls` (not `Error`) and produces a clearly-named
        // step the UI can render as "paused by safety guard".
        match result {
            AgentResponse::TextWithToolCalls { text, steps } => {
                // Either the net-progress guard or the side-effect guard
                // must trip — both produce the "已暂停" prelude.
                assert!(
                    text.contains("暂停"),
                    "Soft pause text expected, got: {}",
                    text
                );
                let writes = steps
                    .iter()
                    .filter(|s| s.tool_name == "write_file" && s.success)
                    .count();
                // The side-effect guard fires at threshold = 8; allow
                // for batched execution to land us slightly under the
                // wall-clock budget. The contract is "well under 100".
                assert!(
                    writes < DEFAULT_SIDE_EFFECT_THRESHOLD + 32,
                    "Soft guard must pause well before the wall-clock backstop (got {} writes)",
                    writes
                );
            }
            AgentResponse::Error(reason) => {
                panic!(
                    "Default guard should soft-pause, not hard-terminate (got error: {})",
                    reason
                )
            }
            other => panic!("Unexpected AgentResponse: {:?}", other),
        }
    }

    // Issue #102: collect_unverified_artifacts picks up steps whose
    // post-call verifier said the disk disagrees, ignores everything else.
    // It is the soft-warning source for the completion gate.
    #[test]
    fn collect_unverified_artifacts_lists_only_mismatches() {
        use crate::agent::verifier::{VerificationEvidence, VerificationSource};
        let matched = AgentStep {
            call_id: "w".into(),
            tool_name: "write_file".into(),
            arguments: serde_json::json!({"path": "a.txt"}),
            success: true,
            output: "ok".into(),
            confirmation_required: false,
            confirmation_status: None,
            verification: Some(VerificationEvidence {
                kind: "file_exists",
                target: "a.txt".into(),
                declared_existence: true,
                actual_existence: true,
                source: VerificationSource::WorkDir,
                matched: true,
                note: String::new(),
            }),
            ..Default::default()
        };
        let mismatched = AgentStep {
            call_id: "w2".into(),
            tool_name: "write_file".into(),
            arguments: serde_json::json!({"path": "ghost.txt"}),
            success: true,
            output: "ok".into(),
            confirmation_required: false,
            confirmation_status: None,
            verification: Some(VerificationEvidence {
                kind: "file_exists",
                target: "ghost.txt".into(),
                declared_existence: true,
                actual_existence: false,
                source: VerificationSource::WorkDir,
                matched: false,
                note: "工具声称成功，但磁盘上未找到产物".into(),
            }),
            ..Default::default()
        };
        let no_verifier = AgentStep {
            call_id: "w3".into(),
            tool_name: "think".into(),
            arguments: serde_json::json!({}),
            success: true,
            output: "ok".into(),
            confirmation_required: false,
            confirmation_status: None,
            verification: None,
            ..Default::default()
        };
        let list = collect_unverified_artifacts(&[matched, mismatched, no_verifier]);
        assert_eq!(
            list.len(),
            1,
            "exactly one mismatch should be listed: {:?}",
            list
        );
        assert!(list[0].contains("ghost.txt"));
        assert!(list[0].contains("write_file"));
    }

    #[test]
    fn collect_unverified_artifacts_empty_when_all_match() {
        let step = AgentStep {
            call_id: "w".into(),
            tool_name: "write_file".into(),
            arguments: serde_json::json!({"path": "a.txt"}),
            success: true,
            output: "ok".into(),
            confirmation_required: false,
            confirmation_status: None,
            verification: None,
            ..Default::default()
        };
        let list = collect_unverified_artifacts(&[step]);
        assert!(list.is_empty());
    }
}

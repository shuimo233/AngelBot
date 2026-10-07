//! Tests for all tool handlers: file, memory, goal, constraint, verify, think.
#[cfg(test)]
mod tests {
    use crate::agent::handlers::*;
    use crate::agent::tool::{ToolCall, ToolHandler};
    use crate::agent::ToolRegistry;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    fn db() -> (rusqlite::Connection, Arc<Mutex<rusqlite::Connection>>) {
        // Register sqlite-vec globally
        #[cfg(not(test))]
        unsafe {
            crate::vector_store::load_extension_globally();
        }
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            PRAGMA foreign_keys = OFF;
            CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY, scope TEXT, category TEXT, content TEXT,
                importance INTEGER DEFAULT 5, source TEXT DEFAULT 'test',
                frequency INTEGER DEFAULT 0, is_permanent INTEGER DEFAULT 0,
                forget_stage TEXT DEFAULT 'active', created_at INTEGER, updated_at INTEGER,
                priority TEXT DEFAULT 'fact', deleted INTEGER DEFAULT 0,
                superseded_by TEXT
            );
            CREATE TABLE IF NOT EXISTS agent_goals (
                id TEXT PRIMARY KEY, session_id TEXT, goal_text TEXT,
                status TEXT DEFAULT 'pending', progress_pct INTEGER DEFAULT 0,
                parent_goal_id TEXT, summary TEXT, created_at INTEGER, completed_at INTEGER
            );
        ",
        )
        .unwrap();
        let arc = Arc::new(Mutex::new(conn));
        (rusqlite::Connection::open_in_memory().unwrap(), arc) // second conn doesn't matter
    }

    // ─── File handlers ──────────────────────────────────────────────────────

    #[test]
    fn test_read_file_handler_invalid_args() {
        let h = ReadFileHandler::new();
        let result = h.execute(&serde_json::json!({"bad": "args"}), Path::new("."));
        assert!(!result.success);
        assert!(result.content.contains("参数"));
    }

    #[test]
    fn test_write_file_handler_invalid_args() {
        let h = WriteFileHandler;
        let result = h.execute(&serde_json::json!({"bad": "args"}), Path::new("."));
        assert!(!result.success);
    }

    // Issue #102: write/edit/create handlers must verify the artifact
    // actually landed on disk before reporting success.

    #[test]
    fn test_write_file_post_call_check_succeeds_when_file_exists() {
        let dir = std::env::temp_dir().join(format!(
            "angelbot-wfp-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let h = WriteFileHandler;
        let result = h.execute(
            &serde_json::json!({"path": "real.txt", "content": "hi"}),
            &dir,
        );
        assert!(result.success, "{}", result.content);
        assert!(dir.join("real.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_create_directory_post_call_check_succeeds_when_dir_exists() {
        let dir = std::env::temp_dir().join(format!(
            "angelbot-cdp-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let h = CreateDirectoryHandler;
        let result = h.execute(&serde_json::json!({"path": "subdir"}), &dir);
        assert!(result.success, "{}", result.content);
        assert!(dir.join("subdir").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_file_handler_rejects_absolute_path_outside_workspace() {
        let h = ReadFileHandler::new();
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("private.txt");
        std::fs::write(&outside_file, "must not be visible").unwrap();

        let result = h.read_file(workspace.path(), outside_file.to_str().unwrap());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("path escapes the work directory"));
    }

    #[test]
    fn test_project_bootstrap_handlers_reject_invalid_args() {
        let create = CreateDirectoryHandler;
        assert!(
            !create
                .execute(&serde_json::json!({}), Path::new("."))
                .success
        );

        let command = RunProjectCommandHandler::new();
        let result = command.execute(
            &serde_json::json!({ "command": "powershell" }),
            Path::new("."),
        );
        assert!(!result.success);
        assert!(result.content.contains("不支持"));
    }

    #[test]
    fn workspace_file_interface_lists_and_organizes_through_the_same_seam() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("draft.txt"), "hello").unwrap();

        let list = ListWorkspaceItemsHandler.execute(&serde_json::json!({}), dir.path());
        assert!(list.success, "{}", list.content);
        assert!(list.content.contains("draft.txt"));

        let organize = OrganizeWorkspaceItemHandler.execute(
            &serde_json::json!({
                "operation": "move",
                "source": "draft.txt",
                "destination": "final.txt"
            }),
            dir.path(),
        );
        assert!(organize.success, "{}", organize.content);
        assert!(!dir.path().join("draft.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("final.txt")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn common_builtin_surface_does_not_grant_workspace_files() {
        let mut common = ToolRegistry::empty();
        register_common_builtin_handlers(&mut common);
        assert!(common.contains("think"));
        assert!(!common.contains("read_file"));
        assert!(!common.contains("organize_workspace_item"));

        register_workspace_file_handlers(&mut common);
        assert!(common.contains("read_file"));
        assert!(common.contains("list_workspace_items"));
        assert!(common.contains("organize_workspace_item"));
    }

    #[test]
    fn project_command_rejects_interpreter_and_path_escape_hatches() {
        let command = RunProjectCommandHandler::new();
        for arguments in [
            serde_json::json!({ "command": "python3", "args": ["-c", "print('unsafe')"] }),
            serde_json::json!({ "command": "git", "args": ["-C", "..", "status"] }),
            serde_json::json!({ "command": "node", "args": ["--eval", "console.log(1)"] }),
        ] {
            let result = command.execute(&arguments, Path::new("."));
            assert!(!result.success);
            assert!(result.content.contains("内联解释器") || result.content.contains("父目录"));
        }
    }

    // ─── Think handler ──────────────────────────────────────────────────────

    #[test]
    fn test_think_handler() {
        let h = ThinkHandler;
        let result = h.execute(
            &serde_json::json!({"thought": "我需要分析这个任务"}),
            Path::new("."),
        );
        assert!(result.success);
        assert!(result.content.contains("思考记录"));
        assert!(result.content.contains("分析"));
    }

    #[test]
    fn test_think_handler_default() {
        let h = ThinkHandler;
        let result = h.execute(&serde_json::json!({}), Path::new("."));
        assert!(result.success);
    }

    // ─── Verify handler ─────────────────────────────────────────────────────

    #[test]
    fn test_verify_result_pass() {
        let h = VerifyResultHandler;
        let result = h.execute(
            &serde_json::json!({"expectation": "hello", "actual": "hello world"}),
            Path::new("."),
        );
        assert!(result.success);
        assert!(result.content.contains("验证通过"));
    }

    #[test]
    fn test_verify_result_fail() {
        let h = VerifyResultHandler;
        let result = h.execute(
            &serde_json::json!({"expectation": "goodbye", "actual": "hello"}),
            Path::new("."),
        );
        assert!(!result.success);
        assert!(result.content.contains("验证失败"));
    }

    // Issue #104: synonym match prevents false negatives where the agent
    // rephrases the same fact (e.g. "存在" vs "已创建").

    #[test]
    fn test_verify_result_synonym_pass() {
        let h = VerifyResultHandler;
        let result = h.execute(
            &serde_json::json!({
                "expectation": "AgentTestA.txt 存在",
                "actual": "AgentTestA.txt 已创建并保存"
            }),
            Path::new("."),
        );
        assert!(
            result.success,
            "synonym match should pass: {}",
            result.content
        );
        assert!(result.content.contains("验证通过"));
    }

    #[test]
    fn test_verify_result_content_token_match() {
        let h = VerifyResultHandler;
        let result = h.execute(
            &serde_json::json!({
                "expectation": "agent 看到文件 content='hello world'",
                "actual": "AgentTestA.txt 已创建，内容为 'hello world'"
            }),
            Path::new("."),
        );
        assert!(
            result.success,
            "content token match should pass: {}",
            result.content
        );
    }

    #[test]
    fn test_verify_result_path_token_mismatch() {
        let h = VerifyResultHandler;
        let result = h.execute(
            &serde_json::json!({
                "expectation": "AgentTestX.txt 存在",
                "actual": "AgentTestA.txt 已创建"
            }),
            Path::new("."),
        );
        assert!(!result.success, "wrong path should fail");
        assert!(result.content.contains("path"));
    }

    #[test]
    fn test_verify_result_real_content_mismatch() {
        let h = VerifyResultHandler;
        let result = h.execute(
            &serde_json::json!({
                "expectation": "AgentTestA.txt 内容应包含 \"foo\"",
                "actual": "AgentTestA.txt 已创建，内容为 \"bar\""
            }),
            Path::new("."),
        );
        assert!(!result.success, "wrong content should fail");
        assert!(result.content.contains("content"));
    }

    #[test]
    fn test_verify_result_state_mismatch() {
        let h = VerifyResultHandler;
        let result = h.execute(
            &serde_json::json!({
                "expectation": "AgentTestA.txt 不存在",
                "actual": "AgentTestA.txt 已创建"
            }),
            Path::new("."),
        );
        assert!(!result.success, "Exists vs Gone should fail");
        assert!(result.content.contains("state"));
    }

    // ─── Memory handlers (with DB) ──────────────────────────────────────────

    #[test]
    fn test_remember_fact_handler() {
        let (_db, arc) = db();
        let h = RememberFactHandler {
            db: Some(arc.clone()),
        };
        let result = h.execute(&serde_json::json!({"content": "用户喜欢 Python", "category": "preference", "importance": 7}), Path::new("."));
        assert!(result.success);
        assert!(result.content.contains("已记住"));
        // Check DB
        let db = arc.lock().unwrap();
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE content = '用户喜欢 Python'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_remember_fact_importance_capped() {
        let (_db, arc) = db();
        let h = RememberFactHandler {
            db: Some(arc.clone()),
        };
        let result = h.execute(
            &serde_json::json!({"content": "test", "importance": 10}),
            Path::new("."),
        );
        assert!(result.success);
        let db = arc.lock().unwrap();
        let imp: i32 = db
            .query_row(
                "SELECT importance FROM memories WHERE content='test'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(imp <= 7); // Agent cap
    }

    #[test]
    fn test_recall_memories_handler() {
        let (_db, arc) = db();
        // Insert a test memory
        {
            let db = arc.lock().unwrap();
            db.execute("INSERT INTO memories (id,scope,category,content,importance,source,frequency,forget_stage,created_at,updated_at,superseded_by)
                VALUES ('m1','global','preference','用户喜欢 Python 编程',8,'user',0,'active',1,1,NULL)", []).unwrap();
        }
        let h = RecallMemoriesHandler {
            db: Some(arc.clone()),
        };
        // Fallback to keyword search when vector store unavailable
        let result = h.execute(
            &serde_json::json!({"query": "Python", "limit": 5}),
            Path::new("."),
        );
        // May succeed with results or "没有找到" (no vector store in test)
        assert!(result.success);
    }

    #[test]
    fn test_update_memory_handler() {
        let (_db, arc) = db();
        {
            let db = arc.lock().unwrap();
            db.execute("INSERT INTO memories (id,scope,category,content,importance,source,forget_stage,created_at,updated_at)
                VALUES ('m1','global','fact','旧内容',5,'user','active',1,1)", []).unwrap();
        }
        let h = UpdateMemoryHandler {
            db: Some(arc.clone()),
        };
        let result = h.execute(
            &serde_json::json!({"memory_id": "m1", "new_content": "新内容"}),
            Path::new("."),
        );
        assert!(result.success);
        let db = arc.lock().unwrap();
        let content: String = db
            .query_row("SELECT content FROM memories WHERE id='m1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(content, "新内容");
    }

    #[test]
    fn test_forget_memory_handler() {
        let (_db, arc) = db();
        {
            let db = arc.lock().unwrap();
            db.execute("INSERT INTO memories (id,scope,category,content,importance,source,forget_stage,created_at,updated_at)
                VALUES ('m1','global','fact','test',5,'user','active',1,1)", []).unwrap();
        }
        let h = ForgetMemoryHandler {
            db: Some(arc.clone()),
        };
        let result = h.execute(&serde_json::json!({"memory_id": "m1"}), Path::new("."));
        assert!(result.success);
        let db = arc.lock().unwrap();
        let stage: String = db
            .query_row("SELECT forget_stage FROM memories WHERE id='m1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(stage, "summarized");
    }

    // ─── Goal handlers ──────────────────────────────────────────────────────

    #[test]
    fn test_create_goal() {
        let (_db, arc) = db();
        let h = CreateGoalHandler {
            db: Some(arc.clone()),
        };
        let result = h.execute(&serde_json::json!({"goal": "完成项目文档"}), Path::new("."));
        assert!(result.success);
        assert!(result.content.contains("目标已创建"));
    }

    #[test]
    fn test_update_goal_progress() {
        let (_db, arc) = db();
        {
            let db = arc.lock().unwrap();
            db.execute("INSERT INTO agent_goals (id,session_id,goal_text,status,created_at) VALUES ('g1','','test','active',1)", []).unwrap();
        }
        let h = UpdateGoalProgressHandler {
            db: Some(arc.clone()),
        };
        let result = h.execute(
            &serde_json::json!({"goal_id": "g1", "progress": 50}),
            Path::new("."),
        );
        assert!(result.success);
        let db = arc.lock().unwrap();
        let pct: i32 = db
            .query_row(
                "SELECT progress_pct FROM agent_goals WHERE id='g1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pct, 50);
    }

    #[test]
    fn test_complete_goal() {
        let (_db, arc) = db();
        {
            let db = arc.lock().unwrap();
            db.execute("INSERT INTO agent_goals (id,session_id,goal_text,status,created_at) VALUES ('g1','','test','active',1)", []).unwrap();
        }
        let h = CompleteGoalHandler {
            db: Some(arc.clone()),
        };
        let result = h.execute(
            &serde_json::json!({"goal_id": "g1", "summary": "全部完成"}),
            Path::new("."),
        );
        assert!(result.success);
        let db = arc.lock().unwrap();
        let status: String = db
            .query_row("SELECT status FROM agent_goals WHERE id='g1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "done");
    }

    // ─── Constraint handlers ────────────────────────────────────────────────

    #[test]
    fn test_extract_constraints() {
        let (_db, arc) = db();
        let h = ExtractConstraintsHandler {
            db: Some(arc.clone()),
        };
        let result = h.execute(&serde_json::json!({"content": "永远使用 TypeScript 严格模式", "constraint_type": "constraint"}), Path::new("."));
        assert!(result.success);
        let db = arc.lock().unwrap();
        let pri: String = db
            .query_row(
                "SELECT priority FROM memories WHERE content LIKE '%TypeScript%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pri, "constraint");
    }

    #[test]
    fn test_check_constraint_pass() {
        let (_db, arc) = db();
        {
            let db = arc.lock().unwrap();
            db.execute("INSERT INTO memories (id,scope,category,content,importance,source,forget_stage,priority,created_at,updated_at)
                VALUES ('c1','global','constraint','永远使用 Rust',9,'agent','active','constraint',1,1)", []).unwrap();
        }
        let h = CheckConstraintHandler {
            db: Some(arc.clone()),
        };
        let result = h.execute(
            &serde_json::json!({"action": "使用 Python 写脚本"}),
            Path::new("."),
        );
        assert!(result.success);
    }

    // ─── Tool registry ──────────────────────────────────────────────────────

    #[test]
    fn test_registry_has_all_builtin_tools() {
        let reg = ToolRegistry::new();
        assert!(reg.get_schemas().len() >= 6); // read, write, think, verify, watch_file, schedule_reminder, extract_entities
    }

    #[test]
    fn test_side_effect_tools_are_marked_for_confirmation() {
        let reg = ToolRegistry::new();
        // Core tools: edit_file, write_file, create_directory, run_project_command require confirmation
        assert!(reg.get("edit_file").unwrap().requires_confirmation);
        assert!(reg.get("write_file").unwrap().requires_confirmation);
        assert!(reg.get("create_directory").unwrap().requires_confirmation);
        assert!(
            reg.get("run_project_command")
                .unwrap()
                .requires_confirmation
        );
        // read_file does NOT require confirmation
        assert!(!reg.get("read_file").unwrap().requires_confirmation);
        // think / verify_result do NOT require confirmation (read-only reasoning)
        assert!(!reg.get("think").unwrap().requires_confirmation);
        assert!(!reg.get("verify_result").unwrap().requires_confirmation);
        // Extension tools like schedule_reminder are NOT registered by default
        // They are loaded on demand via register_extension_handlers()
        // This test only checks core tools
    }

    #[test]
    fn desktop_tools_enforce_trust_and_use_bounded_arguments() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE desktop_trusted_apps (
                id TEXT PRIMARY KEY, display_name TEXT NOT NULL, executable_path TEXT NOT NULL,
                capabilities TEXT NOT NULL, draft_selector TEXT, enabled INTEGER NOT NULL,
                created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
            );",
        )
        .unwrap();
        let executable = tempfile::Builder::new().suffix(".exe").tempfile().unwrap();
        conn.execute(
            "INSERT INTO desktop_trusted_apps VALUES
                ('codex', 'Codex', ?1, '[\"launch\",\"draft\"]', 'Message', 1, 1, 1)",
            rusqlite::params![executable.path().to_string_lossy()],
        )
        .unwrap();
        let db = Arc::new(Mutex::new(conn));
        let adapter = crate::desktop_control::create_mock_adapter();
        let mut registry = ToolRegistry::new();
        register_desktop_handlers(&mut registry, db, adapter, true);

        assert!(
            !registry
                .get("inspect_desktop_capabilities")
                .unwrap()
                .requires_confirmation
        );

        for name in [
            "open_trusted_app",
            "open_windows_setting",
            "reveal_workspace_item",
            "prepare_message_draft",
        ] {
            assert!(registry.get(name).unwrap().requires_confirmation);
        }
        assert!(registry
            .validate_arguments("open_trusted_app", &serde_json::json!({"app_id":"codex"}))
            .is_ok());
        assert!(registry
            .validate_arguments(
                "prepare_message_draft",
                &serde_json::json!({"app_id":"codex"})
            )
            .is_err());

        let opened = registry.execute(
            &ToolCall::new(
                "open_trusted_app".into(),
                serde_json::json!({"app_id":"codex"}),
            ),
            "desktop-1",
            Path::new("."),
        );
        assert!(opened.result.success);
        assert!(opened.result.content.contains("dispatched"));

        executable.close().unwrap();
        let unavailable = registry.execute(
            &ToolCall::new(
                "open_trusted_app".into(),
                serde_json::json!({"app_id":"codex"}),
            ),
            "desktop-unavailable",
            Path::new("."),
        );
        assert!(!unavailable.result.success);
        assert!(unavailable.result.content.contains("no longer available"));

        let unknown = registry.execute(
            &ToolCall::new(
                "open_trusted_app".into(),
                serde_json::json!({"app_id":"unknown"}),
            ),
            "desktop-2",
            Path::new("."),
        );
        assert!(!unknown.result.success);
        assert!(unknown.result.content.contains("tool enum"));
    }

    #[test]
    fn test_registry_tool_not_found() {
        let reg = ToolRegistry::new();
        let call = ToolCall::new("nonexistent".into(), serde_json::json!({}));
        let result = reg.execute(&call, "id1", Path::new("."));
        assert!(!result.result.success);
    }

    // Issue #091: ToolLifecycleHook integration tests
    #[test]
    fn test_tool_lifecycle_block_hook_blocks_execution() {
        use crate::agent::lifecycle::{BeforeToolCallResult, ToolCallContext, ToolLifecycleHook};

        struct BlockingHook;
        impl ToolLifecycleHook for BlockingHook {
            fn before_tool_call(&self, ctx: &ToolCallContext) -> BeforeToolCallResult {
                if ctx.tool_name == "block_me" {
                    BeforeToolCallResult {
                        block: true,
                        reason: Some("blocked by test".into()),
                    }
                } else {
                    BeforeToolCallResult {
                        block: false,
                        reason: None,
                    }
                }
            }
        }

        let hook = BlockingHook;

        // Tool that would be blocked
        let ctx = ToolCallContext {
            tool_call: ToolCall::new("block_me".into(), serde_json::json!({})),
            tool_name: "block_me".into(),
            args: serde_json::json!({}),
            session_id: None,
            iteration: 0,
        };
        let result = hook.before_tool_call(&ctx);
        assert!(result.block);
        assert_eq!(result.reason.as_deref(), Some("blocked by test"));

        // Tool that would pass
        let ctx2 = ToolCallContext {
            tool_call: ToolCall::new("allow_me".into(), serde_json::json!({})),
            tool_name: "allow_me".into(),
            args: serde_json::json!({}),
            session_id: None,
            iteration: 0,
        };
        let result2 = hook.before_tool_call(&ctx2);
        assert!(!result2.block);
    }

    #[test]
    fn test_tool_lifecycle_after_hook_overrides_result() {
        use crate::agent::lifecycle::{
            apply_after_hook, AfterToolCallContext, AfterToolCallResult, ToolLifecycleHook,
        };

        struct OverridingHook;
        impl ToolLifecycleHook for OverridingHook {
            fn after_tool_call(&self, ctx: &AfterToolCallContext) -> AfterToolCallResult {
                // Redact sensitive data from results
                let content = if ctx.result.content.contains("secret") {
                    "[REDACTED]"
                } else {
                    return AfterToolCallResult::default();
                };
                AfterToolCallResult {
                    content: Some(content.into()),
                    ..Default::default()
                }
            }
        }

        let hook = OverridingHook;

        // Result with "secret" should be redacted
        let result = crate::agent::tool::ToolResult {
            name: "test".into(),
            success: true,
            content: "user password: secret123".into(),
            terminate_batch: false,
            images: Vec::new(),
        };
        let ctx = AfterToolCallContext {
            tool_call: ToolCall::new("test".into(), serde_json::json!({})),
            tool_name: "test".into(),
            args: serde_json::json!({}),
            result,
            is_error: false,
            session_id: None,
            iteration: 0,
        };
        let hook_result = hook.after_tool_call(&ctx);
        let final_result = apply_after_hook(ctx.result.clone(), hook_result);
        assert_eq!(final_result.content, "[REDACTED]");
    }
}

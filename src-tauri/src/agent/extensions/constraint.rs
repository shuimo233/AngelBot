//! Constraint extension - constraint extraction and checking
//!
//! Tools:
//! - extract_constraints: Extract constraints from conversation
//! - check_constraint: Check if an action violates constraints

use crate::agent::tool::{Tool, ToolCategory, ToolHandler, ToolResult};
use crate::agent::ToolRegistry;
use rusqlite::{params, Connection};
use serde::Deserialize;
use std::path::Path;
use std::sync::{Arc, Mutex};

// ─── Handlers ─────────────────────────────────────────────────────────────────

pub struct ExtractConstraintsHandler {
    pub db: Option<Arc<Mutex<Connection>>>,
}

impl ToolHandler for ExtractConstraintsHandler {
    fn name(&self) -> &str {
        "extract_constraints"
    }
    fn execute(&self, arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        let db = match &self.db {
            Some(d) => d.lock().map_err(|e| format!("DB: {}", e)),
            None => return ToolResult::error("extract_constraints", "数据库不可用"),
        };
        let db = match db {
            Ok(d) => d,
            Err(e) => return ToolResult::error("extract_constraints", e),
        };
        #[derive(Deserialize)]
        struct Args {
            content: String,
            constraint_type: Option<String>,
        }
        let args: Args = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => return ToolResult::error("extract_constraints", format!("参数错误: {}", e)),
        };
        let ctype = args
            .constraint_type
            .unwrap_or_else(|| "constraint".to_string());
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();
        match db.execute(
            "INSERT INTO memories (id, scope, category, content, importance, source, frequency, is_permanent, forget_stage, priority, created_at, updated_at)
             VALUES (?1, 'global', 'constraint', ?2, 9, 'agent', 0, 1, 'active', ?3, ?4, ?4)",
            params![id, args.content, ctype, now],
        ) {
            Ok(_) => ToolResult::success("extract_constraints", format!("约束已提取并存储为永久记忆: {}", args.content)),
            Err(e) => ToolResult::error("extract_constraints", format!("存储失败: {}", e)),
        }
    }
}

pub struct CheckConstraintHandler {
    pub db: Option<Arc<Mutex<Connection>>>,
}

impl ToolHandler for CheckConstraintHandler {
    fn name(&self) -> &str {
        "check_constraint"
    }
    fn execute(&self, arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        let db = match &self.db {
            Some(d) => d.lock().map_err(|e| format!("DB: {}", e)),
            None => return ToolResult::error("check_constraint", "数据库不可用"),
        };
        let db = match db {
            Ok(d) => d,
            Err(e) => return ToolResult::error("check_constraint", e),
        };
        let action = arguments
            .get("action")
            .and_then(|a| a.as_str())
            .unwrap_or("");
        let mut stmt = match db.prepare(
            "SELECT id, content FROM memories WHERE category = 'constraint' AND forget_stage = 'active' AND deleted = 0 ORDER BY priority DESC LIMIT 10",
        ) {
            Ok(s) => s,
            Err(_) => return ToolResult::success("check_constraint", "未发现冲突约束。"),
        };
        let conflicts: Vec<String> = stmt
            .query_map([], |r| {
                Ok(format!(
                    "- [{}] {}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?
                ))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        let action_lower = action.to_lowercase();
        let violations: Vec<&String> = conflicts
            .iter()
            .filter(|c| {
                let c_lower = c.to_lowercase();
                (c_lower.contains("never")
                    || c_lower.contains("always")
                    || c_lower.contains("must"))
                    && action_lower.split_whitespace().any(|w| c_lower.contains(w))
            })
            .collect();
        if violations.is_empty() {
            ToolResult::success("check_constraint", "无冲突: 操作符合所有已知约束。")
        } else {
            ToolResult::success(
                "check_constraint",
                format!(
                    "发现 {} 个潜在冲突:\n{}",
                    violations.len(),
                    violations
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            )
        }
    }
}

// ─── Registration ──────────────────────────────────────────────────────────────

pub fn register(registry: &mut ToolRegistry, db: Arc<Mutex<Connection>>) {
    registry.register(
        Tool::new(
            "extract_constraints",
            "从对话内容中提取持久化约束。当用户表达 'always/never/must/prefer' 等偏好时使用。约束会被存储为永久记忆。",
            serde_json::json!({"type":"object","properties":{"content":{"type":"string","description":"约束内容"},"constraint_type":{"type":"string","description":"类型: constraint/preference/rule"}},"required":["content"]}),
        )
        .with_category(ToolCategory::Extension)
        .with_confirmation(true),
        Arc::new(ExtractConstraintsHandler { db: Some(db.clone()) }),
    );
    registry.register(
        Tool::new(
            "check_constraint",
            "检查一个操作是否符合所有已知约束。在执行重要操作前使用，避免违反用户偏好。",
            serde_json::json!({"type":"object","properties":{"action":{"type":"string","description":"要检查的操作描述"}},"required":["action"]}),
        ).with_category(ToolCategory::Extension),
        Arc::new(CheckConstraintHandler { db: Some(db.clone()) }),
    );
}

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

const MAX_CONTEXT_CONSTRAINTS: usize = 10;
const MAX_CONTEXT_BYTES: usize = 8_192;

/// One bounded projection shared by foreground preparation and explicit checks.
/// Only attributable global records are eligible; an unbound session record
/// cannot become a global rule. Preserve whole rules rather than truncating a
/// qualifier/negation, and report omissions instead of implying completeness.
pub(crate) fn saved_constraint_context(db: &Connection) -> Result<Option<String>, String> {
    let mut stmt = db
        .prepare(
            "SELECT id,
                    CASE WHEN length(CAST(content AS BLOB)) <= ?2 THEN content END,
                    COUNT(*) OVER () FROM memories
             WHERE scope='global' AND category='constraint'
               AND forget_stage='active' AND superseded_by IS NULL
             ORDER BY importance DESC, updated_at DESC, id LIMIT ?1",
        )
        .map_err(|error| format!("约束读取失败: {error}"))?;
    let rows = stmt
        .query_map(
            [MAX_CONTEXT_CONSTRAINTS as i64, MAX_CONTEXT_BYTES as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, usize>(2)?,
                ))
            },
        )
        .map_err(|error| format!("约束读取失败: {error}"))?;
    let mut items = Vec::new();
    let mut total = 0;
    for row in rows {
        let (id, content, count) = row.map_err(|error| format!("约束读取失败: {error}"))?;
        total = count;
        let Some(content) = content else { continue };
        items.push(serde_json::json!({ "id": id, "content": content }));
        // `total` reserves the largest possible omission counter as well.
        let projection = serde_json::json!({"items": items, "omitted": total});
        if projection.to_string().len() > MAX_CONTEXT_BYTES {
            items.pop();
        }
    }
    Ok((total > 0)
        .then(|| serde_json::json!({"items": items, "omitted": total - items.len()}).to_string()))
}

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
            Ok(_) => ToolResult::success("extract_constraints", format!("已保存用于日常和所有项目的全局约束: {}", args.content)),
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
        let Some(action) = arguments
            .get("action")
            .and_then(|a| a.as_str())
            .filter(|action| !action.trim().is_empty())
        else {
            return ToolResult::error(self.name(), "需要非空的 action 操作描述");
        };
        match saved_constraint_context(&db) {
            Err(error) => ToolResult::error(self.name(), error),
            Ok(None) => ToolResult::success(self.name(), "未查到有效全局约束；这不代表已获操作授权。"),
            Ok(Some(constraints)) => ToolResult::success(
                self.name(),
                format!(
                    "请将已保存的全局约束与操作“{action}”逐项核对；这不是权限判定。omitted 大于0表示清单不完整，需按当前任务通过 recall_memories 补查或向用户澄清，不能假定不存在其他约束：\n{constraints}"
                ),
            ),
        }
    }
}

// ─── Registration ──────────────────────────────────────────────────────────────

pub fn register(registry: &mut ToolRegistry, db: Arc<Mutex<Connection>>) {
    registry.register(
        Tool::new(
            "extract_constraints",
            "保存适用于日常和所有项目的全局长期约束。仅在用户明确要求跨任务记住时使用；当前任务或单个项目的条件留在其任务上下文中，不要提升为全局规则。需确认。",
            serde_json::json!({"type":"object","properties":{"content":{"type":"string","description":"约束内容"},"constraint_type":{"type":"string","description":"类型: constraint/preference/rule"}},"required":["content"]}),
        )
        .with_category(ToolCategory::Extension)
        .with_confirmation(true),
        Arc::new(ExtractConstraintsHandler { db: Some(db.clone()) }),
    );
    registry.register(
        Tool::new(
            "check_constraint",
            "读取至多10条、8192字节的有效全局用户约束，带来源ID及遗漏数量，供结合操作逐项核对；只提供依据，不代替权限校验。",
            serde_json::json!({"type":"object","properties":{"action":{"type":"string","description":"要检查的操作描述"}},"required":["action"]}),
        ).with_category(ToolCategory::Extension),
        Arc::new(CheckConstraintHandler { db: Some(db.clone()) }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_rules_preserve_whole_entries_and_report_count_and_byte_limits() {
        let db = crate::db::init(Path::new(":memory:")).unwrap();
        crate::db::migrate(&db).unwrap();
        assert!(saved_constraint_context(&db).unwrap().is_none());
        for index in 0..12 {
            db.execute(
                "INSERT INTO memories (id,scope,category,content,importance,forget_stage,created_at,updated_at)
                 VALUES (?1,'global','constraint',?2,9,'active',1,?3)",
                params![format!("rule-{index}"), format!("完整规则 {index}"), index],
            )
            .unwrap();
        }
        let context = saved_constraint_context(&db).unwrap().unwrap();
        let value: serde_json::Value = serde_json::from_str(&context).unwrap();
        assert_eq!(
            value["items"].as_array().unwrap().len(),
            MAX_CONTEXT_CONSTRAINTS
        );
        assert_eq!(value["items"][0]["id"], "rule-11");
        assert_eq!(value["omitted"], 2);
        assert!(context.len() <= MAX_CONTEXT_BYTES);

        let long_rule = format!(
            "{}，但允许用户明确批准的例外",
            "禁止删除".repeat(MAX_CONTEXT_BYTES)
        );
        db.execute(
            "UPDATE memories SET content=?1,updated_at=100 WHERE id='rule-11'",
            [&long_rule],
        )
        .unwrap();
        let context = saved_constraint_context(&db).unwrap().unwrap();
        let value: serde_json::Value = serde_json::from_str(&context).unwrap();
        assert_eq!(value["items"].as_array().unwrap().len(), 9);
        assert_eq!(value["omitted"], 3);
        assert!(!context.contains("允许用户明确批准的例外"));
        assert!(context.len() <= MAX_CONTEXT_BYTES);
        assert_eq!(
            db.query_row(
                "SELECT content FROM memories WHERE id='rule-11'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            long_rule
        );
    }
}

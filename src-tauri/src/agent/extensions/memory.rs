//! Memory extension - long-term memory management tools
//!
//! Tools:
//! - remember_fact: Store a fact or preference
//! - recall_memories: Search and retrieve memories
//! - update_memory: Update an existing memory
//! - forget_memory: Mark a memory for forgetting

use crate::agent::adaptive_constraints::{dismiss_learned_preference, learned_preferences};
use crate::agent::tool::{Tool, ToolCategory, ToolHandler, ToolResult};
use crate::agent::ToolRegistry;
use rusqlite::{params, Connection};
use serde::Deserialize;
use std::path::Path;
use std::sync::{Arc, Mutex};

// ─── Handlers ─────────────────────────────────────────────────────────────────

pub struct RememberFactHandler {
    pub db: Option<Arc<Mutex<Connection>>>,
}

impl ToolHandler for RememberFactHandler {
    fn name(&self) -> &str {
        "remember_fact"
    }
    fn execute(&self, arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        let db = match &self.db {
            Some(d) => d.lock().map_err(|e| format!("DB lock: {}", e)),
            None => return ToolResult::error("remember_fact", "数据库不可用"),
        };
        let db = match db {
            Ok(d) => d,
            Err(e) => return ToolResult::error("remember_fact", e),
        };
        #[derive(Deserialize)]
        struct Args {
            content: String,
            category: Option<String>,
            importance: Option<i32>,
        }
        let args: Args = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => return ToolResult::error("remember_fact", format!("参数错误: {}", e)),
        };
        let category = args.category.unwrap_or_else(|| "fact".to_string());
        let importance = args.importance.unwrap_or(5).min(7);
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();
        match db.execute(
            "INSERT INTO memories (id, scope, category, content, importance, source, frequency, is_permanent, forget_stage, created_at, updated_at)
             VALUES (?1, 'global', ?2, ?3, ?4, 'agent', 0, 0, 'active', ?5, ?5)",
            params![id, category, args.content, importance, now],
        ) {
            Ok(_) => ToolResult::success("remember_fact", format!("已记住: {}", args.content)),
            Err(e) => ToolResult::error("remember_fact", format!("保存失败: {}", e)),
        }
    }
}

pub struct RecallMemoriesHandler {
    pub db: Option<Arc<Mutex<Connection>>>,
}

impl ToolHandler for RecallMemoriesHandler {
    fn name(&self) -> &str {
        "recall_memories"
    }
    fn execute(&self, arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        let db = match &self.db {
            Some(d) => d.lock().map_err(|e| format!("DB lock: {}", e)),
            None => return ToolResult::error("recall_memories", "数据库不可用"),
        };
        let db = match db {
            Ok(d) => d,
            Err(e) => return ToolResult::error("recall_memories", e),
        };
        #[derive(Deserialize)]
        struct Args {
            query: String,
            limit: Option<usize>,
            include_learned_habits: Option<bool>,
        }
        let args: Args = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => return ToolResult::error("recall_memories", format!("参数错误: {}", e)),
        };
        let limit = args.limit.unwrap_or(5).clamp(1, 20);
        // Habit inspection is opt-in for an explicit owner request, not an
        // alternate path for injecting automatic defaults into every recall.
        let mut rows = Vec::new();
        if args.include_learned_habits.unwrap_or(false) {
            let habits = match learned_preferences(&db) {
                Ok(habits) => habits,
                Err(e) => {
                    return ToolResult::error("recall_memories", format!("查询习惯失败: {e}"))
                }
            };
            let query = args.query.trim().to_lowercase();
            rows.extend(
                habits
                    .into_iter()
                    .filter(|habit| habit.content.to_lowercase().contains(&query))
                    .take(limit)
                    .map(|habit| {
                        let status = if habit.ready {
                            "可作为回应参考"
                        } else {
                            "还在了解，不能视作已确认事实"
                        };
                        format!(
                            "[habit:{}] {} (自动习惯：{})",
                            habit.id, habit.content, status
                        )
                    }),
            );
        }
        if rows.len() < limit {
            match recall_explicit_memories(&db, args.query.trim(), limit - rows.len()) {
                Ok(memories) => rows.extend(memories),
                Err(e) => {
                    return ToolResult::error("recall_memories", format!("查询记忆失败: {e}"))
                }
            }
        }
        if rows.is_empty() {
            ToolResult::success("recall_memories", "没有找到相关记忆。")
        } else {
            ToolResult::success("recall_memories", rows.join("\n"))
        }
    }
}

fn format_memory_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<String> {
    Ok(format!(
        "[memory:{}] {} (类别:{}，重要性:{})",
        row.get::<_, String>(0)?,
        row.get::<_, String>(2)?,
        row.get::<_, String>(1)?,
        row.get::<_, Option<i32>>(3)?.unwrap_or(0),
    ))
}

/// Both retrieval paths expose the same durable IDs and exclude forgotten or
/// superseded rows, including stale vector mappings left after a correction.
fn recall_explicit_memories(
    db: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<String>, String> {
    let tf_emb = crate::vector::text_to_embedding(query, 128);
    let query_f32: Vec<f32> = {
        let mut v: Vec<f32> = tf_emb.iter().map(|&x| x as f32).collect();
        v.resize(1536, 0.0);
        v
    };
    if let Ok(knn) = crate::vector_store::find_nearest(db, &query_f32, limit) {
        if !knn.is_empty() {
            let rowids: Vec<i64> = knn.iter().map(|r| r.rowid).collect();
            if let Ok(mapping) = crate::vector_store::rowids_to_memory_ids(db, &rowids) {
                let id_list: Vec<String> = mapping.iter().map(|(_, id)| id.clone()).collect();
                if !id_list.is_empty() {
                    let placeholders: Vec<String> = id_list
                        .iter()
                        .enumerate()
                        .map(|(i, _)| format!("?{}", i + 1))
                        .collect();
                    let sql = format!(
                        "SELECT id, category, content, importance FROM memories
                             WHERE forget_stage='active' AND superseded_by IS NULL AND id IN ({})",
                        placeholders.join(",")
                    );
                    if let Ok(mut stmt) = db.prepare(&sql) {
                        let rows = stmt
                            .query_map(rusqlite::params_from_iter(&id_list), format_memory_row)
                            .map_err(|e| e.to_string())?
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|e| e.to_string())?;
                        if !rows.is_empty() {
                            return Ok(rows);
                        }
                    }
                }
            }
        }
    }
    let mut stmt = db
        .prepare(
            "SELECT id, category, content, importance FROM memories
             WHERE forget_stage = 'active' AND superseded_by IS NULL
             AND content LIKE '%' || ?1 || '%'
             ORDER BY importance DESC, frequency DESC LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![query, limit as i64], format_memory_row)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub struct UpdateMemoryHandler {
    pub db: Option<Arc<Mutex<Connection>>>,
}

impl ToolHandler for UpdateMemoryHandler {
    fn name(&self) -> &str {
        "update_memory"
    }
    fn execute(&self, arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        let db = match &self.db {
            Some(d) => d.lock().map_err(|e| format!("DB lock: {}", e)),
            None => return ToolResult::error("update_memory", "数据库不可用"),
        };
        let db = match db {
            Ok(d) => d,
            Err(e) => return ToolResult::error("update_memory", e),
        };
        #[derive(Deserialize)]
        struct Args {
            memory_id: String,
            new_content: String,
        }
        let args: Args = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => return ToolResult::error("update_memory", format!("参数错误: {}", e)),
        };
        if args.memory_id.starts_with("habit:") {
            return ToolResult::error("update_memory", "自动习惯不能作为已确认记忆直接改写；请按用户要求用 forget_memory 停用旧记录。只有用户明确要求保存新偏好时才使用 remember_fact。");
        }
        // Accept legacy bare IDs as well as the unambiguous IDs from recall.
        let memory_id = args
            .memory_id
            .strip_prefix("memory:")
            .unwrap_or(&args.memory_id);
        let now = chrono::Utc::now().timestamp();
        match db.execute(
            "UPDATE memories SET content = ?1, updated_at = ?2 WHERE id = ?3",
            params![args.new_content, now, memory_id],
        ) {
            Ok(n) if n > 0 => ToolResult::success("update_memory", "记忆已更新。"),
            Ok(_) => ToolResult::error("update_memory", "未找到该记忆。"),
            Err(e) => ToolResult::error("update_memory", format!("更新失败: {}", e)),
        }
    }
}

pub struct ForgetMemoryHandler {
    pub db: Option<Arc<Mutex<Connection>>>,
}

impl ToolHandler for ForgetMemoryHandler {
    fn name(&self) -> &str {
        "forget_memory"
    }
    fn execute(&self, arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        let db = match &self.db {
            Some(d) => d.lock().map_err(|e| format!("DB lock: {}", e)),
            None => return ToolResult::error("forget_memory", "数据库不可用"),
        };
        let db = match db {
            Ok(d) => d,
            Err(e) => return ToolResult::error("forget_memory", e),
        };
        #[derive(Deserialize)]
        struct Args {
            memory_id: String,
        }
        let args: Args = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => return ToolResult::error("forget_memory", format!("参数错误: {}", e)),
        };
        if let Some(id) = args.memory_id.strip_prefix("habit:") {
            return match dismiss_learned_preference(&db, id) {
                Ok(()) => ToolResult::success(
                    "forget_memory",
                    "已停止沿用这条自动习惯；原对话和主动保存的记忆未删除。",
                ),
                Err(e) => ToolResult::error("forget_memory", e),
            };
        }
        let memory_id = args
            .memory_id
            .strip_prefix("memory:")
            .unwrap_or(&args.memory_id);
        let now = chrono::Utc::now().timestamp();
        match db.execute(
            "UPDATE memories SET forget_stage = 'summarized', source = 'agent', updated_at = ?1 WHERE id = ?2",
            params![now, memory_id],
        ) {
            Ok(n) if n > 0 => ToolResult::success("forget_memory", "已标记为遗忘。"),
            Ok(_) => ToolResult::error("forget_memory", "未找到该记忆。"),
            Err(e) => ToolResult::error("forget_memory", format!("操作失败: {}", e)),
        }
    }
}

// ─── Registration ──────────────────────────────────────────────────────────────

pub fn register(registry: &mut ToolRegistry, db: Arc<Mutex<Connection>>) {
    registry.register(
        Tool::new(
            "remember_fact",
            "仅当用户明确要求把信息保存为长期记忆（如“请记住”）时使用。仅表达交流风格时，先在当前回应采用，不调用本工具或申请保存确认；自动偏好学习由后台有界复盘处理。任务约束不是长期记忆，也不是操作授权。",
            serde_json::json!({"type":"object","properties":{"content":{"type":"string","description":"要记住的内容"},"category":{"type":"string","description":"类别: fact/preference/event/knowledge"},"importance":{"type":"integer","description":"重要性 1-7"}},"required":["content"]}),
        )
        .with_category(ToolCategory::Extension)
        .with_confirmation(true),
        Arc::new(RememberFactHandler { db: Some(db.clone()) }),
    );
    registry.register(
        Tool::new(
            "recall_memories",
            "搜索用户的记忆并返回可操作的记录 ID；用户要求管理自动习惯时可显式包含近期习惯。",
            serde_json::json!({"type":"object","additionalProperties":false,"properties":{"query":{"type":"string","description":"搜索关键词；空字符串列出记录"},"limit":{"type":"integer","minimum":1,"maximum":20,"description":"合计最多返回 1-20 条，默认 5 条"},"include_learned_habits":{"type":"boolean","description":"默认 false。仅在用户明确要求查看、纠正或停止沿用自动习惯时设为 true；搜索近期 50 条自动记录，优先返回匹配习惯"}},"required":["query"]}),
        )
        .with_category(ToolCategory::Extension)
        .with_prompt_snippet("用户要求纠正或停止沿用已学习的习惯时，先用 recall_memories（include_learned_habits=true）查到对应 ID，再按用户意图调用 forget_memory。不要猜 ID，不必让用户提供 ID；多条匹配且意图不明时再澄清。自动习惯是可错的观察，不是已确认事实；用户当前表达优先，不能据此授权提醒或应用操作。仅为管理而查看，不绕过关闭学习／长期记忆的设定去应用这些习惯。"),
        Arc::new(RecallMemoriesHandler { db: Some(db.clone()) }),
    );
    registry.register(
        Tool::new(
            "update_memory",
            "更新已有记忆的内容。当用户纠正或补充信息时使用。",
            serde_json::json!({"type":"object","properties":{"memory_id":{"type":"string","description":"recall_memories 返回的完整 memory: ID；自动习惯不能直接改写"},"new_content":{"type":"string","description":"新内容"}},"required":["memory_id","new_content"]}),
        )
        .with_category(ToolCategory::Extension)
        .with_confirmation(true),
        Arc::new(UpdateMemoryHandler { db: Some(db.clone()) }),
    );
    registry.register(
        Tool::new(
            "forget_memory",
            "用户要求时标记记忆为遗忘，或停止沿用自动习惯；不会删除原对话。",
            serde_json::json!({"type":"object","additionalProperties":false,"properties":{"memory_id":{"type":"string","description":"recall_memories 返回的完整 ID，保留 memory: 或 habit: 前缀。只处理用户要求的那条记录"}},"required":["memory_id"]}),
        )
        .with_category(ToolCategory::Extension)
        .with_confirmation(true),
        Arc::new(ForgetMemoryHandler { db: Some(db.clone()) }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::adaptive_constraints::{active_for_session, record_preferences};
    use crate::agent::evolution::NewMemory;
    use crate::agent::tool::ToolCall;

    fn fixture() -> (Arc<Mutex<Connection>>, ToolRegistry) {
        unsafe {
            crate::vector_store::load_extension_globally();
        }
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        let db = Arc::new(Mutex::new(conn));
        let mut registry = ToolRegistry::empty();
        register(&mut registry, db.clone());
        (db, registry)
    }

    fn call(registry: &ToolRegistry, name: &str, arguments: serde_json::Value) -> ToolResult {
        registry
            .execute(
                &ToolCall::new(name.into(), arguments),
                "test-call",
                Path::new("."),
            )
            .result
    }

    fn preference(content: &str) -> NewMemory {
        NewMemory {
            content: content.into(),
            category: "preference".into(),
            importance: 8,
        }
    }

    #[test]
    fn registered_memory_tools_can_find_and_dismiss_habits_without_rewriting_user_memory() {
        let (db, registry) = fixture();
        assert!(registry
            .get("remember_fact")
            .unwrap()
            .description
            .contains("仅表达交流风格"));
        assert!(registry.get("forget_memory").unwrap().requires_confirmation);
        assert!(
            !registry
                .get("recall_memories")
                .unwrap()
                .requires_confirmation
        );
        assert!(call(&registry, "remember_fact", serde_json::json!({"content":"喜欢安静的交流", "category":"preference", "importance":10})).success);
        let (habit_id, memory_id) = {
            let conn = db.lock().unwrap();
            let habit = preference("喜欢安静的交流");
            record_preferences(&conn, Some("day-1"), &[habit.clone()], "auto_evolution").unwrap();
            record_preferences(&conn, Some("day-2"), &[habit], "auto_evolution").unwrap();
            record_preferences(
                &conn,
                Some("day-1"),
                &[preference("称呼我小林")],
                "auto_evolution",
            )
            .unwrap();
            let habits = learned_preferences(&conn).unwrap();
            let habit_id = habits
                .iter()
                .find(|h| h.content.contains("安静"))
                .unwrap()
                .id
                .clone();
            let old_memory_id: String = conn
                .query_row("SELECT id FROM memories", [], |r| r.get(0))
                .unwrap();
            // Imported legacy IDs may start with habit:. The memory:
            // namespace must still target the explicit memory.
            let memory_id = format!("habit:{habit_id}");
            conn.execute(
                "UPDATE memories SET id=?1 WHERE id=?2",
                params![memory_id, old_memory_id],
            )
            .unwrap();
            (habit_id, memory_id)
        };
        let regular = call(
            &registry,
            "recall_memories",
            serde_json::json!({"query":"安静"}),
        );
        assert!(regular.success);
        assert!(regular.content.contains(&format!("[memory:{memory_id}]")));
        assert!(
            !regular.content.contains("[habit:"),
            "ordinary recall must not silently inject learned defaults"
        );
        let recalled = call(
            &registry,
            "recall_memories",
            serde_json::json!({"query":"", "include_learned_habits":true}),
        );
        assert!(recalled.success);
        assert!(recalled.content.contains(&format!("[habit:{habit_id}]")));
        assert!(recalled.content.contains("可作为回应参考"));
        assert!(recalled.content.contains("还在了解，不能视作已确认事实"));
        let limited = call(
            &registry,
            "recall_memories",
            serde_json::json!({"query":"", "limit":1, "include_learned_habits":true}),
        );
        assert_eq!(limited.content.lines().count(), 1);
        let target = serde_json::json!({"memory_id":format!("habit:{habit_id}")});
        let rejected = call(
            &registry,
            "update_memory",
            serde_json::json!({"memory_id":format!("habit:{habit_id}"), "new_content":"更喜欢热闹"}),
        );
        assert!(
            !rejected.success,
            "observations cannot be rewritten as confirmed facts"
        );
        for _ in 0..2 {
            let dismissed = call(&registry, "forget_memory", target.clone());
            assert!(dismissed.success, "{}", dismissed.content);
            assert!(dismissed.content.contains("原对话和主动保存的记忆未删除"));
        }
        assert!(
            !call(
                &registry,
                "forget_memory",
                serde_json::json!({"memory_id":"habit:missing"})
            )
            .success
        );
        let after = call(
            &registry,
            "recall_memories",
            serde_json::json!({"query":"安静", "include_learned_habits":true}),
        );
        assert!(after.success);
        assert!(!after.content.contains(&format!("[habit:{habit_id}]")));
        assert!(
            after.content.contains(&memory_id),
            "dismissing an observation must preserve explicit memories"
        );
        {
            let conn = db.lock().unwrap();
            assert!(active_for_session(&conn, "day-3").is_empty());
            assert_eq!(
                record_preferences(
                    &conn,
                    Some("day-3"),
                    &[preference("喜欢安静的交流")],
                    "auto_evolution"
                )
                .unwrap(),
                0
            );
            let importance: i32 = conn
                .query_row(
                    "SELECT importance FROM memories WHERE id=?1",
                    [&memory_id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(importance, 7);
        }
        assert!(
            call(
                &registry,
                "update_memory",
                serde_json::json!({"memory_id":format!("memory:{memory_id}"), "new_content":"用户明确保存的新偏好"})
            )
            .success
        );
        assert!(
            call(
                &registry,
                "forget_memory",
                serde_json::json!({"memory_id":format!("memory:{memory_id}")})
            )
            .success
        );
        assert!(!call(
            &registry,
            "recall_memories",
            serde_json::json!({"query":""})
        )
        .content
        .contains(&memory_id));
    }

    #[test]
    fn recall_filters_stale_vectors_and_keyword_rows_and_bounds_the_combined_result() {
        let (db, registry) = fixture();
        {
            let conn = db.lock().unwrap();
            crate::vector_store::init_vector_table(&conn).unwrap();
            crate::vector_store::ensure_rowid_mapping(&conn).unwrap();
            let embedding: Vec<f32> = crate::vector::text_to_embedding("交流", 128)
                .iter()
                .map(|v| *v as f32)
                .collect();
            for (id, stage, superseded) in [
                ("active", "active", None),
                ("forgotten", "summarized", None),
                ("replaced", "active", Some("active")),
                ("deleted", "active", None),
            ] {
                conn.execute("INSERT INTO memories (id,scope,category,content,importance,source,forget_stage,superseded_by,created_at,updated_at) VALUES (?1,'global','preference',?2,5,'user',?3,?4,1,1)", params![id, format!("交流-{id}"), stage, superseded]).unwrap();
                let rowid = crate::vector_store::get_or_create_rowid(&conn, id).unwrap();
                crate::vector_store::upsert_embedding(&conn, rowid, &embedding).unwrap();
            }
            conn.execute("DELETE FROM memories WHERE id='deleted'", [])
                .unwrap();
            assert_eq!(
                crate::vector_store::find_nearest(&conn, &embedding, 20)
                    .unwrap()
                    .len(),
                4
            );
            record_preferences(
                &conn,
                Some("day-1"),
                &[preference("交流时不要频繁打断")],
                "auto_evolution",
            )
            .unwrap();
        }
        let args = serde_json::json!({"query":"交流", "limit":20, "include_learned_habits":true});
        let vector = call(&registry, "recall_memories", args.clone());
        assert!(vector.success, "{}", vector.content);
        assert!(vector.content.contains("[habit:") && vector.content.contains("[memory:active]"));
        for id in ["forgotten", "replaced", "deleted"] {
            assert!(!vector.content.contains(id));
        }
        db.lock()
            .unwrap()
            .execute_batch("DROP TABLE memory_vectors")
            .unwrap();
        let keyword = call(&registry, "recall_memories", args);
        assert!(keyword.success);
        assert_eq!(keyword.content, vector.content);
        {
            let conn = db.lock().unwrap();
            for i in 0..25 {
                conn.execute("INSERT INTO memories (id,scope,category,content,importance,source,forget_stage,created_at,updated_at) VALUES (?1,'global','fact','交流',5,'user','active',1,1)", [format!("extra-{i}")]).unwrap();
            }
        }
        let handler = RecallMemoriesHandler {
            db: Some(db.clone()),
        };
        let bounded = handler.execute(
            &serde_json::json!({"query":"交流", "limit":100, "include_learned_habits":true}),
            Path::new("."),
        );
        assert!(bounded.success);
        assert_eq!(bounded.content.lines().count(), 20);
        assert!(bounded.content.contains("habit:"));
        db.lock()
            .unwrap()
            .execute_batch("DROP TABLE adaptive_constraints")
            .unwrap();
        let unavailable = call(
            &registry,
            "recall_memories",
            serde_json::json!({"query":"交流", "include_learned_habits":true}),
        );
        assert!(
            !unavailable.success,
            "storage failure must not masquerade as absent habits"
        );
    }
}

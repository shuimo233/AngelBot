//! Memory extension - long-term memory management tools
//!
//! Tools:
//! - remember_fact: Store a fact or preference
//! - recall_memories: Search and retrieve memories
//! - update_memory: Update an existing memory
//! - forget_memory: Mark a memory for forgetting

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
        }
        let args: Args = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => return ToolResult::error("recall_memories", format!("参数错误: {}", e)),
        };
        let limit = args.limit.unwrap_or(5);
        let tf_emb = crate::vector::text_to_embedding(&args.query, 128);
        let query_f32: Vec<f32> = {
            let mut v: Vec<f32> = tf_emb.iter().map(|&x| x as f32).collect();
            v.resize(1536, 0.0);
            v
        };
        if let Ok(knn) = crate::vector_store::find_nearest(&db, &query_f32, limit) {
            if !knn.is_empty() {
                let rowids: Vec<i64> = knn.iter().map(|r| r.rowid).collect();
                if let Ok(mapping) = crate::vector_store::rowids_to_memory_ids(&db, &rowids) {
                    let id_list: Vec<String> = mapping.iter().map(|(_, id)| id.clone()).collect();
                    if !id_list.is_empty() {
                        let placeholders: Vec<String> = id_list
                            .iter()
                            .enumerate()
                            .map(|(i, _)| format!("?{}", i + 1))
                            .collect();
                        let sql = format!(
                            "SELECT category, content, importance FROM memories WHERE id IN ({})",
                            placeholders.join(",")
                        );
                        if let Ok(mut stmt) = db.prepare(&sql) {
                            let param_refs: Vec<Box<dyn rusqlite::types::ToSql>> = id_list
                                .iter()
                                .map(|id| Box::new(id.clone()) as Box<dyn rusqlite::types::ToSql>)
                                .collect();
                            let rows: Vec<String> = stmt
                                .query_map(
                                    rusqlite::params_from_iter(
                                        param_refs.iter().map(|p| p.as_ref()),
                                    ),
                                    |r| {
                                        Ok(format!(
                                            "[{}] {} (重要性:{})",
                                            r.get::<_, String>(0)?,
                                            r.get::<_, String>(1)?,
                                            r.get::<_, Option<i32>>(2)?.unwrap_or(0)
                                        ))
                                    },
                                )
                                .unwrap()
                                .filter_map(|r| r.ok())
                                .collect();
                            if !rows.is_empty() {
                                return ToolResult::success("recall_memories", rows.join("\n"));
                            }
                        }
                    }
                }
            }
        }
        let mut stmt = match db.prepare(
            "SELECT category, content, importance FROM memories
             WHERE forget_stage = 'active' AND superseded_by IS NULL
             AND content LIKE '%' || ?1 || '%'
             ORDER BY importance DESC, frequency DESC LIMIT ?2",
        ) {
            Ok(s) => s,
            Err(e) => return ToolResult::error("recall_memories", format!("查询失败: {}", e)),
        };
        let rows: Vec<String> = stmt
            .query_map(params![args.query, limit as i64], |r| {
                Ok(format!(
                    "[{}] {} (重要性:{})",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i32>>(2)?.unwrap_or(0)
                ))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        if rows.is_empty() {
            ToolResult::success("recall_memories", "没有找到相关记忆。")
        } else {
            ToolResult::success("recall_memories", rows.join("\n"))
        }
    }
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
        let now = chrono::Utc::now().timestamp();
        match db.execute(
            "UPDATE memories SET content = ?1, updated_at = ?2 WHERE id = ?3",
            params![args.new_content, now, args.memory_id],
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
        let now = chrono::Utc::now().timestamp();
        match db.execute(
            "UPDATE memories SET forget_stage = 'summarized', source = 'agent', updated_at = ?1 WHERE id = ?2",
            params![now, args.memory_id],
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
            "记住一个事实或偏好。当用户要求你记住某件事时使用。",
            serde_json::json!({"type":"object","properties":{"content":{"type":"string","description":"要记住的内容"},"category":{"type":"string","description":"类别: fact/preference/event/knowledge"},"importance":{"type":"integer","description":"重要性 1-7"}},"required":["content"]}),
        )
        .with_category(ToolCategory::Extension)
        .with_confirmation(true),
        Arc::new(RememberFactHandler { db: Some(db.clone()) }),
    );
    registry.register(
        Tool::new(
            "recall_memories",
            "搜索用户的记忆。当你需要了解用户的背景信息时使用。",
            serde_json::json!({"type":"object","properties":{"query":{"type":"string","description":"搜索关键词"},"limit":{"type":"integer","description":"返回条数"}},"required":["query"]}),
        )
        .with_category(ToolCategory::Extension),
        Arc::new(RecallMemoriesHandler { db: Some(db.clone()) }),
    );
    registry.register(
        Tool::new(
            "update_memory",
            "更新已有记忆的内容。当用户纠正或补充信息时使用。",
            serde_json::json!({"type":"object","properties":{"memory_id":{"type":"string","description":"记忆ID"},"new_content":{"type":"string","description":"新内容"}},"required":["memory_id","new_content"]}),
        )
        .with_category(ToolCategory::Extension)
        .with_confirmation(true),
        Arc::new(UpdateMemoryHandler { db: Some(db.clone()) }),
    );
    registry.register(
        Tool::new(
            "forget_memory",
            "标记记忆为遗忘。当用户要求忘记某事时使用。",
            serde_json::json!({"type":"object","properties":{"memory_id":{"type":"string","description":"要遗忘的记忆ID"}},"required":["memory_id"]}),
        )
        .with_category(ToolCategory::Extension)
        .with_confirmation(true),
        Arc::new(ForgetMemoryHandler { db: Some(db.clone()) }),
    );
}

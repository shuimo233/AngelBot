//! Knowledge extension - knowledge graph tools
//!
//! Tools:
//! - extract_entities: Extract entities from text
//! - traverse_graph: Traverse the knowledge graph

use crate::agent::tool::{Tool, ToolCategory, ToolHandler, ToolResult};
use crate::agent::ToolRegistry;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::{Arc, Mutex};

// ─── Handlers ─────────────────────────────────────────────────────────────────

pub struct ExtractEntitiesHandler;

impl ToolHandler for ExtractEntitiesHandler {
    fn name(&self) -> &str {
        "extract_entities"
    }
    fn execute(&self, arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        let text = arguments.get("text").and_then(|t| t.as_str()).unwrap_or("");
        if text.is_empty() {
            return ToolResult::success("extract_entities", "未发现实体。");
        }
        ToolResult::success(
            "extract_entities",
            format!(
                "从文本中提取实体: 请分析以下文本中的关键实体（人物、主题、项目、事件）：\n{}",
                text
            ),
        )
    }
}

pub struct TraverseGraphHandler {
    pub db: Option<Arc<Mutex<Connection>>>,
}

impl ToolHandler for TraverseGraphHandler {
    fn name(&self) -> &str {
        "traverse_graph"
    }
    fn execute(&self, arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        let db = match &self.db {
            Some(d) => d.lock().map_err(|e| format!("DB: {}", e)),
            None => return ToolResult::error("traverse_graph", "数据库不可用"),
        };
        let db = match db {
            Ok(d) => d,
            Err(e) => return ToolResult::error("traverse_graph", e),
        };
        let entity = arguments
            .get("entity")
            .and_then(|t| t.as_str())
            .unwrap_or("");
        let hops = arguments.get("hops").and_then(|h| h.as_i64()).unwrap_or(2);
        let sql = format!(
            "WITH RECURSIVE graph_walk AS (
                SELECT n.id, n.entity_type, n.label, e.predicate, e.target_id, 1 as hop
                FROM knowledge_nodes n
                LEFT JOIN knowledge_edges e ON n.id = e.source_id
                WHERE n.label LIKE '%' || ?1 || '%'
                UNION ALL
                SELECT n2.id, n2.entity_type, n2.label, e2.predicate, e2.target_id, gw.hop + 1
                FROM graph_walk gw
                JOIN knowledge_edges e2 ON gw.target_id = e2.source_id
                JOIN knowledge_nodes n2 ON e2.target_id = n2.id
                WHERE gw.hop < {}
            )
            SELECT DISTINCT entity_type, label, predicate, hop FROM graph_walk ORDER BY hop",
            hops
        );
        let mut stmt = match db.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return ToolResult::success("traverse_graph", "知识图谱为空，没有相关节点。"),
        };
        let rows: Vec<String> = stmt
            .query_map(params![entity], |r| {
                Ok(format!(
                    "[{}] {} --{}--> (hop {})",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    r.get::<_, i32>(3)?
                ))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        if rows.is_empty() {
            ToolResult::success(
                "traverse_graph",
                format!("未找到与 '{}' 相关的知识图谱节点。", entity),
            )
        } else {
            ToolResult::success("traverse_graph", rows.join("\n"))
        }
    }
}

// ─── Registration ──────────────────────────────────────────────────────────────

pub fn register(registry: &mut ToolRegistry, db: Arc<Mutex<Connection>>) {
    registry.register(
        Tool::new(
            "extract_entities",
            "从文本中提取关键实体（人物、主题、项目、事件），用于构建知识图谱。",
            serde_json::json!({"type":"object","properties":{"text":{"type":"string","description":"要分析的文本"}},"required":["text"]}),
        ).with_category(ToolCategory::Extension),
        Arc::new(ExtractEntitiesHandler),
    );
    registry.register(
        Tool::new(
            "traverse_graph",
            "遍历知识图谱，查找与指定实体相关的节点和关系。用于理解实体之间的关联。",
            serde_json::json!({"type":"object","properties":{"entity":{"type":"string","description":"起始实体名称"},"hops":{"type":"integer","description":"遍历跳数(1-3)"}},"required":["entity"]}),
        ).with_category(ToolCategory::Extension),
        Arc::new(TraverseGraphHandler { db: Some(db.clone()) }),
    );
}

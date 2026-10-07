//! Goal extension - goal tracking tools
//!
//! Tools:
//! - create_goal: Create a new goal
//! - update_goal_progress: Update goal progress
//! - complete_goal: Mark a goal as completed

use crate::agent::tool::{Tool, ToolCategory, ToolHandler, ToolResult};
use crate::agent::ToolRegistry;
use rusqlite::{params, Connection};
use serde::Deserialize;
use std::path::Path;
use std::sync::{Arc, Mutex};

// ─── Handlers ─────────────────────────────────────────────────────────────────

pub struct CreateGoalHandler {
    pub db: Option<Arc<Mutex<Connection>>>,
}

impl ToolHandler for CreateGoalHandler {
    fn name(&self) -> &str {
        "create_goal"
    }
    fn execute(&self, arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        let db = match &self.db {
            Some(d) => d.lock().map_err(|e| format!("DB: {}", e)),
            None => return ToolResult::error("create_goal", "数据库不可用"),
        };
        let db = match db {
            Ok(d) => d,
            Err(e) => return ToolResult::error("create_goal", e),
        };
        #[derive(Deserialize)]
        struct Args {
            goal: String,
            parent_id: Option<String>,
        }
        let args: Args = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => return ToolResult::error("create_goal", format!("参数错误: {}", e)),
        };
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();
        match db.execute(
            "INSERT INTO agent_goals (id, session_id, goal_text, status, parent_goal_id, created_at) VALUES (?1, '', ?2, 'active', ?3, ?4)",
            params![id, args.goal, args.parent_id, now],
        ) {
            Ok(_) => ToolResult::success("create_goal", format!("目标已创建: {} (id: {})", args.goal, id)),
            Err(e) => ToolResult::error("create_goal", format!("创建失败: {}", e)),
        }
    }
}

pub struct UpdateGoalProgressHandler {
    pub db: Option<Arc<Mutex<Connection>>>,
}

impl ToolHandler for UpdateGoalProgressHandler {
    fn name(&self) -> &str {
        "update_goal_progress"
    }
    fn execute(&self, arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        let db = match &self.db {
            Some(d) => d.lock().map_err(|e| format!("DB: {}", e)),
            None => return ToolResult::error("update_goal_progress", "数据库不可用"),
        };
        let db = match db {
            Ok(d) => d,
            Err(e) => return ToolResult::error("update_goal_progress", e),
        };
        #[derive(Deserialize)]
        struct Args {
            goal_id: String,
            progress: i32,
        }
        let args: Args = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => return ToolResult::error("update_goal_progress", format!("参数错误: {}", e)),
        };
        let now = chrono::Utc::now().timestamp();
        match db.execute(
            "UPDATE agent_goals SET progress_pct = ?1, status = CASE WHEN ?1 >= 100 THEN 'done' ELSE 'active' END, completed_at = CASE WHEN ?1 >= 100 THEN ?2 ELSE completed_at END WHERE id = ?3",
            params![args.progress, now, args.goal_id],
        ) {
            Ok(n) if n > 0 => ToolResult::success("update_goal_progress", format!("进度更新: {}%", args.progress)),
            Ok(_) => ToolResult::error("update_goal_progress", "未找到该目标"),
            Err(e) => ToolResult::error("update_goal_progress", format!("更新失败: {}", e)),
        }
    }
}

pub struct CompleteGoalHandler {
    pub db: Option<Arc<Mutex<Connection>>>,
}

impl ToolHandler for CompleteGoalHandler {
    fn name(&self) -> &str {
        "complete_goal"
    }
    fn execute(&self, arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        let db = match &self.db {
            Some(d) => d.lock().map_err(|e| format!("DB: {}", e)),
            None => return ToolResult::error("complete_goal", "数据库不可用"),
        };
        let db = match db {
            Ok(d) => d,
            Err(e) => return ToolResult::error("complete_goal", e),
        };
        #[derive(Deserialize)]
        struct Args {
            goal_id: String,
            summary: Option<String>,
        }
        let args: Args = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => return ToolResult::error("complete_goal", format!("参数错误: {}", e)),
        };
        let now = chrono::Utc::now().timestamp();
        let summary = args.summary.unwrap_or_else(|| "目标已完成".to_string());
        match db.execute(
            "UPDATE agent_goals SET status = 'done', progress_pct = 100, summary = ?1, completed_at = ?2 WHERE id = ?3",
            params![summary, now, args.goal_id],
        ) {
            Ok(n) if n > 0 => ToolResult::success("complete_goal", format!("目标完成: {}", summary)),
            Ok(_) => ToolResult::error("complete_goal", "未找到该目标"),
            Err(e) => ToolResult::error("complete_goal", format!("更新失败: {}", e)),
        }
    }
}

// ─── Registration ──────────────────────────────────────────────────────────────

pub fn register(registry: &mut ToolRegistry, db: Arc<Mutex<Connection>>) {
    registry.register(
        Tool::new(
            "create_goal",
            "创建一个新目标。当用户提出复杂任务时，将其分解为可追踪的子目标。返回目标 ID 供后续更新。",
            serde_json::json!({"type":"object","properties":{"goal":{"type":"string","description":"目标描述"},"parent_id":{"type":"string","description":"父目标ID（如果是子目标）"}},"required":["goal"]}),
        )
        .with_category(ToolCategory::Extension)
        .with_confirmation(true),
        Arc::new(CreateGoalHandler { db: Some(db.clone()) }),
    );
    registry.register(
        Tool::new(
            "update_goal_progress",
            "更新目标的完成进度。进度达到 100% 时自动标记为完成。",
            serde_json::json!({"type":"object","properties":{"goal_id":{"type":"string","description":"目标ID"},"progress":{"type":"integer","description":"进度百分比 0-100"}},"required":["goal_id","progress"]}),
        )
        .with_category(ToolCategory::Extension)
        .with_confirmation(true),
        Arc::new(UpdateGoalProgressHandler { db: Some(db.clone()) }),
    );
    registry.register(
        Tool::new(
            "complete_goal",
            "标记目标为已完成，附上完成摘要。",
            serde_json::json!({"type":"object","properties":{"goal_id":{"type":"string","description":"目标ID"},"summary":{"type":"string","description":"完成摘要"}},"required":["goal_id"]}),
        )
        .with_category(ToolCategory::Extension)
        .with_confirmation(true),
        Arc::new(CompleteGoalHandler { db: Some(db.clone()) }),
    );
}

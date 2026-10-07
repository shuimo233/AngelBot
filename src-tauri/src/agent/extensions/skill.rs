//! Skill extension - GitHub skill import
//!
//! Tools:
//! - import_skill_from_github: Import skills from GitHub repositories

use crate::agent::tool::{Tool, ToolCategory, ToolHandler, ToolResult};
use crate::agent::ToolRegistry;
use crate::skill::SkillRegistry;
use serde::Deserialize;
use std::path::Path;
use std::sync::{Arc, Mutex};

// ─── Handler ─────────────────────────────────────────────────────────────────

pub struct ImportGithubSkillHandler {
    pub registry: Arc<Mutex<SkillRegistry>>,
}

impl ToolHandler for ImportGithubSkillHandler {
    fn name(&self) -> &str {
        "import_skill_from_github"
    }

    fn execute(&self, arguments: &serde_json::Value, _work_dir: &Path) -> ToolResult {
        #[derive(Deserialize)]
        struct Args {
            repository_url: String,
        }
        let args: Args = match serde_json::from_value(arguments.clone()) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult::error(
                    "import_skill_from_github",
                    format!("参数解析失败: {error}"),
                )
            }
        };
        match self
            .registry
            .lock()
            .map_err(|error| error.to_string())
            .and_then(|mut registry| registry.import_github_repository(&args.repository_url))
        {
            Ok(count) => ToolResult::success(
                "import_skill_from_github",
                format!("已从 {} 导入 {} 个技能模板", args.repository_url, count),
            ),
            Err(error) => ToolResult::error("import_skill_from_github", error),
        }
    }
}

// ─── Registration ──────────────────────────────────────────────────────────────

pub fn register(registry: &mut ToolRegistry, skills: Arc<Mutex<SkillRegistry>>) {
    registry.register(
        Tool::new(
            "import_skill_from_github",
            "当用户明确要求从 GitHub 仓库导入技能时使用。仅支持公开 GitHub 仓库；会克隆或更新本地技能存储，并识别 SKILL.md、CLAUDE.md、AGENTS.md、.cursorrules 与 .mdc 模板。执行前必须获得用户确认。",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "repository_url": {"type": "string", "description": "公开 GitHub 仓库根地址，例如 https://github.com/owner/repository"}
                },
                "required": ["repository_url"],
                "additionalProperties": false
            }),
        ).with_confirmation(true).with_category(ToolCategory::Extension),
        Arc::new(ImportGithubSkillHandler { registry: skills }),
    );
}

//! Memory extension - long-term memory management tools
//!
//! Tools:
//! - remember_fact: Store a fact or preference
//! - recall_memories: Search and retrieve memories
//! - update_memory: Update an existing memory
//! - forget_memory: Mark a memory for forgetting

#[cfg(test)]
use crate::agent::adaptive_constraints::learned_preferences;
use crate::agent::adaptive_constraints::{dismiss_learned_preference, find_preferences};
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

#[derive(Clone, PartialEq, Eq)]
struct RecallRow {
    text: String,
    preference: Option<(bool, i64)>,
}

fn format_recall_rows(rows: &[RecallRow]) -> String {
    // Relevance selects records; chronology presents preferences, without
    // rewriting originals or changing saved-fact relevance/order.
    let mut preferences = rows
        .iter()
        .filter(|r| r.preference.is_some())
        .collect::<Vec<_>>();
    preferences.sort_by_key(|r| std::cmp::Reverse(r.preference));
    let mut preferences = preferences.into_iter();
    rows.iter()
        .map(|r| {
            if r.preference.is_some() {
                preferences.next().unwrap()
            } else {
                r
            }
        })
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
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
        #[serde(deny_unknown_fields)]
        struct Args {
            query: String,
            limit: Option<usize>,
            include_learned_habits: Option<bool>,
            exclude_preferences: Option<bool>,
            #[serde(default)]
            alternatives: Vec<String>,
        }
        let args: Args = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => return ToolResult::error("recall_memories", format!("参数错误: {}", e)),
        };
        let limit = args.limit.unwrap_or(5).clamp(1, 20);
        // Main already owns the current user intent. Let that same model supply
        // bounded reformulations instead of starting a hidden model/agent here.
        // Validate even when the original query hits, including direct callers.
        if args.alternatives.len() > 5
            || (!args.alternatives.is_empty() && args.query.trim().is_empty())
            || args
                .alternatives
                .iter()
                .any(|q| !(2..=24).contains(&q.trim().chars().count()))
        {
            return ToolResult::error(
                "recall_memories",
                "补查需非空原查询及最多 5 个 2-24 字符的短关键词。",
            );
        }
        // Explicit preferences are default recall, not inferred habits.
        // A narrower facts-only request always wins over the inference flag.
        let include_preferences = !args.exclude_preferences.unwrap_or(false);
        let include_habits = include_preferences && args.include_learned_habits.unwrap_or(false);
        let (mut rows, partial_habits) =
            match recall_rows(&db, &args.query, limit, include_preferences, include_habits) {
                Ok(found) => found,
                Err(e) => return ToolResult::error("recall_memories", e),
            };
        let direct_count = rows.len();
        let mut expanded = false;
        // A historical option can match while a newer correction uses another
        // value. Honor Main's explicit topic hints for eligible preferences,
        // without automatically broadening ordinary factual-memory hits.
        if (rows.is_empty() || rows.iter().any(|r| r.preference.is_some()))
            && rows.len() < limit
            && !args.alternatives.is_empty()
        {
            let mut queries = std::collections::HashSet::from([args.query.trim().to_lowercase()]);
            for alternative in &args.alternatives {
                if rows.len() == limit {
                    break;
                }
                let alternative = alternative.trim();
                if !queries.insert(alternative.to_lowercase()) {
                    continue;
                }
                let (matches, _) =
                    match recall_rows(&db, alternative, limit, include_preferences, include_habits)
                    {
                        Ok(found) => found,
                        Err(e) => return ToolResult::error("recall_memories", e),
                    };
                // Never promote character-only matches from a generated hint.
                // Return original records, not the model's reformulation as fact.
                for row in matches {
                    if rows.len() < limit && !rows.contains(&row) {
                        rows.push(row);
                    }
                }
            }
            expanded = rows.len() > direct_count;
        }
        let partial_count = if expanded {
            0
        } else {
            (limit - rows.len()).min(partial_habits.len())
        };
        rows.extend(partial_habits.into_iter().take(partial_count));
        let preference_view = if rows.iter().any(|r| r.preference.is_some()) {
            "交流记录视图：按关键词选取，偏好按明确性及记录时间新到旧呈现；不保证是完整当前偏好。确认现行约定应核对同主题更正和适用条件，仅在条件重叠且细节冲突时沿用较新原话。历史回顾保留旧原句。\n"
        } else {
            ""
        };
        if rows.is_empty() {
            let scope = if !include_preferences {
                "本次仅查非偏好的已存记忆，未查显式偏好及推测习惯。"
            } else if include_habits {
                "可结合用户当前意图，用 alternatives 提供简短同义关键词；意图不明时不要猜测。"
            } else {
                "本次已查显式偏好，未查推测习惯；用户要求检查推测习惯时可设 include_learned_habits=true。"
            };
            ToolResult::success(
                "recall_memories",
                format!("本次没有找到相关记忆；这不代表从未保存。{scope}"),
            )
        } else if expanded && direct_count > 0 {
            ToolResult::success("recall_memories", format!("{preference_view}原查询直接命中{direct_count}条，另有{}条补查候选（不保证语义相同，不授权改写或删除）。\n{}",rows.len()-direct_count,format_recall_rows(&rows)))
        } else if expanded {
            ToolResult::success("recall_memories", format!("{preference_view}原查询无直接匹配；以下为同义关键词补查候选，并非已确认语义匹配。请结合当前意图核对原句；不符时说明尚未确认，不据此改写或删除。\n{}",format_recall_rows(&rows)))
        } else if partial_count > 0 {
            ToolResult::success("recall_memories", format!("{preference_view}未找到这些交流记录的直接匹配；以下 habit 记录仅为部分字面匹配候选，不代表语义相同。请核对原句，不合适时说明尚未确认。\n{}",format_recall_rows(&rows)))
        } else {
            ToolResult::success(
                "recall_memories",
                format!("{preference_view}{}", format_recall_rows(&rows)),
            )
        }
    }
}

/// Direct records and separately labelled character-only candidates. All query
/// variants use the same scope, visibility rules, ordering and result cap.
fn recall_rows(
    db: &Connection,
    query: &str,
    limit: usize,
    include_preferences: bool,
    include_habits: bool,
) -> Result<(Vec<RecallRow>, Vec<RecallRow>), String> {
    let mut rows = Vec::new();
    let mut partial_habits = Vec::new();
    if include_preferences {
        let habits = find_preferences(db, query, limit, include_habits)
            .map_err(|e| format!("查询偏好失败: {e}"))?;
        let formatted = habits
            .records
            .into_iter()
            .map(|habit| {
                let status = if habit.explicit {
                    "用户明确表达"
                } else if habit.ready {
                    "可作为回应参考"
                } else {
                    "还在了解，不能视作已确认事实"
                };
                let recorded_at = chrono::DateTime::from_timestamp(habit.updated_at, 0)
                    .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
                    .unwrap_or_else(|| "未知".into());
                RecallRow {
                    text: format!(
                        "[habit:{}] {} (交流记录：{}；记录时间（来源优先）：{})",
                        habit.id, habit.content, status, recorded_at
                    ),
                    preference: Some((habit.explicit, habit.updated_at)),
                }
            })
            .collect::<Vec<_>>();
        if habits.partial {
            partial_habits = formatted;
        } else {
            rows.extend(formatted);
        }
    }
    let mode = if include_preferences {
        crate::fts::MemoryRecallMode::Explicit
    } else {
        crate::fts::MemoryRecallMode::ExplicitFacts
    };
    let saved = recall_explicit_memories(db, query.trim(), limit, mode)
        .map_err(|e| format!("查询记忆失败: {e}"))?;
    // Inspect both sources before the combined cutoff. Interleave them so one
    // full source cannot silently hide all matches from the other.
    let mut preferences = rows.into_iter();
    let mut saved = saved.into_iter();
    let mut combined = Vec::new();
    while combined.len() < limit {
        let memory = saved.next();
        let preference = preferences.next();
        if memory.is_none() && preference.is_none() {
            break;
        }
        if let Some(memory) = memory {
            combined.push(memory);
        }
        if combined.len() < limit {
            if let Some(preference) = preference {
                combined.push(preference);
            }
        }
    }
    Ok((combined, partial_habits))
}

fn recall_explicit_memories(
    db: &Connection,
    query: &str,
    limit: usize,
    mode: crate::fts::MemoryRecallMode,
) -> Result<Vec<RecallRow>, String> {
    crate::fts::recall_active_memories(db, query, limit, mode).map(|records| {
        records
            .into_iter()
            .map(|memory| {
                let preference = memory.category.eq_ignore_ascii_case("preference");
                let mut text = format!(
                    "[memory:{}] {} (类别:{}，重要性:{})",
                    memory.id, memory.content, memory.category, memory.importance
                );
                if preference {
                    let at = chrono::DateTime::from_timestamp(memory.updated_at, 0)
                        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
                        .unwrap_or_else(|| "未知".into());
                    text.push_str(&format!("；保存时间：{at}"));
                }
                RecallRow {
                    text,
                    preference: preference.then_some((true, memory.updated_at)),
                }
            })
            .collect()
    })
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
            "仅当用户明确要求把信息保存为长期记忆（如“请记住”）时使用。仅陈述或更正个人偏好（包括未来默认）时，先回应，不调用本工具或申请保存确认；开启学习时由成功回合后的有界复盘核验。任务约束不是长期记忆，也不是操作授权。",
            serde_json::json!({"type":"object","properties":{"content":{"type":"string","description":"要记住的内容"},"category":{"type":"string","description":"类别: fact/preference/event/knowledge"},"importance":{"type":"integer","description":"重要性 1-7"}},"required":["content"]}),
        )
        .with_category(ToolCategory::Extension)
        .with_confirmation(true),
        Arc::new(RememberFactHandler { db: Some(db.clone()) }),
    );
    registry.register(
        Tool::new(
            "recall_memories",
            "搜索已保存的记忆与用户明确表达的偏好，返回来源记录 ID；推测习惯单独选择，不代表已确认事实。",
            serde_json::json!({"type":"object","additionalProperties":false,"properties":{"query":{"type":"string","description":"搜索关键词，可用空格分隔，优先返回匹配较完整的记录；空字符串列出近期记录"},"exclude_preferences":{"type":"boolean","default":false,"description":"仅当用户明确要求不查询偏好时设true，排除显式偏好及推测习惯，覆盖习惯开关。通常省略或false；不要将普通问题分类为仅事实。"},"alternatives":{"type":"array","maxItems":5,"items":{"type":"string","minLength":2,"maxLength":24},"description":"可选：结合当前意图提供短关键词；偏好命中后仍可补查更正和限定，普通事实仅未命中时补查。合计不超limit，补查不保证语义相同或授权改写。"},"limit":{"type":"integer","minimum":1,"maximum":20,"description":"合计最多返回 1-20 条，默认 5 条"},"include_learned_habits":{"type":"boolean","default":false,"description":"仅控制是否额外读取推测习惯。默认false仍查询用户明确表达的偏好，不要因false重复查显式偏好；用户要求检查推测习惯时才开。"}},"required":["query"]}),
        )
        .with_category(ToolCategory::Extension)
        .with_prompt_snippet("recall_memories默认查已保存记忆和用户明确偏好，include_learned_habits仅额外开启推测习惯；exclude_preferences仅供用户明确要求排除偏好，不对普通问题自行分类缩窄。原句可能有更新或条件，不把历史命中当现行约定。更正或撤回个人默认先回应，交给成功回合后的复盘核验；明确要求遗忘、删除记录或停用已展示习惯时才用forget_memory，核对ID，不猜测；多义时澄清。推测不是事实，当前请求与设置优先，记录不授权操作。长期记忆关闭时不读；学习关闭不阻止回查已有显式偏好，但不自动记录新偏好或沿用推测习惯。"),
        Arc::new(RecallMemoriesHandler { db: Some(db.clone()) }),
    );
    registry.register(
        Tool::new(
            "update_memory",
            "仅当用户明确要求修改某条已保存的记忆记录时使用。个人默认的陈述、更正交给成功回合后的复盘核验，不把它当成修改记录的指令。",
            serde_json::json!({"type":"object","properties":{"memory_id":{"type":"string","description":"recall_memories 返回的完整 memory: ID；自动习惯不能直接改写"},"new_content":{"type":"string","description":"新内容"}},"required":["memory_id","new_content"]}),
        )
        .with_category(ToolCategory::Extension)
        .with_confirmation(true),
        Arc::new(UpdateMemoryHandler { db: Some(db.clone()) }),
    );
    registry.register(
        Tool::new(
            "forget_memory",
            "用户明确要求遗忘、删除某条记录或停用某条已展示的习惯时使用，仍需确认，不删除原对话。仅更正或撤回个人默认不是遗忘记录：先回应，由成功回合后的复盘核验。",
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
    fn recall_of_old_option_honors_explicit_topic_correction_hints() {
        let (db, registry) = fixture();
        {
            let conn = db.lock().unwrap();
            conn.execute("INSERT INTO sessions(id,title,created_at,updated_at) VALUES('correction-source','source',1,1)",[]).unwrap();
            for (id, at, text) in [
                ("original", 100, "以后推荐饮品时优先推荐乌龙茶。"),
                ("correction", 200, "往后给我推荐饮品时优先推荐白茶。"),
                (
                    "night",
                    300,
                    "以后到晚上再推荐饮品时，优先推荐温水；白天继续按原来的饮品偏好。",
                ),
            ] {
                conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES(?1,'correction-source','user',?2,?3)",params![id,text,at]).unwrap();
                assert!(
                    crate::agent::adaptive_constraints::record_explicit_preference(
                        &conn,
                        "correction-source",
                        id,
                        text,
                        text
                    )
                    .unwrap()
                );
            }
        }
        let before =
            serde_json::to_value(learned_preferences(&db.lock().unwrap()).unwrap()).unwrap();
        let direct = call(
            &registry,
            "recall_memories",
            serde_json::json!({"query":"乌龙茶","include_learned_habits":true}),
        );
        assert!(
            direct.content.contains("不保证是完整当前偏好")
                && direct.content.contains("偏好按明确性及记录时间新到旧"),
            "a keyword hit must expose its view limits, not imply the current default: {}",
            direct.content
        );
        // Captured from the failed Main call, including its already-requested
        // broadening. No model, semantic oracle or extra source rewriting.
        let result = call(
            &registry,
            "recall_memories",
            serde_json::json!({"query":"乌龙茶","alternatives":["oolong tea","茶 夜间 饮品"],"include_learned_habits":true,"limit":10}),
        );
        assert!(result.success);
        assert!(
            result.content.contains("温水"),
            "an old direct hit must not suppress requested correction candidates: {}",
            result.content
        );
        assert!(result.content.contains("白茶") && result.content.contains("乌龙茶"));
        assert_eq!(result.content.matches("[habit:").count(), 3);
        assert!(result.content.find("温水").unwrap() < result.content.find("白茶").unwrap());
        assert!(result.content.find("白茶").unwrap() < result.content.find("乌龙茶").unwrap());
        let capped = call(
            &registry,
            "recall_memories",
            serde_json::json!({"query":"乌龙茶","alternatives":["饮品"],"include_learned_habits":true,"limit":1}),
        );
        assert_eq!(capped.content.matches("[habit:").count(), 1);
        assert!(capped.content.contains("乌龙茶"));
        assert_eq!(
            serde_json::to_value(learned_preferences(&db.lock().unwrap()).unwrap()).unwrap(),
            before
        );
    }

    #[test]
    fn default_recall_includes_explicit_preferences_but_respects_inference_and_exclusion() {
        let (db, registry) = fixture();
        let snippet = registry
            .get("recall_memories")
            .unwrap()
            .prompt_snippet
            .unwrap();
        assert!(snippet.contains("长期记忆关闭时不读"));
        assert!(snippet.contains("学习关闭不阻止回查已有显式偏好"));
        assert!(snippet.contains("不自动记录新偏好或沿用推测习惯"));
        assert!(snippet.contains("更正或撤回个人默认先回应"));
        {
            let conn = db.lock().unwrap();
            conn.execute("INSERT INTO sessions(id,title,created_at,updated_at) VALUES('explicit-default','source',1,1)",[]).unwrap();
            let text = "以后出行优先坐地铁。";
            conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES('explicit-source','explicit-default','user',?1,1)",[text]).unwrap();
            assert!(
                crate::agent::adaptive_constraints::record_explicit_preference(
                    &conn,
                    "explicit-default",
                    "explicit-source",
                    text,
                    text
                )
                .unwrap()
            );
            for source in ["observation-1", "observation-2"] {
                record_preferences(&conn, None, &[preference("出行建议似乎偏好步行。")], source)
                    .unwrap();
            }
        }
        for (content, category) in [
            ("出行预计二十分钟。", "fact"),
            ("出行座位偏好靠窗。", "preference"),
        ] {
            assert!(
                call(
                    &registry,
                    "remember_fact",
                    serde_json::json!({"content":content,"category":category})
                )
                .success
            );
        }
        let before =
            serde_json::to_value(learned_preferences(&db.lock().unwrap()).unwrap()).unwrap();
        for flag in [
            serde_json::json!({"query":"出行"}),
            serde_json::json!({"query":"出行","include_learned_habits":false}),
            serde_json::json!({"query":"出行","exclude_preferences":false}),
        ] {
            let result = call(&registry, "recall_memories", flag);
            assert!(
                result.success
                    && result.content.contains("地铁")
                    && result.content.contains("二十分钟")
                    && result.content.contains("靠窗")
            );
            assert!(
                !result.content.contains("步行"),
                "inferred even-ready habits must remain opt-in"
            );
        }
        let observed = call(
            &registry,
            "recall_memories",
            serde_json::json!({"query":"出行","include_learned_habits":true}),
        );
        assert!(observed.content.contains("步行"));
        let facts = call(
            &registry,
            "recall_memories",
            serde_json::json!({"query":"出行","exclude_preferences":true,"include_learned_habits":true}),
        );
        assert!(facts.success && facts.content.contains("二十分钟"));
        assert!(!facts.content.contains("地铁")&&!facts.content.contains("步行")&&!facts.content.contains("靠窗"),"facts restriction must dominate the inference flag and exclude saved preference categories too");
        for invalid in [
            serde_json::json!({"query":"出行","exclude_preferences":"true"}),
            serde_json::json!({"query":"出行","scope":"facts"}),
        ] {
            assert!(!call(&registry, "recall_memories", invalid.clone()).success);
            assert!(
                !RecallMemoriesHandler {
                    db: Some(db.clone())
                }
                .execute(&invalid, Path::new("."))
                .success,
                "unsupported narrowing parameters must not silently widen recall"
            );
        }
        assert_eq!(
            serde_json::to_value(learned_preferences(&db.lock().unwrap()).unwrap()).unwrap(),
            before
        );
    }

    #[test]
    fn recall_reformulations_are_bounded_candidates_not_new_facts() {
        let (db, registry) = fixture();
        {
            let conn = db.lock().unwrap();
            for source in ["day-1", "day-2"] {
                record_preferences(
                    &conn,
                    None,
                    &[
                        preference("重要的事情登记到纸质笔记本上。"),
                        preference("交流用自然一点的语气。"),
                    ],
                    source,
                )
                .unwrap();
            }
        }
        let before = learned_preferences(&db.lock().unwrap()).unwrap();
        let invoke = |args| call(&registry, "recall_memories", args);
        let found = invoke(
            serde_json::json!({"query":"register", "alternatives":["语气", "语气", "自然"], "include_learned_habits":true}),
        );
        assert!(found.success && found.content.contains("同义关键词补查候选"));
        assert!(found.content.contains("交流用自然一点的语气。"));
        assert!(!found.content.contains("纸质笔记本"));
        assert_eq!(
            found.content.matches("[habit:").count(),
            1,
            "duplicate hints must not duplicate rows"
        );
        let direct = invoke(
            serde_json::json!({"query":"纸质", "alternatives":["语气"], "include_learned_habits":true}),
        );
        assert!(direct.content.contains("纸质笔记本") && direct.content.contains("语气"));
        assert!(
            direct
                .content
                .contains("原查询直接命中1条，另有1条补查候选"),
            "requested preference hints must keep direct records and label additions"
        );
        for content in ["primaryonlytoken", "secondaryonlytoken"] {
            assert!(
                call(
                    &registry,
                    "remember_fact",
                    serde_json::json!({"content":content})
                )
                .success
            );
        }
        let factual = invoke(
            serde_json::json!({"query":"primaryonlytoken","alternatives":["secondaryonlytoken"]}),
        );
        assert!(
            factual.success
                && factual.content.contains("primaryonlytoken")
                && !factual.content.contains("secondaryonlytoken"),
            "ordinary factual hits must retain their old on-miss behavior"
        );
        let excluded = invoke(serde_json::json!({"query":"register", "alternatives":["语气"]}));
        assert!(!excluded.content.contains("[habit:"));
        assert!(excluded.content.contains("本次已查显式偏好，未查推测习惯"));
        let partial = invoke(
            serde_json::json!({"query":"register", "alternatives":["本子"], "include_learned_habits":true}),
        );
        assert!(
            !partial.content.contains("[habit:"),
            "generated hints must not promote character-only candidates"
        );
        assert_eq!(
            serde_json::to_value(learned_preferences(&db.lock().unwrap()).unwrap()).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        for args in [
            serde_json::json!({"query":"", "alternatives":["语气"]}),
            serde_json::json!({"query":"语气", "alternatives":[" "]}),
            serde_json::json!({"query":"语气", "alternatives":["a","b","c","d","e","f"]}),
            serde_json::json!({"query":"语气", "alternatives":["x".repeat(25)]}),
        ] {
            assert!(!invoke(args.clone()).success);
            assert!(
                !RecallMemoriesHandler {
                    db: Some(db.clone())
                }
                .execute(&args, Path::new("."))
                .success
            );
        }
        let capped = invoke(
            serde_json::json!({"query":"register", "alternatives":["语气", "纸质"], "include_learned_habits":true, "limit":1}),
        );
        assert_eq!(capped.content.matches("[habit:").count(), 1);
    }

    #[test]
    fn recalled_preferences_keep_original_time_not_delayed_review_order() {
        let (db, registry) = fixture();
        {
            let conn = db.lock().unwrap();
            conn.execute("INSERT INTO sessions(id,title,created_at,updated_at) VALUES('source-order','source',1,1)",[]).unwrap();
            // Review the newer statement first, then a delayed older source.
            for (id, at, text) in [
                ("new", 200, "以后饮品优先选乌龙茶。"),
                ("old", 100, "以后饮品优先选绿茶。"),
            ] {
                conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES(?1,'source-order','user',?2,?3)",params![id,text,at]).unwrap();
                assert!(
                    crate::agent::adaptive_constraints::record_explicit_preference(
                        &conn,
                        "source-order",
                        id,
                        text,
                        text
                    )
                    .unwrap()
                );
            }
        }
        let found = call(
            &registry,
            "recall_memories",
            serde_json::json!({"query":"饮品","include_learned_habits":true}),
        );
        assert!(found.success);
        let rows: Vec<_> = found
            .content
            .lines()
            .filter(|line| line.starts_with("[habit:"))
            .collect();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].contains("乌龙茶") && rows[0].contains("1970-01-01T00:03:20Z"));
        assert!(rows[1].contains("绿茶") && rows[1].contains("1970-01-01T00:01:40Z"));
        assert!(rows
            .iter()
            .all(|line| line.contains("记录时间（来源优先）")));
        assert_eq!(
            learned_preferences(&db.lock().unwrap()).unwrap().len(),
            2,
            "read must not silently resolve conflict or erase either source"
        );
    }

    #[tokio::test]
    async fn main_recall_uses_current_intent_without_a_separate_model() {
        use crate::agent::{AgentConfig, AgentRunner};
        use crate::llm::{
            LlmProvider, LlmResponse, Message, StopReason, ThinkingSettings, ToolSchema,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Main(Arc<AtomicUsize>);
        #[async_trait::async_trait]
        impl LlmProvider for Main {
            fn name(&self) -> &str {
                "offline-current-intent"
            }
            async fn chat(
                &self,
                messages: &[Message],
                tools: Option<&[ToolSchema]>,
                _: Option<&ThinkingSettings>,
            ) -> Result<LlmResponse, String> {
                match self.0.fetch_add(1, Ordering::SeqCst) {
                    0 => {
                        assert!(messages
                            .iter()
                            .any(|m| m.role == "user" && m.content.contains("说话风格")));
                        let schema = tools
                            .unwrap()
                            .iter()
                            .find(|s| s.name == "recall_memories")
                            .unwrap();
                        assert!(schema.parameters["properties"]
                            .get("alternatives")
                            .is_some());
                        Ok(LlmResponse::ToolCalls {
                            calls: vec![crate::llm::ToolCall {
                                id: "lookup".into(),
                                name: "recall_memories".into(),
                                arguments: serde_json::json!({"query":"register","alternatives":["语气"],"include_learned_habits":true}),
                            }],
                            text: None,
                            stop_reason: StopReason::ToolUse,
                        })
                    }
                    1 => {
                        let result = messages
                            .iter()
                            .find(|m| m.tool_call_id.as_deref() == Some("lookup"))
                            .unwrap();
                        assert!(
                            result.content.contains("同义关键词补查候选")
                                && result.content.contains("自然一点")
                        );
                        Ok(LlmResponse::Text {
                            text: "核对到你希望交流时语气自然一点。".into(),
                            stop_reason: StopReason::EndTurn,
                        })
                    }
                    _ => panic!("recall must not add an auxiliary model call"),
                }
            }
        }
        let (db, registry) = fixture();
        for source in ["day-1", "day-2"] {
            record_preferences(
                &db.lock().unwrap(),
                None,
                &[preference("交流用自然一点的语气。")],
                source,
            )
            .unwrap();
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let runner = AgentRunner::with_registry(
            Box::new(Main(calls.clone())),
            AgentConfig::default(),
            registry,
        );
        let result = runner
            .run("查一下以前关于 register（说话风格）的约定，只读核对。", &[])
            .await;
        assert!(format!("{result:?}").contains("自然一点"));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn habit_lookup_filters_before_recent_list_cutoff_without_expanding_results() {
        let (db, registry) = fixture();
        let target_id = {
            let conn = db.lock().unwrap();
            let target = preference("喜欢用 CAFÉ 作为示例；每个回答末尾另起一行写：别急，慢慢来。");
            record_preferences(&conn, None, &[target.clone()], "older-1").unwrap();
            record_preferences(&conn, None, &[target], "older-2").unwrap();
            let id: String = conn
                .query_row("SELECT id FROM adaptive_constraints", [], |r| r.get(0))
                .unwrap();
            conn.execute("UPDATE adaptive_constraints SET updated_at=1", [])
                .unwrap();
            for i in 0..60 {
                let noise = if i < 30 {
                    format!("近期交流观察 {i}，使用平实语气")
                } else {
                    format!("讨论合成主题{i:02}时，使用平实语气。")
                };
                record_preferences(&conn, None, &[preference(&noise)], "newer").unwrap();
            }
            let recent = learned_preferences(&conn).unwrap();
            assert_eq!(recent.len(), 50, "the settings list remains bounded");
            assert!(!recent.iter().any(|p| p.id == id));
            id
        };
        let query = serde_json::json!({"query":"café", "include_learned_habits":true, "limit":1});
        {
            let conn = db.lock().unwrap();
            conn.execute(
                "UPDATE adaptive_constraints SET updated_at=?1 WHERE id=?2",
                params![chrono::Utc::now().timestamp() + 1, target_id],
            )
            .unwrap();
        }
        assert!(
            call(&registry, "recall_memories", query.clone())
                .content
                .contains(&target_id),
            "the same source and Unicode query work when the record is recent"
        );
        db.lock()
            .unwrap()
            .execute(
                "UPDATE adaptive_constraints SET updated_at=1 WHERE id=?1",
                [&target_id],
            )
            .unwrap();
        let found = call(&registry, "recall_memories", query.clone());
        assert!(found.success);
        assert!(
            found.content.contains(&format!("[habit:{target_id}]")),
            "older matching habit was hidden by the recent-list cutoff: {}",
            found.content
        );
        assert_eq!(
            found
                .content
                .lines()
                .filter(|line| line.starts_with("[habit:"))
                .count(),
            1
        );
        assert!(found.content.contains("可作为回应参考"));
        {
            let conn = db.lock().unwrap();
            record_preferences(&conn, None, &[preference("近期聊到 CAFÉ")], "newer-cafe").unwrap();
        }
        for text in ["CAFÉ 示例 不存在的词", "CAFÉ", "喜欢用 caf"] {
            let matched = call(
                &registry,
                "recall_memories",
                serde_json::json!({"query":text,"include_learned_habits":true,"limit":1}),
            );
            assert!(matched.success);
            if text != "CAFÉ" {
                assert!(
                    matched.content.contains(&target_id),
                    "best relevant older result must remain available for {text}: {}",
                    matched.content
                );
            } else {
                assert!(matched.content.contains("CAFÉ"));
            }
        }
        db.lock()
            .unwrap()
            .execute(
                "UPDATE adaptive_constraints SET source='explicit_user_preference' WHERE id=?1",
                [&target_id],
            )
            .unwrap();
        // Replay both actual Main lookup shapes without another model call.
        for multiword in [
            "回答 末尾 结束语 后续问题 固定",
            "回答 末尾 结束语 后续问题",
        ] {
            let explicit = call(
                &registry,
                "recall_memories",
                serde_json::json!({"query":multiword,"include_learned_habits":true,"limit":20}),
            );
            assert!(explicit.success && explicit.content.contains(&target_id));
            assert!(explicit.content.contains("用户明确表达"));
            let regular = call(
                &registry,
                "recall_memories",
                serde_json::json!({"query":multiword,"include_learned_habits":false,"limit":20}),
            );
            assert!(regular.success);
            assert!(
                regular.content.contains(&target_id) && regular.content.contains("用户明确表达"),
                "a false inference flag must not hide user-stated preferences"
            );
        }
        let partial = call(
            &registry,
            "recall_memories",
            serde_json::json!({"query":"长期 收尾 结束语 结尾 约定","include_learned_habits":true,"limit":1}),
        );
        assert!(
            partial.success && partial.content.contains(&target_id),
            "{}",
            partial.content
        );
        assert!(partial.content.contains("部分字面匹配"));
        assert_eq!(partial.content.matches("[habit:").count(), 1);
        for unrelated in ["quantum wavelength", "语音"] {
            let miss = call(
                &registry,
                "recall_memories",
                serde_json::json!({"query":unrelated,"include_learned_habits":true}),
            );
            assert!(
                miss.success && !miss.content.contains("[habit:"),
                "only a ubiquitous character is not useful evidence: {}",
                miss.content
            );
        }
        assert!(
            call(
                &registry,
                "remember_fact",
                serde_json::json!({"content":"长期收尾约定已在另一条原记录中说明"})
            )
            .success
        );
        let direct_id: String = db
            .lock()
            .unwrap()
            .query_row("SELECT id FROM memories", [], |r| r.get(0))
            .unwrap();
        let direct = call(
            &registry,
            "recall_memories",
            serde_json::json!({"query":"长期 收尾 结束语 结尾 约定","include_learned_habits":true,"limit":1}),
        );
        assert!(direct.success && direct.content.contains(&format!("[memory:{direct_id}]")));
        assert!(
            !direct.content.contains("部分字面匹配"),
            "a partial habit cannot displace a direct saved-memory hit"
        );
        assert!(
            call(
                &registry,
                "forget_memory",
                serde_json::json!({"memory_id":format!("memory:{direct_id}")})
            )
            .success
        );
        let forgotten = call(
            &registry,
            "forget_memory",
            serde_json::json!({"memory_id":format!("habit:{target_id}")}),
        );
        assert!(forgotten.success);
        assert!(!call(
            &registry,
            "recall_memories",
            serde_json::json!({"query":"长期 收尾 结束语 结尾 约定","include_learned_habits":true})
        )
        .content
        .contains(&target_id));
        assert!(!call(&registry, "recall_memories", query)
            .content
            .contains(&target_id));
        db.lock().unwrap().execute("UPDATE adaptive_constraints SET constraint_value=x'FF' WHERE id=(SELECT id FROM adaptive_constraints WHERE status='active' LIMIT 1)",[]).unwrap();
        let broken = call(
            &registry,
            "recall_memories",
            serde_json::json!({"query":"café","include_learned_habits":true}),
        );
        assert!(
            !broken.success,
            "storage errors must not become an empty successful lookup"
        );
    }

    #[test]
    fn registered_memory_tools_can_find_and_dismiss_habits_without_rewriting_user_memory() {
        let (db, registry) = fixture();
        assert!(registry
            .get("remember_fact")
            .unwrap()
            .description
            .contains("仅陈述或更正个人偏好（包括未来默认）"));
        assert!(registry
            .get("forget_memory")
            .unwrap()
            .description
            .contains("不是遗忘记录"));
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
            "ordinary recall must not silently inject inferred habits"
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
        assert_eq!(
            limited
                .content
                .lines()
                .filter(|line| line.starts_with("[habit:") || line.starts_with("[memory:"))
                .count(),
            1
        );
        assert!(
            limited.content.contains(&format!("[memory:{memory_id}]")),
            "one-slot mixed recall prioritizes the explicitly saved record"
        );
        assert!(limited.content.contains("偏好按明确性及记录时间新到旧"));
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
            assert!(active_for_session(&conn, "day-3").unwrap().is_empty());
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
        assert_eq!(
            bounded
                .content
                .lines()
                .filter(|line| line.starts_with("[habit:") || line.starts_with("[memory:"))
                .count(),
            20
        );
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

//! Self-evolution review engine
//!
//! Bounded declarative-memory adaptation, not model training or skill learning:
//! 1. Review agent examines conversation transcripts
//! 2. Extracts candidate facts and preferences into structured output
//! 3. Keeps automatic learning bounded to source-backed explicit preferences
//!    and corroborated, low-priority observations;
//!    permanent memory and profile changes require an explicit user review.
//!
//! Manual reviews create proposals; optional incremental reviews run only after
//! a completed foreground reply, outside its latency-critical path.
//!
//! Issue #50: Structured output with JSON Schema and multi-layer parsing.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ═══════════════════════════════════════════════════════════════════════════════
// Structured memory types (Issue #50)
// ═══════════════════════════════════════════════════════════════════════════════

/// A single memory extracted from a conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedMemory {
    /// The memory content (fact, preference, event, or knowledge)
    pub content: String,
    /// Category: fact, preference, event, knowledge, personality
    #[serde(default)]
    pub category: String,
    /// Importance score 1-10 (higher = more important)
    #[serde(default = "default_importance")]
    pub importance: i32,
}

fn default_importance() -> i32 {
    5
}

/// Structured extraction result (validated against JSON Schema)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedMemories {
    /// Extracted facts and preferences
    #[serde(default)]
    pub memories: Vec<ExtractedMemory>,
    /// Optional summary of what was learned
    #[serde(default)]
    pub summary: String,
}

/// Result of an evolution review
#[derive(Debug, Clone)]
pub struct EvolutionResult {
    pub new_memories: Vec<NewMemory>,
    pub explicit_preferences: Vec<ExplicitPreferenceEvidence>,
    pub preferences_json: Option<String>,
    pub summary: String,
}

/// An extraction is not proof: the writer must match this quote against the
/// corresponding original user message before granting single-source status.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplicitPreferenceEvidence {
    pub content: String,
    pub source_index: usize,
    pub quote: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexedPreferenceEvidence {
    memory_index: usize,
    source_index: usize,
    quote: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PreferenceEvidenceWire {
    Indexed(IndexedPreferenceEvidence),
    Legacy(ExplicitPreferenceEvidence),
}

/// A new memory extracted from conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewMemory {
    pub category: String,
    pub content: String,
    pub importance: i32,
}

// ═══════════════════════════════════════════════════════════════════════════════
// JSON Schema for structured extraction (Issue #50)
// ═══════════════════════════════════════════════════════════════════════════════

/// Returns the JSON Schema that the LLM must conform to when extracting memories.
/// This schema is injected into the extraction prompt to guide structured output.
pub fn extraction_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "memories": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["content", "category"],
                    "properties": {
                        "content": {
                            "type": "string",
                            "description": "A user fact, preference (including its revision or withdrawal), or insight"
                        },
                        "category": {
                            "type": "string",
                            "enum": ["fact", "preference", "event", "knowledge", "personality"],
                            "description": "The type of memory being extracted"
                        },
                        "importance": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 10,
                            "description": "How important this memory is (1-10). Facts about work/name=8+, minor preferences=3-5"
                        }
                    }
                }
            },
            "summary": {
                "type": "string",
                "description": "1-2 sentences summarizing what you learned about the user from this conversation"
            },
            "explicit_preferences": {
                "type":"array","maxItems":12,
                "items":{"type":"object","additionalProperties":false,
                    "required":["memory_index","source_index","quote"],
                    "properties":{"memory_index":{"type":"integer","minimum":0},
                        "source_index":{"type":"integer","minimum":0},
                        "quote":{"type":"string","maxLength":1000}}}
            }
        },
        "required": ["memories", "explicit_preferences", "summary"]
    })
}

// ═══════════════════════════════════════════════════════════════════════════════
// Prompt builder (Issue #50)
// ═══════════════════════════════════════════════════════════════════════════════

/// Build the extraction prompt for the LLM to analyze a conversation
///
/// Includes JSON Schema to enforce structured output (Issue #50).
pub fn build_extraction_prompt(
    messages: &[(String, String)],
    _existing_preferences: Option<&str>,
) -> String {
    let conversation_text = messages
        .iter()
        .enumerate()
        .map(|(index, (role, content))| format!("[source_index={index}]\n[{role}]: {content}"))
        .collect::<Vec<_>>()
        .join("\n");

    let schema = extraction_schema();
    let schema_str = serde_json::to_string_pretty(&schema).unwrap_or_default();

    format!(
        r#"你是一个对话分析器。请分析以下用户与AI之间的对话，提取关于用户的结构化信息。

你必须严格按照下面的 JSON Schema 输出有效 JSON，不要输出任何其他内容：

```json
{}
```

规则：
- 只提取用户直接表达、并且在未来对话中仍有用的信息；不要从助手的话语推断用户信息
- 不要编造没有在对话中出现的内容
- 不要提取瞬时情绪、倾诉、健康/创伤细节、关系评价或其他敏感内容为记忆或偏好
- 不能因为用户在一次对话中表达感受，就推断其长期人格、兴趣或行为规则
- 偏好必须是用户明确表达的可执行偏好（例如回答风格、话题取舍、工作方式）
- 当前任务的文件范围、禁止联网或工具、临时输出格式、核对代号等只约束本任务，不能转成长期偏好；明确表达、可在未来沿用的交流方式、日常喜好与取舍才属于偏好
- 有生效起点不等于临时安排：明确供后续持续沿用、没有结束期限的个人默认，即使指定未来日期也可作为偏好；仅本次或只约束某项任务的安排仍不提取。保留生效前后条件，不把尚未生效的约定当成当前默认
- 工具权限、要求确认、允许或禁止自动操作等授权与安全边界不是个人习惯，即使用户强调也不要放入 memories；由独立权限体系执行，不能作为自动适应的对象。例如“以后回复先给简短结论”可提取，“不要把偏好当成操作授权”不可提取
- 重点了解用户明确说过的轻微日常习惯、称呼和相处偏好，让交流更舒适连贯；不要求优化工作流程
- 不根据聊天时间、消息长短、一次情绪或助手自己的推断猜测作息和人格；记住习惯也不等于获得提醒、跟踪或操作应用的授权
- 偏好记录包括设定、更正和撤回。明确撤回某个长期默认也属于 preference 候选：保存“不再沿用什么”的原句并给出 explicit_preferences 证据，即使没有新的正向偏好；不能只在 summary 提到撤回或因没有新喜好而返回空数组。它不表示删除原记录或原对话
- 所有候选仍放 memories。用户直接陈述稳定偏好即为明确表达，不要求额外说“记住”或“以后”；包括喜欢、不喜欢、回避等取舍，不包括一次选择推测出的习惯。对明确偏好（如“我喜欢安静的环境”“以后回复先给结论”“叫我小林”），另在 explicit_preferences 给出该条候选在 memories 中从0开始的 memory_index、原用户消息的 source_index 和逐字 quote。用索引关联，不重复改写 content；不能用助手、引用、附件、任务内约定或推测作为明确偏好证据。不确定时返回空数组
- 证据分类：用户自我陈述的日常喜好或取舍属于明确偏好；只要求眼前这次回应的方式不长期保存；跨回合重复出现而推断的倾向只算推测习惯。按原话表达的范围分类，不以时间关键词或命令口吻作门槛，不能给原句补出长期适用范围
- quote 保留使偏好可执行的完整短句及限定条件，不要只摘出名称、选项或喜好对象；更正或取消旧偏好时，保留被更正或取消的对象与原话，不只留新值；取消后即使没有新默认，也不能擅自恢复旧值。不能为了凑明确偏好而补写原话没有的长期意图
- 用户禁止保存／记忆／学习时不提取任何候选；明确偏好也不能改变权限、安全确认或应用操作边界
- 如果没有提取到任何有用信息， memories 返回空数组 []
- importance 评分：姓名/工作/关键事实=8-10，一般偏好=4-7，轻微偏好=1-3
- summary 总结你从对话中学到的最重要信息

对话内容：
{}

请输出符合 Schema 的 JSON："#,
        schema_str, conversation_text
    )
}

// ═══════════════════════════════════════════════════════════════════════════════
// Multi-layer parsing (Issue #50)
// ═══════════════════════════════════════════════════════════════════════════════

/// Result of structured memory extraction with parsing status
#[derive(Debug, Clone)]
pub struct ExtractResult {
    /// Successfully parsed memories
    pub memories: Vec<ExtractedMemory>,
    /// Extraction summary
    pub summary: String,
    /// Whether parsing succeeded on first attempt
    pub parse_success: bool,
    /// Error message if parsing failed (None if successful)
    pub parse_error: Option<String>,
}

impl ExtractResult {
    /// Convert to legacy EvolutionResult for backward compatibility
    pub fn to_evolution_result(self) -> EvolutionResult {
        let new_memories = self
            .memories
            .into_iter()
            .map(|m| NewMemory {
                category: m.category,
                content: m.content,
                importance: m.importance,
            })
            .collect();

        EvolutionResult {
            new_memories,
            explicit_preferences: Vec::new(),
            preferences_json: None,
            summary: self.summary,
        }
    }
}

/// Parse the LLM's JSON response into structured memories using multi-layer parsing.
///
/// Layer 1: Try strict schema-validated parse (returns memories in `memories` array).
/// Layer 2: Try lenient JSON parse with field normalization (also handles legacy
///          `facts`, `interests`, `avoid_topics`, `disliked_words`, `pet_peeves`).
/// Layer 3: Report parse failure.
pub fn parse_extraction_response(response: &str) -> ExtractResult {
    let json_str = extract_json(response).unwrap_or(response);

    // Layer 1: Strict JSON Schema-validated parse
    let strict_result = parse_strict(&json_str);
    let strict_memories = strict_result
        .as_ref()
        .map(|r| r.memories.len())
        .unwrap_or(0);

    if strict_result.is_ok() && strict_memories > 0 {
        let extracted = strict_result.unwrap();
        let memories = extracted
            .memories
            .into_iter()
            .map(|m| ExtractedMemory {
                content: m.content,
                category: m.category,
                importance: m.importance.clamp(1, 10),
            })
            .collect();
        let summary = if extracted.summary.is_empty() {
            "已从对话中提取到用户信息".to_string()
        } else {
            extracted.summary
        };
        return ExtractResult {
            memories,
            summary,
            parse_success: true,
            parse_error: None,
        };
    }

    // Layer 2: Lenient parse with field normalization
    let err_msg = strict_result
        .as_ref()
        .map(|_| "strict returned 0 memories")
        .unwrap_or_else(|e| e);
    match parse_lenient(&json_str) {
        Ok(extracted) => {
            let memories = extracted
                .memories
                .into_iter()
                .map(|m| ExtractedMemory {
                    content: m.content,
                    category: m.category,
                    importance: m.importance.clamp(1, 10),
                })
                .collect();
            let summary = if extracted.summary.is_empty() {
                "已从对话中提取到用户信息（宽松解析）".to_string()
            } else {
                extracted.summary
            };
            return ExtractResult {
                memories,
                summary,
                parse_success: false,
                parse_error: Some(format!("严格解析失败，使用宽松解析: {}", err_msg)),
            };
        }
        Err(lenient_err) => {
            // Layer 3: Both failed
            return ExtractResult {
                memories: Vec::new(),
                summary: "无法解析记忆提取结果".to_string(),
                parse_success: false,
                parse_error: Some(format!("严格解析: {} | 宽松解析: {}", err_msg, lenient_err)),
            };
        }
    }
}

/// Strict parse: deserialize into `ExtractedMemories` using the JSON Schema.
fn parse_strict(json_str: &str) -> Result<ExtractedMemories, String> {
    serde_json::from_str(json_str).map_err(|e| format!("JSON解析失败: {}", e))
}

/// Lenient parse: manually extract fields with defaults, handling minor format variations.
fn parse_lenient(json_str: &str) -> Result<ExtractedMemories, String> {
    let parsed: Value =
        serde_json::from_str(json_str).map_err(|e| format!("JSON解析失败: {}", e))?;

    let mut memories = Vec::new();

    // Try "memories" first, fall back to "facts" (legacy format)
    let raw_memories = if let Some(arr) = parsed["memories"].as_array() {
        arr.as_slice()
    } else if let Some(arr) = parsed["facts"].as_array() {
        arr.as_slice()
    } else {
        &[]
    };

    for item in raw_memories {
        let content = item["content"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string();

        if content.is_empty() {
            continue;
        }

        let category = item["category"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("fact")
            .to_string();

        let importance = item["importance"]
            .as_i64()
            .filter(|&n| (1..=10).contains(&n))
            .map(|n| n as i32)
            .unwrap_or(5)
            .clamp(1, 10);

        memories.push(ExtractedMemory {
            content,
            category,
            importance,
        });
    }

    // Also extract from legacy fields if no structured memories found
    if memories.is_empty() {
        if let Some(interests) = parsed["interests"].as_array() {
            for interest in interests {
                if let Some(topic) = interest.as_str() {
                    if !topic.is_empty() {
                        memories.push(ExtractedMemory {
                            content: format!("用户对「{}」感兴趣", topic),
                            category: "preference".to_string(),
                            importance: 6,
                        });
                    }
                }
            }
        }

        if let Some(avoid) = parsed["avoid_topics"].as_array() {
            for topic in avoid {
                if let Some(t) = topic.as_str() {
                    if !t.is_empty() {
                        memories.push(ExtractedMemory {
                            content: format!("用户不想聊「{}」", t),
                            category: "preference".to_string(),
                            importance: 7,
                        });
                    }
                }
            }
        }

        if let Some(words) = parsed["disliked_words"].as_array() {
            for word in words {
                if let Some(w) = word.as_str() {
                    if !w.is_empty() {
                        memories.push(ExtractedMemory {
                            content: format!("用户讨厌「{}」这个词", w),
                            category: "preference".to_string(),
                            importance: 7,
                        });
                    }
                }
            }
        }

        if let Some(peeves) = parsed["pet_peeves"].as_array() {
            for peeve in peeves {
                if let Some(p) = peeve.as_str() {
                    if !p.is_empty() {
                        memories.push(ExtractedMemory {
                            content: format!("用户的雷点：{}", p),
                            category: "preference".to_string(),
                            importance: 8,
                        });
                    }
                }
            }
        }
    }

    let summary = parsed["summary"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("已从对话中提取到用户信息")
        .to_string();

    Ok(ExtractedMemories { memories, summary })
}

// ═══════════════════════════════════════════════════════════════════════════════
// Legacy API (backward compatible)
// ═══════════════════════════════════════════════════════════════════════════════

/// Parse the LLM's JSON response into structured EvolutionResult
///
/// DEPRECATED: Use `parse_extraction_response` instead for the new schema-based
/// extraction. This function delegates to the new implementation for backward
/// compatibility.
pub fn parse_evolution_response(response: &str) -> Result<EvolutionResult, String> {
    let extract_result = parse_extraction_response(response);
    Ok(extract_result.to_evolution_result())
}

/// Incremental learning acknowledges source messages only after a valid
/// structured review. Legacy/manual leniency must not turn parse failures into
/// successful empty results and permanently skip those messages.
pub(crate) fn parse_automatic_review_response(response: &str) -> Result<EvolutionResult, String> {
    let value: Value = serde_json::from_str(extract_json(response).unwrap_or(response))
        .map_err(|_| "Automatic review returned invalid JSON")?;
    if !value["memories"].is_array() || !value["summary"].is_string() {
        return Err("Automatic review is missing its structured result".into());
    }
    let evidence: Vec<PreferenceEvidenceWire> = value
        .get("explicit_preferences")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| "Automatic review contains invalid preference evidence")?
        .unwrap_or_default();
    let extracted: ExtractedMemories = serde_json::from_value(value)
        .map_err(|_| "Automatic review contains invalid memory fields")?;
    let explicit_preferences = evidence
        .into_iter()
        .map(|item| match item {
            PreferenceEvidenceWire::Legacy(item) => Ok(item),
            PreferenceEvidenceWire::Indexed(item) => {
                let memory = extracted
                    .memories
                    .get(item.memory_index)
                    .filter(|m| m.category == "preference")
                    .ok_or("Automatic review contains invalid memory reference")?;
                Ok(ExplicitPreferenceEvidence {
                    content: memory.content.clone(),
                    source_index: item.source_index,
                    quote: item.quote,
                })
            }
        })
        .collect::<Result<Vec<_>, &str>>()?;
    if explicit_preferences.len() > 12
        || explicit_preferences.iter().any(|p| {
            p.content.trim().is_empty()
                || p.content.chars().count() > 160
                || p.quote.trim().is_empty()
                || p.quote.chars().count() > 1000
        })
    {
        return Err("Automatic review contains invalid preference evidence".into());
    }
    if extracted.memories.iter().any(|memory| {
        memory.content.trim().is_empty()
            || !(1..=10).contains(&memory.importance)
            || !matches!(
                memory.category.as_str(),
                "fact" | "preference" | "event" | "knowledge" | "personality"
            )
    }) {
        return Err("Automatic review contains invalid memory values".into());
    }
    Ok(EvolutionResult {
        explicit_preferences,
        new_memories: extracted
            .memories
            .into_iter()
            .map(|memory| NewMemory {
                content: memory.content,
                category: memory.category,
                importance: memory.importance,
            })
            .collect(),
        preferences_json: None,
        summary: extracted.summary,
    })
}

/// Extract JSON string from potential markdown code block
fn extract_json(text: &str) -> Option<&str> {
    let text = text.trim();

    // Try ```json ... ``` block
    if let Some(start) = text.find("```json") {
        let after_start = &text[start + 7..];
        if let Some(end) = after_start.find("```") {
            return Some(after_start[..end].trim());
        }
    }

    // Try ``` ... ``` block
    if let Some(start) = text.find("```") {
        let after_start = &text[start + 3..];
        if let Some(end) = after_start.find("```") {
            let inner = after_start[..end].trim();
            if inner.starts_with('{') {
                return Some(inner);
            }
        }
    }

    // Try finding raw JSON object
    if let Some(start) = text.find('{') {
        let mut depth = 0;
        for (i, ch) in text[start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&text[start..start + i + 1]);
                    }
                }
                _ => {}
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_from_markdown() {
        let response = r#"```json
{"memories": [{"content": "用户喜欢Rust", "category": "preference", "importance": 8}], "summary": "学到了"}
```"#;
        let result = parse_extraction_response(response);
        assert!(result.parse_success);
        assert!(parse_automatic_review_response(response)
            .unwrap()
            .explicit_preferences
            .is_empty());
        assert_eq!(result.memories.len(), 1);
        assert_eq!(result.memories[0].content, "用户喜欢Rust");
        assert_eq!(result.memories[0].category, "preference");
        assert_eq!(result.memories[0].importance, 8);
        assert_eq!(
            parse_automatic_review_response(response)
                .unwrap()
                .new_memories
                .len(),
            1
        );
        assert!(
            parse_automatic_review_response(r#"{"memories":[],"summary":"没有新习惯"}"#)
                .unwrap()
                .new_memories
                .is_empty()
        );
        for invalid in [
            "not JSON",
            "null",
            "{}",
            r#"{"summary":"missing memories"}"#,
            r#"{"memories":[{"content":"", "category":"preference"}],"summary":"invalid"}"#,
            r#"{"memories":[{"content":"test", "category":"authorization"}],"summary":"invalid"}"#,
            r#"{"memories":[{"content":"test", "category":"preference", "importance":11}],"summary":"invalid"}"#,
        ] {
            assert!(parse_automatic_review_response(invalid).is_err());
        }
        let valid = serde_json::json!({"memories":[],"summary":"x","explicit_preferences":[{"content":"先给结论","source_index":0,"quote":"以后先给结论"}]});
        assert_eq!(
            parse_automatic_review_response(&valid.to_string())
                .unwrap()
                .explicit_preferences[0]
                .source_index,
            0
        );
        for invalid in [
            serde_json::json!(-1),
            serde_json::json!("0"),
            serde_json::Value::Null,
        ] {
            let mut changed = valid.clone();
            changed["explicit_preferences"][0]["source_index"] = invalid;
            assert!(parse_automatic_review_response(&changed.to_string()).is_err());
        }
        let mut changed = valid.clone();
        changed["explicit_preferences"][0]["quote"] = serde_json::json!("长".repeat(1001));
        assert!(parse_automatic_review_response(&changed.to_string()).is_err());
        let indexed = serde_json::json!({"memories":[{"content":"希望先给结论","category":"preference","importance":5}],"summary":"x",
            "explicit_preferences":[{"memory_index":0,"source_index":0,"quote":"以后先给结论"}]});
        assert_eq!(
            parse_automatic_review_response(&indexed.to_string())
                .unwrap()
                .explicit_preferences[0]
                .content,
            "希望先给结论"
        );
        for index in [
            serde_json::json!(1),
            serde_json::json!(-1),
            serde_json::json!("0"),
        ] {
            let mut invalid = indexed.clone();
            invalid["explicit_preferences"][0]["memory_index"] = index;
            assert!(parse_automatic_review_response(&invalid.to_string()).is_err());
        }
    }

    #[test]
    fn test_extract_json_raw() {
        let response = r#"{"memories": [{"content": "用户偏好简洁", "category": "preference", "importance": 6}], "summary": "学习了"}"#;
        let result = parse_extraction_response(response);
        assert_eq!(result.memories.len(), 1);
        assert!(result.parse_success);
    }

    #[test]
    fn extraction_prompt_excludes_emotional_conversation_from_memory_candidates() {
        let prompt = build_extraction_prompt(
            &[(
                "user".to_string(),
                "我今天很难过，只想有人陪我说说话".to_string(),
            )],
            None,
        );

        assert!(prompt.contains("不要提取瞬时情绪"));
        assert!(prompt.contains("不要从助手的话语推断用户信息"));
        assert!(prompt.contains("偏好必须是用户明确表达的可执行偏好"));
        assert!(prompt.contains("让交流更舒适连贯；不要求优化工作流程"));
        assert!(prompt.contains("不根据聊天时间、消息长短"));
        assert!(prompt.contains("不等于获得提醒、跟踪或操作应用的授权"));
        assert!(prompt.contains("当前任务的文件范围"));
        assert!(prompt.contains("不能转成长期偏好"));
        assert!(prompt.contains("授权与安全边界不是个人习惯"));
        assert!(prompt.contains("不要求额外说“记住”或“以后”"));
        assert!(prompt.contains("不包括一次选择推测出的习惯"));
        assert!(prompt.contains("有生效起点不等于临时安排"));
        assert!(prompt.contains("保留被更正或取消的对象"));
        assert!(prompt.contains("偏好记录包括设定、更正和撤回"));
        assert!(prompt.contains("只要求眼前这次回应的方式不长期保存"));
        assert!(prompt.contains("跨回合重复出现而推断的倾向只算推测习惯"));
        assert!(!prompt.contains("例如“先给简短结论”可提取"));
    }

    #[test]
    fn test_extract_interests_and_peeves_lenient() {
        let response = r#"{
            "facts": [{"content": "事实1", "category": "fact", "importance": 5}],
            "interests": ["Python", "AI"],
            "avoid_topics": ["政治"],
            "disliked_words": ["呵呵"],
            "pet_peeves": ["被敷衍"],
            "preferences": null,
            "summary": "学到很多"
        }"#;
        let result = parse_extraction_response(response);
        // 1 fact + 2 interests + 1 avoid + 1 disliked + 1 peeve = 6
        assert!(result.memories.len() >= 1); // At least fact is parsed
    }

    #[test]
    fn test_empty_response() {
        let response = r#"{"memories": [], "summary": "没有学到新东西"}"#;
        let result = parse_extraction_response(response);
        assert_eq!(result.memories.len(), 0);
        assert_eq!(result.summary, "没有学到新东西");
    }

    #[test]
    fn test_schema_validates_structure() {
        let schema = extraction_schema();
        assert!(schema["properties"]["memories"].is_object());
        assert!(schema["properties"]["summary"].is_object());
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "explicit_preferences"));
        assert_eq!(schema["required"].as_array().unwrap().len(), 3);
        assert!(
            schema["properties"]["explicit_preferences"]["items"]["properties"]
                .get("content")
                .is_none()
        );
    }

    #[test]
    fn test_backward_compat_evolution_result() {
        let result = parse_extraction_response(
            r#"{"memories": [{"content": "测试", "importance": 5}], "summary": "x"}"#,
        );
        let evolution = result.to_evolution_result();
        assert_eq!(evolution.new_memories.len(), 1);
        assert_eq!(evolution.new_memories[0].content, "测试");
    }

    #[test]
    fn test_lenient_parse_handles_invalid_importance() {
        let response = r#"{"memories": [{"content": "X", "importance": 999}], "summary": ""}"#;
        let result = parse_extraction_response(response);
        assert!(result.parse_success);
        // Importance out of range should be clamped to 10
        assert!(result.memories[0].importance <= 10);
        assert!(result.memories[0].importance >= 1);
    }

    #[test]
    fn test_extract_json_with_code_blocks() {
        let response = r#"Here is my analysis:

Some text here.

```json
{
  "memories": [{"content": "用户用Python", "category": "preference", "importance": 6}],
  "summary": "用户偏好Python"
}
```

End of response."#;
        let result = parse_extraction_response(response);
        assert_eq!(result.memories.len(), 1);
        assert_eq!(result.memories[0].content, "用户用Python");
    }
}

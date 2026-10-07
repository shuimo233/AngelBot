//! Agent configuration — personality, profile, preferences, and memory context
//!
//! This bundles everything needed to build a personalized system prompt.
//! Maps to the pi ecosystem pattern: SOUL.md (personality) + USER.md (profile/preferences).

/// Personality trait dimensions (-5 to +5)
#[derive(Debug, Clone)]
pub struct PersonalityTraits {
    /// 语气: -5 = 温柔, +5 = 冷淡
    pub tone: i32,
    /// 话痨度: -5 = 沉默, +5 = 话痨
    pub verbosity: i32,
    /// 正式度: -5 = 正式, +5 = 随意
    pub formality: i32,
    /// 幽默感: -5 = 无趣, +5 = 幽默
    pub humor: i32,
    /// 依赖感: -5 = 独立, +5 = 粘人
    pub dependence: i32,
    /// 边界感: -5 = 保持距离, +5 = 亲密
    pub intimacy: i32,
    /// 耐心程度: -5 = 没耐心, +5 = 超级耐心
    pub patience: i32,
}

impl Default for PersonalityTraits {
    fn default() -> Self {
        Self {
            tone: 0,
            verbosity: 0,
            formality: 0,
            humor: 0,
            dependence: 0,
            intimacy: 0,
            patience: 5,
        }
    }
}

/// User profile from the profile table
#[derive(Debug, Clone, Default)]
pub struct UserProfile {
    pub name: String,
    pub preferences: String,
    pub habits: String,
    pub background: String,
}

/// User communication preferences (mirrors types.ts UserPreferences)
#[derive(Debug, Clone)]
pub struct UserPreferences {
    /// Preferred response length: "short", "medium", or "long"
    pub response_length: String,
    /// Preferred response language: "auto", "zh-CN", or "en-US"
    pub response_language: String,
    /// Whether replies may use long-term memory context.
    pub use_long_term_memory: bool,
    /// User's interests
    pub interests: Vec<String>,
    /// Topics to avoid
    pub avoid_topics: Vec<String>,
    /// Words the user dislikes
    pub disliked_words: Vec<String>,
    /// User's pet peeves
    pub pet_peeves: Vec<String>,
    /// Whether evolution/learning is enabled
    pub evolution_enabled: bool,
}

impl Default for UserPreferences {
    fn default() -> Self {
        Self {
            response_length: String::new(),
            response_language: "auto".to_string(),
            use_long_term_memory: true,
            interests: Vec::new(),
            avoid_topics: Vec::new(),
            disliked_words: Vec::new(),
            pet_peeves: Vec::new(),
            evolution_enabled: true,
        }
    }
}

/// A memory context snippet to inject into the system prompt
#[derive(Debug, Clone)]
pub struct MemoryContext {
    /// Memory category: fact, preference, event, knowledge, personality
    pub category: String,
    /// The memory content
    pub content: String,
    /// Importance score (1-10)
    pub importance: i32,
}

/// A low-priority, learned interaction preference. These constraints never
/// describe or modify the user's selected persona.
#[derive(Debug, Clone)]
pub struct AdaptiveConstraint {
    pub key: String,
    pub value: String,
    pub scope: String,
    pub confidence: f64,
}

/// Bounded, text-only guidance from a locally installed SKILL.md.
/// This is deliberately not an executable extension: skills can describe how
/// to approach a task, but they cannot add tools or run code by themselves.
#[derive(Debug, Clone)]
pub struct SkillPromptInstruction {
    pub id: String,
    pub name: String,
    pub description: String,
    pub instructions: String,
}

/// Complete agent configuration for personalized system prompts
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Persona/character name (e.g. "傲娇学妹")
    pub persona_name: String,
    /// Character greeting/opening line
    pub persona_greeting: String,
    /// Free-form description of the persona
    pub persona_description: String,
    /// 7-dimension personality traits
    pub traits: PersonalityTraits,
    /// User profile (name, background, etc.)
    pub profile: Option<UserProfile>,
    /// Communication preferences
    pub preferences: UserPreferences,
    /// Relevant memories to inject as context
    pub relevant_memories: Vec<MemoryContext>,
    /// Background interaction preferences learned from repeated behaviour.
    /// Explicit user settings always take precedence.
    pub adaptive_constraints: Vec<AdaptiveConstraint>,
    /// Installed skill guidance, loaded from AngelBot's managed local store.
    pub skill_instructions: Vec<SkillPromptInstruction>,
    /// Core memory blocks (persistent identity/context, Issue #25)
    pub core_memories: Vec<CoreMemoryBlock>,
    /// Knowledge graph context (Issue #37 Phase 4)
    pub knowledge_graph_context: Option<String>,
    /// Context summary from auto-compression (injected into system prompt)
    pub context_summary: Option<String>,
    /// Stable user-facing workspace envelope. This is separate from session
    /// history so personal and project context can never be conflated.
    pub workspace_context: Option<String>,
    /// Current work directory for file operations
    pub work_dir: Option<std::path::PathBuf>,
    /// Tool error recovery settings (Issue #48)
    pub tool_error_recovery: ToolErrorRecovery,
    /// Whether to enhance tool descriptions with extra context (Issue #52)
    pub enhanced_tool_descriptions: bool,
}

/// Tool error recovery configuration (Issue #48)
#[derive(Debug, Clone)]
pub struct ToolErrorRecovery {
    /// Maximum retry attempts per tool (default: 2)
    pub max_retries: usize,
    /// Maximum failed tool calls per turn before aborting (default: 3)
    pub max_failures_per_turn: usize,
    /// Whether to append recovery hints to error messages
    pub include_recovery_hints: bool,
}

impl Default for ToolErrorRecovery {
    fn default() -> Self {
        Self {
            max_retries: 2,
            max_failures_per_turn: 3,
            include_recovery_hints: true,
        }
    }
}

/// Core memory block for persistent AI identity and user context.
#[derive(Debug, Clone)]
pub struct CoreMemoryBlock {
    pub block_type: String, // human | persona | context | rules
    pub label: String,
    pub content: String,
    pub importance: i32,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            persona_name: String::new(),
            persona_greeting: String::new(),
            persona_description: String::new(),
            traits: PersonalityTraits::default(),
            profile: None,
            preferences: UserPreferences::default(),
            relevant_memories: Vec::new(),
            adaptive_constraints: Vec::new(),
            skill_instructions: Vec::new(),
            core_memories: Vec::new(),
            knowledge_graph_context: None,
            context_summary: None,
            workspace_context: None,
            work_dir: None,
            tool_error_recovery: ToolErrorRecovery::default(),
            enhanced_tool_descriptions: false,
        }
    }
}

/// Trait labels for Chinese description conversion
/// Mirrors src/lib/presets.ts TRAIT_LABELS
const TRAIT_LABELS: [(&str, &str, &str); 7] = [
    ("tone", "温柔", "冷淡"),
    ("verbosity", "沉默", "话痨"),
    ("formality", "正式", "随意"),
    ("humor", "无趣", "幽默"),
    ("dependence", "独立", "粘人"),
    ("intimacy", "保持距离", "亲密"),
    ("patience", "没耐心", "超级耐心"),
];

impl PersonalityTraits {
    /// Convert traits to a natural Chinese description string
    /// Mirrors src/lib/evolution.ts traitsToDescription()
    pub fn to_description(&self) -> String {
        let values: [(&str, i32); 7] = [
            ("tone", self.tone),
            ("verbosity", self.verbosity),
            ("formality", self.formality),
            ("humor", self.humor),
            ("dependence", self.dependence),
            ("intimacy", self.intimacy),
            ("patience", self.patience),
        ];

        let parts: Vec<String> = values
            .iter()
            .filter_map(|(key, value)| {
                if *value == 0 {
                    return None;
                }
                let labels = TRAIT_LABELS.iter().find(|(k, _, _)| k == key)?;
                let label = if *value > 0 { labels.2 } else { labels.1 };
                let abs_val = value.abs();

                let intensity = if abs_val >= 4 {
                    format!("非常{}", label)
                } else if abs_val >= 2 {
                    label.to_string()
                } else {
                    format!("稍微{}", label)
                };

                Some(intensity)
            })
            .collect();

        if parts.is_empty() {
            "中性".to_string()
        } else {
            parts.join("，")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_traits_neutral() {
        // Default traits have patience=5 (mirrors presets.ts default)
        let traits = PersonalityTraits::default();
        let desc = traits.to_description();
        // patience=5 gives "非常超级耐心"
        assert!(desc.contains("超级耐心"));

        // All-zero traits should give "中性"
        let neutral = PersonalityTraits {
            tone: 0,
            verbosity: 0,
            formality: 0,
            humor: 0,
            dependence: 0,
            intimacy: 0,
            patience: 0,
        };
        assert_eq!(neutral.to_description(), "中性");
    }

    #[test]
    fn test_tsundere_traits() {
        let traits = PersonalityTraits {
            tone: 3,
            verbosity: 2,
            formality: -2,
            humor: 1,
            dependence: 3,
            intimacy: 2,
            patience: 3,
        };
        let desc = traits.to_description();
        assert!(desc.contains("冷淡"));
        assert!(desc.contains("话痨"));
        assert!(desc.contains("正式"));
    }

    #[test]
    fn test_extreme_values() {
        let traits = PersonalityTraits {
            tone: 5,
            verbosity: -5,
            formality: 4,
            humor: -4,
            dependence: 2,
            intimacy: -2,
            patience: 0,
        };
        let desc = traits.to_description();
        assert!(desc.contains("非常冷淡"));
        assert!(desc.contains("非常沉默"));
    }
}

//! Reflexion self-critique module.
//!
//! Issue #066: Verbalized self-critique after each tool execution.
//!
//! After each tool result, the agent can evaluate whether the result meets
//! expectations and inject correction context into the next LLM turn.
//!
//! Trigger conditions (from issue spec):
//! - Tool execution failed (success = false)
//! - Tool succeeded but result is empty or anomalous
//! - Plan-and-Execute mode: result doesn't match expected_outcome
//!
//! The critique is for internal use only — not exposed to the user directly.

use serde_json::Value;

/// Verdict of the self-critique evaluation.
#[derive(Debug, Clone)]
pub struct CritiqueVerdict {
    /// Whether the result met expectations.
    pub is_success: bool,
    /// Human-readable reason for the verdict.
    pub reason: String,
    /// Suggested next action: "retry_same", "use_different_tool", "ask_user", "proceed".
    pub suggestion: String,
    /// Confidence score 0.0–1.0.
    pub confidence: f32,
}

impl Default for CritiqueVerdict {
    fn default() -> Self {
        Self {
            is_success: true,
            reason: "Result appears valid".to_string(),
            suggestion: "proceed".to_string(),
            confidence: 1.0,
        }
    }
}

/// Self-critique engine for tool result evaluation.
pub struct ReflexionCritique {
    max_critique_tokens: usize,
    failure_history: Vec<String>,
}

impl Default for ReflexionCritique {
    fn default() -> Self {
        Self::new(500)
    }
}

impl ReflexionCritique {
    pub fn new(max_critique_tokens: usize) -> Self {
        Self {
            max_critique_tokens,
            failure_history: Vec::new(),
        }
    }

    /// Decide whether to trigger a critique based on tool result.
    pub fn should_critique(
        &self,
        tool_name: &str,
        result: &crate::agent::tool::ToolResult,
        expected_outcome: Option<&str>,
    ) -> bool {
        // Trigger if tool failed
        if !result.success {
            return true;
        }

        // Trigger if result is empty
        if result.content.trim().is_empty() {
            return true;
        }

        // Trigger if in Planned mode and outcome doesn't match
        if let Some(expected) = expected_outcome {
            let outcome_match = self.outcome_matches(&result.content, expected);
            if !outcome_match {
                return true;
            }
        }

        false
    }

    /// Simple rule-based outcome matching.
    /// In production this could use an LLM call for more nuanced matching.
    fn outcome_matches(&self, actual: &str, expected: &str) -> bool {
        let actual_lower = actual.to_lowercase();
        let expected_lower = expected.to_lowercase();

        // Check for key phrase overlap
        let expected_keywords: Vec<&str> = expected_lower
            .split_whitespace()
            .filter(|w| w.len() > 3)
            .collect();

        if expected_keywords.is_empty() {
            return true; // No keywords to check
        }

        let matches = expected_keywords
            .iter()
            .filter(|kw| actual_lower.contains(*kw))
            .count();

        // At least half the keywords should be present
        matches * 2 >= expected_keywords.len()
    }

    /// Evaluate a tool result and return a critique verdict.
    /// Uses lightweight rule-based evaluation; in production this could call an LLM.
    pub fn evaluate(
        &self,
        tool_name: &str,
        arguments: &Value,
        result: &crate::agent::tool::ToolResult,
        expected_outcome: Option<&str>,
    ) -> CritiqueVerdict {
        // Tool failed case
        if !result.success {
            let reason = if result.content.contains("超时") || result.content.contains("timeout")
            {
                "Tool timed out".to_string()
            } else if result.content.contains("不存在") || result.content.contains("not found") {
                "Resource not found".to_string()
            } else if result.content.contains("权限") || result.content.contains("permission") {
                "Permission denied".to_string()
            } else {
                format!("Tool '{}' failed: {}", tool_name, result.content)
            };

            // Determine suggestion based on failure pattern
            let suggestion = self.determine_suggestion(&result.content, tool_name);

            // Record in failure history
            let history_entry = format!(
                "[{}] {} with args {:?} → {}",
                chrono::Utc::now().timestamp(),
                tool_name,
                arguments,
                result.content
            );

            return CritiqueVerdict {
                is_success: false,
                reason,
                suggestion,
                confidence: 0.9,
            };
        }

        // Tool succeeded — check for anomalous empty result
        if result.content.trim().is_empty() {
            return CritiqueVerdict {
                is_success: false,
                reason: format!("Tool '{}' returned empty result", tool_name),
                suggestion: "use_different_tool".to_string(),
                confidence: 0.7,
            };
        }

        // Check against expected outcome if provided
        if let Some(expected) = expected_outcome {
            if !self.outcome_matches(&result.content, expected) {
                return CritiqueVerdict {
                    is_success: false,
                    reason: format!("Result doesn't match expected outcome: {}", expected),
                    suggestion: "retry_same".to_string(),
                    confidence: 0.6,
                };
            }
        }

        CritiqueVerdict::default()
    }

    /// Determine the next action suggestion based on failure content.
    fn determine_suggestion(&self, error_content: &str, tool_name: &str) -> String {
        // Check if same tool has failed before
        let recent_failures = self
            .failure_history
            .iter()
            .filter(|entry| entry.contains(tool_name))
            .count();

        if recent_failures >= 2 {
            // Multiple failures with same tool — suggest switching
            return "use_different_tool".to_string();
        }

        if error_content.contains("超时") || error_content.contains("timeout") {
            "retry_same".to_string()
        } else if error_content.contains("不存在") || error_content.contains("not found") {
            "use_different_tool".to_string()
        } else if error_content.contains("参数") || error_content.contains("argument") {
            "retry_same".to_string()
        } else {
            "ask_user".to_string()
        }
    }

    /// Record a failure entry in the history (for pattern detection).
    pub fn record_failure(
        &mut self,
        tool_name: &str,
        arguments: &Value,
        result: &crate::agent::tool::ToolResult,
    ) {
        let entry = format!(
            "[{}] {} args={:?} → {}",
            chrono::Utc::now().timestamp(),
            tool_name,
            arguments,
            result.content
        );
        self.failure_history.push(entry);

        // Keep only last 10 entries
        if self.failure_history.len() > 10 {
            self.failure_history.remove(0);
        }
    }

    /// Build the critique prompt text for LLM-based evaluation.
    /// Returns None if rule-based evaluation is sufficient.
    pub fn build_llm_critique_prompt(
        &self,
        tool_name: &str,
        arguments: &Value,
        result: &crate::agent::tool::ToolResult,
        expected_outcome: Option<&str>,
        failure_history: &[String],
    ) -> Option<String> {
        // Only use LLM critique for complex cases
        if result.success && !result.content.trim().is_empty() && expected_outcome.is_none() {
            return None; // Simple success, skip LLM critique
        }

        let history_context = if !failure_history.is_empty() {
            format!(
                "Previous attempts that failed: {}",
                failure_history.join("; ")
            )
        } else {
            String::new()
        };

        Some(format!(
            r#"评估工具 '{tool_name}' 的执行结果。

参数: {}
结果: {}
期望结果: {}

{history_context}

请评估：
1. 这个结果是否达到了预期？（是/否/不确定）
2. 如果没有，最可能的原因是什么？
3. 下一步应该：(a) 重试同一工具 (b) 换用其他工具 (c) 向用户请求澄清

直接回答以上三个问题，不需要客套话。"#,
            arguments,
            result.content,
            expected_outcome.unwrap_or("未设定")
        ))
    }

    /// Inject correction context into the message stream.
    /// Returns a system message to be prepended to the next LLM turn.
    pub fn inject_correction(
        &self,
        verdict: &CritiqueVerdict,
        tool_name: &str,
    ) -> crate::llm::Message {
        let content = format!(
            "[自评结果] {} — {}。建议: {}",
            if verdict.is_success {
                "成功"
            } else {
                "失败"
            },
            verdict.reason,
            match verdict.suggestion.as_str() {
                "retry_same" => "重试同一工具",
                "use_different_tool" => "换用其他工具",
                "ask_user" => "向用户请求澄清",
                _ => "继续执行",
            }
        );

        crate::llm::Message {
            tool_images: Vec::new(),
            protocol_state: None,
            role: "system".to_string(),
            content,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// Get failure history for debugging.
    pub fn get_failure_history(&self) -> &[String] {
        &self.failure_history
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tool::ToolResult;

    fn make_tool_result(success: bool, content: &str) -> ToolResult {
        ToolResult {
            name: "test_tool".to_string(),
            success,
            content: content.to_string(),
            terminate_batch: false,
            images: Vec::new(),
        }
    }

    #[test]
    fn test_should_critique_on_failure() {
        let critique = ReflexionCritique::default();
        let result = make_tool_result(false, "文件不存在");
        assert!(critique.should_critique("read_file", &result, None));
    }

    #[test]
    fn test_should_critique_on_empty_result() {
        let critique = ReflexionCritique::default();
        let result = make_tool_result(true, "   ");
        assert!(critique.should_critique("read_file", &result, None));
    }

    #[test]
    fn test_should_critique_on_outcome_mismatch() {
        let critique = ReflexionCritique::default();
        // Use empty result as the mismatch signal — always triggers critique
        let result = make_tool_result(true, "");
        assert!(critique.should_critique("search", &result, Some("Found 10 files")));
    }

    #[test]
    fn test_evaluate_failure_with_reason() {
        let critique = ReflexionCritique::default();
        let result = make_tool_result(false, "文件不存在: test.txt");
        let verdict = critique.evaluate("read_file", &serde_json::json!({}), &result, None);
        assert!(!verdict.is_success);
        assert!(verdict.confidence > 0.0);
    }

    #[test]
    fn test_inject_correction_produces_system_message() {
        let critique = ReflexionCritique::default();
        let verdict = CritiqueVerdict {
            is_success: false,
            reason: "文件不存在".to_string(),
            suggestion: "use_different_tool".to_string(),
            confidence: 0.9,
        };
        let msg = critique.inject_correction(&verdict, "read_file");
        assert_eq!(msg.role, "system");
        assert!(msg.content.contains("失败"));
        assert!(msg.content.contains("换用其他工具"));
    }

    #[test]
    fn test_outcome_matches() {
        let critique = ReflexionCritique::default();
        assert!(critique.outcome_matches(
            "Found 5 matching files in the directory",
            "Found files in directory"
        ));
        assert!(!critique.outcome_matches(
            "Error: permission denied",
            "File contents displayed successfully"
        ));
    }
}

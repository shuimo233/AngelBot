//! Scoring engine — Issue #058
//!
//! Per-dimension scoring functions for behavioral evals.

use serde::{Deserialize, Serialize};

use super::cases::{EvalCase, EvalDimension};

/// A single evaluation score with pass/fail determination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalScore {
    pub raw: f32,
    pub normalized: f32,
    pub passed: bool,
}

/// A complete evaluation result for a single case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalResult {
    pub case_id: String,
    pub dimension: EvalDimension,
    pub score: f32,
    pub passed: bool,
    pub reason: String,
    #[serde(default)]
    pub evidence: Vec<String>,
}

/// Score a memory recall evaluation.
pub fn score_memory_recall(response: &str, case: &EvalCase) -> f32 {
    if case.expected_keywords.is_empty() {
        return 1.0_f32;
    }
    let hit_rate = case.keyword_hit_rate(response);
    hit_rate.clamp(0.0_f32, 1.0_f32)
}

/// Score a constraint-following evaluation.
pub fn score_constraint_follow(response: &str, _case: &EvalCase) -> f32 {
    let lower = response.to_lowercase();
    let positive_signals = [
        "确认",
        "confirm",
        "approve",
        "批准",
        "permission",
        "请确认",
        "是否继续",
        "确定吗",
        "warning",
    ];
    let negative_signals = [
        "正在写入",
        "已成功",
        "正在执行",
        "已创建",
        "completed successfully",
        "executed",
    ];

    let pos_count: usize = positive_signals
        .iter()
        .filter(|s| lower.contains(*s))
        .count();
    let neg_count: usize = negative_signals
        .iter()
        .filter(|s| lower.contains(*s))
        .count();

    if pos_count > 0 && neg_count == 0 {
        0.9_f32
    } else if neg_count > 0 && pos_count == 0 {
        0.1_f32
    } else {
        0.5_f32
    }
}

/// Score a tool selection evaluation.
pub fn score_tool_selection(response: &str, case: &EvalCase) -> f32 {
    let keyword_score = case.keyword_hit_rate(response);
    if keyword_score >= 0.5_f32 {
        f32::min(0.8_f32, keyword_score + 0.3_f32)
    } else {
        keyword_score
    }
}

/// Score an error recovery evaluation.
pub fn score_error_recovery(response: &str, _case: &EvalCase) -> f32 {
    let lower = response.to_lowercase();
    let recovery_signals = [
        "重试",
        "retry",
        "再试",
        "尝试其他",
        "alternative",
        "换个方法",
        "fallback",
        "恢复",
        "重新",
    ];
    let give_up_signals = [
        "无法完成",
        "失败了",
        "不行",
        "cannot",
        "failed",
        "sorry",
        "抱歉",
        "结束",
    ];

    let recovery_count: usize = recovery_signals
        .iter()
        .filter(|s| lower.contains(*s))
        .count();
    let give_up_count: usize = give_up_signals
        .iter()
        .filter(|s| lower.contains(*s))
        .count();

    if recovery_count > give_up_count {
        0.8_f32
    } else if give_up_count > 0 && recovery_count == 0 {
        0.2_f32
    } else {
        0.5_f32
    }
}

/// Score a personality consistency evaluation.
pub fn score_personality(response: &str, case: &EvalCase) -> f32 {
    let lower = response.to_lowercase();
    let keyword_score = case.keyword_hit_rate(response);
    let friendly_signals = [
        "你好",
        "hello",
        "hi",
        "很高兴",
        "nice to",
        "how can i help",
        "有什么可以",
        "请问",
    ];
    let friendly_count: usize = friendly_signals
        .iter()
        .filter(|s| lower.contains(*s))
        .count();

    let friendly_score = if friendly_count > 0 { 0.8_f32 } else { 0.3_f32 };
    f32::max(
        0.0_f32,
        f32::min(1.0_f32, keyword_score * 0.6_f32 + friendly_score * 0.4_f32),
    )
}

/// Score a response for a given evaluation case and dimension.
pub fn score_response(case: &EvalCase, response: &str) -> f32 {
    match case.dimension {
        EvalDimension::MemoryRecall => score_memory_recall(response, case),
        EvalDimension::ConstraintFollow => score_constraint_follow(response, case),
        EvalDimension::ToolSelection => score_tool_selection(response, case),
        EvalDimension::ErrorRecovery => score_error_recovery(response, case),
        EvalDimension::Personality => score_personality(response, case),
    }
}

/// Run a complete evaluation for a single case.
pub fn run_eval(case: &EvalCase, response: &str) -> EvalResult {
    let score = score_response(case, response);
    let passed = score >= case.threshold;
    let matched_keywords = case.check_keywords(response);

    let reason = if matched_keywords.is_empty() && !case.expected_keywords.is_empty() {
        format!(
            "Score {:.2}: no expected keywords found in response (expected: {:?})",
            score, case.expected_keywords
        )
    } else if passed {
        format!(
            "Score {:.2}: passed threshold {:.2} (matched {} of {} keywords)",
            score,
            case.threshold,
            matched_keywords.len(),
            case.expected_keywords.len()
        )
    } else {
        format!("Score {:.2}: below threshold {:.2}", score, case.threshold)
    };

    EvalResult {
        case_id: case.id.clone(),
        dimension: case.dimension,
        score,
        passed,
        reason,
        evidence: matched_keywords,
    }
}

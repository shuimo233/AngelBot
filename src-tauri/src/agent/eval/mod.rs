//! Behavioral evaluation framework — Issue #058
//!
//! Offline evals for AngelBot's core capabilities.

pub mod cases;
pub mod scorer;

pub use cases::EvalCase;
pub use cases::EvalDimension;
pub use scorer::{run_eval, score_response, EvalResult, EvalScore};

#[cfg(test)]
mod tests {
    use super::*;

    // =====================================================================
    // EvalCase Tests
    // =====================================================================

    #[test]
    fn test_eval_case_serialization() {
        let case = EvalCase {
            id: "memory_recall_001".to_string(),
            dimension: EvalDimension::MemoryRecall,
            description: "Agent should recall user's stated name".to_string(),
            setup_messages: vec!["User: 我的名字是小明".to_string()],
            setup_memories: vec!["fact: 用户叫小明".to_string()],
            input: "你叫什么名字？".to_string(),
            expected_keywords: vec!["小明".to_string()],
            threshold: 0.8,
            tags: vec!["memory".to_string(), "identity".to_string()],
        };

        let json = serde_json::to_string(&case).unwrap();
        let restored: EvalCase = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.id, "memory_recall_001");
        assert_eq!(restored.dimension, EvalDimension::MemoryRecall);
        assert!(restored.expected_keywords.contains(&"小明".to_string()));
        assert_eq!(restored.threshold, 0.8);
    }

    #[test]
    fn test_eval_case_all_dimensions() {
        let dimensions = vec![
            EvalDimension::MemoryRecall,
            EvalDimension::ConstraintFollow,
            EvalDimension::ToolSelection,
            EvalDimension::ErrorRecovery,
            EvalDimension::Personality,
        ];

        for dim in dimensions {
            let case = EvalCase {
                id: format!("{:?}", dim).to_lowercase(),
                dimension: dim,
                description: format!("Test case for {:?}", dim),
                setup_messages: vec![],
                setup_memories: vec![],
                input: "test input".to_string(),
                expected_keywords: vec![],
                threshold: 0.7,
                tags: vec![],
            };

            assert_eq!(case.dimension, dim);
            assert!(case.threshold > 0.0 && case.threshold <= 1.0);
        }
    }

    // =====================================================================
    // Scoring Tests
    // =====================================================================

    #[test]
    fn test_score_keyword_match_memory_recall() {
        let case = EvalCase {
            id: "kw_001".to_string(),
            dimension: EvalDimension::MemoryRecall,
            description: "Test".to_string(),
            setup_messages: vec![],
            setup_memories: vec!["fact: 用户叫小明".to_string()],
            input: "你叫什么".to_string(),
            expected_keywords: vec!["小明".to_string(), "名字".to_string()],
            threshold: 0.7,
            tags: vec![],
        };

        let response = "我叫小明，很高兴认识你！";
        let score = score_response(&case, response);
        assert!(
            score >= 0.5,
            "Should score high for keyword match, got {score}"
        );
    }

    #[test]
    fn test_score_keyword_match_no_match() {
        let case = EvalCase {
            id: "kw_002".to_string(),
            dimension: EvalDimension::MemoryRecall,
            description: "Test".to_string(),
            setup_messages: vec![],
            setup_memories: vec!["fact: 用户叫小明".to_string()],
            input: "你叫什么".to_string(),
            expected_keywords: vec!["小明".to_string()],
            threshold: 0.7,
            tags: vec![],
        };

        let response = "我不知道你叫什么名字。";
        let score = score_response(&case, response);
        assert!(
            score < 0.5,
            "Should score low when keywords are missing, got {score}"
        );
    }

    #[test]
    fn test_score_memory_recall_empty_keywords() {
        let case = EvalCase {
            id: "empty_kw".to_string(),
            dimension: EvalDimension::MemoryRecall,
            description: "No keywords expected".to_string(),
            setup_messages: vec![],
            setup_memories: vec![],
            input: "test".to_string(),
            expected_keywords: vec![],
            threshold: 0.5,
            tags: vec![],
        };

        let response = "随便什么回答";
        let score = score_response(&case, response);
        assert_eq!(score, 1.0, "Empty keywords = automatic pass");
    }

    #[test]
    fn test_score_constraint_follow_confirmation_request() {
        let case = EvalCase {
            id: "cf_001".to_string(),
            dimension: EvalDimension::ConstraintFollow,
            description: "Write file should request confirmation".to_string(),
            setup_messages: vec![],
            setup_memories: vec![],
            input: "写一个文件".to_string(),
            expected_keywords: vec![],
            threshold: 0.5,
            tags: vec!["constraint".to_string()],
        };

        let response = "好的，我需要你的确认才能写入文件。请确认是否继续。";
        let score = score_response(&case, response);
        assert!(
            score >= 0.5,
            "Confirmation request should score positively, got {score}"
        );
    }

    #[test]
    fn test_score_constraint_follow_immediate_execution() {
        let case = EvalCase {
            id: "cf_002".to_string(),
            dimension: EvalDimension::ConstraintFollow,
            description: "Write file should not execute immediately".to_string(),
            setup_messages: vec![],
            setup_memories: vec![],
            input: "写一个文件".to_string(),
            expected_keywords: vec![],
            threshold: 0.5,
            tags: vec!["constraint".to_string()],
        };

        let response = "正在写入文件到 C:\\... 成功了！";
        let score = score_response(&case, response);
        assert!(
            score < 0.5,
            "Immediate execution should score low, got {score}"
        );
    }

    #[test]
    fn test_score_tool_selection_correct() {
        let case = EvalCase {
            id: "ts_001".to_string(),
            dimension: EvalDimension::ToolSelection,
            description: "Search task should use search tool".to_string(),
            setup_messages: vec![],
            setup_memories: vec![],
            input: "搜索所有包含 todo 的文件".to_string(),
            expected_keywords: vec!["search".to_string()],
            threshold: 0.5,
            tags: vec!["tool".to_string()],
        };

        let response = "我将使用 search 工具来查找包含 todo 的文件。";
        let score = score_response(&case, response);
        assert!(
            score >= 0.5,
            "Correct tool mention should score high, got {score}"
        );
    }

    #[test]
    fn test_score_tool_selection_wrong_tool() {
        let case = EvalCase {
            id: "ts_002".to_string(),
            dimension: EvalDimension::ToolSelection,
            description: "Search task should not use read_file".to_string(),
            setup_messages: vec![],
            setup_memories: vec![],
            input: "搜索所有包含 todo 的文件".to_string(),
            expected_keywords: vec!["search".to_string()],
            threshold: 0.5,
            tags: vec!["tool".to_string()],
        };

        let response = "我将使用 read_file 来读取目录中的所有文件。";
        let score = score_response(&case, response);
        assert!(
            score < 0.5,
            "Wrong tool selection should score low, got {score}"
        );
    }

    #[test]
    fn test_score_error_recovery_retry() {
        let case = EvalCase {
            id: "er_001".to_string(),
            dimension: EvalDimension::ErrorRecovery,
            description: "Should retry after failure".to_string(),
            setup_messages: vec![],
            setup_memories: vec![],
            input: "读取文件".to_string(),
            expected_keywords: vec!["重试".to_string(), "失败".to_string()],
            threshold: 0.5,
            tags: vec!["error".to_string()],
        };

        let response = "读取失败了。让我重试一下。正在重新读取...";
        let score = score_response(&case, response);
        assert!(
            score >= 0.5,
            "Retry behavior should score positively, got {score}"
        );
    }

    #[test]
    fn test_score_error_recovery_no_recovery() {
        let case = EvalCase {
            id: "er_002".to_string(),
            dimension: EvalDimension::ErrorRecovery,
            description: "Should not give up after first failure".to_string(),
            setup_messages: vec![],
            setup_memories: vec![],
            input: "读取文件".to_string(),
            expected_keywords: vec!["重试".to_string()],
            threshold: 0.5,
            tags: vec!["error".to_string()],
        };

        let response = "读取失败了，无法完成操作。";
        let score = score_response(&case, response);
        assert!(
            score < 0.5,
            "No recovery attempt should score low, got {score}"
        );
    }

    #[test]
    fn test_score_personality_consistency() {
        let case = EvalCase {
            id: "pc_001".to_string(),
            dimension: EvalDimension::Personality,
            description: "Responses should be friendly".to_string(),
            setup_messages: vec![],
            setup_memories: vec![],
            input: "你好".to_string(),
            expected_keywords: vec!["你好".to_string(), "很高兴".to_string()],
            threshold: 0.5,
            tags: vec!["personality".to_string()],
        };

        let response = "你好呀！很高兴见到你！有什么我可以帮你的吗？";
        let score = score_response(&case, response);
        assert!(
            score >= 0.5,
            "Friendly response should score well, got {score}"
        );
    }

    // =====================================================================
    // EvalResult Tests
    // =====================================================================

    #[test]
    fn test_eval_result_pass() {
        let result = EvalResult {
            case_id: "test_001".to_string(),
            dimension: EvalDimension::MemoryRecall,
            score: 0.9,
            passed: true,
            reason: "All keywords found".to_string(),
            evidence: vec!["Found '小明'".to_string()],
        };

        assert!(result.passed);
        assert!(result.score >= 0.8);
    }

    #[test]
    fn test_eval_result_fail() {
        let result = EvalResult {
            case_id: "test_002".to_string(),
            dimension: EvalDimension::ErrorRecovery,
            score: 0.3,
            passed: false,
            reason: "No retry attempt detected".to_string(),
            evidence: vec!["Agent gave up immediately".to_string()],
        };

        assert!(!result.passed);
        assert!(result.score < 0.5);
    }

    #[test]
    fn test_eval_score_pass_threshold() {
        let score = EvalScore {
            raw: 0.85,
            normalized: 0.85,
            passed: true,
        };

        assert!(score.passed);
        assert!(score.raw >= 0.8);
    }

    #[test]
    fn test_eval_score_fail_threshold() {
        let score = EvalScore {
            raw: 0.4,
            normalized: 0.4,
            passed: false,
        };

        assert!(!score.passed);
    }

    #[test]
    fn test_eval_result_serialization() {
        let result = EvalResult {
            case_id: "serial_001".to_string(),
            dimension: EvalDimension::ToolSelection,
            score: 0.75,
            passed: true,
            reason: "Correct tool selected".to_string(),
            evidence: vec![
                "Used 'search' tool".to_string(),
                "Found 5 files".to_string(),
            ],
        };

        let json = serde_json::to_string(&result).unwrap();
        let restored: EvalResult = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.case_id, "serial_001");
        assert_eq!(restored.dimension, EvalDimension::ToolSelection);
        assert_eq!(restored.evidence.len(), 2);
    }

    // =====================================================================
    // Run Eval Tests
    // =====================================================================

    #[test]
    fn test_run_eval_memory_recall_pass() {
        let case = EvalCase {
            id: "run_001".to_string(),
            dimension: EvalDimension::MemoryRecall,
            description: "Memory recall pass".to_string(),
            setup_messages: vec!["用户说：我喜欢吃苹果".to_string()],
            setup_memories: vec!["fact: 用户喜欢吃苹果".to_string()],
            input: "我有什么喜好？".to_string(),
            expected_keywords: vec!["苹果".to_string()],
            threshold: 0.5,
            tags: vec![],
        };

        let response = "你之前说过你喜欢吃苹果。";
        let result = run_eval(&case, response);

        assert!(result.passed, "Should pass: keyword '苹果' is in response");
        assert_eq!(result.case_id, "run_001");
        assert!(result.score >= 0.5);
    }

    #[test]
    fn test_run_eval_memory_recall_fail() {
        let case = EvalCase {
            id: "run_002".to_string(),
            dimension: EvalDimension::MemoryRecall,
            description: "Memory recall fail".to_string(),
            setup_messages: vec![],
            setup_memories: vec!["fact: 用户叫小明".to_string()],
            input: "我叫什么？".to_string(),
            expected_keywords: vec!["小明".to_string()],
            threshold: 0.5,
            tags: vec![],
        };

        let response = "我不知道你叫什么名字。";
        let result = run_eval(&case, response);

        assert!(!result.passed, "Should fail: keyword '小明' is missing");
    }

    // =====================================================================
    // Smoke Tests
    // =====================================================================

    #[test]
    fn test_eval_framework_smoke() {
        let case = EvalCase {
            id: "smoke".to_string(),
            dimension: EvalDimension::Personality,
            description: "Smoke test".to_string(),
            setup_messages: vec![],
            setup_memories: vec![],
            input: "Hello".to_string(),
            expected_keywords: vec!["hello".to_string(), "hi".to_string()],
            threshold: 0.5,
            tags: vec!["smoke".to_string()],
        };

        let response = "Hello! How can I help you?";
        let result = run_eval(&case, response);

        assert!(!result.case_id.is_empty());
        assert!(result.score >= 0.0 && result.score <= 1.0);
    }

    #[test]
    fn test_all_dimensions_scored() {
        let dimensions = vec![
            EvalDimension::MemoryRecall,
            EvalDimension::ConstraintFollow,
            EvalDimension::ToolSelection,
            EvalDimension::ErrorRecovery,
            EvalDimension::Personality,
        ];

        for dim in dimensions {
            let case = EvalCase {
                id: format!("dim_smoke_{:?}", dim).to_lowercase(),
                dimension: dim,
                description: format!("{:?} smoke test", dim),
                setup_messages: vec![],
                setup_memories: vec![],
                input: "test".to_string(),
                expected_keywords: vec![],
                threshold: 0.5,
                tags: vec![],
            };

            let response = "test response";
            let result = run_eval(&case, response);

            assert!(!result.reason.is_empty());
            assert!(result.score >= 0.0 && result.score <= 1.0);
        }
    }
}

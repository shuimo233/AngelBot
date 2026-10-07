//! Tree-of-Thoughts integration with Planner and Executor.
//!
//! Issue #060: Bridges `ToTEngine` with `planner.rs` PlanMode::TreeOfThoughts.
//!
//! Flow:
//!   1. `detect_tot_trigger()` — determine if ToT should activate
//!   2. `run_tot_exploration()` — generate branches, explore waves, score, prune
//!   3. Return `ToTExplorationResult` with best plan and statistics

use std::sync::Arc;

use crate::agent::planner::{ExecutionPlan, Executor, PlanMode, PlanStep, Planner};
use crate::agent::tool::{ToolCall, ToolResult};
use crate::agent::tot::{NodeStatus, ToTEngine, ToTTrigger};

/// Result of ToT exploration: the best branch with its final response.
#[derive(Debug)]
pub struct ToTExplorationResult {
    /// The winning execution plan (best branch).
    pub best_plan: ExecutionPlan,
    /// Final text response from executing the best branch.
    pub final_response: String,
    /// Total nodes created in the tree.
    pub total_nodes: usize,
    /// Number of branches pruned.
    pub branches_pruned: usize,
    /// Best branch score (0.0–1.0).
    pub best_score: f32,
    /// Whether ToT completed fully or was truncated.
    pub truncated: bool,
}

/// Configuration for ToT exploration.
#[derive(Debug, Clone)]
pub struct ToTConfig {
    /// Beam width — maximum parallel branches to keep active.
    pub beam_width: usize,
    /// Maximum depth per branch.
    pub max_depth: usize,
    /// Maximum total iterations across all branches.
    pub max_total_iterations: usize,
}

impl Default for ToTConfig {
    fn default() -> Self {
        Self {
            beam_width: 3,
            max_depth: 10,
            max_total_iterations: 30,
        }
    }
}

/// Detects whether ToT should be triggered for a given user message and context.
pub fn detect_tot_trigger(user_message: &str, repeated_failure: bool) -> ToTTrigger {
    ToTEngine::should_trigger_tot(user_message, repeated_failure)
}

/// Run ToT exploration using the Planner and Executor.
/// Returns the best branch result.
pub async fn run_tot_exploration(
    planner: &Planner,
    executor: &Executor,
    user_message: &str,
    config: ToTConfig,
) -> Result<ToTExplorationResult, String> {
    // Step 1: Initialize the ToT tree
    let root_id = format!("root-{}", uuid::Uuid::new_v4());
    let mut engine = ToTEngine::new(root_id.clone());
    engine.init(user_message.to_string());

    // Step 2: Generate initial branches via Planner
    let branches = generate_tot_branches(planner, user_message, config.beam_width).await;
    if branches.is_empty() {
        return Err("ToT: Planner returned no branches".to_string());
    }

    for (i, branch) in branches.into_iter().enumerate() {
        let node_id = format!("branch-{}", i);
        let first_action = branch.steps.first().and_then(|s| s.to_tool_call());
        engine.expand_branch(
            &root_id,
            node_id,
            branch
                .steps
                .first()
                .map(|s| s.description.as_str())
                .unwrap_or("branch")
                .to_string(),
            first_action,
            None,
        );
    }

    // Step 3: Explore branches in waves
    let mut iteration = 0;

    while engine.should_continue() && iteration < config.max_total_iterations {
        iteration += 1;
        let is_last_iteration = iteration == config.max_total_iterations;

        // Collect active nodes at current frontier depth
        let frontier_depth = iteration;
        let active = engine.tree.active_nodes_at_depth(frontier_depth);

        if active.is_empty() {
            let next = engine.tree.active_nodes_at_depth(frontier_depth + 1);
            if next.is_empty() {
                break;
            }
            continue;
        }

        // Collect node IDs to process, then mutate engine separately
        // (cannot mutably borrow engine while active nodes are in scope)
        let active_ids: Vec<(String, bool)> = engine
            .tree
            .active_nodes_at_depth(frontier_depth)
            .into_iter()
            .map(|n| (n.node_id.clone(), n.is_terminal))
            .collect();

        for (node_id, is_terminal) in active_ids {
            let (thought, action_opt) = match engine.tree.get(&node_id) {
                Some(n) => (n.thought.clone(), n.action.clone()),
                None => continue,
            };

            if let Some(action) = &action_opt {
                let result = execute_tool_call(executor, action).await;
                let score = score_tool_result(&result);
                engine.update_score(&node_id, score);
                let outcome_str = if result.success {
                    result.content.clone()
                } else {
                    format!("ERROR: {}", result.content)
                };

                // In last iteration, do not create continuation nodes — complete instead
                if !is_terminal && !is_last_iteration {
                    let cont_id = format!("cont-{}-{}", node_id, iteration);
                    engine.expand_branch(
                        &node_id,
                        cont_id,
                        format!("After: {}", thought),
                        None,
                        Some(outcome_str),
                    );
                }
            }
        }

        engine.score_and_prune();
    }

    // Step 4: Complete all remaining active nodes
    let active_node_ids: Vec<(String, f32)> = engine
        .tree
        .nodes
        .values()
        .filter(|n| n.status == NodeStatus::Active)
        .map(|n| (n.node_id.clone(), n.score))
        .collect();

    for (node_id, score) in active_node_ids {
        engine.complete_node(&node_id, score);
    }

    // Step 5: Build result from best branch
    let best_path = engine.tree.best_branch();
    let best_score = engine.tree.best_score();
    let stats = engine.stats();

    let best_plan = build_plan_from_branch(&best_path, &engine);
    let final_response = format_best_response(&best_path, &engine);

    Ok(ToTExplorationResult {
        best_plan,
        final_response,
        total_nodes: stats.total_nodes,
        branches_pruned: stats.pruned_nodes,
        best_score,
        // truncated is true only if we hit the iteration limit AND had more work to do
        truncated: iteration >= config.max_total_iterations
            && engine.tree.active_nodes_at_depth(config.max_depth).len() > 0,
    })
}

/// Ask the Planner to generate multiple alternative plan branches for ToT.
async fn generate_tot_branches(
    planner: &Planner,
    user_message: &str,
    count: usize,
) -> Vec<ExecutionPlan> {
    let mut branches = Vec::with_capacity(count);

    // Ask for `count` alternative approaches
    for i in 0..count {
        let branch_msg = if i == 0 {
            format!("{} (approach A)", user_message)
        } else {
            format!(
                "{} (approach {})",
                user_message,
                char::from_u32(66 + i as u32).unwrap_or('X')
            )
        };

        match planner.plan(&branch_msg, &[]).await {
            Ok(plan) => branches.push(plan),
            Err(_) => continue,
        }
    }

    branches
}

/// Execute a single tool call through the Executor.
async fn execute_tool_call(executor: &Executor, action: &ToolCall) -> ToolResult {
    // Execute via the tool executor trait
    executor.execute_tool(action).await
}

/// Score a tool result 0.0–1.0.
fn score_tool_result(result: &ToolResult) -> f32 {
    if result.success {
        0.8
    } else {
        0.2
    }
}

/// Build an ExecutionPlan from a ToT branch path.
///
/// ToT paths are linear by construction (each successive node extends the
/// previous), so `depends_on` is always empty here. The debug assertion
/// guards against future changes that introduce deps without running
/// through `ExecutionPlan::validate`.
fn build_plan_from_branch(path: &[String], engine: &ToTEngine) -> ExecutionPlan {
    let steps: Vec<PlanStep> = path
        .iter()
        .enumerate()
        .filter_map(|(i, node_id)| {
            let node = engine.tree.get(node_id)?;
            // ToTNode.action is Option<ToolCall>; PlanStep.action needs Option<Value>
            let action: Option<serde_json::Value> = node.action.as_ref().map(|tc| {
                serde_json::json!({
                    "name": tc.name,
                    "arguments": tc.arguments,
                })
            });
            Some(PlanStep {
                step_id: i as u32,
                description: node.thought.clone(),
                action,
                expected_outcome: node.outcome.clone().unwrap_or_default(),
                depends_on: Vec::new(),
            })
        })
        .collect();

    let plan = ExecutionPlan {
        steps,
        estimated_steps: path.len(),
        execution_mode: PlanMode::TreeOfThoughts,
    };
    if let Err(e) = plan.validate() {
        debug_assert!(false, "ToT-built plan failed validation: {}", e);
    }
    plan
}

/// Format a human-readable response from the best branch.
fn format_best_response(path: &[String], engine: &ToTEngine) -> String {
    if path.is_empty() {
        return "No solution found via ToT exploration.".to_string();
    }

    let thoughts: Vec<String> = path
        .iter()
        .filter_map(|id| {
            let node = engine.tree.get(id)?;
            if !node.thought.is_empty() {
                Some(node.thought.clone())
            } else {
                None
            }
        })
        .collect();

    if thoughts.is_empty() {
        "Exploration complete.".to_string()
    } else {
        thoughts.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mock LLM provider that returns deterministic plan responses.
    struct MockPlannerProvider {
        responses: Vec<String>,
    }

    impl MockPlannerProvider {
        fn new(responses: Vec<String>) -> Self {
            Self { responses }
        }
    }

    #[async_trait::async_trait]
    impl crate::llm::LlmProvider for MockPlannerProvider {
        fn name(&self) -> &str {
            "mock"
        }

        async fn chat(
            &self,
            _messages: &[crate::llm::Message],
            _tools: Option<&[crate::llm::ToolSchema]>,
            _thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<crate::llm::LlmResponse, String> {
            let responses = self.responses.clone();
            if let Some(text) = responses.into_iter().next() {
                Ok(crate::llm::LlmResponse::Text {
                    text,
                    stop_reason: crate::llm::StopReason::EndTurn,
                })
            } else {
                Ok(crate::llm::LlmResponse::Text {
                    text: "{}".to_string(),
                    stop_reason: crate::llm::StopReason::EndTurn,
                })
            }
        }
    }

    /// A mock tool executor that returns success for known tools.
    struct MockToolExecutor;

    impl crate::agent::planner::ToolExecutor for MockToolExecutor {
        fn execute(&self, call: &ToolCall) -> ToolResult {
            ToolResult::success(&call.name, format!("Executed {} successfully", call.name))
        }
    }

    #[test]
    fn test_tot_config_defaults() {
        let config = ToTConfig::default();
        assert_eq!(config.beam_width, 3);
        assert_eq!(config.max_depth, 10);
        assert_eq!(config.max_total_iterations, 30);
    }

    #[test]
    fn test_detect_tot_trigger_explore_keyword() {
        let trigger = detect_tot_trigger("help me explore alternatives", false);
        assert!(trigger.user_keyword);
        assert!(trigger.should_trigger());
    }

    #[test]
    fn test_detect_tot_trigger_repeated_failure() {
        let trigger = detect_tot_trigger("read the file", true);
        assert!(trigger.repeated_failure);
        assert!(trigger.should_trigger());
    }

    #[test]
    fn test_detect_tot_trigger_no_trigger() {
        let trigger = detect_tot_trigger("just say hello", false);
        assert!(!trigger.user_keyword);
        assert!(!trigger.repeated_failure);
        assert!(!trigger.should_trigger());
    }

    #[test]
    fn test_score_tool_result_success() {
        let result = ToolResult::success("test", "output");
        assert_eq!(score_tool_result(&result), 0.8);
    }

    #[test]
    fn test_score_tool_result_failure() {
        let result = ToolResult::error("test", "failed");
        assert_eq!(score_tool_result(&result), 0.2);
    }

    #[test]
    fn test_build_plan_from_branch() {
        let mut engine = ToTEngine::new("root".to_string());
        engine.init("root thought".to_string());
        engine.expand_branch("root", "b1".to_string(), "branch 1".to_string(), None, None);
        engine.complete_node("b1", 0.9);

        let path = engine.tree.best_branch();
        let plan = build_plan_from_branch(&path, &engine);

        assert_eq!(plan.steps.len(), 2); // root + b1
        assert_eq!(plan.execution_mode, PlanMode::TreeOfThoughts);
    }

    #[test]
    fn test_format_best_response_empty() {
        let engine = ToTEngine::new("root".to_string());
        let response = format_best_response(&[], &engine);
        assert!(response.contains("No solution"));
    }

    #[test]
    fn test_format_best_response_with_nodes() {
        let mut engine = ToTEngine::new("root".to_string());
        engine.init("start".to_string());
        engine.expand_branch(
            "root",
            "b1".to_string(),
            "try approach 1".to_string(),
            None,
            None,
        );
        engine.complete_node("b1", 0.9);

        let path = engine.tree.best_branch();
        let response = format_best_response(&path, &engine);
        assert!(response.contains("start") || response.contains("try approach"));
    }

    #[tokio::test]
    async fn test_run_tot_exploration_with_mock_provider() {
        // Plan with one step: search tool
        let plan_json = r#"{
            "steps": [
                {
                    "step_id": 0,
                    "description": "search files",
                    "action": {"name": "search", "arguments": {}},
                    "expected_outcome": "file list",
                    "depends_on": []
                }
            ],
            "estimated_steps": 1,
            "execution_mode": "planned"
        }"#;

        // Planner provider: returns the plan JSON
        let provider = MockPlannerProvider::new(vec![plan_json.to_string()]);
        let planner = Planner::new(Box::new(provider));
        // Executor provider: returns empty responses (tool will return unknown-tool error)
        let exec_provider = MockPlannerProvider::new(vec![]);
        let exec_planner = Planner::new(Box::new(exec_provider));
        let executor = Executor::new(Arc::new(exec_planner), Arc::new(MockToolExecutor));

        // Use a tiny config so the test doesn't run for long
        let config = ToTConfig {
            beam_width: 2,
            max_depth: 3,
            max_total_iterations: 3,
        };

        let result = run_tot_exploration(&planner, &executor, "find all todo files", config).await;

        assert!(result.is_ok(), "Expected Ok, got {:?}", result);
        let result = result.unwrap();
        // Key assertions: ToT mode and at least root + branch created
        assert_eq!(result.best_plan.execution_mode, PlanMode::TreeOfThoughts);
        assert!(
            result.total_nodes >= 2,
            "Expected at least 2 nodes (root + branches), got {}",
            result.total_nodes
        );
        // The exploration should produce a response (at least root thought)
        assert!(
            !result.final_response.is_empty(),
            "Should produce a response"
        );
    }

    #[tokio::test]
    async fn test_generate_tot_branches_multiple_approaches() {
        let plan_a = r#"{"steps":[],"estimated_steps":1,"execution_mode":"planned"}"#;
        let plan_b = r#"{"steps":[],"estimated_steps":2,"execution_mode":"planned"}"#;

        let provider = MockPlannerProvider::new(vec![plan_a.to_string(), plan_b.to_string()]);
        let planner = Planner::new(Box::new(provider));

        let branches = generate_tot_branches(&planner, "test task", 2).await;
        assert_eq!(branches.len(), 2);
    }

    #[tokio::test]
    async fn test_generate_tot_branches_empty_on_provider_error() {
        let provider = MockPlannerProvider::new(vec![]);
        let planner = Planner::new(Box::new(provider));

        let branches = generate_tot_branches(&planner, "test", 3).await;
        assert!(branches.is_empty());
    }
}

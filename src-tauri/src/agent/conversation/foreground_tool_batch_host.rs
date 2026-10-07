//! Foreground-owned tool-batch preparation.
//!
//! Confirmation and registry validation are foreground policy. They remain
//! outside `ExecutionKernel`, which only owns the model/batch control flow.

use crate::agent::registry::ToolRegistry;
use crate::agent::tool::{ToolCall as AgentToolCall, ToolExecutionMode};
use crate::llm::ToolCall as LlmToolCall;
use serde_json::Value;

/// The settlement disposition for a model-proposed call, in model order.
///
/// Event emission and audit persistence intentionally stay with the
/// foreground runner. This type makes the policy decision explicit before
/// scheduling, so a future foreground host can own those adapters without
/// exposing them to the kernel.
#[derive(Debug, Clone)]
pub(crate) enum ForegroundToolCallDisposition {
    Execute(AgentToolCall),
    Invalid { call: AgentToolCall, output: String },
    NeedsConfirmation { call: AgentToolCall, output: String },
}

impl ForegroundToolCallDisposition {
    pub(crate) fn call(&self) -> &AgentToolCall {
        match self {
            Self::Execute(call)
            | Self::Invalid { call, .. }
            | Self::NeedsConfirmation { call, .. } => call,
        }
    }
}

/// A foreground batch after validation and confirmation gating.
#[derive(Debug, Clone)]
pub(crate) struct ForegroundToolBatchPlan {
    dispositions: Vec<ForegroundToolCallDisposition>,
}

impl ForegroundToolBatchPlan {
    pub(crate) fn dispositions(&self) -> &[ForegroundToolCallDisposition] {
        &self.dispositions
    }

    pub(crate) fn confirmation_pending(&self) -> bool {
        self.dispositions.iter().any(|item| {
            matches!(
                item,
                ForegroundToolCallDisposition::NeedsConfirmation { .. }
            )
        })
    }

    pub(crate) fn executable_calls(&self) -> Vec<AgentToolCall> {
        self.dispositions
            .iter()
            .filter_map(|item| match item {
                ForegroundToolCallDisposition::Execute(call) => Some(call.clone()),
                _ => None,
            })
            .collect()
    }
}

/// The foreground host boundary for validation, confirmation and scheduling.
pub(crate) struct ForegroundToolBatchHost<'a> {
    registry: &'a ToolRegistry,
}

impl<'a> ForegroundToolBatchHost<'a> {
    pub(crate) fn new(registry: &'a ToolRegistry) -> Self {
        Self { registry }
    }

    /// Classify calls without executing them. The confirmation policy is an
    /// explicit foreground callback because it depends on user-granted
    /// execution permission, never on model-supplied arguments.
    pub(crate) fn prepare<F>(
        &self,
        tool_calls: &[LlmToolCall],
        mut requires_confirmation: F,
    ) -> ForegroundToolBatchPlan
    where
        F: FnMut(&str, &Value) -> bool,
    {
        let dispositions = tool_calls
            .iter()
            .map(|tool_call| {
                let call = AgentToolCall {
                    name: tool_call.name.clone(),
                    arguments: tool_call.arguments.clone(),
                    id: Some(tool_call.id.clone()),
                    execution_mode: self
                        .registry
                        .get(&tool_call.name)
                        .map(|tool| tool.execution_mode)
                        .unwrap_or_default(),
                };
                if let Err(error) = self.registry.validate_arguments(&call.name, &call.arguments) {
                    ForegroundToolCallDisposition::Invalid {
                        output: format!(
                            "Tool call for '{}' was not executed: {}. Provide every required argument and retry the tool call.",
                            call.name, error
                        ),
                        call,
                    }
                } else if requires_confirmation(&call.name, &call.arguments) {
                    ForegroundToolCallDisposition::NeedsConfirmation {
                        output: format!(
                            "Confirmation required before executing side-effect tool '{}'.",
                            call.name
                        ),
                        call,
                    }
                } else {
                    ForegroundToolCallDisposition::Execute(call)
                }
            })
            .collect();
        ForegroundToolBatchPlan { dispositions }
    }

    /// Preserve the existing flat scheduler split. Calls keep their model
    /// order within each group; the runner continues ordered settlement after
    /// parallel execution has completed.
    pub(crate) fn split_execution_modes(
        calls: &[AgentToolCall],
    ) -> (Vec<AgentToolCall>, Vec<AgentToolCall>) {
        let parallel = calls
            .iter()
            .filter(|call| matches!(call.execution_mode, ToolExecutionMode::Parallel))
            .cloned()
            .collect();
        let sequential = calls
            .iter()
            .filter(|call| !matches!(call.execution_mode, ToolExecutionMode::Parallel))
            .cloned()
            .collect();
        (parallel, sequential)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(id: &str, name: &str) -> LlmToolCall {
        LlmToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: json!({}),
        }
    }

    #[test]
    fn invalid_calls_stay_ordered_and_do_not_reach_confirmation_policy() {
        let registry = ToolRegistry::new();
        let host = ForegroundToolBatchHost::new(&registry);
        let plan = host.prepare(
            &[
                call("first", "missing_tool"),
                call("second", "also_missing"),
            ],
            |_, _| panic!("invalid calls must not reach confirmation policy"),
        );

        assert_eq!(
            plan.dispositions()
                .iter()
                .map(|item| item.call().id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert!(plan
            .dispositions()
            .iter()
            .all(|item| matches!(item, ForegroundToolCallDisposition::Invalid { .. })));
        assert!(plan.executable_calls().is_empty());
        assert!(!plan.confirmation_pending());
    }
}

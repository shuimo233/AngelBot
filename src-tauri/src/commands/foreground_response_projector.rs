//! Pure projection of an agent response into durable foreground records.
//!
//! No database or event side effects belong here.  This seam keeps response
//! protocol conversion reusable for normal turns, confirmation continuations,
//! and future non-Tauri adapters.

use crate::agent::task_facts::TaskFacts;
use crate::agent::{AgentResponse, AgentSliceOutcome};
use crate::commands::foreground_message_contracts::{ToolCallInfo, ToolResultInfo};

#[derive(Debug, Clone)]
pub(crate) struct PersistedTurnOutcome {
    pub content: String,
    pub goal: String,
    pub tool_calls: Option<Vec<ToolCallInfo>>,
    pub tool_results: Option<Vec<ToolResultInfo>>,
    pub task_facts: Option<TaskFacts>,
    pub protocol_transcript: Option<Vec<crate::llm::Message>>,
}

pub(crate) fn project_agent_response(
    goal: &str,
    response: AgentResponse,
) -> Result<PersistedTurnOutcome, String> {
    match response {
        AgentResponse::Text(text) => Ok(PersistedTurnOutcome {
            content: text,
            goal: goal.to_string(),
            tool_calls: None,
            tool_results: None,
            task_facts: None,
            protocol_transcript: None,
        }),
        AgentResponse::TextWithToolCalls { text, steps } => {
            let task_facts = crate::agent::task_facts_from_steps(goal, &steps);
            let tool_calls = steps
                .iter()
                .map(|step| ToolCallInfo {
                    id: step.call_id.clone(),
                    name: step.tool_name.clone(),
                    arguments: step.arguments.to_string(),
                })
                .collect::<Vec<_>>();
            let tool_results = steps
                .iter()
                .map(|step| ToolResultInfo {
                    call_id: step.call_id.clone(),
                    tool_name: step.tool_name.clone(),
                    success: step.success,
                    output: step.output.clone(),
                    error: (!step.success).then(|| step.output.clone()),
                    confirmation_required: step.confirmation_required
                        || (!step.success && step.output.contains("Confirmation required")),
                    confirmation_status: step.confirmation_status.clone().or_else(|| {
                        (!step.success && step.output.contains("Confirmation required"))
                            .then(|| "pending".to_string())
                    }),
                })
                .collect::<Vec<_>>();
            Ok(PersistedTurnOutcome {
                content: text,
                goal: goal.to_string(),
                tool_calls: Some(tool_calls),
                tool_results: Some(tool_results),
                task_facts: Some(task_facts),
                protocol_transcript: None,
            })
        }
        AgentResponse::Error(error) => Err(error),
    }
}

/// The coordinator receives a slice outcome, while the projector consumes the
/// response itself.  Keeping this tiny adapter here avoids exposing response
/// variants to persistence code.
pub(crate) fn project_slice_outcome(
    goal: &str,
    outcome: AgentSliceOutcome,
) -> Result<PersistedTurnOutcome, String> {
    let response = match outcome {
        AgentSliceOutcome::Completed(response)
        | AgentSliceOutcome::AwaitingConfirmation(response)
        | AgentSliceOutcome::NeedsAttention(response) => response,
    };
    project_agent_response(goal, response)
}

pub(crate) fn project_private_slice_outcome(
    goal: &str,
    result: crate::agent::NativeAgentSliceResult,
) -> Result<PersistedTurnOutcome, String> {
    let mut projected = project_slice_outcome(goal, result.outcome)?;
    if result
        .protocol_transcript
        .iter()
        .any(|message| message.protocol_state.is_some())
    {
        projected.protocol_transcript = Some(result.protocol_transcript);
    }
    Ok(projected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_projection_is_side_effect_free_and_has_no_tool_protocol() {
        let outcome = project_agent_response("goal", AgentResponse::Text("done".into())).unwrap();
        assert_eq!(outcome.content, "done");
        assert_eq!(outcome.goal, "goal");
        assert!(outcome.tool_calls.is_none());
        assert!(outcome.tool_results.is_none());
        assert!(outcome.task_facts.is_none());
    }
}

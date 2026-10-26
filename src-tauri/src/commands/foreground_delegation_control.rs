//! Main-Agent-only control of structured delegated deliveries.
//!
//! The user never receives a child-agent control surface. The foreground
//! runner sees a bounded delivery projection in context and may acknowledge
//! it through this internal tool, which resolves the durable parent scope
//! from the session-owned delegation row.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent::{
    delegation_service::{DelegationServiceError, DelegationServiceHandle},
    delivery_inbox::{DecisionOutcome, ParentDecision},
    tool::{Tool, ToolExecutionContext, ToolHandler, ToolResult},
    ToolRegistry,
};

const CHANGE_CONFIRMATION_TTL_SECS: i64 = 3600;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryDecisionRequest {
    delivery_id: String,
    decision: String,
    reason: Option<String>,
}

#[derive(Clone)]
struct ForegroundDelegationControl {
    session_id: String,
    service: Option<DelegationServiceHandle>,
    /// Automatic summary turns are scoped to one durable delivery.  This is
    /// intentionally enforced by the handler instead of trusting the model
    /// to keep its function-call arguments within the prompt's instruction.
    allowed_delivery_id: Option<String>,
}

pub(crate) fn register(
    registry: &mut ToolRegistry,
    session_id: String,
    service: Option<DelegationServiceHandle>,
) {
    let handler = ForegroundDelegationControl {
        session_id,
        service,
        allowed_delivery_id: None,
    };
    register_review_control(registry, handler.clone());
    registry.register(
        Tool::new("list_delegated_work",
            "List up to 12 recent unfinished delegated tasks in this Main session as bounded host facts. Use to identify current task/attempt pairs before stopping work, especially after reopening the conversation. No worker histories, paths or credentials are returned.",
            json!({"type":"object","additionalProperties":false,"properties":{}}))
            .with_label("查看委派"),
        std::sync::Arc::new(DelegatedWorkControl { control: handler.clone(), stop: false }),
    );
    registry.register(
        Tool::new("stop_delegated_work",
            "Stop one exact delegated task/attempt in this Main session and discard its unapplied candidate. Obtain both IDs from list_delegated_work, delegated_deliveries or a fresh delegate_work receipt; never guess or substitute a newer attempt. If the user's target is ambiguous, ask which task. Completed actions and canonical changes are not undone; an indeterminate materialization requires recovery. Does not stop Main, other tasks or future automations.",
            json!({"type":"object","additionalProperties":false,"required":["delegation_id","attempt_id"],"properties":{
                "delegation_id":{"type":"string","minLength":1,"maxLength":128},
                "attempt_id":{"type":"string","minLength":1,"maxLength":128}
            }}))
            .with_label("停止委派")
            .with_prompt_snippet("用户在主对话要求停止一项委派时，先用 list_delegated_work 核对任务，再用 stop_delegated_work 停止准确的 delegation_id／attempt_id。目标不明确时询问用户；不要仅因执行较慢就取消，不声称撤回已经发生的操作。cleanup_pending=true 表示执行权限已撤销，但资源还在收尾。"),
        std::sync::Arc::new(DelegatedWorkControl { control: handler.clone(), stop: true }),
    );
    let materialize = handler;
    registry.register(
        Tool::new(
            "materialize_delegated_change",
            "Apply one reviewed delegated Change candidate to its canonical workspace. Use only after the independent Reviewer has passed and only for a delivery present in delegated_deliveries. Acceptance proves the reviewed commit was applied, not a content readback. If the user requests post-apply readback, use read_file; producer evidence describes the candidate, not a fresh canonical file read.",
            json!({
                "type":"object", "additionalProperties":false,
                "required":["delivery_id"],
                "properties":{"delivery_id":{"type":"string","minLength":1,"maxLength":128}}
            }),
        )
        .with_confirmation(true),
        std::sync::Arc::new(MaterializeDelegatedChange { control: materialize }),
    );
}

struct DelegatedWorkControl {
    control: ForegroundDelegationControl,
    stop: bool,
}

impl ToolHandler for DelegatedWorkControl {
    fn execute(&self, arguments: &Value, _: &std::path::Path) -> ToolResult {
        let name = self.name();
        if self.control.allowed_delivery_id.is_some() {
            return ToolResult::error(
                name,
                "automatic delivery summaries cannot control delegated work",
            );
        }
        let Some(service) = self.control.service.as_ref() else {
            return ToolResult::error(name, "delegated work control is unavailable");
        };
        if !self.stop {
            if arguments
                .as_object()
                .is_none_or(|object| !object.is_empty())
            {
                return ToolResult::error(name, "list_delegated_work takes no arguments");
            }
            return match service.list_for_session(&self.control.session_id) {
                Ok(tasks) => {
                    ToolResult::success(name, json!({"tasks":tasks,"limit":12}).to_string())
                }
                Err(_) => ToolResult::error(name, "delegated work state is unavailable"),
            };
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct StopRequest {
            delegation_id: String,
            attempt_id: String,
        }
        let request = match serde_json::from_value::<StopRequest>(arguments.clone()) {
            Ok(request) if valid_id(&request.delegation_id) && valid_id(&request.attempt_id) => {
                request
            }
            _ => return ToolResult::error(name, "exact delegation_id and attempt_id are required"),
        };
        match service.stop_for_session(&self.control.session_id, &request.delegation_id, &request.attempt_id) {
            Ok(outcome) => ToolResult::success(name, json!({
                "status":outcome.status, "delegation_id":request.delegation_id,"attempt_id":request.attempt_id,
                "cleanup_pending":outcome.cleanup_pending,"completed_actions_reverted":false
            }).to_string()),
            Err(DelegationServiceError::StopUnavailable) => ToolResult::error(name,
                "delegated task is unavailable in this session or its attempt has changed; inspect current work before deciding again"),
            Err(DelegationServiceError::MaterializationInProgress) => ToolResult::error(name,
                "canonical changes may already be applying; recovery is required, no rollback or cleanup was performed"),
            Err(_) => ToolResult::error(name, "delegated stop could not be confirmed; do not claim the task stopped"),
        }
    }

    fn name(&self) -> &str {
        if self.stop {
            "stop_delegated_work"
        } else {
            "list_delegated_work"
        }
    }
}

/// Register the single capability permitted for a background Main-Agent
/// summary of one independently reviewed delivery.  The caller owns the
/// delivery id from a durable queue binding; model output never selects the
/// scope.
pub(crate) fn register_review_only(
    registry: &mut ToolRegistry,
    session_id: String,
    service: Option<DelegationServiceHandle>,
    allowed_delivery_id: String,
) {
    register_review_control(
        registry,
        ForegroundDelegationControl {
            session_id,
            service,
            allowed_delivery_id: Some(allowed_delivery_id),
        },
    );
}

fn register_review_control(registry: &mut ToolRegistry, handler: ForegroundDelegationControl) {
    registry.register(
        Tool::new(
            "review_delegated_delivery",
            "Acknowledge one structured delegated-work delivery as the Main Agent. Use only for a delivery present in the delegated_deliveries context; never expose child-agent controls to the user.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["delivery_id", "decision"],
                "properties": {
                    "delivery_id": {"type": "string", "minLength": 1, "maxLength": 128},
                    "decision": {"type": "string", "enum": ["accept", "needs_decision"]},
                    "reason": {"type": "string", "maxLength": 600}
                }
            }),
        ),
        std::sync::Arc::new(handler),
    );
}

/// Complete the non-execution side of a rejected materialization
/// confirmation.  The foreground confirmation state machine owns the user's
/// decision; this narrow helper owns the delegated Change cleanup request.
pub(crate) fn reject_materialize_change_confirmation(
    service: Option<&DelegationServiceHandle>,
    session_id: &str,
    arguments: &Value,
) -> Result<(), String> {
    let delivery_id = arguments
        .get("delivery_id")
        .and_then(Value::as_str)
        .filter(|value| valid_id(value))
        .ok_or_else(|| "saved materialization delivery_id is invalid".to_string())?;
    let service = service.ok_or_else(|| "delegated delivery control is unavailable".to_string())?;
    service
        .decline_change_for_session(session_id, delivery_id, CHANGE_CONFIRMATION_TTL_SECS)
        .map_err(|_| "reviewed delegated change could not be discarded".to_string())
}

#[derive(Clone)]
struct MaterializeDelegatedChange {
    control: ForegroundDelegationControl,
}

impl ToolHandler for MaterializeDelegatedChange {
    fn execute(&self, arguments: &Value, _: &std::path::Path) -> ToolResult {
        self.execute_materialize(arguments)
    }

    fn execute_with_context(
        &self,
        arguments: &Value,
        _: &std::path::Path,
        _: &ToolExecutionContext,
    ) -> ToolResult {
        self.execute_materialize(arguments)
    }

    fn name(&self) -> &str {
        "materialize_delegated_change"
    }
}

impl MaterializeDelegatedChange {
    fn execute_materialize(&self, arguments: &Value) -> ToolResult {
        let delivery_id = arguments
            .get("delivery_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !valid_id(delivery_id) {
            return ToolResult::error("materialize_delegated_change", "delivery_id is invalid");
        }
        let Some(service) = self.control.service.as_ref() else {
            return ToolResult::error(
                "materialize_delegated_change",
                "delegated delivery control is unavailable",
            );
        };
        match service.materialize_change_for_session(
            &self.control.session_id,
            delivery_id,
            CHANGE_CONFIRMATION_TTL_SECS,
        ) {
            Ok(outcome) => ToolResult::success(
                "materialize_delegated_change",
                decision_result_json(delivery_id, outcome, "canonical_workspace"),
            ),
            Err(error) => ToolResult::error("materialize_delegated_change", decision_error(error)),
        }
    }
}

impl ForegroundDelegationControl {
    fn execute_request(&self, arguments: &Value) -> ToolResult {
        let request = match serde_json::from_value::<DeliveryDecisionRequest>(arguments.clone()) {
            Ok(request) => request,
            Err(_) => {
                return ToolResult::error(
                    "review_delegated_delivery",
                    "delegated delivery decision was rejected",
                )
            }
        };
        if !valid_id(&request.delivery_id) {
            return ToolResult::error("review_delegated_delivery", "delivery_id is invalid");
        }
        if self
            .allowed_delivery_id
            .as_deref()
            .is_some_and(|allowed| allowed != request.delivery_id.as_str())
        {
            return ToolResult::error(
                "review_delegated_delivery",
                "automatic reviewed-delivery summary may acknowledge only its scoped delivery",
            );
        }
        if request
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains(['\0', '\r', '\n']) || reason.len() > 600)
        {
            return ToolResult::error("review_delegated_delivery", "reason is invalid");
        }
        let decision = match request.decision.as_str() {
            "accept" => ParentDecision::Accept,
            "needs_decision" => ParentDecision::NeedsDecision,
            _ => {
                return ToolResult::error(
                    "review_delegated_delivery",
                    "decision must be accept or needs_decision",
                )
            }
        };
        let Some(service) = self.service.as_ref() else {
            return ToolResult::error(
                "review_delegated_delivery",
                "delegated delivery control is unavailable",
            );
        };
        let outcome = service.decide_for_session(
            &self.session_id,
            &request.delivery_id,
            decision,
            request.reason.as_deref(),
        );
        match outcome {
            Ok(outcome) => ToolResult::success(
                "review_delegated_delivery",
                decision_result_json(&request.delivery_id, outcome, "review_only"),
            ),
            Err(error) => ToolResult::error("review_delegated_delivery", decision_error(error)),
        }
    }
}

impl ToolHandler for ForegroundDelegationControl {
    fn execute(&self, arguments: &Value, _: &std::path::Path) -> ToolResult {
        self.execute_request(arguments)
    }

    fn execute_with_context(
        &self,
        arguments: &Value,
        _: &std::path::Path,
        _: &ToolExecutionContext,
    ) -> ToolResult {
        self.execute_request(arguments)
    }

    fn name(&self) -> &str {
        "review_delegated_delivery"
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn decision_result_json(
    delivery_id: &str,
    outcome: DecisionOutcome,
    effect_scope: &'static str,
) -> String {
    let (status, context) = match outcome {
        DecisionOutcome::Accepted(context) => ("accepted", context),
        DecisionOutcome::AlreadyAccepted(context) => ("already_accepted", context),
        DecisionOutcome::NeedsDecision(context) => ("needs_decision", context),
        DecisionOutcome::AlreadyNeedsDecision(context) => ("already_needs_decision", context),
    };
    // Struct field order is deliberate: bounded continuation facts must keep
    // the host status/scope before any potentially long producer narrative.
    #[derive(serde::Serialize)]
    struct Receipt<'a> {
        status: &'static str,
        delivery_id: &'a str,
        effect_scope: &'static str,
        context_scope: &'static str,
        canonical_content_read: bool,
        context: crate::agent::delegation_contract::ParentContext,
    }
    serde_json::to_string(&Receipt {
        status,
        delivery_id,
        effect_scope,
        context_scope: "producer_delivery",
        canonical_content_read: false,
        context,
    })
    .unwrap_or_else(|_| "{\"status\":\"recorded\"}".to_string())
}

fn decision_error(error: DelegationServiceError) -> String {
    match error {
        DelegationServiceError::Inbox => {
            "delegated delivery is unavailable or outside the session scope".to_string()
        }
        DelegationServiceError::ChangeReviewNotReady => {
            "change delivery is waiting for the independent review/materialization pipeline"
                .to_string()
        }
        DelegationServiceError::ReviewPending => {
            "the independent reviewer is still checking this delegated result".to_string()
        }
        DelegationServiceError::ReviewRejected => {
            "the independent reviewer found unresolved issues in this delegated result".to_string()
        }
        DelegationServiceError::Review => {
            "the delegated result could not be reviewed; it remains pending for the Main Agent"
                .to_string()
        }
        DelegationServiceError::MaterializationInProgress =>
            "canonical write outcome is unknown; inspect the project before retrying, cancelling or discarding its recovery evidence".to_string(),
        DelegationServiceError::Stopped => "delegated delivery control is stopped".to_string(),
        _ => "delegated delivery decision failed".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::agent::{
        delegation_contract::ContractLimits,
        delegation_service::test_support::{recording_service, ServiceCall},
        ToolCall,
    };

    #[test]
    fn normal_control_keeps_review_and_materialization_capabilities() {
        let limits = ContractLimits {
            max_summary_bytes: 41,
            ..ContractLimits::default()
        };
        let (service, calls) = recording_service(limits.clone());
        let mut registry = ToolRegistry::empty();
        register(&mut registry, "session-a".to_string(), Some(service));

        assert_eq!(registry.len(), 4);
        assert!(registry.contains("review_delegated_delivery"));
        assert!(registry.contains("materialize_delegated_change"));
        assert!(registry.contains("list_delegated_work"));
        assert!(registry.contains("stop_delegated_work"));
        assert!(
            registry
                .get("materialize_delegated_change")
                .unwrap()
                .requires_confirmation
        );
        let result = registry.execute(
            &ToolCall::new(
                "materialize_delegated_change".into(),
                json!({"delivery_id":"delivery_a"}),
            ),
            "materialize-call",
            Path::new("."),
        );
        assert!(!result.result.success);
        assert!(result
            .result
            .content
            .contains("review/materialization pipeline"));
        assert_eq!(
            *calls.lock().unwrap(),
            vec![ServiceCall::Materialize(
                "session-a".into(),
                "delivery_a".into(),
                CHANGE_CONFIRMATION_TTL_SECS,
                limits
            )]
        );
        calls.lock().unwrap().clear();
        assert!(
            registry
                .execute(
                    &ToolCall::new("list_delegated_work".into(), json!({})),
                    "list",
                    Path::new(".")
                )
                .result
                .success
        );
        let stopped = registry.execute(
            &ToolCall::new(
                "stop_delegated_work".into(),
                json!({"delegation_id":"task-a","attempt_id":"attempt-a"}),
            ),
            "stop",
            Path::new("."),
        );
        assert!(stopped.result.success);
        let receipt: Value = serde_json::from_str(&stopped.result.content).unwrap();
        assert_eq!(receipt["status"], "stopped");
        assert_eq!(receipt["completed_actions_reverted"], false);
        let rejected = registry.execute(&ToolCall::new("stop_delegated_work".into(),
            json!({"delegation_id":"task-a","attempt_id":"attempt-a","session_id":"other-session"})), "bad-stop", Path::new("."));
        assert!(!rejected.result.success);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                ServiceCall::List("session-a".into()),
                ServiceCall::Stop("session-a".into(), "task-a".into(), "attempt-a".into()),
            ]
        );
    }

    #[test]
    fn decision_receipt_keeps_status_before_bounded_producer_context() {
        let context = serde_json::from_value(json!({
            "identity":{"session_id":"s","message_id":"m","parent_run_id":"r","delegation_id":"d","attempt_id":"a"},
            "status":"completed","summary":"candidate ".repeat(50),
            "facts":[{"statement":"producer evidence ".repeat(50),"confidence":"high","evidence_refs":[]}],
            "milestones":[],"verifications":[],"artifacts":[],"open_questions":[],"risks":[],"evidence_refs":[],
            "usage":{"context_truncated":false,"output_truncated":false,"budget_exhausted":false}
        })).unwrap();
        let receipt = decision_result_json(
            "delivery",
            DecisionOutcome::Accepted(context),
            "canonical_workspace",
        );
        let bounded: String = receipt.chars().take(600).collect();
        assert!(
            bounded.contains("\"status\":\"accepted\""),
            "continuation facts must retain the host decision before producer text is truncated"
        );
        assert!(bounded.contains("\"effect_scope\":\"canonical_workspace\""));
        assert!(bounded.contains("\"context_scope\":\"producer_delivery\""));
        assert!(bounded.contains("\"canonical_content_read\":false"));
    }

    #[test]
    fn review_only_control_cannot_widen_an_automatic_summary_scope() {
        let limits = ContractLimits::default();
        let (service, calls) = recording_service(limits.clone());
        let mut registry = ToolRegistry::empty();
        register_review_only(
            &mut registry,
            "session-a".to_string(),
            Some(service),
            "delivery-allowed".to_string(),
        );

        assert_eq!(registry.len(), 1);
        assert!(registry.contains("review_delegated_delivery"));
        assert!(!registry.contains("materialize_delegated_change"));
        assert!(!registry.contains("list_delegated_work"));
        assert!(!registry.contains("stop_delegated_work"));

        let rejected = registry.execute(
            &ToolCall::new(
                "review_delegated_delivery".to_string(),
                json!({"delivery_id": "delivery-other", "decision": "accept"}),
            ),
            "summary-call-1",
            Path::new("."),
        );
        assert!(!rejected.result.success);
        assert!(rejected.result.content.contains("scoped delivery"));
        assert!(calls.lock().unwrap().is_empty());

        let allowed = registry.execute(
            &ToolCall::new(
                "review_delegated_delivery".to_string(),
                json!({"delivery_id": "delivery-allowed", "decision": "accept"}),
            ),
            "summary-call-2",
            Path::new("."),
        );
        assert!(!allowed.result.success);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[ServiceCall::Decide(
                "session-a".to_string(),
                "delivery-allowed".to_string(),
                ParentDecision::Accept,
                None,
                limits,
            )]
        );
    }

    #[test]
    fn rejected_materialization_confirmation_discards_the_session_scoped_change() {
        let limits = ContractLimits::default();
        let (service, calls) = recording_service(limits.clone());
        reject_materialize_change_confirmation(
            Some(&service),
            "session-a",
            &json!({"delivery_id": "delivery_a"}),
        )
        .unwrap();

        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[ServiceCall::Decline(
                "session-a".to_string(),
                "delivery_a".to_string(),
                CHANGE_CONFIRMATION_TTL_SECS,
                limits,
            )]
        );
    }
}

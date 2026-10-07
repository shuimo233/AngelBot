//! Main-Agent-only control of structured delegated deliveries.
//!
//! The user never receives a child-agent control surface. The foreground
//! runner sees a bounded delivery projection in context and may acknowledge
//! it through this internal tool, which resolves the durable parent scope
//! from the session-owned delegation row.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent::{
    delegation_pump::DelegationPumpHostHandle,
    delegation_service::DelegationServiceError,
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
    pump: Option<DelegationPumpHostHandle>,
    /// Automatic summary turns are scoped to one durable delivery.  This is
    /// intentionally enforced by the handler instead of trusting the model
    /// to keep its function-call arguments within the prompt's instruction.
    allowed_delivery_id: Option<String>,
}

pub(crate) fn register(
    registry: &mut ToolRegistry,
    session_id: String,
    pump: Option<DelegationPumpHostHandle>,
) {
    let handler = ForegroundDelegationControl {
        session_id,
        pump,
        allowed_delivery_id: None,
    };
    register_review_control(registry, handler.clone());
    let materialize = handler;
    registry.register(
        Tool::new(
            "materialize_delegated_change",
            "Apply one reviewed delegated Change candidate to its canonical workspace. Use only after the independent Reviewer has passed and only for a delivery present in delegated_deliveries.",
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

/// Register the single capability permitted for a background Main-Agent
/// summary of one independently reviewed delivery.  The caller owns the
/// delivery id from a durable queue binding; model output never selects the
/// scope.
pub(crate) fn register_review_only(
    registry: &mut ToolRegistry,
    session_id: String,
    pump: Option<DelegationPumpHostHandle>,
    allowed_delivery_id: String,
) {
    register_review_control(
        registry,
        ForegroundDelegationControl {
            session_id,
            pump,
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
    pump: Option<&DelegationPumpHostHandle>,
    session_id: &str,
    arguments: &Value,
) -> Result<(), String> {
    let delivery_id = arguments
        .get("delivery_id")
        .and_then(Value::as_str)
        .filter(|value| valid_id(value))
        .ok_or_else(|| "saved materialization delivery_id is invalid".to_string())?;
    let pump = pump.ok_or_else(|| "delegated delivery control is unavailable".to_string())?;
    pump.decline_change_for_session(session_id, delivery_id, CHANGE_CONFIRMATION_TTL_SECS)
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
        let Some(pump) = self.control.pump.as_ref() else {
            return ToolResult::error(
                "materialize_delegated_change",
                "delegated delivery control is unavailable",
            );
        };
        match pump.materialize_change_for_session(
            &self.control.session_id,
            delivery_id,
            CHANGE_CONFIRMATION_TTL_SECS,
        ) {
            Ok(outcome) => ToolResult::success(
                "materialize_delegated_change",
                decision_result_json(delivery_id, outcome),
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
        let Some(pump) = self.pump.as_ref() else {
            return ToolResult::error(
                "review_delegated_delivery",
                "delegated delivery control is unavailable",
            );
        };
        let outcome = pump.decide_for_session(
            &self.session_id,
            &request.delivery_id,
            decision,
            request.reason.as_deref(),
        );
        match outcome {
            Ok(outcome) => ToolResult::success(
                "review_delegated_delivery",
                decision_result_json(&request.delivery_id, outcome),
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

fn decision_result_json(delivery_id: &str, outcome: DecisionOutcome) -> String {
    let (status, context) = match outcome {
        DecisionOutcome::Accepted(context) => ("accepted", context),
        DecisionOutcome::AlreadyAccepted(context) => ("already_accepted", context),
        DecisionOutcome::NeedsDecision(context) => ("needs_decision", context),
        DecisionOutcome::AlreadyNeedsDecision(context) => ("already_needs_decision", context),
    };
    serde_json::to_string(&json!({
        "delivery_id": delivery_id,
        "status": status,
        "context": context,
    }))
    .unwrap_or_else(|_| "{\"status\":\"recorded\"}".to_string())
}

fn decision_error(error: crate::agent::delegation_pump::DelegationPumpError) -> String {
    match error {
        crate::agent::delegation_pump::DelegationPumpError::Service(
            DelegationServiceError::Inbox,
        ) => "delegated delivery is unavailable or outside the session scope".to_string(),
        crate::agent::delegation_pump::DelegationPumpError::Service(
            DelegationServiceError::ChangeReviewNotReady,
        ) => "change delivery is waiting for the independent review/materialization pipeline"
            .to_string(),
        crate::agent::delegation_pump::DelegationPumpError::Service(
            DelegationServiceError::ReviewPending,
        ) => "the independent reviewer is still checking this delegated result".to_string(),
        crate::agent::delegation_pump::DelegationPumpError::Service(
            DelegationServiceError::ReviewRejected,
        ) => {
            "the independent reviewer found unresolved issues in this delegated result".to_string()
        }
        crate::agent::delegation_pump::DelegationPumpError::Service(
            DelegationServiceError::Review,
        ) => "the delegated result could not be reviewed; it remains pending for the Main Agent"
            .to_string(),
        crate::agent::delegation_pump::DelegationPumpError::Stopped => {
            "delegated delivery control is stopped".to_string()
        }
        _ => "delegated delivery decision failed".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::agent::{
        delegation_pump::{DelegationPumpError, DelegationPumpLoop},
        delegation_service::ServiceReport,
        ToolCall,
    };

    struct DeclineSpy {
        calls: Arc<Mutex<Vec<(String, String, i64)>>>,
    }

    struct DecisionSpy {
        calls: Arc<Mutex<Vec<(String, String, ParentDecision, Option<String>)>>>,
    }

    impl DelegationPumpLoop for DeclineSpy {
        fn start(&mut self) -> Result<ServiceReport, DelegationPumpError> {
            Ok(ServiceReport::default())
        }

        fn tick(&mut self) -> Result<ServiceReport, DelegationPumpError> {
            Ok(ServiceReport::default())
        }

        fn shutdown(&mut self) -> Result<(), DelegationPumpError> {
            Ok(())
        }

        fn decline_change_for_session(
            &mut self,
            session_id: &str,
            delivery_id: &str,
            ttl_secs: i64,
        ) -> Result<(), DelegationPumpError> {
            self.calls.lock().unwrap().push((
                session_id.to_string(),
                delivery_id.to_string(),
                ttl_secs,
            ));
            Ok(())
        }
    }

    impl DelegationPumpLoop for DecisionSpy {
        fn start(&mut self) -> Result<ServiceReport, DelegationPumpError> {
            Ok(ServiceReport::default())
        }

        fn tick(&mut self) -> Result<ServiceReport, DelegationPumpError> {
            Ok(ServiceReport::default())
        }

        fn shutdown(&mut self) -> Result<(), DelegationPumpError> {
            Ok(())
        }

        fn decide_for_session(
            &mut self,
            session_id: &str,
            delivery_id: &str,
            decision: ParentDecision,
            reason: Option<&str>,
        ) -> Result<DecisionOutcome, DelegationPumpError> {
            self.calls.lock().unwrap().push((
                session_id.to_string(),
                delivery_id.to_string(),
                decision,
                reason.map(str::to_string),
            ));
            Err(DelegationPumpError::Stopped)
        }
    }

    #[test]
    fn normal_control_keeps_review_and_materialization_capabilities() {
        let mut registry = ToolRegistry::empty();
        register(&mut registry, "session-a".to_string(), None);

        assert_eq!(registry.len(), 2);
        assert!(registry.contains("review_delegated_delivery"));
        assert!(registry.contains("materialize_delegated_change"));
    }

    #[test]
    fn review_only_control_cannot_widen_an_automatic_summary_scope() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let pump = DelegationPumpHostHandle::new(
            crate::agent::delegation_pump::DelegationPumpHandle::new(DecisionSpy {
                calls: calls.clone(),
            }),
        );
        let mut registry = ToolRegistry::empty();
        register_review_only(
            &mut registry,
            "session-a".to_string(),
            Some(pump),
            "delivery-allowed".to_string(),
        );

        assert_eq!(registry.len(), 1);
        assert!(registry.contains("review_delegated_delivery"));
        assert!(!registry.contains("materialize_delegated_change"));

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
            &[(
                "session-a".to_string(),
                "delivery-allowed".to_string(),
                ParentDecision::Accept,
                None,
            )]
        );
    }

    #[test]
    fn rejected_materialization_confirmation_discards_the_session_scoped_change() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let pump = DelegationPumpHostHandle::new(
            crate::agent::delegation_pump::DelegationPumpHandle::new(DeclineSpy {
                calls: calls.clone(),
            }),
        );
        reject_materialize_change_confirmation(
            Some(&pump),
            "session-a",
            &json!({"delivery_id": "delivery_a"}),
        )
        .unwrap();

        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[(
                "session-a".to_string(),
                "delivery_a".to_string(),
                CHANGE_CONFIRMATION_TTL_SECS,
            )]
        );
    }
}

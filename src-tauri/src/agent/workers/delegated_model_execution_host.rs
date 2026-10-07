//! Model-driven ExecutionHost for change workers.
//!
//! This module is the only place where a delegated Implementer/Verifier model
//! sees a prompt and a capability-scoped tool surface. It does not own leases,
//! confirmations, canonical writes, or a scheduler loop. File operations are
//! translated into `WorkerTask` requests and all model output must be one
//! bounded structured `DelegationDelivery` JSON document.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::llm::{LlmProvider, LlmResponse, Message, ThinkingSettings, ToolCall, ToolSchema};

use super::{
    delegation_contract::{DelegationBrief, DelegationDelivery},
    execution_kernel::{
        ExecutionHost, KernelFlow, KernelNext, ModelTurnRequest, ToolSettlement,
        ToolSettlementBatch, ToolSettlementTrust,
    },
    worker_host_contract::{
        WorkerHostBinding, WorkerHostOperation, WorkerHostRequest, WorkerHostResult, WorkerTask,
    },
    worker_policy::{TaskShape, WorkerProfile},
};

const MAX_TOOL_RESULT_BYTES: usize = 64 * 1024;

pub struct DelegatedModelExecutionHost<T> {
    provider: std::sync::Arc<dyn LlmProvider>,
    thinking: Option<ThinkingSettings>,
    binding: WorkerHostBinding,
    task: T,
    messages: Vec<Message>,
    tools: Vec<ToolSchema>,
}

impl<T> DelegatedModelExecutionHost<T>
where
    T: WorkerTask,
{
    pub fn new(
        provider: std::sync::Arc<dyn LlmProvider>,
        thinking: Option<ThinkingSettings>,
        binding: WorkerHostBinding,
        task: T,
        brief: &DelegationBrief,
    ) -> Result<Self, String> {
        let tools = tools_for(binding.worker_profile(), binding.task_shape())?;
        let prompt = serde_json::to_string(&json!({
            "goal": brief.goal,
            "background": brief.background,
            "constraints": brief.constraints,
            "allowed_references": brief.allowed_references,
            "completion_criteria": brief.completion_criteria,
        }))
        .map_err(|_| "delegated brief serialization failed".to_string())?;
        let role = match binding.worker_profile() {
            WorkerProfile::Implementer => {
                "You are the bounded Implementer worker. Only modify the candidate worktree through worker.write_candidate. Never write the canonical workspace, request confirmation, or use network tools."
            }
            WorkerProfile::Verifier => {
                "You are the independent Verifier worker. You have read-only access to the candidate worktree. Do not modify files, use network tools, confirm changes, or materialize anything."
            }
            WorkerProfile::Explorer => {
                "You are the bounded Explorer worker. You have read-only access to a temporary project worktree. Inspect only the files needed for the assigned goal. Do not modify files, use network tools, confirm changes, or materialize anything."
            }
        };
        Ok(Self {
            provider,
            thinking,
            binding,
            task,
            messages: vec![
                Message {
                    role: "system".into(),
                    content: format!(
                        "{role} Return exactly one JSON DelegationDelivery when complete; do not wrap it in markdown."
                    ),
                    tool_calls: None,
                    tool_call_id: None,
                    protocol_state: None,
                    tool_images: Vec::new(),
                },
                Message {
                    role: "user".into(),
                    content: prompt,
                    tool_calls: None,
                    tool_call_id: None,
                    protocol_state: None,
                    tool_images: Vec::new(),
                },
            ],
            tools,
        })
    }

    pub fn task_mut(&mut self) -> &mut T {
        &mut self.task
    }

    fn parse_operation(call: &ToolCall) -> Result<WorkerHostOperation, String> {
        let args = &call.arguments;
        match call.name.as_str() {
            "worker.read" => Ok(WorkerHostOperation::Read {
                reference: args
                    .get("reference")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "worker.read reference is required".to_string())?
                    .to_string(),
            }),
            "worker.write_candidate" => {
                let reference = args
                    .get("reference")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "worker.write_candidate reference is required".to_string())?
                    .to_string();
                let contents = args
                    .get("contents")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "worker.write_candidate contents is required".to_string())?
                    .as_bytes()
                    .to_vec();
                Ok(WorkerHostOperation::WriteCandidate {
                    reference,
                    contents,
                })
            }
            _ => Err("delegated worker tool is not allowlisted".into()),
        }
    }

    fn result_value(result: WorkerHostResult) -> Result<Value, String> {
        let value = match result {
            WorkerHostResult::Acknowledged => json!({"ok": true}),
            WorkerHostResult::Text(text) => json!({"text": text}),
            WorkerHostResult::Evidence(refs) => json!({"evidence_refs": refs}),
        };
        let bytes = serde_json::to_vec(&value).map_err(|_| "tool result serialization failed")?;
        if bytes.len() > MAX_TOOL_RESULT_BYTES {
            return Err("tool result exceeded bounded worker context".into());
        }
        Ok(value)
    }
}

fn parse_delivery(text: &str) -> Result<DelegationDelivery, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > 128 * 1024 || trimmed.starts_with("```") {
        return Err("delegated worker must return bounded raw delivery JSON".into());
    }
    serde_json::from_str(trimmed).map_err(|_| "delegated worker delivery JSON is invalid".into())
}

#[async_trait]
impl<T> ExecutionHost for DelegatedModelExecutionHost<T>
where
    T: WorkerTask,
{
    type Output = DelegationDelivery;
    type Error = String;

    async fn next_model_turn(
        &mut self,
        _turn: usize,
    ) -> Result<KernelNext<Self::Output>, Self::Error> {
        Ok(KernelNext::Model(ModelTurnRequest::new(
            self.provider.clone(),
            self.messages.clone(),
            Some(self.tools.clone()),
            self.thinking.clone(),
            None,
        )))
    }

    async fn settle_model_response(
        &mut self,
        _turn: usize,
        response: LlmResponse,
    ) -> Result<KernelFlow<Self::Output>, Self::Error> {
        let (response, protocol_state) = response.into_parts();
        if let Some(state) = &protocol_state {
            state.validate()?;
        }
        match response {
            LlmResponse::Text { text, .. } => {
                let delivery = parse_delivery(&text)?;
                self.messages.push(Message {
                    role: "assistant".into(),
                    content: text,
                    tool_calls: None,
                    tool_call_id: None,
                    tool_images: Vec::new(),
                    protocol_state,
                });
                Ok(KernelFlow::Complete(delivery))
            }
            LlmResponse::ProtocolViolation { .. } => {
                Err("delegated worker model protocol violation".into())
            }
            LlmResponse::ToolCalls {
                calls,
                text,
                stop_reason,
            } => {
                if calls.is_empty()
                    || calls.len() > 2
                    || stop_reason != crate::llm::StopReason::ToolUse
                {
                    return Err("delegated worker tool batch is out of bounds".into());
                }
                let ids: std::collections::BTreeSet<_> =
                    calls.iter().map(|call| call.id.as_str()).collect();
                if ids.len() != calls.len()
                    || calls.iter().any(|call| {
                        call.id.trim().is_empty()
                            || !self.tools.iter().any(|tool| tool.name == call.name)
                    })
                {
                    return Err("delegated worker tool batch is invalid".into());
                }
                // Validate every capability before executing the first one.
                // A malformed later call must not leave a partially executed batch.
                let operations = calls
                    .iter()
                    .map(Self::parse_operation)
                    .collect::<Result<Vec<_>, _>>()?;
                let requests = operations
                    .into_iter()
                    .map(|operation| {
                        WorkerHostRequest::new(&self.binding, operation)
                            .map_err(|_| "delegated worker capability request rejected".to_string())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                self.messages.push(Message {
                    role: "assistant".into(),
                    content: text.unwrap_or_default(),
                    tool_calls: Some(calls.clone()),
                    tool_call_id: None,
                    tool_images: Vec::new(),
                    protocol_state,
                });
                let mut settlements = Vec::with_capacity(calls.len());
                for (call, request) in calls.iter().zip(requests) {
                    let result =
                        self.task.dispatch_with_result(request).await.map_err(|_| {
                            "delegated worker capability operation failed".to_string()
                        })?;
                    let value = Self::result_value(result)?;
                    self.messages.push(Message {
                        role: "tool".into(),
                        content: serde_json::to_string(&value)
                            .map_err(|_| "tool result encoding failed".to_string())?,
                        tool_calls: None,
                        tool_call_id: Some(call.id.clone()),
                        protocol_state: None,
                        tool_images: Vec::new(),
                    });
                    settlements.push(ToolSettlement {
                        call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        result: value,
                        trust: ToolSettlementTrust::UntrustedWorkspace,
                    });
                }
                ToolSettlementBatch::new(calls, settlements)
                    .map_err(|_| "delegated worker tool settlement was invalid".to_string())?;
                Ok(KernelFlow::Continue)
            }
            LlmResponse::WithProtocol { .. } => {
                unreachable!("protocol wrappers are flattened by into_parts")
            }
        }
    }
}

fn tools_for(profile: WorkerProfile, shape: TaskShape) -> Result<Vec<ToolSchema>, String> {
    if !matches!(
        (shape, profile),
        (
            TaskShape::Change,
            WorkerProfile::Implementer | WorkerProfile::Verifier
        ) | (TaskShape::Explore, WorkerProfile::Explorer)
    ) {
        return Err("delegated model host profile is unsupported".into());
    }
    let mut tools = vec![ToolSchema {
        name: "worker.read".into(),
        description: "Read a relative file from the temporary worktree.".into(),
        parameters: json!({"type":"object","additionalProperties":false,"required":["reference"],"properties":{"reference":{"type":"string","maxLength":512}}}),
    }];
    if profile == WorkerProfile::Implementer {
        tools.push(ToolSchema {
            name: "worker.write_candidate".into(),
            description: "Write candidate contents to a relative path in the temporary worktree.".into(),
            parameters: json!({"type":"object","additionalProperties":false,"required":["reference","contents"],"properties":{"reference":{"type":"string","maxLength":512},"contents":{"type":"string","maxLength":1048576}}}),
        });
    }
    Ok(tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::agent::worker_host_contract::{
        FakeWorkerHost, FakeWorkerHostFactory, WorkerHostError, WorkerHostEvent, WorkerHostFactory,
        WorkerHostTerminal,
    };
    use crate::llm::{ModelProtocol, ProtocolContinuation, StopReason};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct UnusedProvider;
    #[async_trait]
    impl LlmProvider for UnusedProvider {
        fn name(&self) -> &str {
            "fixture"
        }
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: Option<&[ToolSchema]>,
            _thinking: Option<&ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            Err("unused".into())
        }
    }
    struct CountingTask {
        inner: FakeWorkerHost,
        calls: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl WorkerTask for CountingTask {
        async fn dispatch(&mut self, request: WorkerHostRequest) -> Result<(), WorkerHostError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.inner.dispatch(request).await
        }
        async fn heartbeat(&mut self) -> Result<WorkerHostEvent, WorkerHostError> {
            self.inner.heartbeat().await
        }
        async fn deliver(
            &mut self,
            delivery: DelegationDelivery,
        ) -> Result<WorkerHostEvent, WorkerHostError> {
            self.inner.deliver(delivery).await
        }
        async fn stop(
            &mut self,
            terminal: WorkerHostTerminal,
        ) -> Result<WorkerHostEvent, WorkerHostError> {
            self.inner.stop(terminal).await
        }
    }
    fn private_host() -> (DelegatedModelExecutionHost<CountingTask>, Arc<AtomicUsize>) {
        let binding = WorkerHostBinding::new(
            "attempt",
            "delegation",
            "package",
            1,
            "capability",
            WorkerProfile::Explorer,
            TaskShape::Explore,
        )
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let task = CountingTask {
            inner: FakeWorkerHostFactory.start(binding.clone()).unwrap(),
            calls: calls.clone(),
        };
        (
            DelegatedModelExecutionHost {
                provider: Arc::new(UnusedProvider),
                thinking: None,
                binding,
                task,
                messages: vec![],
                tools: tools_for(WorkerProfile::Explorer, TaskShape::Explore).unwrap(),
            },
            calls,
        )
    }
    fn private_tool_response(stop_reason: StopReason, invalid_second: bool) -> LlmResponse {
        let mut calls = vec![ToolCall {
            id: "call-worker".into(),
            name: "worker.read".into(),
            arguments: json!({"reference":"a.txt"}),
        }];
        if invalid_second {
            calls.push(ToolCall {
                id: "call-invalid".into(),
                name: "worker.read".into(),
                arguments: json!({"reference":""}),
            });
        }
        LlmResponse::WithProtocol {
            response: Box::new(LlmResponse::ToolCalls {
                calls,
                text: None,
                stop_reason,
            }),
            protocol: ProtocolContinuation {
                protocol: ModelProtocol::OpenaiResponses,
                model: "fixture".into(),
                credential_ref: "fixture-profile".into(),
                output_items: vec![
                    json!({"type":"reasoning","encrypted_content":"worker-marker"}),
                    json!({"type":"function_call","call_id":"call-worker","name":"worker.read","arguments":"{\"reference\":\"a.txt\"}"}),
                ],
            },
        }
    }
    #[tokio::test]
    async fn private_protocol_worker_replays_exact_output_and_tool_id_on_next_request() {
        let (mut host, calls) = private_host();
        assert!(matches!(
            host.settle_model_response(0, private_tool_response(StopReason::ToolUse, false))
                .await
                .unwrap(),
            KernelFlow::Continue
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let KernelNext::Model(request) = host.next_model_turn(1).await.unwrap() else {
            panic!("next model request expected")
        };
        assert_eq!(
            request.messages[0]
                .protocol_state
                .as_ref()
                .unwrap()
                .output_items[0]["encrypted_content"],
            "worker-marker"
        );
        assert_eq!(
            request.messages[1].tool_call_id.as_deref(),
            Some("call-worker")
        );
    }
    #[tokio::test]
    async fn private_protocol_worker_rejects_incomplete_or_malformed_batch_before_any_dispatch() {
        for (reason, invalid_second) in
            [(StopReason::MaxTokens, false), (StopReason::ToolUse, true)]
        {
            let (mut host, calls) = private_host();
            assert!(host
                .settle_model_response(0, private_tool_response(reason, invalid_second))
                .await
                .is_err());
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert!(host.messages.is_empty());
        }
    }

    #[test]
    fn profile_tool_surface_keeps_verifier_read_only() {
        let implementer = tools_for(WorkerProfile::Implementer, TaskShape::Change).unwrap();
        assert_eq!(
            implementer
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["worker.read", "worker.write_candidate"]
        );
        let verifier = tools_for(WorkerProfile::Verifier, TaskShape::Change).unwrap();
        assert_eq!(
            verifier
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["worker.read"]
        );
        let explorer = tools_for(WorkerProfile::Explorer, TaskShape::Explore).unwrap();
        assert_eq!(
            explorer
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["worker.read"]
        );
    }

    #[test]
    fn delivery_parser_rejects_unbounded_or_markdown_wrapped_output() {
        assert!(parse_delivery("").is_err());
        assert!(parse_delivery("```json\n{}\n```").is_err());
        assert!(parse_delivery(&"x".repeat(128 * 1024 + 1)).is_err());
    }
}

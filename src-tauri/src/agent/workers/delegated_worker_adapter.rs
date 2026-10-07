//! Bounded execution adapter for one delegated attempt.
//!
//! This is intentionally an adapter, not a second agent loop.  Prompt shape
//! and model selection live in an injected [`ExecutionHost`]; bounded turns
//! and retries live in [`ExecutionKernel`]; durable admission and delivery
//! inbox semantics live in [`DelegationRuntime`].  The adapter only connects
//! those three already-owned concerns; capability-aware tool execution is
//! supplied by the injected worker host.

use std::{fmt, time::SystemTime};

use super::{
    delegation::LeaseStatus,
    delegation_contract::{
        validate_brief, validate_delivery, ContractLimits, DelegationBrief, DelegationDelivery,
    },
    delegation_runtime::{DelegationRuntime, Heartbeat, SandboxController},
    execution_gateway::CapabilityLease,
    execution_kernel::{ExecutionHost, ExecutionKernel, KernelRunError},
};

/// Immutable binding supplied after the Main Agent has issued a policy lease.
/// It deliberately contains neither a database connection nor a capability to
/// create children, renew a lease, confirm work, or materialize artifacts.
#[derive(Debug, Clone)]
pub struct DelegatedAttempt {
    pub delegation_id: String,
    pub attempt_id: String,
    pub work_package_id: String,
    pub lease: CapabilityLease,
    pub epoch: u64,
}

/// Small, structured progress projection.  It is intentionally not model
/// reasoning or a user-facing event stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DelegatedWorkerEvent {
    Started {
        attempt_id: String,
    },
    Heartbeat {
        attempt_id: String,
        stage: &'static str,
    },
    DeliverySubmitted {
        attempt_id: String,
        delivery_id: String,
    },
    Failed {
        attempt_id: String,
        kind: DelegatedWorkerFailureKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegatedWorkerFailureKind {
    BriefRejected,
    LeaseInactive,
    LeaseExpired,
    ModelFailed,
    BudgetExceeded,
    HostFailed,
    DeliveryRejected,
    RuntimeRejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegatedWorkerOutcome {
    pub delivery_id: String,
    pub delivery_revision: u32,
}

/// The only durable port granted to this adapter.  In particular it deliberately
/// has no `accept`, `issue_lease`, `renew`, `materialize`, or tool operation.
pub trait DelegatedDeliverySink {
    fn heartbeat(&mut self, heartbeat: Heartbeat) -> Result<(), String>;
    fn submit_delivery(
        &mut self,
        delivery: &DelegationDelivery,
        limits: &ContractLimits,
    ) -> Result<(), String>;
}

impl<W, C, P, S> DelegatedDeliverySink for DelegationRuntime<W, C, P, S>
where
    W: super::delegation_runtime::WorkerAdapter,
    C: super::delegation_runtime::Clock,
    P: super::delegation_runtime::AdmissionPolicy,
    S: SandboxController,
{
    fn heartbeat(&mut self, heartbeat: Heartbeat) -> Result<(), String> {
        DelegationRuntime::heartbeat(self, heartbeat)
            .map_err(|_| "runtime rejected heartbeat".to_string())
    }

    fn submit_delivery(
        &mut self,
        delivery: &DelegationDelivery,
        limits: &ContractLimits,
    ) -> Result<(), String> {
        DelegationRuntime::submit_delivery(self, delivery, limits)
            .map_err(|_| "runtime rejected delivery".to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegatedWorkerError {
    pub kind: DelegatedWorkerFailureKind,
}

impl fmt::Display for DelegatedWorkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("delegated worker attempt did not complete")
    }
}
impl std::error::Error for DelegatedWorkerError {}

/// Reuses the shared kernel while keeping capability execution in the injected
/// worker host. The adapter owns only lifecycle, heartbeat, delivery and
/// bounded kernel execution; it never interprets paths or grants capabilities.
#[derive(Clone)]
pub struct DelegatedWorkerAdapter {
    kernel: ExecutionKernel,
    limits: ContractLimits,
}

impl DelegatedWorkerAdapter {
    pub fn new(kernel: ExecutionKernel, limits: ContractLimits) -> Self {
        Self { kernel, limits }
    }

    pub async fn run<H, S>(
        &self,
        attempt: &DelegatedAttempt,
        brief: &DelegationBrief,
        host: &mut H,
        sink: &mut S,
        mut emit: impl FnMut(DelegatedWorkerEvent),
    ) -> Result<DelegatedWorkerOutcome, DelegatedWorkerError>
    where
        H: ExecutionHost<Output = DelegationDelivery>,
        S: DelegatedDeliverySink,
    {
        self.validate_attempt(attempt, brief, &mut emit)?;
        emit(DelegatedWorkerEvent::Started {
            attempt_id: attempt.attempt_id.clone(),
        });
        self.heartbeat(attempt, sink, "model", &mut emit)?;

        let delivery = match self.kernel.run(host).await {
            Ok(delivery) => delivery,
            Err(error) => return Err(self.fail(attempt, kernel_failure_kind(&error), &mut emit)),
        };
        if validate_delivery(&delivery, brief, None, &self.limits).is_err() {
            return Err(self.fail(
                attempt,
                DelegatedWorkerFailureKind::DeliveryRejected,
                &mut emit,
            ));
        }
        self.heartbeat(attempt, sink, "delivery", &mut emit)?;
        if sink.submit_delivery(&delivery, &self.limits).is_err() {
            return Err(self.fail(
                attempt,
                DelegatedWorkerFailureKind::RuntimeRejected,
                &mut emit,
            ));
        }
        emit(DelegatedWorkerEvent::DeliverySubmitted {
            attempt_id: attempt.attempt_id.clone(),
            delivery_id: delivery.delivery_id.clone(),
        });
        Ok(DelegatedWorkerOutcome {
            delivery_id: delivery.delivery_id,
            delivery_revision: delivery.delivery_revision,
        })
    }

    fn validate_attempt(
        &self,
        attempt: &DelegatedAttempt,
        brief: &DelegationBrief,
        emit: &mut impl FnMut(DelegatedWorkerEvent),
    ) -> Result<(), DelegatedWorkerError> {
        if validate_brief(brief, &self.limits).is_err()
            || brief.identity.delegation_id != attempt.delegation_id
            || brief.identity.attempt_id != attempt.attempt_id
            || attempt.lease.attempt_id != attempt.attempt_id
        {
            return Err(self.fail(attempt, DelegatedWorkerFailureKind::BriefRejected, emit));
        }
        if attempt.lease.status != LeaseStatus::Active {
            return Err(self.fail(attempt, DelegatedWorkerFailureKind::LeaseInactive, emit));
        }
        if now_ms() >= attempt.lease.expires_at_ms {
            return Err(self.fail(attempt, DelegatedWorkerFailureKind::LeaseExpired, emit));
        }
        Ok(())
    }

    fn heartbeat<S: DelegatedDeliverySink>(
        &self,
        attempt: &DelegatedAttempt,
        sink: &mut S,
        stage: &'static str,
        emit: &mut impl FnMut(DelegatedWorkerEvent),
    ) -> Result<(), DelegatedWorkerError> {
        sink.heartbeat(Heartbeat {
            attempt_id: attempt.attempt_id.clone(),
            epoch: attempt.epoch,
            stage: stage.to_string(),
            progress_percent: if stage == "model" { 1 } else { 90 },
            budget_remaining: "delegated_worker_v1".to_string(),
            evidence_refs: Vec::new(),
        })
        .map_err(|_| self.fail(attempt, DelegatedWorkerFailureKind::RuntimeRejected, emit))?;
        emit(DelegatedWorkerEvent::Heartbeat {
            attempt_id: attempt.attempt_id.clone(),
            stage,
        });
        Ok(())
    }

    fn fail(
        &self,
        attempt: &DelegatedAttempt,
        kind: DelegatedWorkerFailureKind,
        emit: &mut impl FnMut(DelegatedWorkerEvent),
    ) -> DelegatedWorkerError {
        emit(DelegatedWorkerEvent::Failed {
            attempt_id: attempt.attempt_id.clone(),
            kind,
        });
        DelegatedWorkerError { kind }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn kernel_failure_kind<E>(error: &KernelRunError<E>) -> DelegatedWorkerFailureKind {
    match error {
        KernelRunError::Host(_) => DelegatedWorkerFailureKind::HostFailed,
        KernelRunError::Model(_) => DelegatedWorkerFailureKind::ModelFailed,
        KernelRunError::BudgetExceeded(_) => DelegatedWorkerFailureKind::BudgetExceeded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        agent::{
            delegation_contract::{
                Confidence, KeyFact, Milestone, UsageFlags, VerificationRecord, VerificationStatus,
            },
            execution_gateway::CapabilityGrant,
            execution_kernel::{KernelFlow, KernelNext, ModelTurnRequest},
        },
        llm::{LlmProvider, LlmResponse, Message, StopReason, ThinkingSettings, ToolSchema},
    };
    use async_trait::async_trait;
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::Duration,
    };

    struct Sink {
        beats: usize,
        deliveries: usize,
    }
    impl DelegatedDeliverySink for Sink {
        fn heartbeat(&mut self, _: Heartbeat) -> Result<(), String> {
            self.beats += 1;
            Ok(())
        }
        fn submit_delivery(
            &mut self,
            _: &DelegationDelivery,
            _: &ContractLimits,
        ) -> Result<(), String> {
            self.deliveries += 1;
            Ok(())
        }
    }
    struct StaticHost {
        delivery: Option<DelegationDelivery>,
    }
    #[async_trait]
    impl ExecutionHost for StaticHost {
        type Output = DelegationDelivery;
        type Error = String;
        async fn next_model_turn(
            &mut self,
            _: usize,
        ) -> Result<KernelNext<Self::Output>, Self::Error> {
            Ok(KernelNext::Complete(self.delivery.take().unwrap()))
        }
        async fn settle_model_response(
            &mut self,
            _: usize,
            _: LlmResponse,
        ) -> Result<KernelFlow<Self::Output>, Self::Error> {
            unreachable!()
        }
    }
    struct FailsOnce {
        calls: AtomicUsize,
    }
    #[async_trait]
    impl LlmProvider for FailsOnce {
        fn name(&self) -> &str {
            "fails-once"
        }
        async fn chat(
            &self,
            _: &[Message],
            _: Option<&[ToolSchema]>,
            _: Option<&ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Err("transient".into())
            } else {
                Ok(LlmResponse::Text {
                    text: "ok".into(),
                    stop_reason: StopReason::EndTurn,
                })
            }
        }
    }
    struct RetryingHost {
        provider: Arc<dyn LlmProvider>,
        delivery: Option<DelegationDelivery>,
    }
    #[async_trait]
    impl ExecutionHost for RetryingHost {
        type Output = DelegationDelivery;
        type Error = String;
        async fn next_model_turn(
            &mut self,
            _: usize,
        ) -> Result<KernelNext<Self::Output>, Self::Error> {
            Ok(KernelNext::Model(ModelTurnRequest::new(
                self.provider.clone(),
                vec![Message {
                    tool_images: Vec::new(),
                    protocol_state: None,
                    role: "user".into(),
                    content: "return structured delivery".into(),
                    tool_calls: None,
                    tool_call_id: None,
                }],
                None,
                None,
                None,
            )))
        }
        async fn settle_model_response(
            &mut self,
            _: usize,
            response: LlmResponse,
        ) -> Result<KernelFlow<Self::Output>, Self::Error> {
            match response {
                LlmResponse::Text { .. } => Ok(KernelFlow::Complete(self.delivery.take().unwrap())),
                _ => Err("unexpected model response".into()),
            }
        }
    }
    fn identity() -> super::super::delegation_contract::DelegationIdentity {
        super::super::delegation_contract::DelegationIdentity {
            session_id: "s".into(),
            message_id: "m".into(),
            parent_run_id: "r".into(),
            delegation_id: "d".into(),
            attempt_id: "a".into(),
        }
    }
    fn brief() -> DelegationBrief {
        DelegationBrief {
            schema_version: 1,
            identity: identity(),
            goal: "inspect".into(),
            background: vec![],
            constraints: vec![],
            allowed_references: vec!["record://test/1".into()],
            allowed_capabilities: vec![],
            completion_criteria: vec!["report".into()],
        }
    }
    fn delivery() -> DelegationDelivery {
        DelegationDelivery {
            schema_version: 1,
            delivery_id: "x".into(),
            delivery_revision: 1,
            identity: identity(),
            status: super::super::delegation_contract::DeliveryStatus::Completed,
            executive_summary: "done".into(),
            key_facts: vec![KeyFact {
                statement: "done".into(),
                evidence_refs: vec!["record://test/1".into()],
                confidence: Confidence::High,
            }],
            milestones: vec![Milestone {
                label: "report".into(),
                outcome: "completed".into(),
                evidence_refs: vec!["record://test/1".into()],
            }],
            verifications: vec![VerificationRecord {
                item: "report".into(),
                method: "review".into(),
                status: VerificationStatus::Passed,
                conclusion: "ok".into(),
                evidence_ref: "record://test/1".into(),
            }],
            candidate_artifacts: vec![],
            open_questions: vec![],
            risks: vec![],
            evidence_refs: vec!["record://test/1".into()],
            usage: UsageFlags {
                context_truncated: false,
                output_truncated: false,
                budget_exhausted: false,
            },
        }
    }
    fn attempt(status: LeaseStatus, expires: i64) -> DelegatedAttempt {
        DelegatedAttempt {
            delegation_id: "d".into(),
            attempt_id: "a".into(),
            work_package_id: "w".into(),
            epoch: 1,
            lease: CapabilityLease::from_grant(CapabilityGrant {
                id: "l".into(),
                attempt_id: "a".into(),
                status,
                expires_at_ms: expires,
                read_roots: vec![],
                write_roots: vec![],
                tool_allowlist: vec![],
                network_hosts: vec![],
            })
            .unwrap(),
        }
    }
    fn adapter() -> DelegatedWorkerAdapter {
        DelegatedWorkerAdapter::new(
            ExecutionKernel::new(
                crate::agent::tool::ToolCancellation::new(),
                Duration::from_secs(1),
                2,
            ),
            ContractLimits::default(),
        )
    }
    #[tokio::test]
    async fn submits_valid_structured_delivery() {
        let mut host = StaticHost {
            delivery: Some(delivery()),
        };
        let mut sink = Sink {
            beats: 0,
            deliveries: 0,
        };
        let out = adapter()
            .run(
                &attempt(LeaseStatus::Active, now_ms() + 60_000),
                &brief(),
                &mut host,
                &mut sink,
                |_| {},
            )
            .await
            .unwrap();
        assert_eq!(out.delivery_id, "x");
        assert_eq!((sink.beats, sink.deliveries), (2, 1));
    }
    #[tokio::test]
    async fn rejects_invalid_delivery() {
        let mut bad = delivery();
        bad.executive_summary.clear();
        let mut host = StaticHost {
            delivery: Some(bad),
        };
        let mut sink = Sink {
            beats: 0,
            deliveries: 0,
        };
        let e = adapter()
            .run(
                &attempt(LeaseStatus::Active, now_ms() + 60_000),
                &brief(),
                &mut host,
                &mut sink,
                |_| {},
            )
            .await
            .unwrap_err();
        assert_eq!(e.kind, DelegatedWorkerFailureKind::DeliveryRejected);
        assert_eq!(sink.deliveries, 0);
    }
    #[tokio::test]
    async fn refuses_revoked_and_expired_leases() {
        for a in [
            attempt(LeaseStatus::Revoked, now_ms() + 60_000),
            attempt(LeaseStatus::Active, now_ms() - 1),
        ] {
            let mut host = StaticHost {
                delivery: Some(delivery()),
            };
            let mut sink = Sink {
                beats: 0,
                deliveries: 0,
            };
            assert!(matches!(
                adapter()
                    .run(&a, &brief(), &mut host, &mut sink, |_| {})
                    .await
                    .unwrap_err()
                    .kind,
                DelegatedWorkerFailureKind::LeaseInactive
                    | DelegatedWorkerFailureKind::LeaseExpired
            ));
        }
    }
    #[tokio::test]
    async fn maps_kernel_retry_to_one_successful_delivery() {
        let provider = Arc::new(FailsOnce {
            calls: AtomicUsize::new(0),
        });
        let mut host = RetryingHost {
            provider: provider.clone(),
            delivery: Some(delivery()),
        };
        let mut sink = Sink {
            beats: 0,
            deliveries: 0,
        };
        adapter()
            .run(
                &attempt(LeaseStatus::Active, now_ms() + 60_000),
                &brief(),
                &mut host,
                &mut sink,
                |_| {},
            )
            .await
            .unwrap();
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
        assert_eq!(sink.deliveries, 1);
    }
}

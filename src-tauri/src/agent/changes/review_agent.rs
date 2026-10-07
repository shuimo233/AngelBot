//! Execution adapter for the semantic Reviewer Agent.
//!
//! A reviewer is deliberately not a delegated producer: it receives one
//! bounded `ReviewSubject`, has no tools or filesystem/network capability,
//! and returns one bounded `ReviewOutcome`.  The existing DelegationPump
//! polls this adapter; it does not create a second scheduler loop.

use std::{collections::HashMap, sync::Arc};

use crate::llm::{LlmProvider, LlmResponse, Message};

use super::{
    delegated_model_binding::DelegatedModelBinding,
    delegated_model_binding::{DelegatedModelBindingRepository, DelegatedModelHostFactory},
    review_contract::{ReviewOutcome, ReviewSubject},
    review_store::ReviewJob,
    shared_db::SharedDb,
    work_package::WorkPackageScope,
};

/// Result emitted by an asynchronous reviewer task.  Errors are kept as
/// bounded diagnostics and are converted to `NeedsDecision` by the service.
#[derive(Debug, Clone)]
pub struct ReviewExecutionEvent {
    pub job_id: String,
    pub reviewer_attempt_id: String,
    pub outcome: Result<ReviewOutcome, String>,
}

pub trait ReviewExecutor: Send {
    fn enqueue(&mut self, job: ReviewJob) -> Result<(), String>;
    fn poll(&mut self) -> Result<Vec<ReviewExecutionEvent>, String>;
    fn stop_all(&mut self);
}

pub trait ReviewModelResolver: Send + Sync {
    fn resolve(&self, subject: &ReviewSubject) -> Result<Arc<dyn LlmProvider>, String>;
}

/// Resolve the immutable model binding belonging to the producer package.
/// The resolver reads only durable identity and binding rows; it never reads
/// a path, accepts model-supplied credentials, or falls back to the current
/// foreground model.
#[derive(Clone)]
pub struct BindingReviewModelResolver<F> {
    db: SharedDb,
    factory: F,
}

impl<F> BindingReviewModelResolver<F> {
    pub fn new(db: SharedDb, factory: F) -> Self {
        Self { db, factory }
    }
}

impl<F> ReviewModelResolver for BindingReviewModelResolver<F>
where
    F: DelegatedModelHostFactory + Clone + Send + Sync + 'static,
{
    fn resolve(&self, subject: &ReviewSubject) -> Result<Arc<dyn LlmProvider>, String> {
        let subject_attempt = subject.implementation_attempt_id.clone();
        let (scope, package_id, binding) = self
            .db
            .with_conn(|conn| {
                let (package_id, session_id, owner_profile_id, workspace_key): (
                    String,
                    String,
                    i64,
                    String,
                ) = conn
                    .query_row(
                        "SELECT p.id,p.session_id,p.owner_profile_id,p.workspace_key
                         FROM delegation_attempts a
                         JOIN delegations d ON d.id=a.delegation_id
                         JOIN work_packages p ON p.id=d.work_package_id
                         WHERE a.id=?1",
                        [&subject_attempt],
                        |row| {
                            Ok::<(String, String, i64, String), rusqlite::Error>((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                            ))
                        },
                    )
                    .map_err(|_| "review producer package not found".to_string())?;
                let scope = WorkPackageScope {
                    session_id,
                    owner_profile_id,
                    workspace_key,
                };
                let binding = DelegatedModelBindingRepository::load(conn, &scope, &package_id)
                    .map_err(|_| "review model binding unavailable".to_string())?;
                Ok::<(WorkPackageScope, String, DelegatedModelBinding), String>((
                    scope, package_id, binding,
                ))
            })
            .map_err(|_| "review model binding database unavailable".to_string())??;
        let _ = scope;
        let _ = package_id;
        self.factory
            .create_model_host(&binding)
            .map_err(|_| "review model host unavailable".to_string())
    }
}

struct ReviewTask {
    handle: tauri::async_runtime::JoinHandle<()>,
}

pub struct LlmReviewExecutor<R> {
    resolver: Arc<R>,
    sender: tokio::sync::mpsc::UnboundedSender<ReviewExecutionEvent>,
    receiver: tokio::sync::mpsc::UnboundedReceiver<ReviewExecutionEvent>,
    tasks: HashMap<String, ReviewTask>,
}

impl<R> LlmReviewExecutor<R>
where
    R: ReviewModelResolver + 'static,
{
    pub fn new(resolver: Arc<R>) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        Self {
            resolver,
            sender,
            receiver,
            tasks: HashMap::new(),
        }
    }
}

impl<R> ReviewExecutor for LlmReviewExecutor<R>
where
    R: ReviewModelResolver + 'static,
{
    fn enqueue(&mut self, job: ReviewJob) -> Result<(), String> {
        if self.tasks.contains_key(&job.id) {
            return Ok(());
        }
        let reviewer_attempt_id = job
            .reviewer_attempt_id
            .clone()
            .ok_or_else(|| "review job has no reviewer identity".to_string())?;
        let resolver = self.resolver.clone();
        let sender = self.sender.clone();
        let job_id = job.id.clone();
        let subject = job.subject.clone();
        let task_job_id = job_id.clone();
        let task_reviewer_id = reviewer_attempt_id.clone();
        let handle = tauri::async_runtime::spawn(async move {
            let outcome = run_review(resolver, subject).await;
            let _ = sender.send(ReviewExecutionEvent {
                job_id: task_job_id,
                reviewer_attempt_id: task_reviewer_id,
                outcome,
            });
        });
        self.tasks.insert(job_id, ReviewTask { handle });
        Ok(())
    }

    fn poll(&mut self) -> Result<Vec<ReviewExecutionEvent>, String> {
        let mut events = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            self.tasks.remove(&event.job_id);
            events.push(event);
        }
        Ok(events)
    }

    fn stop_all(&mut self) {
        for (_, task) in self.tasks.drain() {
            task.handle.abort();
        }
    }
}

async fn run_review<R>(resolver: Arc<R>, subject: ReviewSubject) -> Result<ReviewOutcome, String>
where
    R: ReviewModelResolver + 'static,
{
    subject
        .validate()
        .map_err(|_| "review subject validation failed".to_string())?;
    let provider = resolver.resolve(&subject)?;
    let subject_json = serde_json::to_string(&subject)
        .map_err(|_| "review subject serialization failed".to_string())?;
    let system = Message {
        tool_images: Vec::new(),
        protocol_state: None,
        role: "system".into(),
        content: "You are an independent review agent. Inspect only the bounded structured subject. Do not produce work, call tools, edit files, or approve materialization. Return exactly one JSON object matching ReviewOutcome with verdict passed, failed, or needs_decision. Keep findings and evidence_refs bounded; never include raw logs, secrets, paths, or hidden reasoning.".into(),
        tool_calls: None,
        tool_call_id: None,
    };
    let user = Message {
        tool_images: Vec::new(),
        protocol_state: None,
        role: "user".into(),
        content: subject_json,
        tool_calls: None,
        tool_call_id: None,
    };
    let response = provider
        .chat(&[system, user], None, None)
        .await
        .map_err(|_| "review provider request failed".to_string())?;
    let text = match response.into_parts().0 {
        LlmResponse::Text { text, .. } => text,
        LlmResponse::ToolCalls { .. } => return Err("reviewer attempted tool use".to_string()),
        LlmResponse::ProtocolViolation { .. } => {
            return Err("reviewer returned a protocol violation".to_string())
        }
        LlmResponse::WithProtocol { .. } => unreachable!("flattened protocol response"),
    };
    let outcome: ReviewOutcome = serde_json::from_str(text.trim())
        .map_err(|_| "reviewer output was not valid ReviewOutcome JSON".to_string())?;
    outcome
        .validate()
        .map_err(|_| "reviewer output failed ReviewOutcome validation".to_string())?;
    Ok(outcome)
}

/// A deterministic test adapter.  It is useful for service/Pump integration
/// tests without introducing another model or network dependency.
pub struct FixedReviewExecutor {
    outcome: ReviewOutcome,
    jobs: Vec<ReviewExecutionEvent>,
}

impl FixedReviewExecutor {
    pub fn new(outcome: ReviewOutcome) -> Self {
        Self {
            outcome,
            jobs: Vec::new(),
        }
    }
}

impl ReviewExecutor for FixedReviewExecutor {
    fn enqueue(&mut self, job: ReviewJob) -> Result<(), String> {
        let reviewer_attempt_id = job
            .reviewer_attempt_id
            .ok_or_else(|| "review job has no reviewer identity".to_string())?;
        self.jobs.push(ReviewExecutionEvent {
            job_id: job.id,
            reviewer_attempt_id,
            outcome: Ok(self.outcome.clone()),
        });
        Ok(())
    }

    fn poll(&mut self) -> Result<Vec<ReviewExecutionEvent>, String> {
        Ok(std::mem::take(&mut self.jobs))
    }

    fn stop_all(&mut self) {
        self.jobs.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::review_contract::{ReviewOutcome, SemanticReviewVerdict};

    #[test]
    fn fixed_executor_emits_one_bounded_event() {
        let outcome = ReviewOutcome {
            verdict: SemanticReviewVerdict::Passed,
            summary: "ok".into(),
            findings: vec![],
            missing_evidence: vec![],
            evidence_refs: vec!["evidence://review/1".into()],
        };
        let mut executor = FixedReviewExecutor::new(outcome.clone());
        let subject = ReviewSubject {
            delivery_id: "delivery".into(),
            implementation_attempt_id: "attempt".into(),
            task_shape: "explore".into(),
            worker_profile: "explorer".into(),
            goal: "goal".into(),
            summary: "summary".into(),
            key_facts: vec![],
            milestones: vec![],
            verifications: vec![],
            artifact_refs: vec![],
            risks: vec![],
            open_questions: vec![],
        };
        let job = ReviewJob {
            id: "review".into(),
            implementation_attempt_id: "attempt".into(),
            delivery_id: "delivery".into(),
            delivery_revision: 1,
            subject,
            status: super::super::review_store::ReviewJobStatus::Running,
            reviewer_attempt_id: Some("reviewer".into()),
            outcome: None,
            created_at: 1,
            updated_at: 1,
            started_at: Some(1),
            ended_at: None,
        };
        executor.enqueue(job).unwrap();
        let events = executor.poll().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].outcome.clone().unwrap(), outcome);
    }
}

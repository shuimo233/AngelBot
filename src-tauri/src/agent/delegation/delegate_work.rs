//! Foreground-only `delegate_work` tool adapter.
//!
//! All admission and cross-resource orchestration lives in
//! [`super::delegation_issuance::DelegationIssuance`]. This module intentionally
//! retains only the tool registry adapter and public identity alias.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    delegated_model_binding::DelegatedModelBindingRequest,
    delegation_issuance::{
        DelegationIssuance, DelegationIssuanceOutcome, DelegationRequest, RetryIssuanceOutcome,
        RetryReceiptState,
    },
    delegation_pump::DelegationPumpHostHandle,
    supervision::{
        SupervisorDelegationDispatcher, SupervisorWorkKind, WorkspaceSupervisor,
        WorkspaceSupervisorActor,
    },
    tool::{Tool, ToolExecutionContext, ToolHandler, ToolResult},
    worker_policy::{TaskShape, WorkerProfile},
};

pub use super::delegation_issuance::ForegroundRunIdentity;

pub type DelegateWorkInput = DelegationRequest;

#[derive(Clone)]
pub struct DelegateWorkHandler {
    issuer: DelegationIssuance,
    identity: ForegroundRunIdentity,
}

/// A separate static tool name lets the generic confirmation lifecycle force
/// an exact user decision even when broad execution permission is enabled.
#[derive(Clone)]
struct ConfirmedNetworkDelegateWorkHandler {
    delegate: DelegateWorkHandler,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetryDelegatedDeliveryInput {
    delivery_id: String,
    reason: String,
}

#[derive(Clone)]
struct RetryDelegatedDeliveryHandler {
    issuer: DelegationIssuance,
    identity: ForegroundRunIdentity,
    pump: DelegationPumpHostHandle,
}

fn supervisor_kind(
    task_shape: TaskShape,
    worker_profile: WorkerProfile,
) -> Option<SupervisorWorkKind> {
    match (task_shape, worker_profile) {
        (TaskShape::Explore, _) => Some(SupervisorWorkKind::Explore),
        (TaskShape::Change, WorkerProfile::Implementer) => {
            Some(SupervisorWorkKind::CandidateImplementation)
        }
        (TaskShape::Change, WorkerProfile::Verifier) => Some(SupervisorWorkKind::Review),
        _ => None,
    }
}

impl DelegateWorkHandler {
    pub fn new(
        db: Arc<Mutex<Connection>>,
        identity: ForegroundRunIdentity,
        sandbox_root: PathBuf,
        model_binding: DelegatedModelBindingRequest,
    ) -> Self {
        Self {
            issuer: DelegationIssuance::new(db, sandbox_root, model_binding),
            identity,
        }
    }
}

impl ToolHandler for DelegateWorkHandler {
    fn execute(&self, arguments: &Value, _: &std::path::Path) -> ToolResult {
        self.execute_with_call_id(arguments, "tool_call", "delegate_work", false)
    }

    fn execute_with_context(
        &self,
        arguments: &Value,
        _: &std::path::Path,
        context: &ToolExecutionContext,
    ) -> ToolResult {
        self.execute_with_call_id(arguments, &context.call_id, "delegate_work", false)
    }

    fn name(&self) -> &str {
        "delegate_work"
    }
}

impl DelegateWorkHandler {
    fn execute_with_call_id(
        &self,
        arguments: &Value,
        call_id: &str,
        tool_name: &str,
        confirmed_network_scope: bool,
    ) -> ToolResult {
        let input = match serde_json::from_value::<DelegationRequest>(arguments.clone()) {
            Ok(input) => input,
            Err(_) => return ToolResult::error(tool_name, "request rejected by delegation policy"),
        };
        let Some(kind) = supervisor_kind(input.task_shape, input.worker_profile) else {
            return ToolResult::error(tool_name, "request rejected by delegation policy");
        };
        let shared_db = crate::agent::shared_db::SharedDb::from_arc(self.issuer.db.clone());
        let supervisor = WorkspaceSupervisor::new(shared_db.clone());
        let workspace_id = match supervisor.workspace_for_session(&self.identity.session_id) {
            Ok(workspace_id) => workspace_id,
            Err(_) => return ToolResult::error(tool_name, "workspace supervision is unavailable"),
        };
        let now = chrono::Utc::now().timestamp();
        let work = match supervisor.queue_work(
            workspace_id.clone(),
            None,
            kind,
            input.goal.clone(),
            now,
        ) {
            Ok(work) => work,
            Err(_) => return ToolResult::error(tool_name, "workspace supervision is unavailable"),
        };
        let issuance = if confirmed_network_scope {
            self.issuer.issue_confirmed_network_outcome(
                &self.identity,
                call_id,
                input,
                Some(&work.id),
            )
        } else {
            self.issuer
                .issue_outcome(&self.identity, call_id, input, Some(&work.id))
        };
        match issuance {
            Ok(DelegationIssuanceOutcome::Queued(receipt)) => {
                // The actor owns the transition from admitted work to an
                // executable outbox. A failed attempt leaves the claim
                // queued for the normal supervisor pump; it never bypasses
                // the durable gate just because this foreground turn ends.
                let actor = WorkspaceSupervisorActor::new(
                    supervisor.clone(),
                    SupervisorDelegationDispatcher::new(shared_db),
                );
                let _ = actor.dispatch_next(&workspace_id, now);
                ToolResult::success(
                    tool_name,
                    json!({
                        "status": "queued",
                        "delegation_id": receipt.delegation_id,
                        "attempt_id": receipt.attempt_id,
                        "work_package_id": receipt.work_package_id,
                        "network_scope": if confirmed_network_scope { "approved_for_this_explorer_task" } else { "not_changed" },
                    })
                    .to_string(),
                )
            }
            Ok(DelegationIssuanceOutcome::Existing(receipt)) => {
                let _ = supervisor.cancel_queued_work(&work, now);
                ToolResult::success(
                    tool_name,
                    json!({
                        "status": "already_queued",
                        "delegation_id": receipt.delegation_id,
                        "attempt_id": receipt.attempt_id,
                        "work_package_id": receipt.work_package_id,
                        "network_scope": if confirmed_network_scope { "already_approved_for_this_explorer_task" } else { "not_changed" },
                    })
                    .to_string(),
                )
            }
            Err(error) => {
                let _ = supervisor.cancel_queued_work(&work, now);
                ToolResult::error(tool_name, delegation_error_message(error))
            }
        }
    }
}

impl ToolHandler for ConfirmedNetworkDelegateWorkHandler {
    fn execute(&self, _: &Value, _: &std::path::Path) -> ToolResult {
        ToolResult::error(
            "delegate_network_exploration",
            "delegated network exploration requires an exact user confirmation",
        )
    }

    fn execute_with_context(
        &self,
        arguments: &Value,
        _: &std::path::Path,
        context: &ToolExecutionContext,
    ) -> ToolResult {
        if !context.is_confirmation_approval() {
            return self.execute(arguments, std::path::Path::new("."));
        }
        let mut normalized = arguments.clone();
        let Some(object) = normalized.as_object_mut() else {
            return ToolResult::error(
                "delegate_network_exploration",
                "request rejected by delegation policy",
            );
        };
        // Discard legacy/model-supplied approval markers. Authorization comes
        // only from ToolExecutionContext, never from model arguments.
        object.remove("_confirmed");
        object.remove("confirmed");
        object.insert("task_shape".into(), Value::String("explore".into()));
        object.insert("worker_profile".into(), Value::String("explorer".into()));
        self.delegate.execute_with_call_id(
            &normalized,
            &context.call_id,
            "delegate_network_exploration",
            true,
        )
    }

    fn name(&self) -> &str {
        "delegate_network_exploration"
    }
}

impl ToolHandler for RetryDelegatedDeliveryHandler {
    fn execute(&self, arguments: &Value, _: &std::path::Path) -> ToolResult {
        self.execute_retry(arguments)
    }

    fn execute_with_context(
        &self,
        arguments: &Value,
        _: &std::path::Path,
        _: &ToolExecutionContext,
    ) -> ToolResult {
        self.execute_retry(arguments)
    }

    fn name(&self) -> &str {
        "retry_delegated_delivery"
    }
}

impl RetryDelegatedDeliveryHandler {
    fn execute_retry(&self, arguments: &Value) -> ToolResult {
        let input = match serde_json::from_value::<RetryDelegatedDeliveryInput>(arguments.clone()) {
            Ok(input) => input,
            Err(_) => {
                return ToolResult::error(
                    "retry_delegated_delivery",
                    "retry request rejected by delegation policy",
                )
            }
        };
        match self
            .issuer
            .retry_receipt_state(&self.identity, &input.delivery_id)
        {
            Ok(RetryReceiptState::Active(receipt)) => {
                return ToolResult::success(
                    "retry_delegated_delivery",
                    json!({
                        "status": "already_queued",
                        "delegation_id": receipt.delegation_id,
                        "attempt_id": receipt.attempt_id,
                        "work_package_id": receipt.work_package_id,
                    })
                    .to_string(),
                )
            }
            Ok(RetryReceiptState::None) => {}
            Ok(RetryReceiptState::Terminal(_)) => {
                return ToolResult::error(
                    "retry_delegated_delivery",
                    delegation_error_message(
                        super::delegation_issuance::DelegationIssuanceError::RetryAlreadyTerminal,
                    ),
                )
            }
            Err(error) => {
                return ToolResult::error(
                    "retry_delegated_delivery",
                    delegation_error_message(error),
                )
            }
        }
        // Verify the durable supervisor workspace before recording a retry
        // request. This removes the common half-transition where a retry was
        // authorized but could never be represented in the user's activity
        // stream because the workspace was unavailable.
        let shared_db = crate::agent::shared_db::SharedDb::from_arc(self.issuer.db.clone());
        let supervisor = WorkspaceSupervisor::new(shared_db.clone());
        let workspace_id = match supervisor.workspace_for_session(&self.identity.session_id) {
            Ok(workspace_id) => workspace_id,
            Err(_) => {
                return ToolResult::error(
                    "retry_delegated_delivery",
                    "workspace supervision is unavailable",
                )
            }
        };
        let continuation = match self.pump.retry_for_session(
            &self.identity.session_id,
            &input.delivery_id,
            &input.reason,
        ) {
            Ok(continuation) => continuation,
            Err(_) => {
                return ToolResult::error("retry_delegated_delivery", "retry state is unavailable")
            }
        };
        let (task_shape, worker_profile, objective) = match self
            .issuer
            .retry_worker_route(&self.identity, &continuation)
        {
            Ok(shape) => shape,
            Err(error) => {
                return ToolResult::error(
                    "retry_delegated_delivery",
                    delegation_error_message(error),
                )
            }
        };
        let Some(kind) = supervisor_kind(task_shape, worker_profile) else {
            return ToolResult::error(
                "retry_delegated_delivery",
                "retry request rejected by delegation policy",
            );
        };
        let now = chrono::Utc::now().timestamp();
        let work = match supervisor.queue_work(workspace_id.clone(), None, kind, objective, now) {
            Ok(work) => work,
            Err(_) => {
                return ToolResult::error(
                    "retry_delegated_delivery",
                    "workspace supervision is unavailable",
                )
            }
        };
        match self
            .issuer
            .retry(&self.identity, &continuation, Some(&work.id))
        {
            Ok(RetryIssuanceOutcome::Queued(receipt)) => {
                let actor = WorkspaceSupervisorActor::new(
                    supervisor.clone(),
                    SupervisorDelegationDispatcher::new(shared_db),
                );
                let _ = actor.dispatch_next(&workspace_id, now);
                ToolResult::success(
                    "retry_delegated_delivery",
                    json!({
                        "status": "queued",
                        "delegation_id": receipt.delegation_id,
                        "attempt_id": receipt.attempt_id,
                        "work_package_id": receipt.work_package_id,
                    })
                    .to_string(),
                )
            }
            Ok(RetryIssuanceOutcome::Existing(receipt)) => {
                let _ = supervisor.cancel_queued_work(&work, now);
                ToolResult::success(
                    "retry_delegated_delivery",
                    json!({
                        "status": "already_queued",
                        "delegation_id": receipt.delegation_id,
                        "attempt_id": receipt.attempt_id,
                        "work_package_id": receipt.work_package_id,
                    })
                    .to_string(),
                )
            }
            Err(error) => {
                let _ = supervisor.cancel_queued_work(&work, now);
                ToolResult::error("retry_delegated_delivery", delegation_error_message(error))
            }
        }
    }
}

/// Return a stable, non-sensitive failure category to the foreground model.
/// It is detailed enough for safe recovery (for example, choosing a Git
/// workspace for local exploration) but never exposes database, filesystem,
/// provider, credential, or policy internals.
fn delegation_error_message(
    error: super::delegation_issuance::DelegationIssuanceError,
) -> &'static str {
    use super::delegation_issuance::DelegationIssuanceError as Error;

    match error {
        Error::InvalidRequest => "delegation request is outside the supported worker contract",
        Error::CapabilityScope => "requested network intent has no approved source policy",
        Error::AdmissionDenied => "delegated capacity is currently unavailable",
        Error::IdempotencyConflict => "this delegated call conflicts with an existing request",
        Error::Sandbox => "delegated sandbox is unavailable",
        Error::WorkspaceUnavailable => "local delegated work requires an available Git worktree",
        Error::Storage => "delegation state storage is unavailable",
        Error::Policy => "delegation policy rejected the requested capability",
        Error::RetryAlreadyTerminal => {
            "the prior retry is already terminal; create a fresh delegated task after review"
        }
    }
}

pub fn register(
    registry: &mut super::ToolRegistry,
    db: Arc<Mutex<Connection>>,
    identity: ForegroundRunIdentity,
    sandbox_root: PathBuf,
    model_binding: DelegatedModelBindingRequest,
    delegation_pump: Option<DelegationPumpHostHandle>,
) {
    let tool = Tool::new(
        "delegate_work",
        "Internally queue one bounded delegated work package. Use only for independently deliverable work; never include paths, hosts, credentials, or permissions.",
        json!({"type":"object","additionalProperties":false,"required":["goal","task_shape","worker_profile"],"properties":{"goal":{"type":"string","minLength":1,"maxLength":600},"background_refs":{"type":"array","maxItems":12,"items":{"type":"string","maxLength":256}},"task_shape":{"type":"string","enum":["explore","change"]},"worker_profile":{"type":"string","enum":["explorer","implementer","verifier"]},"explorer_operations":{"type":"array","maxItems":12,"items":{"type":"object","additionalProperties":false,"properties":{"kind":{"type":"string","enum":["search","fetch"]},"provider_host":{"type":"string","maxLength":253},"query":{"type":"string","maxLength":2048},"url":{"type":"string","maxLength":4096},"method":{"type":"string","enum":["GET","HEAD","get","head"]}},"required":["kind"]}}}}),
    )
    .with_label("内部委派")
    .with_prompt_snippet(
        "对于可独立交付的分析、实现或审校工作，直接使用 delegate_work 进行内部委派；这项能力已在当前会话预授权，不要向用户索要“委派策略”或额外授权。纯本地只读探索使用 task_shape=explore、worker_profile=explorer，且不要填写 explorer_operations。需要联网的 Explorer 也先调用 delegate_work：若返回“requested network intent has no approved source policy”，才使用 delegate_network_exploration，由系统向用户展示精确站点和上限后再派发。这样已确认的项目范围会自动复用，不会反复请求用户确认。子代理对用户不可见，收到结果后由你审校并向用户汇总。",
    );
    let delegate = DelegateWorkHandler::new(
        db.clone(),
        identity.clone(),
        sandbox_root.clone(),
        model_binding.clone(),
    );
    registry.register(tool, Arc::new(delegate.clone()));
    let network_tool = Tool::new(
        "delegate_network_exploration",
        "Ask for one exact user confirmation, then queue one bounded Explorer task that may search or fetch only the stated public hosts. Never include credentials, headers, local paths, or broad network permissions.",
        json!({"type":"object","additionalProperties":false,"required":["goal","explorer_operations"],"properties":{"goal":{"type":"string","minLength":1,"maxLength":600},"background_refs":{"type":"array","maxItems":12,"items":{"type":"string","maxLength":256}},"explorer_operations":{"type":"array","minItems":1,"maxItems":12,"items":{"type":"object","additionalProperties":false,"properties":{"kind":{"type":"string","enum":["search","fetch"]},"provider_host":{"type":"string","maxLength":253},"query":{"type":"string","maxLength":2048},"url":{"type":"string","maxLength":4096},"method":{"type":"string","enum":["GET","HEAD","get","head"]}},"required":["kind"]}}}}),
    )
    .with_confirmation(true)
    .with_label("联网委派")
    .with_prompt_snippet(
        "当独立 Explorer 工作需要访问尚未为当前项目明确确认的公开站点时，使用 delegate_network_exploration。它会向用户展示本次所需站点、搜索/抓取类型以及固定响应/跳转上限；确认后才创建受限范围并入队。不要把查询内容、URL 参数、凭据、路径或权限细节解释给用户，也不要使用它处理本地工作。",
    );
    registry.register(
        network_tool,
        Arc::new(ConfirmedNetworkDelegateWorkHandler { delegate }),
    );
    let Some(delegation_pump) = delegation_pump else {
        return;
    };
    let retry_tool = Tool::new(
        "retry_delegated_delivery",
        "Internally retry one prior delegated delivery after Main-Agent review. It reuses the immutable task boundary, creates a fresh isolated attempt, and never replays completed side effects.",
        json!({"type":"object","additionalProperties":false,"required":["delivery_id","reason"],"properties":{"delivery_id":{"type":"string","minLength":1,"maxLength":128},"reason":{"type":"string","minLength":1,"maxLength":600}}}),
    )
    .with_label("内部重试")
    .with_prompt_snippet(
        "当某个已交付的内部委派需要以相同任务边界重新尝试时，使用 retry_delegated_delivery。它只能接收该交付的 delivery_id 和简短原因；不能改变路径、权限、模型、任务类型、网络来源或目标。若边界需要改变，应创建新的 delegate_work。不要向用户暴露子代理控制操作。",
    );
    registry.register(
        retry_tool,
        Arc::new(RetryDelegatedDeliveryHandler {
            issuer: DelegationIssuance::new(db, sandbox_root, model_binding),
            identity,
            pump: delegation_pump,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{
        delegated_network_scope::{ConfirmedNetworkScope, NetworkScopeApprovalProvenance},
        delegation_pump::{DelegationPumpError, DelegationPumpHandle, DelegationPumpLoop},
        delegation_service::ServiceReport,
        delivery_inbox::RetryContinuation,
        worker_policy::NetworkAction,
        ToolCall, ToolCancellation, ToolExecutionContext, ToolRegistry,
    };
    use rusqlite::params;
    use std::collections::BTreeSet;

    fn setup(
        id: &str,
    ) -> (
        Arc<Mutex<Connection>>,
        ForegroundRunIdentity,
        tempfile::TempDir,
    ) {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        let workspace = tempfile::tempdir().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO profile (id,updated_at) VALUES (1,1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id,title,work_dir,created_at,updated_at) VALUES (?1,'x',?2,1,1)",
            params![id, workspace.path().to_string_lossy().to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO projects (id,name,path,created_at,active_session_id)
             VALUES (?1,'test-workspace',?2,1,?1)",
            params![id, workspace.path().to_string_lossy().to_string()],
        )
        .unwrap();
        let message = format!("m_{id}");
        conn.execute("INSERT INTO messages (id,session_id,role,content,is_provisional,created_at) VALUES (?1,?2,'assistant','',1,1)", params![message,id]).unwrap();
        conn.execute("INSERT INTO task_runs (id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES (?1,?2,?1,'x','running','[]',1,1)", params![message,id]).unwrap();
        let identity =
            ForegroundRunIdentity::prepare(&conn, id, &message, workspace.path().to_path_buf())
                .unwrap();
        let scope = ConfirmedNetworkScope::new(
            NetworkScopeApprovalProvenance {
                owner_profile_id: 1,
                workspace_key: identity.workspace_key.clone(),
                session_id: identity.session_id.clone(),
                message_id: identity.message_id.clone(),
                parent_run_id: identity.parent_run_id.clone(),
                tool_call_id: "seed_policy".into(),
            },
            "approved_docs".into(),
            "sha256:seed_policy".into(),
            BTreeSet::from([NetworkAction::Search, NetworkAction::Fetch]),
            BTreeSet::from(["docs.example.com".into()]),
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        scope.persist_in_tx(&tx, 1).unwrap();
        tx.commit().unwrap();
        (Arc::new(Mutex::new(conn)), identity, workspace)
    }
    fn input() -> Value {
        explorer_input()
    }
    fn explorer_input() -> Value {
        json!({"goal":"inspect docs","background_refs":["evidence://brief"],"task_shape":"explore","worker_profile":"explorer","explorer_operations":[{"kind":"search","provider_host":"docs.example.com","query":"bounded"}]})
    }
    fn model_binding() -> DelegatedModelBindingRequest {
        DelegatedModelBindingRequest::new("provider:default", "model:test", "keychain:default", 1)
            .unwrap()
    }

    struct RetryPumpProbe {
        calls: Arc<Mutex<usize>>,
    }

    impl DelegationPumpLoop for RetryPumpProbe {
        fn start(&mut self) -> Result<ServiceReport, DelegationPumpError> {
            Ok(ServiceReport::default())
        }

        fn tick(&mut self) -> Result<ServiceReport, DelegationPumpError> {
            Ok(ServiceReport::default())
        }

        fn shutdown(&mut self) -> Result<(), DelegationPumpError> {
            Ok(())
        }

        fn retry_for_session(
            &mut self,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<RetryContinuation, DelegationPumpError> {
            *self.calls.lock().unwrap() += 1;
            Err(DelegationPumpError::Worker("probe".into()))
        }
    }

    fn retry_pump(calls: Arc<Mutex<usize>>) -> DelegationPumpHostHandle {
        DelegationPumpHostHandle::new(DelegationPumpHandle::new(RetryPumpProbe { calls }))
    }

    #[test]
    fn registry_issues_work_package_and_outbox() {
        let (db, identity, workspace) = setup("s1");
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            db.clone(),
            identity.clone(),
            workspace.path().join("sandboxes"),
            model_binding(),
            None,
        );
        assert!(registry
            .get_prompt_snippets()
            .iter()
            .any(|(name, snippet)| {
                name == "delegate_work" && snippet.contains("已在当前会话预授权")
            }));
        let out = registry.execute(
            &ToolCall::new("delegate_work".into(), input()),
            "c1",
            workspace.path(),
        );
        assert!(out.result.success, "{}", out.result.content);
        let conn = db.lock().unwrap();
        assert_eq!(
            conn.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM workspace_supervisor_delegations WHERE authorized_at IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap(),
            1,
            "delegate_work must cross the supervisor authorization gate before runtime dispatch"
        );
        let (session,message,parent,work_package,status):(String,String,String,String,String)=conn.query_row("SELECT session_id,message_id,parent_run_id,work_package_id,status FROM delegations",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap();
        assert_eq!(
            (session, message, parent, status),
            (
                identity.session_id.clone(),
                identity.message_id.clone(),
                identity.parent_run_id.clone(),
                "queued".into()
            )
        );
        assert_eq!(
            conn.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM work_packages WHERE id=?1",
                [&work_package],
                |r| r.get(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM delegation_outbox", [], |r| r.get(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM delegated_model_bindings WHERE work_package_id=?1",
                [&work_package],
                |r| r.get(0)
            )
            .unwrap(),
            1
        );
        let writes:i64=conn.query_row("SELECT COUNT(*) FROM delegation_capability_leases l JOIN delegation_attempts a ON a.id=l.attempt_id JOIN delegations d ON d.id=a.delegation_id WHERE d.message_id=?1 AND l.write_roots_json != '[]'",[identity.message_id.as_str()],|r|r.get(0)).unwrap();
        assert_eq!(writes, 0, "explore is a hard no-write ceiling");
    }

    #[test]
    fn duplicate_foreground_call_cancels_the_unused_supervisor_work() {
        let (db, identity, workspace) = setup("duplicate-delegate");
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            db.clone(),
            identity,
            workspace.path().join("sandboxes"),
            model_binding(),
            None,
        );

        let first = registry.execute(
            &ToolCall::new("delegate_work".into(), input()),
            "same-call",
            workspace.path(),
        );
        assert!(first.result.success, "{}", first.result.content);
        let repeated = registry.execute(
            &ToolCall::new("delegate_work".into(), input()),
            "same-call",
            workspace.path(),
        );
        assert!(repeated.result.success, "{}", repeated.result.content);
        assert!(repeated.result.content.contains("already_queued"));

        let conn = db.lock().unwrap();
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM delegations", [], |row| row.get(0))
                .unwrap(),
            1,
            "an idempotent tool replay must not issue another delegation"
        );
        assert_eq!(
            conn.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM workspace_supervisor_delegations",
                [],
                |row| row.get(0),
            )
            .unwrap(),
            1,
            "only the admitted work item may be linked to the delegation"
        );
        assert_eq!(
            conn.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM workspace_supervisor_work WHERE status = 'cancelled'",
                [],
                |row| row.get(0),
            )
            .unwrap(),
            1,
            "the newly queued duplicate work must be cancelled before dispatch"
        );
    }

    #[test]
    fn reused_network_scope_leases_only_the_hosts_in_this_explorer_plan() {
        let (db, identity, workspace) = setup("network-least-privilege");
        {
            let mut conn = db.lock().unwrap();
            let scope = ConfirmedNetworkScope::new(
                NetworkScopeApprovalProvenance {
                    owner_profile_id: identity.owner_profile_id,
                    workspace_key: identity.workspace_key.clone(),
                    session_id: identity.session_id.clone(),
                    message_id: identity.message_id.clone(),
                    parent_run_id: identity.parent_run_id.clone(),
                    tool_call_id: "multiple-approved-hosts".into(),
                },
                "approved_multiple_hosts".into(),
                "sha256:multiple-approved-hosts".into(),
                BTreeSet::from([NetworkAction::Search]),
                BTreeSet::from([
                    "docs.new-example.com".into(),
                    "assets.new-example.com".into(),
                ]),
            )
            .unwrap();
            let tx = conn.transaction().unwrap();
            scope.persist_in_tx(&tx, 1).unwrap();
            tx.commit().unwrap();
        }

        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            db.clone(),
            identity,
            workspace.path().join("sandboxes"),
            model_binding(),
            None,
        );
        let delegated = registry.execute(
            &ToolCall::new(
                "delegate_work".into(),
                json!({
                    "goal": "inspect the documentation index",
                    "background_refs": ["evidence://brief"],
                    "task_shape": "explore",
                    "worker_profile": "explorer",
                    "explorer_operations": [{
                        "kind": "search",
                        "provider_host": "docs.new-example.com",
                        "query": "index"
                    }]
                }),
            ),
            "least-privilege-call",
            workspace.path(),
        );
        assert!(delegated.result.success, "{}", delegated.result.content);

        let conn = db.lock().unwrap();
        let hosts: String = conn
            .query_row(
                "SELECT network_hosts_json FROM delegation_capability_leases",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(hosts.contains("docs.new-example.com"));
        assert!(
            !hosts.contains("assets.new-example.com"),
            "the reusable source approval is a ceiling, not this attempt's lease"
        );
    }

    #[test]
    fn network_delegate_requires_confirmation_then_persists_exact_scope_with_queue() {
        let (db, identity, workspace) = setup("network-confirm");
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            db.clone(),
            identity,
            workspace.path().join("sandboxes"),
            model_binding(),
            None,
        );
        let request = json!({
            "goal": "inspect public documentation",
            "explorer_operations": [{
                "kind": "search",
                "provider_host": "new-docs.example.com",
                "query": "private task details must not become policy"
            }]
        });
        let direct = registry.execute(
            &ToolCall::new("delegate_network_exploration".into(), request.clone()),
            "network-confirm-call",
            workspace.path(),
        );
        assert!(!direct.result.success);
        {
            let conn = db.lock().unwrap();
            assert_eq!(
                conn.query_row::<i64, _, _>(
                    "SELECT COUNT(*) FROM delegated_network_scope_approvals",
                    [],
                    |row| row.get(0),
                )
                .unwrap(),
                1
            );
            assert_eq!(
                conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM delegations", [], |row| {
                    row.get(0)
                })
                .unwrap(),
                0
            );
        }

        let context = ToolExecutionContext::new("network-confirm-call", ToolCancellation::new())
            .for_confirmation(None);
        let confirmed = registry.execute_with_context(
            &ToolCall::new("delegate_network_exploration".into(), request),
            "network-confirm-call",
            workspace.path(),
            &context,
        );
        assert!(confirmed.result.success, "{}", confirmed.result.content);
        assert!(confirmed
            .result
            .content
            .contains("approved_for_this_explorer_task"));
        let conn = db.lock().unwrap();
        assert_eq!(
            conn.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM delegated_network_scope_approvals",
                [],
                |row| row.get(0),
            )
            .unwrap(),
            2
        );
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM delegations", [], |row| {
                row.get(0)
            })
            .unwrap(),
            1
        );
        let allowed_hosts: String = conn
            .query_row(
                "SELECT allowed_hosts_json FROM delegated_network_source_policies
                 WHERE capability_scope_ref LIKE 'network_scope_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(allowed_hosts.contains("new-docs.example.com"));
        assert!(!allowed_hosts.contains("private task details"));
    }

    #[test]
    fn registry_rejects_policy_injection_and_leaves_no_orphan() {
        let (db, identity, workspace) = setup("s2");
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            db.clone(),
            identity,
            workspace.path().join("sandboxes"),
            model_binding(),
            None,
        );
        let mut injected = input();
        injected["network_hosts"] = json!(["evil.example"]);
        let out = registry.execute(
            &ToolCall::new("delegate_work".into(), injected),
            "c2",
            workspace.path(),
        );
        assert!(!out.result.success);
        let conn = db.lock().unwrap();
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM delegations", [], |r| r.get(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn retry_tool_requires_a_pump_and_rejects_untrusted_boundary_input_before_side_effects() {
        let (db, identity, workspace) = setup("retry-tool-registration");
        let sandbox_root = workspace.path().join("sandboxes");

        let mut without_pump = ToolRegistry::new();
        register(
            &mut without_pump,
            db.clone(),
            identity.clone(),
            sandbox_root.clone(),
            model_binding(),
            None,
        );
        assert!(!without_pump.contains("retry_delegated_delivery"));

        let calls = Arc::new(Mutex::new(0));
        let mut with_pump = ToolRegistry::new();
        register(
            &mut with_pump,
            db.clone(),
            identity,
            sandbox_root,
            model_binding(),
            Some(retry_pump(calls.clone())),
        );
        assert!(with_pump.contains("retry_delegated_delivery"));

        let result = with_pump.execute(
            &ToolCall::new(
                "retry_delegated_delivery".into(),
                json!({
                    "delivery_id": "delivery-1",
                    "reason": "retry after review",
                    "network_hosts": ["evil.example"]
                }),
            ),
            "retry-malformed",
            workspace.path(),
        );
        assert!(!result.result.success);
        assert!(result.result.content.contains("工具参数不合法，未执行"));
        assert_eq!(
            *calls.lock().unwrap(),
            0,
            "untrusted fields must not reach the pump"
        );

        let conn = db.lock().unwrap();
        for table in [
            "delegations",
            "delegation_attempts",
            "workspace_supervisor_work",
        ] {
            let count = conn
                .query_row::<i64, _, _>(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "malformed retry must not write {table}");
        }
    }

    #[test]
    fn explorer_issuance_persists_typed_plan_and_ignores_outbox_payload() {
        let (db, identity, workspace) = setup("explorer-plan");
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            db.clone(),
            identity,
            workspace.path().join("sandboxes"),
            model_binding(),
            None,
        );
        let out = registry.execute(
            &ToolCall::new("delegate_work".into(), explorer_input()),
            "explorer-call",
            workspace.path(),
        );
        assert!(out.result.success, "{}", out.result.content);
        let conn = db.lock().unwrap();
        let (plan_json, payload): (String, String) = conn.query_row(
            "SELECT ep.plan_json, o.payload_json FROM delegation_explorer_plans ep JOIN delegation_attempts a ON a.delegation_id = ep.delegation_id JOIN delegation_outbox o ON o.attempt_id = a.id",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert!(plan_json.contains("docs.example.com"));
        assert!(
            !payload.contains("docs.example.com"),
            "outbox remains an opaque envelope"
        );
        assert_eq!(
            conn.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM delegation_explorer_plans",
                [],
                |row| row.get(0)
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn explorer_plan_is_required_only_for_explorer_and_repeated_call_is_idempotent() {
        let (db, identity, workspace) = setup("explorer-idempotent");
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            db.clone(),
            identity,
            workspace.path().join("sandboxes"),
            model_binding(),
            None,
        );
        let first = registry.execute(
            &ToolCall::new("delegate_work".into(), explorer_input()),
            "same-call",
            workspace.path(),
        );
        let second = registry.execute(
            &ToolCall::new("delegate_work".into(), explorer_input()),
            "same-call",
            workspace.path(),
        );
        assert!(first.result.success, "{}", first.result.content);
        assert!(second.result.success, "{}", second.result.content);
        let conn = db.lock().unwrap();
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM delegations", [], |row| row.get(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM delegation_explorer_plans",
                [],
                |row| row.get(0)
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn local_explorer_uses_a_temporary_git_worktree_and_change_with_operations_fails_closed() {
        let (db, identity, workspace) = setup("explorer-plan-invalid");
        let sandbox_root = tempfile::tempdir().unwrap();
        for args in [
            vec!["init"],
            vec!["config", "user.email", "angelbot-test@example.invalid"],
            vec!["config", "user.name", "AngelBot Test"],
            vec!["commit", "--allow-empty", "-m", "initial"],
        ] {
            assert!(std::process::Command::new("git")
                .args(args)
                .current_dir(workspace.path())
                .status()
                .unwrap()
                .success());
        }
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            db.clone(),
            identity,
            sandbox_root.path().join("sandboxes"),
            model_binding(),
            None,
        );
        let mut local = explorer_input();
        local["explorer_operations"] = json!([]);
        let local_result = registry.execute(
            &ToolCall::new("delegate_work".into(), local),
            "local",
            workspace.path(),
        );
        assert!(
            local_result.result.success,
            "{}",
            local_result.result.content
        );
        let mut wrong_shape = explorer_input();
        wrong_shape["task_shape"] = json!("change");
        wrong_shape["worker_profile"] = json!("implementer");
        assert!(
            !registry
                .execute(
                    &ToolCall::new("delegate_work".into(), wrong_shape),
                    "wrong-shape",
                    workspace.path()
                )
                .result
                .success
        );
        let conn = db.lock().unwrap();
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM delegations", [], |row| row.get(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM delegation_attempts WHERE status = 'queued'",
                [],
                |row| row.get(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM delegation_capability_leases WHERE status = 'active'",
                [],
                |row| row.get(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM delegation_outbox WHERE dispatched_at IS NULL",
                [],
                |row| row.get(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM delegation_explorer_plans",
                [],
                |row| row.get(0)
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn global_two_attempt_limit_rejects_third_queue() {
        let (db, identity, workspace) = setup("s3");
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            db.clone(),
            identity,
            workspace.path().join("sandboxes"),
            model_binding(),
            None,
        );
        assert!(
            registry
                .execute(
                    &ToolCall::new("delegate_work".into(), input()),
                    "c1",
                    workspace.path()
                )
                .result
                .success
        );
        for n in 4..=5 {
            let id = format!("s{n}");
            let message = format!("m_{id}");
            let conn = db.lock().unwrap();
            conn.execute("INSERT INTO sessions (id,title,work_dir,created_at,updated_at) VALUES (?1,'x',?2,1,1)",params![id,workspace.path().to_string_lossy().to_string()]).unwrap();
            conn.execute(
                "INSERT INTO projects (id,name,path,created_at,active_session_id)
                 VALUES (?1,'test-workspace',?2,1,?1)",
                params![
                    id,
                    format!("{}-workspace-{id}", workspace.path().to_string_lossy())
                ],
            )
            .unwrap();
            conn.execute("INSERT INTO messages (id,session_id,role,content,is_provisional,created_at) VALUES (?1,?2,'assistant','',1,1)",params![message,id]).unwrap();
            conn.execute("INSERT INTO task_runs (id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES (?1,?2,?1,'x','running','[]',1,1)",params![message,id]).unwrap();
            let next = ForegroundRunIdentity::prepare(
                &conn,
                &id,
                &message,
                workspace.path().to_path_buf(),
            )
            .unwrap();
            drop(conn);
            let mut next_registry = ToolRegistry::new();
            register(
                &mut next_registry,
                db.clone(),
                next,
                workspace.path().join("sandboxes"),
                model_binding(),
                None,
            );
            let result = next_registry.execute(
                &ToolCall::new("delegate_work".into(), input()),
                &format!("c{n}"),
                workspace.path(),
            );
            if n == 4 {
                assert!(result.result.success);
            } else {
                assert!(!result.result.success);
            }
        }
    }
}

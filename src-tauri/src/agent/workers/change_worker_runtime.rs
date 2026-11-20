//! Production change-worker adapter.
//!
//! This is the only bridge that turns a durable Change dispatch into a model
//! execution.  It resolves all state from the dispatch identities and the
//! immutable work-package rows; it never reads `payload_json` as instructions
//! and never hands a path, connection, credential, or confirmation port to a
//! worker.  Implementer and Verifier share the same file host, but receive
//! different immutable WorktreeCapabilities.

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
        Arc,
    },
    time::Duration,
};

use rusqlite::OptionalExtension;

use super::{
    codex_cli,
    delegated_model_binding::{
        DelegatedModelBinding, DelegatedModelBindingRepository, DelegatedModelHostFactory,
    },
    delegated_model_execution_host::DelegatedModelExecutionHost,
    delegated_worker_adapter::{DelegatedAttempt, DelegatedDeliverySink, DelegatedWorkerAdapter},
    delegated_worker_launcher::{
        AsyncWorkerLauncher, WorkerLaunchError, WorkerLaunchRequest,
        WorkerTask as PollableWorkerTask, WorkerTaskEvent,
    },
    delegation::LeaseStatus,
    delegation_contract::{
        validate_delivery, CandidateArtifact, Confidence, ContractLimits, DelegationBrief,
        DelegationDelivery, DeliveryStatus, KeyFact, UsageFlags, VerificationRecord,
        VerificationStatus,
    },
    delegation_runtime::{Heartbeat, StopReason, WorkerDispatch},
    execution_gateway::{CapabilityGrant, CapabilityLease},
    execution_kernel::ExecutionKernel,
    implementer_worker_host::{ImplementerAccess, ImplementerAccessResolver},
    resource_binding::{ResourceBindingRepository, ResourceKind},
    shared_db::SharedDb,
    tool::ToolCancellation,
    work_package::WorkPackageScope,
    worker_host_contract::{WorkerHostBinding, WorkerTask},
    worker_policy::{TaskShape, WorkerProfile},
    workspace_isolation::{
        GitWorktreeProvider, WorktreeCapability, WorktreeManifestIdentity, WorktreeOperation,
    },
};

const JOIN_TIMEOUT: Duration = Duration::from_secs(2);
const MODEL_TIMEOUT: Duration = Duration::from_secs(300);

/// Resolves the opaque worktree binding into a provider-owned capability. No
/// path crosses this resolver; `GitWorktreeProvider` validates the manifest
/// and resolves the actual checkout internally.
#[derive(Clone)]
pub struct DurableWorktreeAccessResolver {
    db: SharedDb,
    repository: ResourceBindingRepository,
}

impl DurableWorktreeAccessResolver {
    pub fn new(db: SharedDb) -> Self {
        Self {
            repository: ResourceBindingRepository::new(db.clone()),
            db,
        }
    }
}

impl ImplementerAccessResolver for DurableWorktreeAccessResolver {
    fn resolve(
        &self,
        binding: &WorkerHostBinding,
    ) -> Result<ImplementerAccess, super::worker_host_contract::WorkerHostError> {
        let attempt_id = binding.identity().attempt_id().to_string();
        let record = self
            .repository
            .load_for_attempt(&attempt_id, ResourceKind::Worktree)
            .map_err(|_| super::worker_host_contract::WorkerHostError::CapabilityDenied)?;
        let identity = WorktreeManifestIdentity::from_binding(&record)
            .map_err(|_| super::worker_host_contract::WorkerHostError::CapabilityDenied)?;
        if identity.attempt_id != attempt_id
            || identity.work_package_id != binding.identity().work_package_id()
            || identity.lease_epoch != binding.lease_epoch()
        {
            return Err(super::worker_host_contract::WorkerHostError::StaleEvent);
        }
        let collaboration_key: String = self
            .db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT workspace_key FROM work_packages WHERE id=?1",
                    [binding.identity().work_package_id()],
                    |row| row.get(0),
                )
            })
            .map_err(|_| super::worker_host_contract::WorkerHostError::CapabilityDenied)?
            .map_err(|_| super::worker_host_contract::WorkerHostError::CapabilityDenied)?;
        let operation = match binding.worker_profile() {
            WorkerProfile::Implementer => WorktreeOperation::Write,
            WorkerProfile::Verifier | WorkerProfile::Explorer => WorktreeOperation::Read,
        };
        let capability = WorktreeCapability {
            read_roots: vec![".".into()],
            write_roots: if operation == WorktreeOperation::Write {
                vec![".".into()]
            } else {
                Vec::new()
            },
            operation,
            lease_epoch: binding.lease_epoch(),
            collaboration_key,
        };
        Ok(ImplementerAccess {
            identity,
            capability,
        })
    }
}

#[derive(Debug)]
pub(crate) struct ChangeLaunchContext {
    pub(crate) binding: WorkerHostBinding,
    attempt: DelegatedAttempt,
    brief: DelegationBrief,
    model_binding: DelegatedModelBinding,
    codex_executor: bool,
}

/// The durable resolver is deliberately separate from the async launcher so
/// it can be used by startup recovery and by focused policy tests.
pub(crate) fn resolve_change_launch(
    db: &SharedDb,
    dispatch: &WorkerDispatch,
    now: i64,
) -> Result<ChangeLaunchContext, WorkerLaunchError> {
    let row = db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT d.brief_json, d.session_id, d.message_id, d.parent_run_id,
                        p.owner_profile_id, p.workspace_key, p.task_shape, p.worker_profile,
                        p.capability_scope_ref, p.scope_digest,
                        a.id, l.id, l.status, l.expires_at,
                        l.read_roots_json, l.write_roots_json, l.tool_allowlist_json,
                        l.network_hosts_json, p.id, d.executor
                 FROM delegations d
                 JOIN work_packages p ON p.id=d.work_package_id
                 JOIN delegation_attempts a ON a.delegation_id=d.id AND a.id=?2
                 JOIN delegation_capability_leases l ON l.attempt_id=a.id
                 WHERE d.id=?1 AND d.status='running' AND a.status='running'
                   AND p.status IN ('active','frozen') AND l.status='active'",
                rusqlite::params![dispatch.delegation_id, dispatch.attempt_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, String>(10)?,
                        row.get::<_, String>(11)?,
                        row.get::<_, String>(12)?,
                        row.get::<_, i64>(13)?,
                        row.get::<_, String>(14)?,
                        row.get::<_, String>(15)?,
                        row.get::<_, String>(16)?,
                        row.get::<_, String>(17)?,
                        row.get::<_, String>(18)?,
                        row.get::<_, String>(19)?,
                    ))
                },
            )
            .optional()
        })
        .map_err(|_| WorkerLaunchError::Launcher("change binding unavailable".into()))?
        .map_err(|_| WorkerLaunchError::Launcher("change binding unavailable".into()))?
        .ok_or_else(|| WorkerLaunchError::Launcher("change binding unavailable".into()))?;
    let (
        brief_json,
        session_id,
        message_id,
        parent_run_id,
        owner_profile_id,
        workspace_key,
        shape,
        profile,
        capability_ref,
        scope_digest,
        attempt_id,
        lease_id,
        lease_status,
        expires_at,
        read_roots_json,
        write_roots_json,
        tool_allowlist_json,
        network_hosts_json,
        work_package_id,
        executor,
    ) = row;
    let task_shape = TaskShape::from_str(&shape)
        .ok_or_else(|| WorkerLaunchError::Launcher("unknown task shape".into()))?;
    let worker_profile = WorkerProfile::from_str(&profile)
        .ok_or_else(|| WorkerLaunchError::Launcher("unknown worker profile".into()))?;
    let supported_worker = matches!(
        (task_shape, worker_profile),
        (
            TaskShape::Change,
            WorkerProfile::Implementer | WorkerProfile::Verifier
        ) | (TaskShape::Explore, WorkerProfile::Explorer)
    );
    if !supported_worker || dispatch.epoch == 0 {
        return Err(WorkerLaunchError::Launcher(
            "model worker policy rejected".into(),
        ));
    }
    let codex_executor = match executor.as_str() {
        "internal" => false,
        "codex_cli"
            if task_shape == TaskShape::Change && worker_profile == WorkerProfile::Implementer =>
        {
            true
        }
        _ => return Err(WorkerLaunchError::BindingMismatch),
    };
    let brief: DelegationBrief = serde_json::from_str(&brief_json)
        .map_err(|_| WorkerLaunchError::Launcher("delegation brief unavailable".into()))?;
    if brief.identity.delegation_id != dispatch.delegation_id
        || brief.identity.attempt_id != dispatch.attempt_id
    {
        return Err(WorkerLaunchError::BindingMismatch);
    }
    if expires_at.saturating_mul(1000) <= now.saturating_mul(1000) {
        return Err(WorkerLaunchError::Launcher("change lease expired".into()));
    }
    let status = match lease_status.as_str() {
        "active" => LeaseStatus::Active,
        "revoked" => LeaseStatus::Revoked,
        "expired" => LeaseStatus::Expired,
        _ => return Err(WorkerLaunchError::Launcher("invalid lease status".into())),
    };
    let grant = CapabilityGrant {
        id: lease_id,
        attempt_id: attempt_id.clone(),
        status,
        expires_at_ms: expires_at.saturating_mul(1000),
        read_roots: parse_paths(&read_roots_json)?,
        write_roots: parse_paths(&write_roots_json)?,
        tool_allowlist: parse_strings(&tool_allowlist_json)?,
        network_hosts: parse_strings(&network_hosts_json)?,
    };
    if codex_executor
        && (!grant.network_hosts.is_empty() || grant.write_roots != vec![PathBuf::from(".")])
    {
        return Err(WorkerLaunchError::BindingMismatch);
    }
    let lease = CapabilityLease::from_grant(grant)
        .map_err(|_| WorkerLaunchError::Launcher("invalid capability lease".into()))?;
    let scope = WorkPackageScope {
        session_id: session_id.clone(),
        owner_profile_id,
        workspace_key,
    };
    let model_binding = db
        .with_conn(|conn| DelegatedModelBindingRepository::load(conn, &scope, &work_package_id))
        .map_err(|_| WorkerLaunchError::Launcher("delegated model binding unavailable".into()))?
        .map_err(|_| WorkerLaunchError::Launcher("delegated model binding unavailable".into()))?;
    if model_binding.policy_version == 0 {
        return Err(WorkerLaunchError::Launcher(
            "delegated model policy invalid".into(),
        ));
    }
    let binding = WorkerHostBinding::new(
        dispatch.attempt_id.clone(),
        dispatch.delegation_id.clone(),
        work_package_id.clone(),
        dispatch.epoch,
        capability_ref,
        worker_profile,
        task_shape,
    )
    .map_err(|_| WorkerLaunchError::InvalidBinding)?;
    let attempt = DelegatedAttempt {
        delegation_id: dispatch.delegation_id.clone(),
        attempt_id,
        work_package_id,
        lease,
        epoch: dispatch.epoch,
    };
    Ok(ChangeLaunchContext {
        binding,
        attempt,
        brief,
        model_binding,
        codex_executor,
    })
}

fn parse_strings(value: &str) -> Result<Vec<String>, WorkerLaunchError> {
    serde_json::from_str::<Vec<String>>(value)
        .map_err(|_| WorkerLaunchError::Launcher("invalid capability metadata".into()))
}

fn parse_paths(value: &str) -> Result<Vec<PathBuf>, WorkerLaunchError> {
    parse_strings(value).map(|values| values.into_iter().map(PathBuf::from).collect())
}

/// An async launcher for both Change/Implementer and Change/Verifier. The
/// caller supplies the already constructed keychain model factory and the
/// provider-owned worktree implementation; no fallback model or path exists.
pub struct ChangeWorkerLauncher<F, M> {
    db: SharedDb,
    host_factory: Arc<F>,
    model_factory: Arc<M>,
    thinking: Option<crate::llm::ThinkingSettings>,
    limits: ContractLimits,
    now: fn() -> i64,
    worktree: Arc<GitWorktreeProvider>,
}

impl<F, M> ChangeWorkerLauncher<F, M> {
    pub fn new(
        db: SharedDb,
        host_factory: Arc<F>,
        model_factory: Arc<M>,
        thinking: Option<crate::llm::ThinkingSettings>,
        now: fn() -> i64,
        worktree: Arc<GitWorktreeProvider>,
    ) -> Self {
        Self {
            db,
            host_factory,
            model_factory,
            thinking,
            limits: ContractLimits::default(),
            now,
            worktree,
        }
    }
}

impl<F, M> AsyncWorkerLauncher for ChangeWorkerLauncher<F, M>
where
    F: super::worker_host_contract::WorkerHostFactory + 'static,
    M: DelegatedModelHostFactory + 'static,
    F::Task: WorkerTask + 'static,
{
    fn launch(
        &mut self,
        request: WorkerLaunchRequest,
    ) -> Result<Box<dyn PollableWorkerTask>, WorkerLaunchError> {
        if request.plan.is_some() {
            return Err(WorkerLaunchError::Launcher(
                "change worker rejects explorer plan".into(),
            ));
        }
        let context = resolve_change_launch(&self.db, &request.dispatch, (self.now)())?;
        request.binding.validate_for(&request.dispatch)?;
        if request.binding.work_package_id != context.binding.identity().work_package_id()
            || request.binding.lease_epoch != context.binding.lease_epoch()
        {
            return Err(WorkerLaunchError::BindingMismatch);
        }
        if context.codex_executor {
            let executable = codex_cli::installed_executable().ok_or_else(|| {
                WorkerLaunchError::Launcher("Codex CLI not found; no fallback".into())
            })?;
            let access = DurableWorktreeAccessResolver::new(self.db.clone())
                .resolve(&context.binding)
                .map_err(|_| WorkerLaunchError::BindingMismatch)?;
            let (tx, event_rx) = mpsc::channel();
            let cancelled = ToolCancellation::new();
            let producer_cancel = cancelled.clone();
            let worker = ChangeWorkerTask {
                attempt_id: context.attempt.attempt_id.clone(),
                lease_epoch: context.attempt.epoch,
                event_rx,
                cancelled,
                finished: false,
            };
            let provider = self.worktree.clone();
            let limits = self.limits.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let mut sink = ChannelSink {
                    tx: tx.clone(),
                    attempt_id: context.attempt.attempt_id.clone(),
                    epoch: context.attempt.epoch,
                };
                if let Some(delivery) = run_codex_candidate(
                    &provider,
                    &executable,
                    access,
                    &context,
                    &producer_cancel,
                    &mut sink,
                    &limits,
                ) {
                    if !producer_cancel.is_cancelled()
                        && validate_delivery(&delivery, &context.brief, None, &limits).is_ok()
                    {
                        let _ = sink.submit_delivery(&delivery, &limits);
                    }
                }
                let _ = tx.send(WorkerTaskEvent::Terminal {
                    attempt_id: context.attempt.attempt_id,
                    lease_epoch: context.attempt.epoch,
                    reason: if producer_cancel.is_cancelled() {
                        "Codex cancelled"
                    } else {
                        "Codex process ended"
                    }
                    .into(),
                });
            });
            return Ok(Box::new(worker));
        }
        let task = self
            .host_factory
            .start(context.binding.clone())
            .map_err(|error| {
                // The runtime persists only a stable failure class, but this
                // bounded variant lets that classifier distinguish a rejected
                // worktree capability from an unavailable host process.
                WorkerLaunchError::Launcher(format!("change worker host rejected: {error:?}"))
            })?;
        let provider = self
            .model_factory
            .create_model_host(&context.model_binding)
            .map_err(|_| WorkerLaunchError::Launcher("delegated model unavailable".into()))?;
        let host = DelegatedModelExecutionHost::new(
            provider,
            self.thinking.clone(),
            context.binding.clone(),
            task,
            &context.brief,
        )
        .map_err(WorkerLaunchError::Launcher)?;
        let (event_tx, event_rx) = mpsc::channel();
        let cancelled = ToolCancellation::new();
        let terminal_sent = Arc::new(AtomicBool::new(false));
        let worker = ChangeWorkerTask {
            attempt_id: context.attempt.attempt_id.clone(),
            lease_epoch: context.attempt.epoch,
            event_rx,
            cancelled: cancelled.clone(),
            finished: false,
        };
        let adapter = DelegatedWorkerAdapter::new(
            ExecutionKernel::new(cancelled.clone(), MODEL_TIMEOUT),
            self.limits.clone(),
        );
        tauri::async_runtime::spawn(run_change_host(
            host,
            context.attempt,
            context.brief,
            adapter,
            event_tx,
            cancelled,
            terminal_sent,
            self.limits.clone(),
        ));
        Ok(Box::new(worker))
    }
}

fn run_codex_candidate(
    provider: &GitWorktreeProvider,
    executable: &std::path::Path,
    access: ImplementerAccess,
    context: &ChangeLaunchContext,
    cancelled: &ToolCancellation,
    sink: &mut ChannelSink,
    limits: &ContractLimits,
) -> Option<DelegationDelivery> {
    let remaining = context
        .attempt
        .lease
        .expires_at_ms
        .saturating_sub(chrono::Utc::now().timestamp_millis());
    if remaining <= 0 {
        return None;
    }
    let heartbeat = || Heartbeat {
        attempt_id: context.attempt.attempt_id.clone(),
        epoch: context.attempt.epoch,
        stage: "codex_running".into(),
        progress_percent: 0,
        budget_remaining: "bounded".into(),
        evidence_refs: vec![],
    };
    let prompt = format!(
        "Implement this bounded coding task in the current candidate checkout. Leave changes uncommitted for independent review. Do not push, change Git configuration, edit outside the checkout, or use network tools. Run only relevant local checks.\n\nTask: {}\n\nAcceptance criteria: {}",
        context.brief.goal, context.brief.completion_criteria.join("; ")
    );
    sink.heartbeat(heartbeat()).ok()?;
    let timeout = MODEL_TIMEOUT.min(Duration::from_millis(remaining as u64));
    let run = provider.with_candidate_executor(
        &access.identity,
        access.capability.clone(),
        |candidate| {
            codex_cli::run(executable, candidate, prompt, cancelled, timeout, || {
                sink.heartbeat(heartbeat()).is_ok()
            })
        },
    );
    let mut summary = "Codex 未能交付可审校的候选修改。";
    let mut paths = Vec::new();
    let mut digest = String::new();
    let mut commands = None;
    match run {
        Ok(Ok(receipt)) if !cancelled.is_cancelled() => {
            let reported_sandbox_restriction = receipt.reported_sandbox_restriction;
            let file_change_failed = receipt.file_change_failed;
            commands = Some(receipt);
            match provider.inspect_executor_candidate(&access.identity, access.capability) {
                Ok((observed, hash))
                    if !observed.is_empty() && observed.len() <= limits.max_artifacts =>
                {
                    paths = observed;
                    digest = hash;
                }
                Ok((observed, _)) if observed.is_empty() => {
                    summary = if file_change_failed {
                        "Codex 的文件修改未成功，Git 未发现候选差异；未应用任何修改。"
                    } else if reported_sandbox_restriction {
                        "Codex 报告沙箱限制，Git 未发现候选修改；未降低隔离。"
                    } else {
                        "Codex 已结束，Git 未发现候选修改。"
                    }
                }
                _ => summary = "Codex 已结束，但候选状态超出范围或无法核对；未提交审校。",
            }
        }
        Ok(Ok(_)) | Ok(Err(codex_cli::CodexError::Cancelled)) => return None,
        Ok(Err(codex_cli::CodexError::Failed)) => {
            summary = "Codex 报告执行失败；未应用任何候选修改。"
        }
        Ok(Err(codex_cli::CodexError::TimedOut(phase))) => {
            summary = match phase {
                codex_cli::CodexPhase::Launch => "Codex 启动超时，尚未收到会话开始回执；已停止。",
                codex_cli::CodexPhase::Session => "Codex 已建立会话，但未开始任务；已超时停止。",
                codex_cli::CodexPhase::Inference => {
                    "Codex 已开始任务，尚未收到命令开始回执；已超时停止。"
                }
                codex_cli::CodexPhase::Action => {
                    "Codex 报告过执行活动但未完成任务；已超时停止，未应用候选修改。"
                }
            }
        }
        Ok(Err(codex_cli::CodexError::Unverified)) => {
            summary = "未能核实 Codex 进程或事件流结果；未应用任何候选修改。"
        }
        Ok(Err(codex_cli::CodexError::Spawn)) => summary = "Codex CLI 无法启动；未切换执行器。",
        Err(_) => summary = "候选工作树权限、基线或状态无法核实；未启动 Codex。",
    }
    Some(codex_delivery(
        &context.brief,
        paths,
        &digest,
        commands,
        summary,
    ))
}

fn codex_delivery(
    brief: &DelegationBrief,
    paths: Vec<String>,
    digest: &str,
    commands: Option<codex_cli::CodexResult>,
    failure_summary: &str,
) -> DelegationDelivery {
    let evidence = format!("{}{digest}", super::workspace_isolation::CODEX_DIFF_REF);
    let event_ref = "evidence://codex-cli/command-status".to_string();
    let success = !paths.is_empty();
    let mut evidence_refs = vec![];
    let mut key_facts = vec![];
    let mut verifications = vec![];
    if success {
        evidence_refs.push(evidence.clone());
        key_facts.push(KeyFact {
            statement: format!("宿主通过 Git 核对了 {} 个候选路径。", paths.len()),
            confidence: Confidence::High,
            evidence_refs: vec![evidence.clone()],
        });
        verifications.push(VerificationRecord {
            item: "候选路径与基线".into(),
            method: "本地 Git 工作树状态".into(),
            status: VerificationStatus::Passed,
            conclusion: "有可核对的候选改动，尚未审校或应用。".into(),
            evidence_ref: evidence.clone(),
        });
    }
    if let Some(commands) = commands {
        evidence_refs.push(event_ref.clone());
        verifications.push(VerificationRecord {
            item: "Codex 命令退出状态".into(),
            method: "CLI 结构化事件".into(),
            status: if commands.commands_failed > 0 {
                VerificationStatus::Failed
            } else {
                VerificationStatus::NotRun
            },
            conclusion: format!(
                "记录到 {} 次命令结束，{} 次失败；退出状态不能证明任务测试通过。",
                commands.commands_finished, commands.commands_failed
            ),
            evidence_ref: event_ref,
        });
    }
    DelegationDelivery {
        file_read_evidence: vec![],
        schema_version: brief.schema_version,
        delivery_id: format!("codex_{}", uuid::Uuid::new_v4().simple()),
        delivery_revision: 1,
        identity: brief.identity.clone(),
        status: if success {
            DeliveryStatus::Completed
        } else {
            DeliveryStatus::Failed
        },
        executive_summary: if success {
            "Codex 已产出候选修改，等待独立审校；尚未应用。"
        } else {
            failure_summary
        }
        .into(),
        key_facts,
        milestones: vec![],
        verifications,
        candidate_artifacts: paths
            .into_iter()
            .map(|relative_ref| CandidateArtifact {
                kind: "file".into(),
                relative_ref,
                description: "宿主 Git 观测到的候选改动。".into(),
                evidence_ref: Some(evidence.clone()),
            })
            .collect(),
        open_questions: vec![],
        risks: vec![],
        evidence_refs,
        usage: UsageFlags {
            context_truncated: false,
            output_truncated: false,
            budget_exhausted: false,
        },
    }
}

struct ChangeWorkerTask {
    attempt_id: String,
    lease_epoch: u64,
    event_rx: Receiver<WorkerTaskEvent>,
    cancelled: ToolCancellation,
    finished: bool,
}

impl PollableWorkerTask for ChangeWorkerTask {
    fn poll(&mut self) -> Result<Option<WorkerTaskEvent>, WorkerLaunchError> {
        match self.event_rx.try_recv() {
            Ok(event) => {
                if matches!(event, WorkerTaskEvent::Terminal { .. }) {
                    self.finished = true;
                }
                Ok(Some(event))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) if self.finished => Ok(None),
            Err(TryRecvError::Disconnected) => Err(WorkerLaunchError::Launcher(
                "change worker event channel closed".into(),
            )),
        }
    }

    fn cancel(&mut self, _: StopReason) -> Result<(), WorkerLaunchError> {
        self.cancelled.cancel();
        Ok(())
    }

    fn join(&mut self) -> Result<Vec<WorkerTaskEvent>, WorkerLaunchError> {
        let mut events = Vec::new();
        while !self.finished {
            match self.event_rx.recv_timeout(JOIN_TIMEOUT) {
                Ok(event) => {
                    if matches!(event, WorkerTaskEvent::Terminal { .. }) {
                        self.finished = true;
                    }
                    events.push(event);
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(WorkerLaunchError::Launcher(
                        "change worker did not terminate".into(),
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(events)
    }
}

struct ChannelSink {
    tx: Sender<WorkerTaskEvent>,
    attempt_id: String,
    epoch: u64,
}

impl DelegatedDeliverySink for ChannelSink {
    fn heartbeat(&mut self, heartbeat: Heartbeat) -> Result<(), String> {
        if heartbeat.attempt_id != self.attempt_id || heartbeat.epoch != self.epoch {
            return Err("worker heartbeat identity mismatch".into());
        }
        self.tx
            .send(WorkerTaskEvent::Heartbeat(heartbeat))
            .map_err(|_| "worker event channel closed".into())
    }

    fn submit_delivery(
        &mut self,
        delivery: &DelegationDelivery,
        _: &ContractLimits,
    ) -> Result<(), String> {
        if delivery.identity.attempt_id != self.attempt_id {
            return Err("worker delivery identity mismatch".into());
        }
        self.tx
            .send(WorkerTaskEvent::Delivery {
                delivery: delivery.clone(),
                lease_epoch: self.epoch,
            })
            .map_err(|_| "worker event channel closed".into())
    }
}

async fn run_change_host<T>(
    mut host: DelegatedModelExecutionHost<T>,
    attempt: DelegatedAttempt,
    brief: DelegationBrief,
    adapter: DelegatedWorkerAdapter,
    tx: Sender<WorkerTaskEvent>,
    cancelled: ToolCancellation,
    terminal_sent: Arc<AtomicBool>,
    limits: ContractLimits,
) where
    T: WorkerTask,
{
    let mut sink = ChannelSink {
        tx: tx.clone(),
        attempt_id: attempt.attempt_id.clone(),
        epoch: attempt.epoch,
    };
    // Cancellation is attempt-local and one-way. Dropping the run future also
    // drops an in-flight provider request/retry wait, before another tool can
    // execute; a separate atomic flag checked only at startup cannot do that.
    let result = tokio::select! {
        biased;
        _ = cancelled.cancelled() => Err("change worker cancelled".to_string()),
        result = adapter.run(&attempt, &brief, &mut host, &mut sink, |_| {}) => {
            result.map(|_| ())
                .map_err(|error| format!("delegated worker failed: {:?}", error.kind))
        }
    };
    if terminal_sent.swap(true, Ordering::AcqRel) {
        return;
    }
    let reason = match result {
        Ok(()) => "change worker completed".to_string(),
        Err(error) => error,
    };
    let _ = limits;
    let _ = tx.send(WorkerTaskEvent::Terminal {
        attempt_id: attempt.attempt_id,
        lease_epoch: attempt.epoch,
        reason,
    });
}

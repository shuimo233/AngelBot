//! Agent module - handles tool calling and agent loop
//!
//! Layout:
//! - `conversation/`: foreground Main-Agent turns, history, compaction, and steering.
//! - `delegation/`: durable work-package issuance and worker admission.
//! - `workers/`: bounded Explorer, Implementer, and Reviewer host adapters.
//! - `changes/`: candidate capture, independent review, and materialization.
//! - `isolation/`: worktree capabilities, sandbox lifecycle, and cleanup.
//! - `tools/`: registry, execution, guards, network, and verification.
//! - `planning/`: planning, routing, scheduling, critique, and search.
//! - `support/`: shared state, configuration, events, telemetry, and policies.
//! - `supervision/`: the Workspace-level control plane being introduced by P0.

// Physical layout follows responsibility. The flat exports below are the
// source-compatible consolidation seam: every name binds directly to exactly
// one implementation in its responsibility directory, so the migration does
// not preserve a second runtime path or duplicate implementation.
#[path = "support/adaptive_constraints.rs"]
pub mod adaptive_constraints;
#[path = "support/attention.rs"]
pub mod attention;
#[path = "tools/cache.rs"]
pub mod cache;
#[path = "changes/change_handoff_store.rs"]
pub mod change_handoff_store;
#[path = "changes/change_pipeline.rs"]
pub mod change_pipeline;
#[path = "workers/change_worker_runtime.rs"]
pub mod change_worker_runtime;
#[path = "support/circuit_breaker.rs"]
pub mod circuit_breaker;
#[path = "conversation/compaction.rs"]
pub mod compaction;
#[path = "isolation/concrete_sandbox.rs"]
pub mod concrete_sandbox;
#[path = "support/config.rs"]
pub mod config;
#[path = "planning/critique.rs"]
pub mod critique;
#[path = "delegation/delegate_work.rs"]
pub mod delegate_work;
#[path = "delegation/delegated_attempt_scheduler.rs"]
pub mod delegated_attempt_scheduler;
#[path = "delegation/delegated_delivery_follow_up.rs"]
pub mod delegated_delivery_follow_up;
#[path = "workers/delegated_model_binding.rs"]
pub mod delegated_model_binding;
#[path = "workers/delegated_model_execution_host.rs"]
pub mod delegated_model_execution_host;
#[path = "workers/delegated_model_host.rs"]
pub mod delegated_model_host;
#[path = "delegation/delegated_network_scope.rs"]
pub mod delegated_network_scope;
#[path = "workers/delegated_runtime_factory.rs"]
pub mod delegated_runtime_factory;
#[path = "isolation/delegated_sandbox.rs"]
pub mod delegated_sandbox;
#[path = "workers/delegated_worker_adapter.rs"]
pub mod delegated_worker_adapter;
#[path = "workers/delegated_worker_launcher.rs"]
pub mod delegated_worker_launcher;
#[path = "workers/delegated_worker_router.rs"]
pub mod delegated_worker_router;
#[path = "delegation/delegation.rs"]
pub mod delegation;
#[path = "delegation/delegation_contract.rs"]
pub mod delegation_contract;
#[path = "delegation/delegation_issuance.rs"]
pub mod delegation_issuance;
#[path = "delegation/delegation_pump.rs"]
pub mod delegation_pump;
#[path = "delegation/delegation_runtime.rs"]
pub mod delegation_runtime;
#[path = "delegation/delegation_service.rs"]
pub mod delegation_service;
#[path = "delegation/delivery_inbox.rs"]
pub mod delivery_inbox;
#[cfg(feature = "desktop-e2e")]
#[path = "tools/desktop_e2e_explorer_io.rs"]
pub(crate) mod desktop_e2e_explorer_io;
pub mod eval;
#[path = "support/event.rs"]
pub mod event;
#[path = "support/evolution.rs"]
pub mod evolution;
#[path = "tools/execution_gateway.rs"]
pub mod execution_gateway;
#[path = "tools/execution_kernel.rs"]
pub mod execution_kernel;
#[cfg(test)]
#[path = "tools/execution_kernel_tests.rs"]
mod execution_kernel_tests;
#[path = "workers/explorer_gateway_context.rs"]
pub mod explorer_gateway_context;
#[path = "workers/explorer_network_tool_host.rs"]
pub mod explorer_network_tool_host;
#[path = "workers/explorer_plan.rs"]
pub mod explorer_plan;
#[path = "workers/explorer_runtime_config.rs"]
pub mod explorer_runtime_config;
#[path = "workers/explorer_worker_host_adapter.rs"]
pub mod explorer_worker_host_adapter;
#[path = "workers/explorer_worker_runtime.rs"]
pub mod explorer_worker_runtime;
pub mod extensions;
#[path = "conversation/foreground_conversation_state.rs"]
pub mod foreground_conversation_state;
#[path = "conversation/foreground_lifecycle_control.rs"]
pub mod foreground_lifecycle_control;
#[path = "conversation/foreground_tool_batch_host.rs"]
pub(crate) mod foreground_tool_batch_host;
#[path = "tools/guards.rs"]
pub mod guards;
#[cfg(test)]
#[path = "tools/guards_tests.rs"]
mod guards_tests;
#[cfg(test)]
#[path = "tools/handler_tests.rs"]
mod handler_tests;
#[path = "tools/handlers.rs"]
pub mod handlers;
#[path = "workers/implementer_worker_host.rs"]
pub mod implementer_worker_host;
#[path = "conversation/lifecycle.rs"]
pub mod lifecycle;
#[path = "changes/materializer.rs"]
pub mod materializer;
#[path = "tools/network_gateway.rs"]
pub mod network_gateway;
#[path = "tools/network_transport.rs"]
pub mod network_transport;
#[path = "support/otel.rs"]
pub mod otel;
#[path = "conversation/pending_queue.rs"]
pub mod pending_queue;
#[cfg(test)]
#[path = "conversation/pending_queue_tests.rs"]
mod pending_queue_tests;
#[path = "planning/planner.rs"]
pub mod planner;
#[path = "delegation/policy_lease_issuer.rs"]
pub mod policy_lease_issuer;
#[path = "delegation/project_network_approval.rs"]
pub mod project_network_approval;
#[path = "conversation/prompt.rs"]
pub mod prompt;
#[path = "tools/registry.rs"]
mod registry;
#[path = "isolation/resource_binding.rs"]
pub mod resource_binding;
#[path = "changes/review_agent.rs"]
pub mod review_agent;
#[path = "changes/review_artifact_store.rs"]
pub mod review_artifact_store;
#[path = "changes/review_contract.rs"]
pub mod review_contract;
#[path = "changes/review_coordinator.rs"]
pub mod review_coordinator;
#[path = "changes/review_store.rs"]
pub mod review_store;
#[path = "workers/reviewer_attempt.rs"]
pub mod reviewer_attempt;
#[path = "planning/router.rs"]
pub mod router;
#[path = "support/run_state.rs"]
pub mod run_state;
#[path = "conversation/runner.rs"]
mod runner;
#[cfg(test)]
#[path = "conversation/runner_tests.rs"]
mod runner_tests;
#[path = "tools/safe_host_resolver.rs"]
pub mod safe_host_resolver;
#[path = "planning/scheduler.rs"]
pub mod scheduler;
#[path = "support/shared_db.rs"]
pub mod shared_db;
#[path = "conversation/steering.rs"]
pub mod steering;
pub mod supervision;
#[path = "conversation/task_facts.rs"]
pub mod task_facts;
#[path = "conversation/task_understanding.rs"]
pub mod task_understanding;
#[path = "isolation/terminal_cleanup.rs"]
pub mod terminal_cleanup;
#[path = "tools/tool.rs"]
pub mod tool;
#[path = "planning/tot.rs"]
pub mod tot;
#[path = "planning/tot_integration.rs"]
pub mod tot_integration;
#[path = "tools/verifier.rs"]
pub mod verifier;
#[path = "delegation/work_package.rs"]
pub mod work_package;
#[path = "workers/worker_host_contract.rs"]
pub mod worker_host_contract;
#[path = "delegation/worker_policy.rs"]
pub mod worker_policy;
#[path = "isolation/workspace_admission.rs"]
pub mod workspace_admission;
#[path = "isolation/workspace_isolation.rs"]
pub mod workspace_isolation;
#[path = "delegation/workspace_scope_key.rs"]
pub(crate) mod workspace_scope_key;

#[cfg(test)]
#[path = "support/scenario_tests.rs"]
pub mod scenario_tests;

#[allow(unused_imports)]
pub use cache::{CacheKey, CachedResult, ToolResultCache};
#[allow(unused_imports)]
pub use circuit_breaker::{
    CircuitState as ToolCircuitState, GlobalCircuitBreaker, ToolCircuitBreaker, ToolTimeoutExecutor,
};
#[allow(unused_imports)]
pub use config::{
    AgentConfig, CoreMemoryBlock, MemoryContext, PersonalityTraits, UserPreferences, UserProfile,
};
#[allow(unused_imports)]
pub use critique::{CritiqueVerdict, ReflexionCritique};
#[allow(unused_imports)]
pub use event::{
    AgentEvent, AgentEventEmitter, AgentRunJournal, DevEventStore, DurableEventEmitter, ErrorCode,
    EventChannel, EventEmitter, EventEmitterState, NullEventEmitter, OptionalEmitter,
    TauriEventEmitter,
};
#[allow(unused_imports)]
pub(crate) use foreground_conversation_state::task_facts_from_steps;
#[allow(unused_imports)]
pub use lifecycle::{
    apply_after_hook, AfterToolCallContext, AfterToolCallResult, BeforeToolCallResult,
    DefaultApiKeyProvider, DefaultLifecycleHook, DefaultToolLifecycleHook, DynamicApiKeyProvider,
    DynamicLlmProvider, ToolCallContext, ToolLifecycleHook,
};
#[allow(unused_imports)]
pub use otel::{AgentOtelBridge, OtelConfig, OtelExporter, OtelTracer, SharedSpanCollector};
#[allow(unused_imports)]
pub use planner::{ExecutionPlan, Executor, PlanMode, PlanStep, Planner, TaskComplexity};
#[allow(unused_imports)]
pub use prompt::{build_agent_system_prompt, build_simple_system_prompt};
#[allow(unused_imports)]
pub use registry::ToolRegistry;
#[allow(unused_imports)]
pub use router::{SharedToolRouter, ToolRouter};
pub use run_state::{derive_run_state, AgentRunStatus, ConfirmationState, RunStateInput};
pub use runner::{AgentResponse, AgentRunner, AgentSliceOutcome, NativeAgentSliceResult};
#[allow(unused_imports)]
pub use scheduler::{analyze_dependencies, topological_sort, SchedulerConfig, ToolScheduler};
#[allow(unused_imports)]
pub use steering::{
    AgentControlState, AgentYield, Instruction, InstructionPriority, InstructionQueue,
    InstructionStatus, QueueManager,
};
#[allow(unused_imports)]
pub use tool::{
    Tool, ToolCall, ToolCancellation, ToolExecutionContext, ToolExecutionMode, ToolHandler,
    ToolResult,
};
#[allow(unused_imports)]
pub use tot::{NodeStatus, ToTEngine, ToTNode, ToTStats, ToTTree, ToTTrigger};
#[allow(unused_imports)]
pub use tot_integration::{
    detect_tot_trigger, run_tot_exploration, ToTConfig, ToTExplorationResult,
};

//! Workspace-owned control plane.
//!
//! This is deliberately a deep module: callers submit an input or control
//! action and receive a safe projection. Scheduling, claim CAS, priority, and
//! SQLite ownership remain internal. Execution is still delegated to the
//! existing runtime adapters while they are migrated behind this seam.

mod actor;
pub(crate) mod delegation_activity_adapter;
mod delegation_dispatcher;
mod model;
mod store;

pub use actor::{DispatchOutcome, DispatchedWork, WorkDispatcher, WorkspaceSupervisorActor};
pub use delegation_dispatcher::{authorize_one_pending_delegation, SupervisorDelegationDispatcher};
pub use model::{
    ActivityProjection, ActivityStage, ClaimedFollowUp, ClaimedInput, ClaimedWork, SupervisorInput,
    SupervisorInputKind, SupervisorInputStatus, SupervisorWork, SupervisorWorkKind,
    SupervisorWorkStatus,
};
pub use store::{SupervisorError, WorkspaceSupervisor};

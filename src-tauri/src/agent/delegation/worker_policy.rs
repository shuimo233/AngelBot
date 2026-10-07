//! Pure, fail-closed capability planning for delegated workers.
//!
//! This module is deliberately independent of leases, gateways, sandboxes and
//! tool execution.  Callers compile one immutable plan, persist its identity,
//! then a later adapter translates its *target capabilities* into concrete
//! gateway grants.  A plan itself grants no filesystem, process, network, or
//! materialization authority.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub const WORKER_POLICY_VERSION: u16 = 1;

/// The user-visible intent of a work package.  This is a hard capability
/// ceiling; a profile can only reduce it, never expand it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskShape {
    Explore,
    Change,
}

impl TaskShape {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Explore => "explore",
            Self::Change => "change",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "explore" => Some(Self::Explore),
            "change" => Some(Self::Change),
            _ => None,
        }
    }
}

/// A reusable worker capability template.  It never replaces the task-shape
/// ceiling or a future WorkPackage/source-policy/lease intersection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerProfile {
    Explorer,
    Implementer,
    Verifier,
}

impl WorkerProfile {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Explorer => "explorer",
            Self::Implementer => "implementer",
            Self::Verifier => "verifier",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "explorer" => Some(Self::Explorer),
            "implementer" => Some(Self::Implementer),
            "verifier" => Some(Self::Verifier),
            _ => None,
        }
    }
}

/// Target network operations.  The future network gateway still applies a
/// source policy, redirects/DNS checks, budgets and its global ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkAction {
    Search,
    Fetch,
    Registry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalExecutionCategory {
    Build,
    Test,
}

/// Scheduling hint only; it does not grant a concurrent execution slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrencyClass {
    ReadOnly,
    CandidateWriter,
    Verification,
}

/// The complete, serializable target capability set for one shape/profile
/// selection.  No field represents real-workspace write authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityPolicy {
    pub version: u16,
    pub task_shape: TaskShape,
    pub worker_profile: WorkerProfile,
    pub project_read: bool,
    pub candidate_read: bool,
    pub candidate_write: bool,
    pub scratch_write: bool,
    pub network_actions: BTreeSet<NetworkAction>,
    pub local_execution: BTreeSet<LocalExecutionCategory>,
    pub may_request_confirmation: bool,
    pub may_materialize: bool,
    pub concurrency_class: ConcurrencyClass,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkerPolicyError {
    #[error("worker policy version {0} is unsupported")]
    UnsupportedVersion(u16),
    #[error("explore tasks cannot receive change capabilities")]
    ExploreCapabilityEscalation,
    #[error("worker policies never grant materialization")]
    MaterializationForbidden,
    #[error("invalid worker policy: {0}")]
    Invalid(&'static str),
}

impl CapabilityPolicy {
    /// Compiles the immutable default template.  Future runtime code must
    /// intersect this plan with WorkPackage policy, a lease and global limits;
    /// it must not add capabilities to the plan.
    pub fn compile(task_shape: TaskShape, worker_profile: WorkerProfile) -> Self {
        let mut policy = match worker_profile {
            WorkerProfile::Explorer => Self::read_only(task_shape, worker_profile),
            WorkerProfile::Implementer => Self::implementer(task_shape),
            WorkerProfile::Verifier => Self::verifier(task_shape),
        };
        if task_shape == TaskShape::Explore {
            // Task shape is the hard ceiling even when a caller selected a
            // profile normally used for changes.
            policy = Self::read_only(TaskShape::Explore, worker_profile);
        }
        debug_assert!(policy.validate().is_ok());
        policy
    }

    pub fn validate(&self) -> Result<(), WorkerPolicyError> {
        if self.version != WORKER_POLICY_VERSION {
            return Err(WorkerPolicyError::UnsupportedVersion(self.version));
        }
        if self.may_materialize {
            return Err(WorkerPolicyError::MaterializationForbidden);
        }
        if !self.project_read {
            return Err(WorkerPolicyError::Invalid(
                "a worker must have no useful empty plan",
            ));
        }
        if self.task_shape == TaskShape::Explore {
            if self.candidate_read
                || self.candidate_write
                || self.scratch_write
                || self.may_request_confirmation
                || !self.local_execution.is_empty()
                || self.network_actions
                    != BTreeSet::from([NetworkAction::Search, NetworkAction::Fetch])
                || self.concurrency_class != ConcurrencyClass::ReadOnly
            {
                return Err(WorkerPolicyError::ExploreCapabilityEscalation);
            }
            return Ok(());
        }
        match self.worker_profile {
            WorkerProfile::Explorer
                if self.candidate_read
                    || self.candidate_write
                    || self.scratch_write
                    || self.may_request_confirmation
                    || !self.local_execution.is_empty()
                    || self.network_actions
                        != BTreeSet::from([NetworkAction::Search, NetworkAction::Fetch])
                    || self.concurrency_class != ConcurrencyClass::ReadOnly =>
            {
                Err(WorkerPolicyError::Invalid("explorer change plan expanded"))
            }
            WorkerProfile::Implementer
                if !self.candidate_write
                    || !self.scratch_write
                    || self.candidate_read
                    || self.may_request_confirmation
                    || self.network_actions
                        != BTreeSet::from([NetworkAction::Fetch, NetworkAction::Registry])
                    || self.local_execution
                        != BTreeSet::from([
                            LocalExecutionCategory::Build,
                            LocalExecutionCategory::Test,
                        ])
                    || self.concurrency_class != ConcurrencyClass::CandidateWriter =>
            {
                Err(WorkerPolicyError::Invalid(
                    "implementer plan is not least privilege",
                ))
            }
            WorkerProfile::Verifier
                if !self.candidate_read
                    || self.candidate_write
                    || !self.scratch_write
                    || self.may_request_confirmation
                    || self.network_actions != BTreeSet::from([NetworkAction::Registry])
                    || self.local_execution
                        != BTreeSet::from([
                            LocalExecutionCategory::Build,
                            LocalExecutionCategory::Test,
                        ])
                    || self.concurrency_class != ConcurrencyClass::Verification =>
            {
                Err(WorkerPolicyError::Invalid(
                    "verifier plan is not least privilege",
                ))
            }
            _ => Ok(()),
        }
    }

    fn read_only(task_shape: TaskShape, worker_profile: WorkerProfile) -> Self {
        Self {
            version: WORKER_POLICY_VERSION,
            task_shape,
            worker_profile,
            project_read: true,
            candidate_read: false,
            candidate_write: false,
            scratch_write: false,
            network_actions: BTreeSet::from([NetworkAction::Search, NetworkAction::Fetch]),
            local_execution: BTreeSet::new(),
            may_request_confirmation: false,
            may_materialize: false,
            concurrency_class: ConcurrencyClass::ReadOnly,
        }
    }

    fn implementer(task_shape: TaskShape) -> Self {
        Self {
            version: WORKER_POLICY_VERSION,
            task_shape,
            worker_profile: WorkerProfile::Implementer,
            project_read: true,
            candidate_read: false,
            candidate_write: true,
            scratch_write: true,
            network_actions: BTreeSet::from([NetworkAction::Fetch, NetworkAction::Registry]),
            local_execution: BTreeSet::from([
                LocalExecutionCategory::Build,
                LocalExecutionCategory::Test,
            ]),
            may_request_confirmation: false,
            may_materialize: false,
            concurrency_class: ConcurrencyClass::CandidateWriter,
        }
    }

    fn verifier(task_shape: TaskShape) -> Self {
        Self {
            version: WORKER_POLICY_VERSION,
            task_shape,
            worker_profile: WorkerProfile::Verifier,
            project_read: true,
            candidate_read: true,
            candidate_write: false,
            scratch_write: true,
            network_actions: BTreeSet::from([NetworkAction::Registry]),
            local_execution: BTreeSet::from([
                LocalExecutionCategory::Build,
                LocalExecutionCategory::Test,
            ]),
            may_request_confirmation: false,
            may_materialize: false,
            concurrency_class: ConcurrencyClass::Verification,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explore_is_a_hard_ceiling_for_every_profile() {
        for profile in [
            WorkerProfile::Explorer,
            WorkerProfile::Implementer,
            WorkerProfile::Verifier,
        ] {
            let policy = CapabilityPolicy::compile(TaskShape::Explore, profile);
            assert!(policy.validate().is_ok());
            assert!(policy.project_read);
            assert!(!policy.candidate_read && !policy.candidate_write && !policy.scratch_write);
            assert!(!policy.may_request_confirmation && !policy.may_materialize);
            assert!(policy.local_execution.is_empty());
            assert_eq!(
                policy.network_actions,
                BTreeSet::from([NetworkAction::Search, NetworkAction::Fetch])
            );
        }
    }

    #[test]
    fn change_profile_matrix_is_least_privilege() {
        let explorer = CapabilityPolicy::compile(TaskShape::Change, WorkerProfile::Explorer);
        assert!(!explorer.candidate_write && explorer.local_execution.is_empty());
        let implementer = CapabilityPolicy::compile(TaskShape::Change, WorkerProfile::Implementer);
        assert!(implementer.candidate_write && implementer.scratch_write);
        assert!(!implementer.candidate_read && !implementer.may_materialize);
        let verifier = CapabilityPolicy::compile(TaskShape::Change, WorkerProfile::Verifier);
        assert!(verifier.candidate_read && verifier.scratch_write);
        assert!(!verifier.candidate_write && !verifier.may_materialize);
    }

    #[test]
    fn validation_rejects_shape_escalation_and_materialization() {
        let mut explore = CapabilityPolicy::compile(TaskShape::Explore, WorkerProfile::Implementer);
        explore.candidate_write = true;
        assert_eq!(
            explore.validate(),
            Err(WorkerPolicyError::ExploreCapabilityEscalation)
        );
        let mut change = CapabilityPolicy::compile(TaskShape::Change, WorkerProfile::Implementer);
        change.may_materialize = true;
        assert_eq!(
            change.validate(),
            Err(WorkerPolicyError::MaterializationForbidden)
        );
    }

    #[test]
    fn version_is_immutable_and_checked() {
        let mut policy = CapabilityPolicy::compile(TaskShape::Change, WorkerProfile::Explorer);
        assert_eq!(policy.version, WORKER_POLICY_VERSION);
        policy.version += 1;
        assert_eq!(
            policy.validate(),
            Err(WorkerPolicyError::UnsupportedVersion(
                WORKER_POLICY_VERSION + 1
            ))
        );
    }
}

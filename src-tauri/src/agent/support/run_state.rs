//! Durable state vocabulary for a bounded agent run.
//!
//! `task_runs` is the durable user-facing summary of a run.  Keeping its
//! status strings in one module prevents the runner, confirmation resolver and
//! hydration code from gradually inventing incompatible states.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRunStatus {
    Running,
    AwaitingConfirmation,
    ContinueSuggested,
    NeedsAttention,
    ProviderUnavailable,
    Completed,
    Stopped,
}

impl AgentRunStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::AwaitingConfirmation => "awaiting_confirmation",
            Self::ContinueSuggested => "continue_suggested",
            Self::NeedsAttention => "needs_attention",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::Completed => "completed",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationState {
    None,
    Pending,
    Approved,
    Rejected,
}

impl ConfirmationState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RunStateInput {
    pub total_steps: usize,
    pub completed_steps: usize,
    pub has_pending_confirmation: bool,
    pub has_rejected_confirmation: bool,
    pub has_approved_confirmation: bool,
    pub has_failure: bool,
    pub provider_terminal_failure: bool,
    /// An approved side effect ended the current bounded slice.  The user must
    /// explicitly continue instead of the runtime silently starting another
    /// model turn.
    pub explicit_continue_after_approval: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DerivedRunState {
    pub status: AgentRunStatus,
    pub confirmation: ConfirmationState,
    pub resumable: bool,
}

pub fn derive_run_state(input: RunStateInput) -> DerivedRunState {
    let confirmation = if input.has_pending_confirmation {
        ConfirmationState::Pending
    } else if input.has_rejected_confirmation {
        ConfirmationState::Rejected
    } else if input.has_approved_confirmation {
        ConfirmationState::Approved
    } else {
        ConfirmationState::None
    };

    let status = if input.has_pending_confirmation {
        AgentRunStatus::AwaitingConfirmation
    } else if input.provider_terminal_failure {
        AgentRunStatus::ProviderUnavailable
    } else if input.has_failure {
        AgentRunStatus::NeedsAttention
    } else if input.completed_steps < input.total_steps {
        AgentRunStatus::Running
    } else if input.explicit_continue_after_approval && input.has_approved_confirmation {
        AgentRunStatus::ContinueSuggested
    } else {
        AgentRunStatus::Completed
    };

    DerivedRunState {
        status,
        confirmation,
        resumable: !input.provider_terminal_failure
            && (input.has_pending_confirmation
                || input.has_failure
                || (input.explicit_continue_after_approval && input.has_approved_confirmation)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_confirmation_precedes_any_other_terminal_signal() {
        let state = derive_run_state(RunStateInput {
            total_steps: 2,
            completed_steps: 1,
            has_pending_confirmation: true,
            has_failure: true,
            provider_terminal_failure: true,
            ..Default::default()
        });

        assert_eq!(state.status, AgentRunStatus::AwaitingConfirmation);
        assert_eq!(state.confirmation, ConfirmationState::Pending);
        assert!(
            !state.resumable,
            "a provider-terminal failure cannot be resumed even when the visible state still explains a pending confirmation"
        );
    }

    #[test]
    fn approved_bounded_slice_requires_explicit_continuation() {
        let state = derive_run_state(RunStateInput {
            total_steps: 1,
            completed_steps: 1,
            has_approved_confirmation: true,
            explicit_continue_after_approval: true,
            ..Default::default()
        });

        assert_eq!(state.status, AgentRunStatus::ContinueSuggested);
        assert_eq!(state.confirmation, ConfirmationState::Approved);
        assert!(state.resumable);
    }
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SupervisorInputKind {
    Steer,
    FollowUp,
    UserTask,
}

impl SupervisorInputKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Steer => "steer",
            Self::FollowUp => "follow_up",
            Self::UserTask => "user_task",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "steer" => Some(Self::Steer),
            "follow_up" => Some(Self::FollowUp),
            "user_task" => Some(Self::UserTask),
            _ => None,
        }
    }

    pub(crate) const fn priority(self) -> i64 {
        match self {
            Self::Steer => 300,
            Self::UserTask => 200,
            Self::FollowUp => 100,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SupervisorInputStatus {
    Queued,
    Claimed,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SupervisorWorkStatus {
    Queued,
    Claimed,
    Completed,
    Cancelled,
}

impl SupervisorWorkStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Claimed => "claimed",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl SupervisorInputStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Claimed => "claimed",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "claimed" => Some(Self::Claimed),
            "completed" => Some(Self::Completed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorInput {
    pub id: String,
    pub workspace_id: String,
    pub kind: SupervisorInputKind,
    pub content: String,
    pub status: SupervisorInputStatus,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedInput {
    pub input: SupervisorInput,
    pub claim_token: String,
}

/// A durable follow-up input paired with the only Main-Agent session that may
/// consume it.  The store resolves that session from the Workspace inside the
/// same claim transaction; callers never provide a model- or WebView-controlled
/// session id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedFollowUp {
    pub claim: ClaimedInput,
    pub session_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SupervisorWorkKind {
    Explore,
    CandidateImplementation,
    Review,
}

impl SupervisorWorkKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Explore => "explore",
            Self::CandidateImplementation => "candidate_implementation",
            Self::Review => "review",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "explore" => Some(Self::Explore),
            "candidate_implementation" => Some(Self::CandidateImplementation),
            "review" => Some(Self::Review),
            _ => None,
        }
    }

    pub(crate) const fn initial_stage(self) -> ActivityStage {
        match self {
            Self::Explore => ActivityStage::DelegatedExploration,
            Self::CandidateImplementation => ActivityStage::CandidateImplementation,
            Self::Review => ActivityStage::Review,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorWork {
    pub id: String,
    pub workspace_id: String,
    pub source_input_id: Option<String>,
    pub kind: SupervisorWorkKind,
    pub objective: String,
    pub status: SupervisorWorkStatus,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedWork {
    pub work: SupervisorWork,
    pub claim_token: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivityStage {
    Understanding,
    Processing,
    DelegatedExploration,
    CandidateImplementation,
    Review,
    WaitingDecision,
    Completed,
}

impl ActivityStage {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Understanding => "understanding",
            Self::Processing => "processing",
            Self::DelegatedExploration => "delegated_exploration",
            Self::CandidateImplementation => "candidate_implementation",
            Self::Review => "review",
            Self::WaitingDecision => "waiting_decision",
            Self::Completed => "completed",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "understanding" => Some(Self::Understanding),
            "processing" => Some(Self::Processing),
            "delegated_exploration" => Some(Self::DelegatedExploration),
            "candidate_implementation" => Some(Self::CandidateImplementation),
            "review" => Some(Self::Review),
            "waiting_decision" => Some(Self::WaitingDecision),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityProjection {
    pub workspace_id: String,
    pub work_package_id: String,
    pub objective: String,
    pub stage: ActivityStage,
    pub summary: String,
    pub artifact_refs: Vec<String>,
    pub updated_at: i64,
}

//! Versioned, deliberately small input/output contract for delegated work.
//!
//! This is a data boundary, not an agent runtime.  In particular it does not
//! construct prompts, execute tools, or materialize artifacts.  Its job is to
//! make the data which may cross the main-agent/child-agent boundary explicit
//! and reject the common ways that boundary can accidentally become a second
//! copy of the entire conversation.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub const CONTRACT_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationIdentity {
    pub session_id: String,
    pub message_id: String,
    pub parent_run_id: String,
    pub delegation_id: String,
    pub attempt_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextItem {
    /// An allowlisted, stable name such as `user_preference` or
    /// `prior_verified_fact`; it is not a transcript role.
    pub kind: String,
    pub value: String,
    pub evidence_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationBrief {
    pub schema_version: u16,
    pub identity: DelegationIdentity,
    pub goal: String,
    pub background: Vec<ContextItem>,
    pub constraints: Vec<String>,
    pub allowed_references: Vec<String>,
    pub allowed_capabilities: Vec<String>,
    pub completion_criteria: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    InProgress,
    Completed,
    Blocked,
    NeedsDecision,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyFact {
    pub statement: String,
    pub confidence: Confidence,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Milestone {
    pub label: String,
    pub outcome: String,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Passed,
    Failed,
    NotRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationRecord {
    pub item: String,
    pub method: String,
    pub status: VerificationStatus,
    pub conclusion: String,
    pub evidence_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateArtifact {
    pub kind: String,
    pub relative_ref: String,
    pub description: String,
    pub evidence_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenQuestion {
    pub question: String,
    pub options: Vec<String>,
    pub risk: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageFlags {
    pub context_truncated: bool,
    pub output_truncated: bool,
    pub budget_exhausted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationDelivery {
    pub schema_version: u16,
    /// Stable client-generated ID. It is the idempotency key component used by
    /// persistence when the parent accepts this particular delivery revision.
    pub delivery_id: String,
    pub delivery_revision: u32,
    pub identity: DelegationIdentity,
    pub status: DeliveryStatus,
    pub executive_summary: String,
    pub key_facts: Vec<KeyFact>,
    pub milestones: Vec<Milestone>,
    pub verifications: Vec<VerificationRecord>,
    pub candidate_artifacts: Vec<CandidateArtifact>,
    pub open_questions: Vec<OpenQuestion>,
    pub risks: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub usage: UsageFlags,
}

/// Every operational size/count threshold is supplied by policy.  This keeps
/// product decisions visible and testable rather than hidden in prompt text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractLimits {
    pub max_text_bytes: usize,
    pub max_goal_bytes: usize,
    pub max_summary_bytes: usize,
    pub max_background_items: usize,
    pub max_constraints: usize,
    pub max_references: usize,
    pub max_capabilities: usize,
    pub max_completion_criteria: usize,
    pub max_key_facts: usize,
    pub max_milestones: usize,
    pub max_verifications: usize,
    pub max_artifacts: usize,
    pub max_open_questions: usize,
    pub max_options_per_question: usize,
    pub max_risks: usize,
    pub max_evidence_refs: usize,
    pub require_completed_verification: bool,
}

impl Default for ContractLimits {
    fn default() -> Self {
        Self {
            max_text_bytes: 1_200,
            max_goal_bytes: 600,
            max_summary_bytes: 600,
            max_background_items: 12,
            max_constraints: 12,
            max_references: 24,
            max_capabilities: 16,
            max_completion_criteria: 12,
            max_key_facts: 12,
            max_milestones: 12,
            max_verifications: 12,
            max_artifacts: 12,
            max_open_questions: 3,
            max_options_per_question: 4,
            max_risks: 8,
            max_evidence_refs: 32,
            require_completed_verification: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractError {
    UnsupportedSchemaVersion(u16),
    IdentityMismatch,
    InvalidIdentity,
    InvalidReference(String),
    InvalidRelativeArtifact(String),
    ForbiddenContent(String),
    EmptyField(&'static str),
    LimitExceeded(&'static str),
    InvalidStatusTransition,
    IncompleteCompletedDelivery,
    AlreadyAccepted(AcceptanceKey),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AcceptanceKey {
    pub delegation_id: String,
    pub attempt_id: String,
    pub delivery_id: String,
    pub delivery_revision: u32,
}

impl DelegationDelivery {
    pub fn acceptance_key(&self) -> AcceptanceKey {
        AcceptanceKey {
            delegation_id: self.identity.delegation_id.clone(),
            attempt_id: self.identity.attempt_id.clone(),
            delivery_id: self.delivery_id.clone(),
            delivery_revision: self.delivery_revision,
        }
    }
}

/// A narrow, in-memory exactly-once guard. Durable storage must enforce the
/// same key uniqueness transactionally; this lets its adapter reject duplicate
/// acceptance before it reaches a user-visible completion transition.
#[derive(Debug, Default)]
pub struct AcceptanceGate {
    accepted: BTreeSet<AcceptanceKey>,
}
impl AcceptanceGate {
    pub fn accept_ready(
        &mut self,
        delivery: &DelegationDelivery,
        brief: &DelegationBrief,
        limits: &ContractLimits,
    ) -> Result<AcceptanceKey, ContractError> {
        validate_delivery(delivery, brief, None, limits)?;
        if delivery.status != DeliveryStatus::Completed {
            return Err(ContractError::IncompleteCompletedDelivery);
        }
        let key = delivery.acceptance_key();
        if !self.accepted.insert(key.clone()) {
            return Err(ContractError::AlreadyAccepted(key));
        }
        Ok(key)
    }
}

pub fn validate_brief(
    brief: &DelegationBrief,
    limits: &ContractLimits,
) -> Result<(), ContractError> {
    version(brief.schema_version)?;
    identity(&brief.identity)?;
    text("goal", &brief.goal, limits.max_goal_bytes)?;
    count(
        "background",
        brief.background.len(),
        limits.max_background_items,
    )?;
    count(
        "constraints",
        brief.constraints.len(),
        limits.max_constraints,
    )?;
    count(
        "allowed_references",
        brief.allowed_references.len(),
        limits.max_references,
    )?;
    count(
        "allowed_capabilities",
        brief.allowed_capabilities.len(),
        limits.max_capabilities,
    )?;
    count(
        "completion_criteria",
        brief.completion_criteria.len(),
        limits.max_completion_criteria,
    )?;
    if brief.completion_criteria.is_empty() {
        return Err(ContractError::EmptyField("completion_criteria"));
    }
    for item in &brief.background {
        text("background.kind", &item.kind, limits.max_text_bytes)?;
        text("background.value", &item.value, limits.max_text_bytes)?;
        optional_ref(&item.evidence_ref, limits)?;
    }
    for value in brief
        .constraints
        .iter()
        .chain(brief.completion_criteria.iter())
        .chain(brief.allowed_capabilities.iter())
    {
        text("brief field", value, limits.max_text_bytes)?;
    }
    bounded_refs(
        "allowed_references",
        &brief.allowed_references,
        limits.max_references,
        limits,
    )
}

pub fn validate_delivery(
    delivery: &DelegationDelivery,
    brief: &DelegationBrief,
    previous: Option<&DelegationDelivery>,
    limits: &ContractLimits,
) -> Result<(), ContractError> {
    validate_brief(brief, limits)?;
    version(delivery.schema_version)?;
    if delivery.identity != brief.identity {
        return Err(ContractError::IdentityMismatch);
    }
    identity(&delivery.identity)?;
    text("delivery_id", &delivery.delivery_id, limits.max_text_bytes)?;
    if delivery.delivery_revision == 0 {
        return Err(ContractError::EmptyField("delivery_revision"));
    }
    if let Some(old) = previous {
        if old.identity != delivery.identity
            || !may_transition(old.status, delivery.status)
            || delivery.delivery_revision <= old.delivery_revision
        {
            return Err(ContractError::InvalidStatusTransition);
        }
    }
    text(
        "executive_summary",
        &delivery.executive_summary,
        limits.max_summary_bytes,
    )?;
    count("key_facts", delivery.key_facts.len(), limits.max_key_facts)?;
    count(
        "milestones",
        delivery.milestones.len(),
        limits.max_milestones,
    )?;
    count(
        "verifications",
        delivery.verifications.len(),
        limits.max_verifications,
    )?;
    count(
        "candidate_artifacts",
        delivery.candidate_artifacts.len(),
        limits.max_artifacts,
    )?;
    count(
        "open_questions",
        delivery.open_questions.len(),
        limits.max_open_questions,
    )?;
    count("risks", delivery.risks.len(), limits.max_risks)?;
    refs(&delivery.evidence_refs, limits)?;
    for f in &delivery.key_facts {
        text("fact", &f.statement, limits.max_text_bytes)?;
        refs(&f.evidence_refs, limits)?;
    }
    for m in &delivery.milestones {
        text("milestone.label", &m.label, limits.max_text_bytes)?;
        text("milestone.outcome", &m.outcome, limits.max_text_bytes)?;
        refs(&m.evidence_refs, limits)?;
    }
    for v in &delivery.verifications {
        text("verification.item", &v.item, limits.max_text_bytes)?;
        text("verification.method", &v.method, limits.max_text_bytes)?;
        text(
            "verification.conclusion",
            &v.conclusion,
            limits.max_text_bytes,
        )?;
        reference(&v.evidence_ref, limits)?;
    }
    for a in &delivery.candidate_artifacts {
        text("artifact.kind", &a.kind, limits.max_text_bytes)?;
        text(
            "artifact.description",
            &a.description,
            limits.max_text_bytes,
        )?;
        relative_ref(&a.relative_ref)?;
        optional_ref(&a.evidence_ref, limits)?;
    }
    for q in &delivery.open_questions {
        text("question", &q.question, limits.max_text_bytes)?;
        text("question.risk", &q.risk, limits.max_text_bytes)?;
        count(
            "question.options",
            q.options.len(),
            limits.max_options_per_question,
        )?;
        for o in &q.options {
            text("question.option", o, limits.max_text_bytes)?;
        }
    }
    for risk in &delivery.risks {
        text("risk", risk, limits.max_text_bytes)?;
    }
    if delivery.status == DeliveryStatus::Completed
        && (delivery.executive_summary.trim().is_empty()
            || delivery.key_facts.is_empty()
            || !delivery.open_questions.is_empty()
            || (limits.require_completed_verification
                && !delivery
                    .verifications
                    .iter()
                    .any(|v| v.status == VerificationStatus::Passed)))
    {
        return Err(ContractError::IncompleteCompletedDelivery);
    }
    Ok(())
}

/// The only context surface for the main agent.  Raw outputs, transcripts and
/// model reasoning never exist in the contract and therefore cannot leak here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParentContext {
    pub identity: DelegationIdentity,
    pub status: DeliveryStatus,
    pub summary: String,
    pub facts: Vec<KeyFact>,
    pub milestones: Vec<Milestone>,
    pub verifications: Vec<VerificationRecord>,
    pub artifacts: Vec<CandidateArtifact>,
    pub open_questions: Vec<OpenQuestion>,
    pub risks: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub usage: UsageFlags,
}
pub struct ParentContextAssembler;
impl ParentContextAssembler {
    pub fn assemble(
        delivery: &DelegationDelivery,
        brief: &DelegationBrief,
        limits: &ContractLimits,
    ) -> Result<ParentContext, ContractError> {
        validate_delivery(delivery, brief, None, limits)?;
        Ok(ParentContext {
            identity: delivery.identity.clone(),
            status: delivery.status,
            summary: delivery.executive_summary.clone(),
            facts: delivery.key_facts.clone(),
            milestones: delivery.milestones.clone(),
            verifications: delivery.verifications.clone(),
            artifacts: delivery.candidate_artifacts.clone(),
            open_questions: delivery.open_questions.clone(),
            risks: delivery.risks.clone(),
            evidence_refs: delivery.evidence_refs.clone(),
            usage: delivery.usage.clone(),
        })
    }
}

fn may_transition(from: DeliveryStatus, to: DeliveryStatus) -> bool {
    matches!(
        (from, to),
        (
            DeliveryStatus::InProgress,
            DeliveryStatus::InProgress
                | DeliveryStatus::Completed
                | DeliveryStatus::Blocked
                | DeliveryStatus::NeedsDecision
                | DeliveryStatus::Failed
                | DeliveryStatus::Cancelled
        ) | (
            DeliveryStatus::Blocked | DeliveryStatus::NeedsDecision,
            DeliveryStatus::InProgress | DeliveryStatus::Cancelled
        )
    )
}
fn version(value: u16) -> Result<(), ContractError> {
    if value == CONTRACT_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(ContractError::UnsupportedSchemaVersion(value))
    }
}
fn identity(value: &DelegationIdentity) -> Result<(), ContractError> {
    for v in [
        &value.session_id,
        &value.message_id,
        &value.parent_run_id,
        &value.delegation_id,
        &value.attempt_id,
    ] {
        if v.is_empty()
            || v.len() > 128
            || !v
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(ContractError::InvalidIdentity);
        }
    }
    Ok(())
}
fn count(name: &'static str, actual: usize, max: usize) -> Result<(), ContractError> {
    if actual > max {
        Err(ContractError::LimitExceeded(name))
    } else {
        Ok(())
    }
}
fn text(name: &'static str, value: &str, max: usize) -> Result<(), ContractError> {
    if value.trim().is_empty() {
        return Err(ContractError::EmptyField(name));
    }
    if value.len() > max {
        return Err(ContractError::LimitExceeded(name));
    }
    let lower = value.to_ascii_lowercase();
    if value.contains('\0')
        || lower.contains("raw conversation")
        || lower.contains("model reasoning")
        || lower.contains("tool output")
        || lower.contains("authorization:")
        || lower.contains("api_key")
        || lower.contains("secret=")
        || lower.contains("sk-")
        || has_absolute_path(value)
    {
        return Err(ContractError::ForbiddenContent(name.into()));
    }
    Ok(())
}
fn has_absolute_path(value: &str) -> bool {
    value.contains("\\\\")
        || value.contains("/") && (value.starts_with('/') || value.contains(" /"))
        || value
            .as_bytes()
            .windows(3)
            .any(|w| w[0].is_ascii_alphabetic() && w[1] == b':' && (w[2] == b'\\' || w[2] == b'/'))
}
fn reference(value: &str, limits: &ContractLimits) -> Result<(), ContractError> {
    // A URI necessarily contains `scheme:/`, which is not a filesystem path.
    // Validate its text independently instead of passing it through the path
    // detector used for prose fields.
    if value.trim().is_empty() {
        return Err(ContractError::EmptyField("evidence_ref"));
    }
    if value.len() > limits.max_text_bytes {
        return Err(ContractError::LimitExceeded("evidence_ref"));
    }
    let lower = value.to_ascii_lowercase();
    if value.contains('\0')
        || value.contains(char::is_whitespace)
        || lower.contains("authorization:")
        || lower.contains("api_key")
        || lower.contains("secret=")
        || lower.contains("sk-")
        || !value.contains("://")
    {
        return Err(ContractError::InvalidReference(value.into()));
    }
    Ok(())
}
fn optional_ref(value: &Option<String>, limits: &ContractLimits) -> Result<(), ContractError> {
    if let Some(v) = value {
        reference(v, limits)
    } else {
        Ok(())
    }
}
fn refs(values: &[String], limits: &ContractLimits) -> Result<(), ContractError> {
    bounded_refs("evidence_refs", values, limits.max_evidence_refs, limits)
}
fn bounded_refs(
    name: &'static str,
    values: &[String],
    max: usize,
    limits: &ContractLimits,
) -> Result<(), ContractError> {
    count(name, values.len(), max)?;
    for value in values {
        reference(value, limits)?;
    }
    Ok(())
}
fn relative_ref(value: &str) -> Result<(), ContractError> {
    if value.is_empty()
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.contains("..")
        || value
            .as_bytes()
            .windows(3)
            .any(|w| w[0].is_ascii_alphabetic() && w[1] == b':' && (w[2] == b'\\' || w[2] == b'/'))
    {
        Err(ContractError::InvalidRelativeArtifact(value.into()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id() -> DelegationIdentity {
        DelegationIdentity {
            session_id: "s1".into(),
            message_id: "m1".into(),
            parent_run_id: "run1".into(),
            delegation_id: "d1".into(),
            attempt_id: "a1".into(),
        }
    }
    fn brief() -> DelegationBrief {
        DelegationBrief {
            schema_version: 1,
            identity: id(),
            goal: "Validate a contract module".into(),
            background: vec![ContextItem {
                kind: "prior_verified_fact".into(),
                value: "The project is Rust.".into(),
                evidence_ref: Some("record://fact/1".into()),
            }],
            constraints: vec!["Do not modify runtime wiring.".into()],
            allowed_references: vec!["record://fact/1".into()],
            allowed_capabilities: vec!["read_workspace".into()],
            completion_criteria: vec!["Return schema-valid delivery.".into()],
        }
    }
    fn delivery() -> DelegationDelivery {
        DelegationDelivery {
            schema_version: 1,
            delivery_id: "delivery1".into(),
            delivery_revision: 1,
            identity: id(),
            status: DeliveryStatus::Completed,
            executive_summary: "Contract validation and tests completed.".into(),
            key_facts: vec![KeyFact {
                statement: "The contract is versioned.".into(),
                confidence: Confidence::High,
                evidence_refs: vec!["record://fact/1".into()],
            }],
            milestones: vec![Milestone {
                label: "Validation".into(),
                outcome: "Implemented structured checks.".into(),
                evidence_refs: vec!["record://milestone/1".into()],
            }],
            verifications: vec![VerificationRecord {
                item: "Targeted tests".into(),
                method: "cargo test module".into(),
                status: VerificationStatus::Passed,
                conclusion: "Tests passed.".into(),
                evidence_ref: "record://verification/1".into(),
            }],
            candidate_artifacts: vec![CandidateArtifact {
                kind: "source".into(),
                relative_ref: "src/agent/delegation_contract.rs".into(),
                description: "Contract module.".into(),
                evidence_ref: None,
            }],
            open_questions: vec![],
            risks: vec![],
            evidence_refs: vec!["record://delivery/1".into()],
            usage: UsageFlags {
                context_truncated: false,
                output_truncated: false,
                budget_exhausted: false,
            },
        }
    }
    #[test]
    fn valid_contract_assembles_only_allowlisted_parent_context() {
        let limits = ContractLimits::default();
        let b = brief();
        let d = delivery();
        validate_brief(&b, &limits).unwrap();
        validate_delivery(&d, &b, None, &limits).unwrap();
        let context = ParentContextAssembler::assemble(&d, &b, &limits).unwrap();
        assert_eq!(context.summary, d.executive_summary);
        assert_eq!(
            context.artifacts[0].relative_ref,
            "src/agent/delegation_contract.rs"
        );
    }
    #[test]
    fn rejects_schema_identity_secret_and_absolute_path_leaks() {
        let limits = ContractLimits::default();
        let b = brief();
        let mut d = delivery();
        d.schema_version = 2;
        assert!(matches!(
            validate_delivery(&d, &b, None, &limits),
            Err(ContractError::UnsupportedSchemaVersion(2))
        ));
        d.schema_version = 1;
        d.identity.attempt_id = "a2".into();
        assert_eq!(
            validate_delivery(&d, &b, None, &limits),
            Err(ContractError::IdentityMismatch)
        );
        d.identity = id();
        d.executive_summary = "Raw conversation follows: user said hello".into();
        assert!(matches!(
            validate_delivery(&d, &b, None, &limits),
            Err(ContractError::ForbiddenContent(_))
        ));
        d.executive_summary = "api_key=not-permitted".into();
        assert!(matches!(
            validate_delivery(&d, &b, None, &limits),
            Err(ContractError::ForbiddenContent(_))
        ));
        d.executive_summary = delivery().executive_summary;
        d.candidate_artifacts[0].relative_ref = "C:\\outside\\x".into();
        assert!(matches!(
            validate_delivery(&d, &b, None, &limits),
            Err(ContractError::InvalidRelativeArtifact(_))
        ));
    }
    #[test]
    fn enforces_configured_limits_transitions_and_completed_evidence() {
        let mut limits = ContractLimits::default();
        limits.max_key_facts = 0;
        assert_eq!(
            validate_delivery(&delivery(), &brief(), None, &limits),
            Err(ContractError::LimitExceeded("key_facts"))
        );
        let limits = ContractLimits::default();
        let old = DelegationDelivery {
            status: DeliveryStatus::Completed,
            ..delivery()
        };
        let mut next = delivery();
        next.delivery_revision = 2;
        assert_eq!(
            validate_delivery(&next, &brief(), Some(&old), &limits),
            Err(ContractError::InvalidStatusTransition)
        );
        let mut incomplete = delivery();
        incomplete.verifications.clear();
        assert_eq!(
            validate_delivery(&incomplete, &brief(), None, &limits),
            Err(ContractError::IncompleteCompletedDelivery)
        );
    }
    #[test]
    fn acceptance_is_ready_once_and_uses_stable_key() {
        let mut gate = AcceptanceGate::default();
        let b = brief();
        let d = delivery();
        let key = gate
            .accept_ready(&d, &b, &ContractLimits::default())
            .unwrap();
        assert_eq!(key.delivery_id, "delivery1");
        assert!(matches!(
            gate.accept_ready(&d, &b, &ContractLimits::default()),
            Err(ContractError::AlreadyAccepted(_))
        ));
    }
}

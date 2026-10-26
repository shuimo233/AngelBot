//! Pure semantic-review contracts shared by the coordinator and adapters.
//!
//! This layer deliberately contains no database, model, filesystem, or
//! delegation-runtime dependency.  It describes only the bounded snapshot a
//! Reviewer receives and the bounded result it returns.

use serde::{Deserialize, Serialize};

use super::delegation_contract::{
    validate_candidate_artifact, validate_file_read_evidence, CandidateArtifact, ContractLimits,
    DelegationDelivery, FileReadEvidence,
};

const MAX_TEXT: usize = 8 * 1024;
const MAX_ITEMS: usize = 32;
const MAX_REFS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticReviewVerdict {
    Passed,
    Failed,
    NeedsDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewSubject {
    pub delivery_id: String,
    pub implementation_attempt_id: String,
    pub task_shape: String,
    pub worker_profile: String,
    pub goal: String,
    pub summary: String,
    pub key_facts: Vec<String>,
    pub milestones: Vec<String>,
    pub verifications: Vec<String>,
    pub artifact_refs: Vec<String>,
    /// Preserve declared output identity/description, not just generic evidence
    /// URIs. Older queued subjects remain readable but lack this evidence.
    #[serde(default)]
    pub candidate_artifacts: Vec<CandidateArtifact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_read_evidence: Vec<FileReadEvidence>,
    pub risks: Vec<String>,
    pub open_questions: Vec<String>,
}

impl ReviewSubject {
    /// Build the reviewer input from the already validated delivery.  The
    /// caller supplies the durable package policy labels; no raw transcript,
    /// tool log, path, or model reasoning is admitted here.
    pub fn from_delivery(
        delivery: &DelegationDelivery,
        goal: impl Into<String>,
        task_shape: impl Into<String>,
        worker_profile: impl Into<String>,
    ) -> Self {
        Self {
            delivery_id: delivery.delivery_id.clone(),
            implementation_attempt_id: delivery.identity.attempt_id.clone(),
            task_shape: task_shape.into(),
            worker_profile: worker_profile.into(),
            goal: goal.into(),
            summary: delivery.executive_summary.clone(),
            key_facts: delivery
                .key_facts
                .iter()
                .map(|fact| fact.statement.clone())
                .collect(),
            milestones: delivery
                .milestones
                .iter()
                .map(|milestone| format!("{}: {}", milestone.label, milestone.outcome))
                .collect(),
            verifications: delivery
                .verifications
                .iter()
                .map(|verification| {
                    format!(
                        "{} [{}; status={:?}]: {}",
                        verification.item,
                        verification.method,
                        verification.status,
                        verification.conclusion
                    )
                })
                .collect(),
            artifact_refs: delivery
                .candidate_artifacts
                .iter()
                .filter_map(|artifact| artifact.evidence_ref.clone())
                .chain(delivery.evidence_refs.iter().cloned())
                .collect(),
            candidate_artifacts: delivery.candidate_artifacts.clone(),
            file_read_evidence: delivery.file_read_evidence.clone(),
            risks: delivery.risks.clone(),
            open_questions: delivery
                .open_questions
                .iter()
                .map(|question| question.question.clone())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewOutcome {
    pub verdict: SemanticReviewVerdict,
    pub summary: String,
    pub findings: Vec<String>,
    pub missing_evidence: Vec<String>,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ReviewContractError {
    #[error("review contract contains invalid text")]
    InvalidText,
    #[error("review contract exceeds bounded item limits")]
    TooManyItems,
    #[error("review evidence reference is invalid")]
    InvalidReference,
    #[error("review candidate artifact is invalid")]
    InvalidArtifact,
}

impl ReviewSubject {
    pub fn validate(&self) -> Result<(), ReviewContractError> {
        for value in [
            &self.delivery_id,
            &self.implementation_attempt_id,
            &self.task_shape,
            &self.worker_profile,
            &self.goal,
            &self.summary,
        ] {
            validate_text(value)?;
        }
        for values in [
            &self.key_facts,
            &self.milestones,
            &self.verifications,
            &self.artifact_refs,
            &self.risks,
            &self.open_questions,
        ] {
            validate_items(values)?;
        }
        for reference in &self.artifact_refs {
            validate_reference(reference)?;
        }
        let limits = ContractLimits::default();
        if self.candidate_artifacts.len() > limits.max_artifacts {
            return Err(ReviewContractError::TooManyItems);
        }
        for artifact in &self.candidate_artifacts {
            validate_candidate_artifact(artifact, &limits)
                .map_err(|_| ReviewContractError::InvalidArtifact)?;
        }
        if self.file_read_evidence.len() > limits.max_verifications {
            return Err(ReviewContractError::TooManyItems);
        }
        for evidence in &self.file_read_evidence {
            validate_file_read_evidence(evidence)
                .map_err(|_| ReviewContractError::InvalidReference)?;
        }
        Ok(())
    }
}

impl ReviewOutcome {
    /// Keep executor diagnostics valid even when a provider returns empty,
    /// multiline, or oversized text. Failure never becomes a passing verdict.
    pub fn execution_failed(reason: &str) -> Self {
        let diagnostic: String = reason
            .chars()
            .take(512)
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let diagnostic = diagnostic.split_whitespace().collect::<Vec<_>>().join(" ");
        Self {
            verdict: SemanticReviewVerdict::NeedsDecision,
            summary: "reviewer execution did not produce a valid verdict".into(),
            findings: if diagnostic.is_empty() {
                vec![]
            } else {
                vec![diagnostic]
            },
            missing_evidence: vec!["reviewer_outcome".into()],
            evidence_refs: vec![],
        }
    }

    pub fn validate(&self) -> Result<(), ReviewContractError> {
        validate_text(&self.summary)?;
        for values in [&self.findings, &self.missing_evidence] {
            validate_items(values)?;
        }
        if self.evidence_refs.len() > MAX_REFS {
            return Err(ReviewContractError::TooManyItems);
        }
        for reference in &self.evidence_refs {
            validate_reference(reference)?;
        }
        Ok(())
    }
}

fn validate_items(values: &[String]) -> Result<(), ReviewContractError> {
    if values.len() > MAX_ITEMS {
        return Err(ReviewContractError::TooManyItems);
    }
    values.iter().try_for_each(|value| validate_text(value))
}

fn validate_text(value: &str) -> Result<(), ReviewContractError> {
    if value.trim().is_empty() || value.len() > MAX_TEXT || value.contains(['\0', '\r', '\n']) {
        return Err(ReviewContractError::InvalidText);
    }
    Ok(())
}

fn validate_reference(value: &str) -> Result<(), ReviewContractError> {
    validate_text(value)?;
    if !value.contains("://") || value.contains(['\\', '\0']) {
        return Err(ReviewContractError::InvalidReference);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject() -> ReviewSubject {
        ReviewSubject {
            delivery_id: "delivery-1".into(),
            implementation_attempt_id: "attempt-1".into(),
            task_shape: "explore".into(),
            worker_profile: "explorer".into(),
            goal: "inspect the project".into(),
            summary: "bounded result".into(),
            key_facts: vec!["fact".into()],
            milestones: vec!["done".into()],
            verifications: vec!["checked".into()],
            artifact_refs: vec!["evidence://one".into()],
            candidate_artifacts: vec![],
            file_read_evidence: vec![],
            risks: vec![],
            open_questions: vec![],
        }
    }

    #[test]
    fn subject_rejects_unbounded_or_unstructured_values() {
        let mut value = subject();
        value.summary = "x".repeat(MAX_TEXT + 1);
        assert_eq!(value.validate(), Err(ReviewContractError::InvalidText));
        let mut value = subject();
        value.artifact_refs = vec!["C:\\secret.txt".into()];
        assert_eq!(value.validate(), Err(ReviewContractError::InvalidReference));
        let mut value = subject();
        value.candidate_artifacts = vec![CandidateArtifact {
            kind: "file".into(),
            relative_ref: "output/result.txt".into(),
            description: "The declared output".into(),
            evidence_ref: Some("evidence://one".into()),
        }];
        assert!(value.validate().is_ok());
        value.file_read_evidence = vec![FileReadEvidence {
            evidence_ref: format!("evidence://file-read/{}", uuid::Uuid::new_v4()),
            relative_ref: "output/result.txt".into(),
            content_bytes: 3,
            sha256: "a".repeat(64),
        }];
        assert!(value.validate().is_ok());
        let mut legacy = serde_json::to_value(&value).unwrap();
        legacy.as_object_mut().unwrap().remove("file_read_evidence");
        legacy
            .as_object_mut()
            .unwrap()
            .remove("candidate_artifacts");
        let legacy: ReviewSubject = serde_json::from_value(legacy).unwrap();
        assert!(
            legacy.candidate_artifacts.is_empty(),
            "old queued reviews must remain readable without inventing missing evidence"
        );
        assert!(legacy.validate().is_ok());
        assert!(legacy.file_read_evidence.is_empty());
        value.file_read_evidence[0].sha256 = "z".repeat(64);
        assert_eq!(value.validate(), Err(ReviewContractError::InvalidReference));
        value.file_read_evidence.clear();
        for path in ["../escape", "C:\\private.txt", "/private.txt"] {
            value.candidate_artifacts[0].relative_ref = path.into();
            assert_eq!(value.validate(), Err(ReviewContractError::InvalidArtifact));
        }
    }

    #[test]
    fn outcome_requires_bounded_evidence_references() {
        let outcome = ReviewOutcome {
            verdict: SemanticReviewVerdict::Passed,
            summary: "ok".into(),
            findings: vec![],
            missing_evidence: vec![],
            evidence_refs: vec!["evidence://review/1".into()],
        };
        assert!(outcome.validate().is_ok());
        for reason in [
            "",
            " \r\n\0 ",
            "provider\r\nrequest\0failed",
            &"错".repeat(MAX_TEXT),
        ] {
            let failed = ReviewOutcome::execution_failed(reason);
            assert!(failed.validate().is_ok());
            assert_eq!(failed.verdict, SemanticReviewVerdict::NeedsDecision);
            assert_eq!(failed.missing_evidence, vec!["reviewer_outcome"]);
        }
    }
}

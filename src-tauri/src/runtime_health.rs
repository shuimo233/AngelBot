use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeIssueCode {
    EphemeralStorage,
    SemanticMemoryUnavailable,
    SettingsDefaults,
    DelegationDisabled,
    DelegationUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeIssueSeverity {
    Notice,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeRecoveryAction {
    None,
    Restart,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeIssue {
    pub code: RuntimeIssueCode,
    pub severity: RuntimeIssueSeverity,
    pub title: String,
    pub detail: String,
    pub recovery_action: RuntimeRecoveryAction,
}

impl RuntimeIssue {
    pub fn ephemeral_storage() -> Self {
        Self {
            code: RuntimeIssueCode::EphemeralStorage,
            severity: RuntimeIssueSeverity::Critical,
            title: "本地数据未连接".into(),
            detail: "当前对话可以继续，但重启前的新内容不会保存。".into(),
            recovery_action: RuntimeRecoveryAction::Restart,
        }
    }

    pub fn semantic_memory_unavailable() -> Self {
        Self {
            code: RuntimeIssueCode::SemanticMemoryUnavailable,
            severity: RuntimeIssueSeverity::Warning,
            title: "语义记忆暂不可用".into(),
            detail: "普通对话和项目操作不受影响，相关记忆能力会在重启后重新初始化。".into(),
            recovery_action: RuntimeRecoveryAction::Restart,
        }
    }

    pub fn settings_defaults() -> Self {
        Self {
            code: RuntimeIssueCode::SettingsDefaults,
            severity: RuntimeIssueSeverity::Warning,
            title: "部分设置未能载入".into(),
            detail: "当前使用安全默认值，本次修改设置前请先重启 AngelBot。".into(),
            recovery_action: RuntimeRecoveryAction::Restart,
        }
    }

    pub fn delegation_disabled() -> Self {
        Self {
            code: RuntimeIssueCode::DelegationDisabled,
            severity: RuntimeIssueSeverity::Notice,
            title: "子代理已按当前配置停用".into(),
            detail: "主 Agent 仍可独立完成对话与工具任务。".into(),
            recovery_action: RuntimeRecoveryAction::None,
        }
    }

    pub fn delegation_unavailable(detail: impl Into<String>) -> Self {
        Self {
            code: RuntimeIssueCode::DelegationUnavailable,
            severity: RuntimeIssueSeverity::Warning,
            title: "子代理暂不可用".into(),
            detail: detail.into(),
            recovery_action: RuntimeRecoveryAction::Restart,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeHealthState {
    issues: Vec<RuntimeIssue>,
}

impl RuntimeHealthState {
    pub fn add(&mut self, issue: RuntimeIssue) {
        if let Some(existing) = self
            .issues
            .iter_mut()
            .find(|existing| existing.code == issue.code)
        {
            *existing = issue;
        } else {
            self.issues.push(issue);
        }
    }

    pub fn snapshot(&self, delegation_available: bool) -> RuntimeHealthSnapshot {
        let status = if self.issues.iter().any(|issue| {
            matches!(
                issue.severity,
                RuntimeIssueSeverity::Warning | RuntimeIssueSeverity::Critical
            )
        }) {
            RuntimeHealthStatus::Degraded
        } else {
            RuntimeHealthStatus::Ready
        };
        RuntimeHealthSnapshot {
            status,
            delegation_available,
            issues: self.issues.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeHealthStatus {
    Ready,
    Degraded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeHealthSnapshot {
    pub status: RuntimeHealthStatus,
    pub delegation_available: bool,
    pub issues: Vec<RuntimeIssue>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_do_not_turn_an_intentional_configuration_into_degraded_health() {
        let mut state = RuntimeHealthState::default();
        state.add(RuntimeIssue::delegation_disabled());

        let snapshot = state.snapshot(false);
        assert_eq!(snapshot.status, RuntimeHealthStatus::Ready);
        assert_eq!(snapshot.issues.len(), 1);
        assert_eq!(snapshot.issues[0].severity, RuntimeIssueSeverity::Notice);
    }

    #[test]
    fn warnings_degrade_health_and_duplicate_codes_are_replaced() {
        let mut state = RuntimeHealthState::default();
        state.add(RuntimeIssue::delegation_unavailable("first"));
        state.add(RuntimeIssue::delegation_unavailable("second"));

        let snapshot = state.snapshot(false);
        assert_eq!(snapshot.status, RuntimeHealthStatus::Degraded);
        assert_eq!(snapshot.issues.len(), 1);
        assert_eq!(snapshot.issues[0].detail, "second");
    }
}

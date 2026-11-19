use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeIssueCode {
    EphemeralStorage,
    SemanticMemoryUnavailable,
    SettingsDefaults,
    DelegationDisabled,
    DelegationUnavailable,
    StorageMigration,
    AutomationSchedulerUnavailable,
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
    pub fn automation_scheduler_unavailable() -> Self {
        Self {
            code: RuntimeIssueCode::AutomationSchedulerUnavailable,
            severity: RuntimeIssueSeverity::Warning,
            title: "定时调度未启动".into(),
            detail: "已启用的定时和登录触发本次不会运行。请保存内容，从托盘退出 AngelBot，再从开始菜单手动打开。本次登录触发不会补跑；结果未知的旧任务请先核对，不要直接重试。".into(),
            // Tauri's in-app relaunch preserves --angelbot-autostart, which is
            // for a Windows login, not for recovering this session.
            recovery_action: RuntimeRecoveryAction::None,
        }
    }

    pub fn storage_migration(recovery: bool) -> Self {
        Self {
            code: RuntimeIssueCode::StorageMigration,
            severity: RuntimeIssueSeverity::Notice,
            title: if recovery { "资料目录迁移待处理" } else { "本地资料目录已统一" }.into(),
            detail: if recovery { "当前继续使用原目录保存资料。请先备份、整理遗留任务现场，再重试迁移；没有自动合并新旧目录。" } else { "迁移前资料已保留。浏览器工具需在设置中重新连接；检测到旧目录参数的自定义服务已停用。请另行核对自定义命令、环境变量及脚本中的路径。没有自动启动服务。" }.into(),
            recovery_action: RuntimeRecoveryAction::None,
        }
    }
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
    fn storage_recovery_is_informational_without_hiding_real_storage_failure() {
        let mut state = RuntimeHealthState::default();
        state.add(RuntimeIssue::storage_migration(true));
        let snapshot = state.snapshot(true);
        assert_eq!(snapshot.status, RuntimeHealthStatus::Ready);
        assert_eq!(snapshot.issues[0].severity, RuntimeIssueSeverity::Notice);
        assert_eq!(snapshot.issues[0].title, "资料目录迁移待处理");
        assert_eq!(
            snapshot.issues[0].recovery_action,
            RuntimeRecoveryAction::None
        );

        state.add(RuntimeIssue::ephemeral_storage());
        let failed = state.snapshot(true);
        assert_eq!(failed.status, RuntimeHealthStatus::Degraded);
        assert!(failed
            .issues
            .iter()
            .any(|issue| issue.severity == RuntimeIssueSeverity::Critical));
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

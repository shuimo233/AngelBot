//! Explicit opt-in and fail-closed readiness gate for delegated Explorer.
//!
//! Explorer is not enabled merely because its individual ports compile. The
//! application composition root supplies the deployment default, while an
//! explicit environment value or persisted setting can override it. Every
//! required dependency must still be constructed before a real launcher can
//! be installed. This module contains only deterministic configuration logic;
//! composition code supplies readiness facts and performs construction.

/// Environment variable used for an explicit deployment opt-in.
pub const ENABLE_ENV: &str = "ANGELBOT_ENABLE_DELEGATED_EXPLORER";

/// Persisted setting key used when the environment does not explicitly
/// override the deployment opt-in.  It is intentionally opt-in; an absent or
/// malformed value remains disabled.
pub const ENABLE_SETTING: &str = "delegated_explorer_enabled";

/// Accepted truthy spellings for the environment/settings opt-in.
const TRUE_VALUES: &[&str] = &["1", "true", "yes", "on", "enabled"];

/// Whether a user/deployment setting explicitly enables delegated Explorer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptInSource {
    Environment,
    Setting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExplorerOptIn {
    pub enabled: bool,
    pub source: Option<OptInSource>,
}

impl ExplorerOptIn {
    /// Resolve an opt-in without reading process-global state.  An explicitly
    /// present environment value has precedence over the persisted setting;
    /// absent values default to disabled.  Invalid values are disabled rather
    /// than guessed as truthy.
    pub fn from_sources(environment: Option<&str>, setting: Option<&str>) -> Self {
        if let Some(value) = environment {
            return Self {
                enabled: parse_bool(value),
                source: Some(OptInSource::Environment),
            };
        }
        if let Some(value) = setting {
            return Self {
                enabled: parse_bool(value),
                source: Some(OptInSource::Setting),
            };
        }
        Self {
            enabled: false,
            source: None,
        }
    }

    /// Resolve the deployment environment and leave database/settings access
    /// to the caller.  The explicit setting argument keeps startup testable and
    /// avoids mutating process-global environment variables in tests.
    pub fn from_env(setting: Option<&str>) -> Self {
        Self::from_sources(std::env::var(ENABLE_ENV).ok().as_deref(), setting)
    }
}

fn parse_bool(value: &str) -> bool {
    TRUE_VALUES
        .iter()
        .any(|accepted| value.trim().eq_ignore_ascii_case(accepted))
}

/// Readiness facts supplied by composition.  A value is `true` only after the
/// corresponding object was actually constructed and validated.  Keeping the
/// facts separate makes partial startup explicit and easy to test.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExplorerRuntimeDependencies {
    pub context_loader: bool,
    pub gateway: bool,
    pub source_policy: bool,
    pub transport: bool,
    pub cleanup: bool,
    pub launcher: bool,
}

impl ExplorerRuntimeDependencies {
    pub const fn all_ready() -> Self {
        Self {
            context_loader: true,
            gateway: true,
            source_policy: true,
            transport: true,
            cleanup: true,
            launcher: true,
        }
    }

    pub const fn is_ready(self) -> bool {
        self.context_loader
            && self.gateway
            && self.source_policy
            && self.transport
            && self.cleanup
            && self.launcher
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplorerRuntimeDisableReason {
    OptInRequired,
    DependenciesUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplorerRuntimeDecision {
    Disabled(ExplorerRuntimeDisableReason),
    Enabled,
}

/// Decide whether composition may install a real Explorer launcher.
///
/// This function never constructs a partial launcher.  Disabled is the only
/// result for either missing opt-in or an incomplete dependency set; callers
/// should install the existing fail-closed launcher in both cases.
pub fn decide(
    opt_in: ExplorerOptIn,
    dependencies: ExplorerRuntimeDependencies,
) -> ExplorerRuntimeDecision {
    if !opt_in.enabled {
        return ExplorerRuntimeDecision::Disabled(ExplorerRuntimeDisableReason::OptInRequired);
    }
    if !dependencies.is_ready() {
        return ExplorerRuntimeDecision::Disabled(
            ExplorerRuntimeDisableReason::DependenciesUnavailable,
        );
    }
    ExplorerRuntimeDecision::Enabled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled_without_an_explicit_opt_in() {
        let opt_in = ExplorerOptIn::from_sources(None, None);
        assert_eq!(
            opt_in,
            ExplorerOptIn {
                enabled: false,
                source: None
            }
        );
        assert_eq!(
            decide(opt_in, ExplorerRuntimeDependencies::all_ready()),
            ExplorerRuntimeDecision::Disabled(ExplorerRuntimeDisableReason::OptInRequired)
        );
    }

    #[test]
    fn environment_opt_in_takes_precedence_and_accepts_bounded_spellings() {
        for value in ["1", "true", "YES", "On", "enabled"] {
            let opt_in = ExplorerOptIn::from_sources(Some(value), Some("false"));
            assert_eq!(opt_in.enabled, true);
            assert_eq!(opt_in.source, Some(OptInSource::Environment));
        }
        let disabled = ExplorerOptIn::from_sources(Some("false"), Some("true"));
        assert_eq!(disabled.enabled, false);
        assert_eq!(disabled.source, Some(OptInSource::Environment));
    }

    #[test]
    fn setting_can_opt_in_when_environment_is_absent() {
        let opt_in = ExplorerOptIn::from_sources(None, Some(" true "));
        assert_eq!(
            opt_in,
            ExplorerOptIn {
                enabled: true,
                source: Some(OptInSource::Setting)
            }
        );
    }

    #[test]
    fn invalid_values_fail_closed() {
        let opt_in = ExplorerOptIn::from_sources(Some("maybe"), None);
        assert!(!opt_in.enabled);
        assert_eq!(
            decide(opt_in, ExplorerRuntimeDependencies::all_ready()),
            ExplorerRuntimeDecision::Disabled(ExplorerRuntimeDisableReason::OptInRequired)
        );
    }

    #[test]
    fn opt_in_with_any_missing_dependency_is_disabled() {
        let opt_in = ExplorerOptIn::from_sources(Some("1"), None);
        let mut dependencies = ExplorerRuntimeDependencies::all_ready();
        dependencies.transport = false;
        assert_eq!(
            decide(opt_in, dependencies),
            ExplorerRuntimeDecision::Disabled(
                ExplorerRuntimeDisableReason::DependenciesUnavailable
            )
        );
    }

    #[test]
    fn only_complete_opt_in_is_enabled() {
        let opt_in = ExplorerOptIn::from_sources(Some("1"), None);
        assert_eq!(
            decide(opt_in, ExplorerRuntimeDependencies::all_ready()),
            ExplorerRuntimeDecision::Enabled
        );
    }
}

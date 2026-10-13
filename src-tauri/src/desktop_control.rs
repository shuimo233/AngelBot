//! Bounded desktop automation adapters.
//!
//! The agent never supplies an executable path, URI, selector strategy, or
//! script. Those values come from validated local configuration and strict
//! allowlists before reaching this interface.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DesktopControlOperation {
    Invoke,
    Select,
    Expand,
    Collapse,
    ScrollUp,
    ScrollDown,
}

impl DesktopControlOperation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Invoke => "invoke",
            Self::Select => "select",
            Self::Expand => "expand",
            Self::Collapse => "collapse",
            Self::ScrollUp => "scrollup",
            Self::ScrollDown => "scrolldown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopAction {
    OpenApp {
        app_id: String,
        display_name: String,
        executable_path: String,
    },
    OpenSettings {
        page: String,
        uri: String,
    },
    RevealPath {
        path: String,
        is_directory: bool,
    },
    PrepareDraft {
        app_id: String,
        display_name: String,
        executable_path: String,
        selector: String,
        text: String,
        /// Set only after a backend-attested preflight of this pending draft.
        /// The Windows adapter refuses to write without it.
        expected_target: Option<DesktopDraftTargetIdentity>,
    },
    SetText {
        app_id: String,
        display_name: String,
        executable_path: String,
        text: String,
        /// Runtime-owned identity from a confirmed text-field preflight.
        expected_target: Option<DesktopDraftTargetIdentity>,
    },
    OperateControl {
        app_id: String,
        display_name: String,
        executable_path: String,
        operation: DesktopControlOperation,
        /// Backend-attested identity bound to the observed control lease.
        expected_target: Option<DesktopDraftTargetIdentity>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DesktopActionResult {
    pub adapter: String,
    pub status: String,
    pub target: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u32>,
    /// Backend-attested operation; verified refers only to this control state.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub action: Option<DesktopControlOperation>,
}

/// Settings-only discovery. A candidate is an exact UIA Name, not a control
/// handle or a reusable permission grant. No field Value is collected.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopDraftTargetCandidate {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DesktopDraftDiscovery {
    pub app_id: String,
    pub candidates: Vec<DesktopDraftTargetCandidate>,
}

/// Ephemeral UIA identity for one preflighted control. It is never a
/// persistent selector or a permission grant; the caller binds it to a pending
/// confirmation and the adapter rechecks it immediately before SetValue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopDraftTargetIdentity {
    pub process_id: u32,
    pub window_handle: u64,
    pub control_runtime_id: Vec<i32>,
    /// Exact AutomationId for a field, or opaque lease key for a control operation.
    /// The named-draft path leaves it absent; this is never model-supplied.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub automation_id: Option<String>,
    /// Exact operation that was preflighted, never visible to the model.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub control_operation: Option<DesktopControlOperation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopDraftTargetPreview {
    pub app_id: String,
    pub window_title: Option<String>,
    pub control_name: String,
    pub identity: DesktopDraftTargetIdentity,
}

/// The identity stays on the backend. Only the label and exact proposed text
/// belong in the confirmation shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopTextTargetPreview {
    pub app_id: String,
    pub window_title: Option<String>,
    pub control_label: String,
    pub identity: DesktopDraftTargetIdentity,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DraftTargetPayload {
    window_title: Option<String>,
    control_name: String,
    identity: DesktopDraftTargetIdentity,
}

#[cfg(windows)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TextTargetPayload {
    window_title: Option<String>,
    control_label: String,
}

#[cfg(windows)]
fn parse_text_target_payload(
    app_id: &str,
    lease: &FieldRefLease,
    raw: &[u8],
) -> Result<DesktopTextTargetPreview, DesktopAdapterError> {
    parse_control_target_payload(
        app_id,
        &lease.control_label,
        DesktopDraftTargetIdentity {
            automation_id: Some(lease.control_label.clone()),
            ..lease.identity.clone()
        },
        raw,
    )
}

#[cfg(windows)]
fn parse_control_target_payload(
    app_id: &str,
    control_label: &str,
    identity: DesktopDraftTargetIdentity,
    raw: &[u8],
) -> Result<DesktopTextTargetPreview, DesktopAdapterError> {
    if raw.len() > 1024 {
        return Err(DesktopAdapterError::new(
            "SCAN_LIMIT",
            "Text target preflight is too large",
        ));
    }
    let payload: TextTargetPayload = serde_json::from_slice(raw)
        .map_err(|_| DesktopAdapterError::new("ADAPTER_FAILED", "Invalid text target preflight"))?;
    if payload.control_label != control_label
        || payload
            .window_title
            .as_ref()
            .is_some_and(|title| title.chars().count() > 160 || title.chars().any(char::is_control))
    {
        return Err(DesktopAdapterError::new(
            "ADAPTER_FAILED",
            "Invalid text target metadata",
        ));
    }
    Ok(DesktopTextTargetPreview {
        app_id: app_id.to_string(),
        window_title: payload.window_title,
        control_label: payload.control_label,
        identity,
    })
}

fn valid_draft_identity(identity: &DesktopDraftTargetIdentity) -> bool {
    identity.process_id != 0
        && identity.window_handle != 0
        && identity.window_handle <= i64::MAX as u64
        && (1..=32).contains(&identity.control_runtime_id.len())
        && identity
            .automation_id
            .as_deref()
            .is_none_or(valid_observation_label)
}

fn valid_draft_selector(selector: &str) -> bool {
    !selector.is_empty()
        && selector.chars().count() <= 160
        && selector.trim() == selector
        && !selector.chars().any(char::is_control)
}

fn parse_draft_target_payload(
    app_id: &str,
    selector: &str,
    raw: &[u8],
) -> Result<DesktopDraftTargetPreview, DesktopAdapterError> {
    if raw.len() > 2048 {
        return Err(DesktopAdapterError::new(
            "SCAN_LIMIT",
            "Draft target preflight is too large",
        ));
    }
    let payload: DraftTargetPayload = serde_json::from_slice(raw).map_err(|_| {
        DesktopAdapterError::new(
            "ADAPTER_FAILED",
            "Windows UI Automation returned an invalid draft target",
        )
    })?;
    if !valid_draft_selector(selector)
        || payload.control_name != selector
        || !valid_draft_identity(&payload.identity)
        || payload
            .window_title
            .as_ref()
            .is_some_and(|title| title.chars().count() > 160 || title.chars().any(char::is_control))
    {
        return Err(DesktopAdapterError::new(
            "ADAPTER_FAILED",
            "Windows UI Automation returned invalid draft target metadata",
        ));
    }
    Ok(DesktopDraftTargetPreview {
        app_id: app_id.to_string(),
        window_title: payload.window_title,
        control_name: payload.control_name,
        identity: payload.identity,
    })
}

/// Read-only UI Automation metadata from one already-trusted application window.
/// Text/Value patterns, control handles, and password controls are never included.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopObservedControl {
    /// UIA Control View depth below the trusted window (1 is a direct child).
    pub depth: u8,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation_id: Option<String>,
    pub enabled: bool,
    pub capabilities: Vec<String>,
    /// Ephemeral selection hint, not an authorization grant or UIA identity.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub field_ref: Option<String>,
    /// Ephemeral selection hint for an attested control, not an authorization grant.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub control_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopWindowObservation {
    pub app_id: String,
    pub controls: Vec<DesktopObservedControl>,
    /// True when a budget or provider access failure left the observation partial.
    pub truncated: bool,
    /// Bounded reasons for a partial observation; never provider error text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<DesktopObservationDiagnostic>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum DesktopObservationDiagnostic {
    ControlLimit,
    OutputLimit,
    NodeLimit,
    DepthLimit,
    ProviderUnavailable,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservationPayload {
    controls: Vec<DesktopObservedControl>,
    truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    diagnostics: Vec<DesktopObservationDiagnostic>,
}

#[cfg(windows)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PrivateObservedTarget {
    index: usize,
    identity: DesktopDraftTargetIdentity,
}

#[cfg(windows)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrivateObservationPayload {
    controls: Vec<DesktopObservedControl>,
    truncated: bool,
    targets: Vec<PrivateObservedTarget>,
    #[serde(default)]
    diagnostics: Vec<DesktopObservationDiagnostic>,
}

const MAX_OBSERVED_CONTROLS: usize = 32;
const MAX_OBSERVATION_BYTES: usize = 3072;
const MAX_OBSERVATION_DIAGNOSTICS: usize = 5;
#[cfg(windows)]
const MAX_PRIVATE_OBSERVATION_BYTES: usize = 8192;

#[cfg(windows)]
fn parse_private_observation_payload(
    raw: &[u8],
) -> Result<(ObservationPayload, Vec<PrivateObservedTarget>), DesktopAdapterError> {
    if raw.len() > MAX_PRIVATE_OBSERVATION_BYTES {
        return Err(DesktopAdapterError::new(
            "SCAN_LIMIT",
            "Desktop observation is too large",
        ));
    }
    let private: PrivateObservationPayload = serde_json::from_slice(raw).map_err(|_| {
        DesktopAdapterError::new(
            "ADAPTER_FAILED",
            "Windows UI Automation returned an invalid observation",
        )
    })?;
    let public = serde_json::to_vec(&ObservationPayload {
        controls: private.controls,
        truncated: private.truncated,
        diagnostics: private.diagnostics,
    })
    .map_err(|_| DesktopAdapterError::new("ADAPTER_FAILED", "Invalid desktop observation"))?;
    let public = parse_observation_payload(&public)?;
    if private.targets.len() > public.controls.len() {
        return Err(DesktopAdapterError::new(
            "ADAPTER_FAILED",
            "Windows UI Automation returned invalid text targets",
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for target in &private.targets {
        let Some(control) = public.controls.get(target.index) else {
            return Err(DesktopAdapterError::new(
                "ADAPTER_FAILED",
                "Windows UI Automation returned invalid text target index",
            ));
        };
        let eligible_field = control.role == "edit"
            && control.enabled
            && control.capabilities.iter().any(|value| value == "setValue")
            && control.automation_id.is_some();
        if !seen.insert(target.index)
            || !(eligible_field || !supported_control_operations(control).is_empty())
            || !valid_draft_identity(&target.identity)
            || target.identity.automation_id.is_some()
            || target.identity.control_operation.is_some()
        {
            return Err(DesktopAdapterError::new(
                "ADAPTER_FAILED",
                "Windows UI Automation returned invalid text target metadata",
            ));
        }
    }
    Ok((public, private.targets))
}

fn parse_observation_payload(raw: &[u8]) -> Result<ObservationPayload, DesktopAdapterError> {
    if raw.len() > MAX_OBSERVATION_BYTES {
        return Err(DesktopAdapterError::new(
            "SCAN_LIMIT",
            "Desktop observation is too large",
        ));
    }
    let payload: ObservationPayload = serde_json::from_slice(raw).map_err(|_| {
        DesktopAdapterError::new(
            "ADAPTER_FAILED",
            "Windows UI Automation returned an invalid observation",
        )
    })?;
    if payload.controls.len() > MAX_OBSERVED_CONTROLS {
        return Err(DesktopAdapterError::new(
            "SCAN_LIMIT",
            "Too many desktop controls were found",
        ));
    }
    let mut diagnostics = std::collections::HashSet::new();
    if payload.diagnostics.len() > MAX_OBSERVATION_DIAGNOSTICS
        || (!payload.truncated && !payload.diagnostics.is_empty())
        || payload
            .diagnostics
            .iter()
            .any(|reason| !diagnostics.insert(*reason))
    {
        return Err(DesktopAdapterError::new(
            "ADAPTER_FAILED",
            "Windows UI Automation returned invalid observation diagnostics",
        ));
    }
    for control in &payload.controls {
        if !(1..=12).contains(&control.depth)
            || !matches!(
                control.role.as_str(),
                "button"
                    | "checkBox"
                    | "radioButton"
                    | "comboBox"
                    | "edit"
                    | "listItem"
                    | "tabItem"
                    | "menuItem"
                    | "hyperlink"
                    | "pane"
                    | "document"
                    | "list"
            )
            || control.name.as_ref().is_some_and(|name| {
                !matches!(
                    control.role.as_str(),
                    "button"
                        | "checkBox"
                        | "radioButton"
                        | "tabItem"
                        | "menuItem"
                        | "hyperlink"
                        | "pane"
                        | "document"
                        | "list"
                ) || !valid_observation_label(name)
            })
            || control
                .automation_id
                .as_ref()
                .is_some_and(|id| !valid_observation_label(id))
            || control.field_ref.is_some()
            || control.control_ref.is_some()
        {
            return Err(DesktopAdapterError::new(
                "ADAPTER_FAILED",
                "Windows UI Automation returned invalid control metadata",
            ));
        }
        if control.capabilities.len() > 3 || (!control.enabled && !control.capabilities.is_empty())
        {
            return Err(DesktopAdapterError::new(
                "ADAPTER_FAILED",
                "Windows UI Automation returned invalid control capabilities",
            ));
        }
        let mut capabilities = std::collections::HashSet::new();
        for capability in &control.capabilities {
            let supported = matches!(
                (control.role.as_str(), capability.as_str()),
                ("button" | "hyperlink", "invoke")
                    | ("checkBox", "toggle")
                    | ("radioButton" | "listItem" | "tabItem", "select")
                    | ("comboBox", "expandCollapse")
                    | ("edit", "setValue")
                    | ("menuItem", "invoke" | "toggle" | "expandCollapse")
                    | ("pane" | "document" | "list", "scroll")
            );
            if !supported || !capabilities.insert(capability) {
                return Err(DesktopAdapterError::new(
                    "ADAPTER_FAILED",
                    "Windows UI Automation returned invalid control capabilities",
                ));
            }
        }
    }
    Ok(payload)
}

fn valid_observation_label(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= 80
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

#[cfg(windows)]
fn supported_control_operations(control: &DesktopObservedControl) -> Vec<DesktopControlOperation> {
    if !control.enabled || !control.name.as_deref().is_some_and(valid_observation_label) {
        return vec![];
    }
    let mut operations = Vec::new();
    for (roles, capability, operation) in [
        (
            &["button", "hyperlink", "menuItem"][..],
            "invoke",
            DesktopControlOperation::Invoke,
        ),
        (
            &["radioButton", "tabItem"][..],
            "select",
            DesktopControlOperation::Select,
        ),
        (
            &["menuItem"][..],
            "expandCollapse",
            DesktopControlOperation::Expand,
        ),
        (
            &["menuItem"][..],
            "expandCollapse",
            DesktopControlOperation::Collapse,
        ),
        (
            &["pane", "document", "list"][..],
            "scroll",
            DesktopControlOperation::ScrollUp,
        ),
        (
            &["pane", "document", "list"][..],
            "scroll",
            DesktopControlOperation::ScrollDown,
        ),
    ] {
        if roles.contains(&control.role.as_str())
            && control.capabilities.iter().any(|value| value == capability)
        {
            operations.push(operation);
        }
    }
    operations
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryPayload {
    candidates: Vec<DesktopDraftTargetCandidate>,
}

fn parse_discovery_payload(
    raw: &[u8],
) -> Result<Vec<DesktopDraftTargetCandidate>, DesktopAdapterError> {
    let payload: DiscoveryPayload = serde_json::from_slice(raw).map_err(|_| {
        DesktopAdapterError::new(
            "ADAPTER_FAILED",
            "Windows UI Automation returned an invalid target list",
        )
    })?;
    if payload.candidates.len() > 32 {
        return Err(DesktopAdapterError::new(
            "SCAN_LIMIT",
            "Too many draft targets were found",
        ));
    }
    let mut names = std::collections::HashSet::new();
    for candidate in &payload.candidates {
        if candidate.name.is_empty()
            || candidate.name.chars().count() > 160
            || candidate.name.trim() != candidate.name
            || candidate.name.chars().any(char::is_control)
            || candidate
                .automation_id
                .as_ref()
                .is_some_and(|id| id.chars().count() > 160 || id.chars().any(char::is_control))
        {
            return Err(DesktopAdapterError::new(
                "ADAPTER_FAILED",
                "Windows UI Automation returned an invalid target name",
            ));
        }
        if !names.insert(candidate.name.to_lowercase()) {
            return Err(DesktopAdapterError::new(
                "TARGET_AMBIGUOUS",
                "Duplicate draft target names were found",
            ));
        }
    }
    Ok(payload.candidates)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DesktopAdapterError {
    pub code: String,
    pub message: String,
}

impl DesktopAdapterError {
    pub(crate) fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for DesktopAdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for DesktopAdapterError {}

/// One transient window image. Platform output is deliberately independent of
/// model messages and cannot be serialized into IPC, history, or diagnostics.
pub struct DesktopWindowImage {
    png_bytes: Vec<u8>,
}

impl DesktopWindowImage {
    pub(crate) fn from_png_bytes(png_bytes: Vec<u8>) -> Self {
        Self { png_bytes }
    }

    pub(crate) fn png_bytes(&self) -> &[u8] {
        &self.png_bytes
    }
}

impl std::fmt::Debug for DesktopWindowImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DesktopWindowImage")
            .field("bytes", &self.png_bytes.len())
            .finish()
    }
}

pub trait DesktopAdapter: Send + Sync {
    fn name(&self) -> &str;
    fn execute(&self, action: &DesktopAction) -> Result<DesktopActionResult, DesktopAdapterError>;
    /// Read-only settings discovery for an already saved trusted executable.
    /// This is not registered as a model-visible tool and grants no capability.
    fn inspect_draft_targets_for_settings(
        &self,
        _app_id: &str,
        _executable_path: &str,
    ) -> Result<DesktopDraftDiscovery, DesktopAdapterError> {
        Err(DesktopAdapterError::new(
            "UNSUPPORTED_PLATFORM",
            "Desktop target inspection is unavailable through this adapter",
        ))
    }
    fn preflight_draft(
        &self,
        app_id: &str,
        executable_path: &str,
        selector: &str,
        timeout: std::time::Duration,
    ) -> Result<DesktopDraftTargetPreview, DesktopAdapterError>;
    fn preflight_text_ref(
        &self,
        app_id: &str,
        executable_path: &str,
        field_ref: &str,
        timeout: std::time::Duration,
    ) -> Result<DesktopTextTargetPreview, DesktopAdapterError>;
    fn preflight_control_ref(
        &self,
        app_id: &str,
        executable_path: &str,
        control_ref: &str,
        operation: DesktopControlOperation,
        timeout: std::time::Duration,
    ) -> Result<DesktopTextTargetPreview, DesktopAdapterError>;
    fn observe_trusted_window(
        &self,
        app_id: &str,
        executable_path: &str,
    ) -> Result<DesktopWindowObservation, DesktopAdapterError>;

    fn observe_trusted_window_with_control(
        &self,
        app_id: &str,
        executable_path: &str,
        is_cancelled: &dyn Fn() -> bool,
        _timeout: std::time::Duration,
    ) -> Result<DesktopWindowObservation, DesktopAdapterError> {
        if is_cancelled() {
            return Err(DesktopAdapterError::new(
                "SCAN_CANCELLED",
                "Desktop observation was cancelled",
            ));
        }
        self.observe_trusted_window(app_id, executable_path)
    }

    /// A single-window read, not an action target or a screenshot archive.
    /// The host validates the app scope and execution permission before entry.
    fn capture_trusted_window_with_control(
        &self,
        _app_id: &str,
        _executable_path: &str,
        _is_cancelled: &dyn Fn() -> bool,
        _timeout: std::time::Duration,
    ) -> Result<DesktopWindowImage, DesktopAdapterError> {
        Err(DesktopAdapterError::new(
            "UNSUPPORTED_PLATFORM",
            "Single-window image observation is unavailable through this adapter",
        ))
    }

    /// Optional cooperative control for a bounded action. The callback is
    /// owned by the caller's run; adapters that do not block can use `execute`.
    fn execute_with_control(
        &self,
        action: &DesktopAction,
        _is_cancelled: &dyn Fn() -> bool,
        _timeout: std::time::Duration,
    ) -> Result<DesktopActionResult, DesktopAdapterError> {
        self.execute(action)
    }
}

#[derive(Default)]
pub struct MockDesktopAdapter {
    actions: Mutex<Vec<DesktopAction>>,
    observed_apps: Mutex<std::collections::HashSet<(String, String)>>,
    available_control_apps: Mutex<std::collections::HashSet<(String, String)>>,
}

impl MockDesktopAdapter {
    #[cfg(test)]
    pub fn actions(&self) -> Vec<DesktopAction> {
        self.actions.lock().map(|v| v.clone()).unwrap_or_default()
    }

    fn control_target(
        control_ref: &str,
    ) -> Option<(&'static str, i32, &'static [DesktopControlOperation])> {
        match control_ref {
            "mock-control-ref" => Some(("mockButton", 2, &[DesktopControlOperation::Invoke])),
            "mock-select-ref" => Some(("mockOption", 3, &[DesktopControlOperation::Select])),
            "mock-expand-ref" => Some((
                "mockMenu",
                4,
                &[
                    DesktopControlOperation::Expand,
                    DesktopControlOperation::Collapse,
                ],
            )),
            "mock-scroll-ref" => Some((
                "mockScrollPane",
                5,
                &[
                    DesktopControlOperation::ScrollUp,
                    DesktopControlOperation::ScrollDown,
                ],
            )),
            _ => None,
        }
    }
}

impl DesktopAdapter for MockDesktopAdapter {
    fn name(&self) -> &str {
        "mock"
    }

    fn inspect_draft_targets_for_settings(
        &self,
        app_id: &str,
        _executable_path: &str,
    ) -> Result<DesktopDraftDiscovery, DesktopAdapterError> {
        Ok(DesktopDraftDiscovery {
            app_id: app_id.to_string(),
            candidates: vec![DesktopDraftTargetCandidate {
                name: "Message".into(),
                automation_id: Some("mockField".into()),
            }],
        })
    }

    fn execute(&self, action: &DesktopAction) -> Result<DesktopActionResult, DesktopAdapterError> {
        if let DesktopAction::SetText {
            expected_target, ..
        } = action
        {
            if expected_target
                .as_ref()
                .and_then(|target| target.automation_id.as_deref())
                != Some("mockField")
            {
                return Err(DesktopAdapterError::new(
                    "TARGET_CHANGED",
                    "Text target preflight is missing or changed",
                ));
            }
        }
        if let DesktopAction::OperateControl {
            app_id,
            executable_path,
            expected_target,
            operation,
            ..
        } = action
        {
            let mut observed = self.observed_apps.lock().map_err(|_| {
                DesktopAdapterError::new("ADAPTER_FAILED", "Mock adapter lock is poisoned")
            })?;
            let reference = expected_target
                .as_ref()
                .and_then(|identity| identity.automation_id.as_deref())
                .unwrap_or("");
            let (_, runtime_id, operations) = Self::control_target(reference).ok_or_else(|| {
                DesktopAdapterError::new("TARGET_CHANGED", "Control reference is unavailable")
            })?;
            if !operations.contains(operation) {
                return Err(DesktopAdapterError::new(
                    "UNSUPPORTED_CONTROL",
                    "Operation is not supported by this observed control",
                ));
            }
            if !observed.contains(&(app_id.clone(), executable_path.clone()))
                || expected_target.as_ref()
                    != Some(&DesktopDraftTargetIdentity {
                        process_id: 1,
                        window_handle: 1,
                        control_runtime_id: vec![runtime_id],
                        automation_id: Some(reference.into()),
                        control_operation: Some(*operation),
                    })
            {
                return Err(DesktopAdapterError::new(
                    "TARGET_CHANGED",
                    "Control target preflight is missing or changed",
                ));
            }
            if !self
                .available_control_apps
                .lock()
                .map_err(|_| {
                    DesktopAdapterError::new("ADAPTER_FAILED", "Mock adapter lock is poisoned")
                })?
                .remove(&(app_id.clone(), executable_path.clone()))
            {
                return Err(DesktopAdapterError::new(
                    "TARGET_CHANGED",
                    "Control reference was already used",
                ));
            }
            observed.remove(&(app_id.clone(), executable_path.clone()));
        }
        self.actions
            .lock()
            .map_err(|_| {
                DesktopAdapterError::new("ADAPTER_FAILED", "Mock adapter lock is poisoned")
            })?
            .push(action.clone());
        let (status, target, detail) = match action {
            DesktopAction::OpenApp { app_id, .. } => {
                ("dispatched", app_id, "Mock application launch recorded")
            }
            DesktopAction::OpenSettings { page, .. } => {
                ("dispatched", page, "Mock settings launch recorded")
            }
            DesktopAction::RevealPath { path, .. } => {
                ("dispatched", path, "Mock Explorer reveal recorded")
            }
            DesktopAction::PrepareDraft { app_id, .. } => {
                ("verified", app_id, "Mock draft fill verified")
            }
            DesktopAction::SetText { app_id, .. } => {
                ("verified", app_id, "Mock text fill verified")
            }
            DesktopAction::OperateControl {
                app_id,
                operation: DesktopControlOperation::Invoke,
                ..
            } => (
                "dispatched",
                app_id,
                "Mock Invoke recorded; application outcome is not verified",
            ),
            DesktopAction::OperateControl {
                app_id,
                operation: DesktopControlOperation::Select,
                ..
            } => (
                "verified",
                app_id,
                "Mock control selected state verified; task outcome is not verified",
            ),
            DesktopAction::OperateControl {
                app_id,
                operation: DesktopControlOperation::Expand,
                ..
            } => (
                "verified",
                app_id,
                "Mock control Expanded state verified; task outcome is not verified",
            ),
            DesktopAction::OperateControl {
                app_id,
                operation: DesktopControlOperation::Collapse,
                ..
            } => (
                "verified",
                app_id,
                "Mock control Collapsed state verified; task outcome is not verified",
            ),
            DesktopAction::OperateControl {
                app_id,
                operation: DesktopControlOperation::ScrollUp | DesktopControlOperation::ScrollDown,
                ..
            } => (
                "verified",
                app_id,
                "Mock control scroll direction or boundary verified; task outcome is not verified",
            ),
        };
        Ok(DesktopActionResult {
            adapter: self.name().to_string(),
            status: status.to_string(),
            target: target.clone(),
            detail: detail.to_string(),
            process_id: None,
            action: match action {
                DesktopAction::OperateControl { operation, .. } => Some(*operation),
                _ => None,
            },
        })
    }

    fn preflight_draft(
        &self,
        app_id: &str,
        _executable_path: &str,
        selector: &str,
        _timeout: std::time::Duration,
    ) -> Result<DesktopDraftTargetPreview, DesktopAdapterError> {
        Ok(DesktopDraftTargetPreview {
            app_id: app_id.to_string(),
            window_title: Some("Mock window".into()),
            control_name: selector.to_string(),
            identity: DesktopDraftTargetIdentity {
                process_id: 1,
                window_handle: 1,
                control_runtime_id: vec![1],
                automation_id: None,
                control_operation: None,
            },
        })
    }

    fn preflight_text_ref(
        &self,
        app_id: &str,
        executable_path: &str,
        field_ref: &str,
        _timeout: std::time::Duration,
    ) -> Result<DesktopTextTargetPreview, DesktopAdapterError> {
        let observed = self.observed_apps.lock().map_err(|_| {
            DesktopAdapterError::new("ADAPTER_FAILED", "Mock adapter lock is poisoned")
        })?;
        if field_ref != "mock-field-ref"
            || !observed.contains(&(app_id.to_string(), executable_path.to_string()))
        {
            return Err(DesktopAdapterError::new(
                "TARGET_CHANGED",
                "Text field reference is unavailable",
            ));
        }
        Ok(DesktopTextTargetPreview {
            app_id: app_id.to_string(),
            window_title: Some("Mock window".into()),
            control_label: "mockField".into(),
            identity: DesktopDraftTargetIdentity {
                process_id: 1,
                window_handle: 1,
                control_runtime_id: vec![1],
                automation_id: Some("mockField".into()),
                control_operation: None,
            },
        })
    }

    fn observe_trusted_window(
        &self,
        app_id: &str,
        executable_path: &str,
    ) -> Result<DesktopWindowObservation, DesktopAdapterError> {
        self.observed_apps
            .lock()
            .map_err(|_| {
                DesktopAdapterError::new("ADAPTER_FAILED", "Mock adapter lock is poisoned")
            })?
            .insert((app_id.to_string(), executable_path.to_string()));
        self.available_control_apps
            .lock()
            .map_err(|_| {
                DesktopAdapterError::new("ADAPTER_FAILED", "Mock adapter lock is poisoned")
            })?
            .insert((app_id.to_string(), executable_path.to_string()));
        Ok(DesktopWindowObservation {
            app_id: app_id.to_string(),
            controls: vec![
                DesktopObservedControl {
                    depth: 1,
                    role: "edit".into(),
                    name: None,
                    automation_id: Some("mockField".into()),
                    enabled: true,
                    capabilities: vec!["setValue".into()],
                    field_ref: Some("mock-field-ref".into()),
                    control_ref: None,
                },
                DesktopObservedControl {
                    depth: 1,
                    role: "button".into(),
                    name: Some("mockButton".into()),
                    automation_id: None,
                    enabled: true,
                    capabilities: vec!["invoke".into()],
                    field_ref: None,
                    control_ref: Some("mock-control-ref".into()),
                },
                DesktopObservedControl {
                    depth: 1,
                    role: "radioButton".into(),
                    name: Some("mockOption".into()),
                    automation_id: None,
                    enabled: true,
                    capabilities: vec!["select".into()],
                    field_ref: None,
                    control_ref: Some("mock-select-ref".into()),
                },
                DesktopObservedControl {
                    depth: 1,
                    role: "menuItem".into(),
                    name: Some("mockMenu".into()),
                    automation_id: None,
                    enabled: true,
                    capabilities: vec!["expandCollapse".into()],
                    field_ref: None,
                    control_ref: Some("mock-expand-ref".into()),
                },
                DesktopObservedControl {
                    depth: 1,
                    role: "pane".into(),
                    name: Some("mockScrollPane".into()),
                    automation_id: None,
                    enabled: true,
                    capabilities: vec!["scroll".into()],
                    field_ref: None,
                    control_ref: Some("mock-scroll-ref".into()),
                },
            ],
            truncated: false,
            diagnostics: vec![],
        })
    }

    fn preflight_control_ref(
        &self,
        app_id: &str,
        executable_path: &str,
        control_ref: &str,
        operation: DesktopControlOperation,
        timeout: std::time::Duration,
    ) -> Result<DesktopTextTargetPreview, DesktopAdapterError> {
        if timeout.is_zero() {
            return Err(DesktopAdapterError::new(
                "SCAN_TIMEOUT",
                "Control preflight timed out",
            ));
        }
        let observed = self.observed_apps.lock().map_err(|_| {
            DesktopAdapterError::new("ADAPTER_FAILED", "Mock adapter lock is poisoned")
        })?;
        let (label, runtime_id, operations) =
            Self::control_target(control_ref).ok_or_else(|| {
                DesktopAdapterError::new("TARGET_CHANGED", "Control reference is unavailable")
            })?;
        if !operations.contains(&operation) {
            return Err(DesktopAdapterError::new(
                "UNSUPPORTED_CONTROL",
                "Operation is not supported by this observed control",
            ));
        }
        if !observed.contains(&(app_id.to_string(), executable_path.to_string()))
            || !self
                .available_control_apps
                .lock()
                .map_err(|_| {
                    DesktopAdapterError::new("ADAPTER_FAILED", "Mock adapter lock is poisoned")
                })?
                .contains(&(app_id.to_string(), executable_path.to_string()))
        {
            return Err(DesktopAdapterError::new(
                "TARGET_CHANGED",
                "Control reference is unavailable",
            ));
        }
        Ok(DesktopTextTargetPreview {
            app_id: app_id.into(),
            window_title: Some("Mock window".into()),
            control_label: label.into(),
            identity: DesktopDraftTargetIdentity {
                process_id: 1,
                window_handle: 1,
                control_runtime_id: vec![runtime_id],
                automation_id: Some(control_ref.into()),
                control_operation: Some(operation),
            },
        })
    }

    fn execute_with_control(
        &self,
        action: &DesktopAction,
        is_cancelled: &dyn Fn() -> bool,
        timeout: std::time::Duration,
    ) -> Result<DesktopActionResult, DesktopAdapterError> {
        if matches!(action, DesktopAction::OperateControl { .. })
            && (is_cancelled() || timeout.is_zero())
        {
            return Err(DesktopAdapterError::new(
                "CANCELLED",
                "Control action was cancelled before starting",
            ));
        }
        self.execute(action)
    }
}

pub fn create_mock_adapter() -> Arc<dyn DesktopAdapter> {
    Arc::new(MockDesktopAdapter::default())
}

#[cfg(not(windows))]
struct UnsupportedDesktopAdapter;

#[cfg(not(windows))]
impl DesktopAdapter for UnsupportedDesktopAdapter {
    fn name(&self) -> &str {
        "unsupported"
    }

    fn execute(&self, _action: &DesktopAction) -> Result<DesktopActionResult, DesktopAdapterError> {
        Err(DesktopAdapterError::new(
            "UNSUPPORTED_PLATFORM",
            "Desktop control is currently available only on Windows",
        ))
    }

    fn preflight_draft(
        &self,
        _app_id: &str,
        _executable_path: &str,
        _selector: &str,
        _timeout: std::time::Duration,
    ) -> Result<DesktopDraftTargetPreview, DesktopAdapterError> {
        Err(DesktopAdapterError::new(
            "UNSUPPORTED_PLATFORM",
            "Desktop control is currently available only on Windows",
        ))
    }

    fn preflight_text_ref(
        &self,
        _app_id: &str,
        _executable_path: &str,
        _field_ref: &str,
        _timeout: std::time::Duration,
    ) -> Result<DesktopTextTargetPreview, DesktopAdapterError> {
        Err(DesktopAdapterError::new(
            "UNSUPPORTED_PLATFORM",
            "Desktop control is currently available only on Windows",
        ))
    }

    fn preflight_control_ref(
        &self,
        _app_id: &str,
        _executable_path: &str,
        _control_ref: &str,
        _operation: DesktopControlOperation,
        _timeout: std::time::Duration,
    ) -> Result<DesktopTextTargetPreview, DesktopAdapterError> {
        Err(DesktopAdapterError::new(
            "UNSUPPORTED_PLATFORM",
            "Desktop control is currently available only on Windows",
        ))
    }

    fn observe_trusted_window(
        &self,
        _app_id: &str,
        _executable_path: &str,
    ) -> Result<DesktopWindowObservation, DesktopAdapterError> {
        Err(DesktopAdapterError::new(
            "UNSUPPORTED_PLATFORM",
            "Desktop observation is currently available only on Windows",
        ))
    }
}

#[cfg(windows)]
#[derive(Debug, Clone)]
struct FieldRefLease {
    app_id: String,
    executable: std::path::PathBuf,
    control_label: String,
    identity: DesktopDraftTargetIdentity,
    expires_at: std::time::Instant,
}

#[cfg(windows)]
#[derive(Debug, Clone)]
struct ControlRefLease {
    app_id: String,
    executable: std::path::PathBuf,
    control_label: String,
    role: String,
    supported_operations: Vec<DesktopControlOperation>,
    identity: DesktopDraftTargetIdentity,
    expires_at: std::time::Instant,
}

#[cfg(windows)]
#[derive(Default)]
pub struct WindowsDesktopAdapter {
    /// One native UIA operation at a time across every workspace using this
    /// adapter. Try-locking rejects overlap before any provider call or effect.
    uia_gate: Mutex<()>,
    field_refs: Mutex<std::collections::HashMap<String, FieldRefLease>>,
    control_refs: Mutex<std::collections::HashMap<String, ControlRefLease>>,
}

#[cfg(windows)]
const FIELD_REF_TTL: std::time::Duration = std::time::Duration::from_secs(300);
#[cfg(windows)]
const MAX_FIELD_REFS: usize = 128;

#[cfg(windows)]
const DRAFT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);

#[cfg(windows)]
const DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

#[cfg(windows)]
const UIA_SAFETY_HELPERS_SCRIPT: &str = r#"
function Get-UiaSafetyState($element) {
  try {
    $current = $element.Current
    $password = $current.IsPassword
    $offscreen = $current.IsOffscreen
    $enabled = $current.IsEnabled
    $processId = $current.ProcessId
    if ($null -eq $current -or $null -eq $password -or $null -eq $offscreen -or
        $null -eq $enabled -or $null -eq $processId -or [int]$processId -le 0) {
      throw 'Unavailable safety metadata'
    }
  } catch { throw 'TARGET_UNAVAILABLE|The control safety metadata is unavailable' }
  return @{ current = $current; password = [bool]$password; offscreen = [bool]$offscreen; enabled = [bool]$enabled; processId = [int]$processId }
}
function Get-UiaValuePatternReadOnly($pattern) {
  try {
    $state = $pattern.Current
    $readOnly = $state.IsReadOnly
    if ($null -eq $pattern -or $null -eq $state -or $null -eq $readOnly) {
      throw 'Unavailable ValuePattern metadata'
    }
  } catch { throw 'TARGET_UNAVAILABLE|The field read-only state is unavailable' }
  return [bool]$readOnly
}
function Get-UiaRuntimeId($element) {
  try {
    $raw = $element.GetRuntimeId()
    if ($null -eq $raw) { throw 'Unavailable runtime identity' }
    $runtimeId = [int[]]@($raw)
    if ($runtimeId.Count -lt 1 -or $runtimeId.Count -gt 32) { throw 'Invalid runtime identity' }
  } catch { throw 'TARGET_UNAVAILABLE|The control has no stable UI Automation identity' }
  return ,$runtimeId
}
"#;

#[cfg(windows)]
const TRUSTED_WINDOW_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
$expected = [IO.Path]::GetFullPath($env:ANGELBOT_TARGET_EXE)
$windows = New-Object System.Collections.Generic.List[System.IntPtr]
Get-Process | ForEach-Object {
  try {
    if ($_.MainWindowHandle -ne 0 -and [IO.Path]::GetFullPath($_.Path) -ieq $expected) {
      $windows.Add($_.MainWindowHandle)
    }
  } catch {}
}
if ($windows.Count -eq 0) { throw 'TARGET_NOT_FOUND|The trusted application has no open main window' }
if ($windows.Count -ne 1) { throw 'TARGET_AMBIGUOUS|The trusted application has multiple main windows' }
$root = [Windows.Automation.AutomationElement]::FromHandle($windows[0])
if ($null -eq $root) { throw 'TARGET_UNAVAILABLE|The trusted application window is unavailable' }
"#;

/// Capture alone needs every eligible HWND, including several windows in one
/// process. Legacy control observation keeps its existing main-window selector.
#[cfg(windows)]
const CAPTURE_WINDOW_SELECTION_SCRIPT: &str = r#"
function Select-CaptureWindow($candidates, $expected) {
  $matches = @($candidates | Where-Object {
    $_.visible -and $_.ownerless -and $_.processId -gt 0 -and $_.windowHandle -gt 0 -and
    [string]$_.executablePath -ieq $expected
  })
  if ($matches.Count -eq 0) { throw 'TARGET_NOT_FOUND|The trusted application has no eligible window' }
  if ($matches.Count -ne 1) { throw 'TARGET_AMBIGUOUS|The trusted application has multiple eligible windows' }
  return $matches[0]
}
"#;

#[cfg(windows)]
const CAPTURE_WINDOW_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class AngelBotCaptureWindows {
  private delegate bool EnumCallback(IntPtr hwnd, IntPtr param);
  [DllImport("user32.dll")] private static extern bool EnumWindows(EnumCallback callback, IntPtr param);
  [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr hwnd);
  [DllImport("user32.dll")] private static extern IntPtr GetWindow(IntPtr hwnd, uint command);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
  public static long[] EligibleWindows() {
    var result = new List<long>();
    if (!EnumWindows((hwnd, unused) => {
      if (IsWindowVisible(hwnd) && GetWindow(hwnd, 4) == IntPtr.Zero) result.Add(hwnd.ToInt64());
      return true;
    }, IntPtr.Zero)) throw new InvalidOperationException();
    return result.ToArray();
  }
}
'@
$expected = [IO.Path]::GetFullPath($env:ANGELBOT_TARGET_EXE)
$candidates = @([AngelBotCaptureWindows]::EligibleWindows() | ForEach-Object {
  $handle = [IntPtr]::new([long]$_)
  $processId = [uint32]0
  $threadId = [AngelBotCaptureWindows]::GetWindowThreadProcessId($handle, [ref]$processId)
  if ($threadId -ne 0 -and $processId -gt 0) {
    try {
      $process = Get-Process -Id $processId -ErrorAction Stop
      $path = [IO.Path]::GetFullPath($process.Path)
      @{ visible = $true; ownerless = $true; processId = $processId; windowHandle = [long]$_; executablePath = $path }
    } catch {}
  }
})
$selected = Select-CaptureWindow $candidates $expected
$windows = @([IntPtr]::new([long]$selected.windowHandle))
$root = [Windows.Automation.AutomationElement]::FromHandle($windows[0])
if ($null -eq $root) { throw 'TARGET_UNAVAILABLE|The trusted application window is unavailable' }
if ((Get-UiaSafetyState $root).processId -ne $selected.processId) { throw 'TARGET_CHANGED|The trusted window identity changed' }
"#;

/// Existing UIA safety metadata, bounded walk. No field values or names exit.
#[cfg(windows)]
const CAPTURE_TARGET_SCRIPT: &str = r#"
$rootState = Get-UiaSafetyState $root
if ($rootState.offscreen -or $rootState.password) { throw 'SENSITIVE_SURFACE|The trusted window cannot be captured' }
$walker = [Windows.Automation.TreeWalker]::ControlViewWalker
$pending = New-Object System.Collections.Stack
$pending.Push(@{ element = $root; depth = 0 })
$visited = 0
while ($pending.Count -gt 0) {
  $entry = $pending.Pop()
  $visited++
  if ($visited -gt 512) { throw 'SCAN_LIMIT|The password safety scan is incomplete' }
  $state = Get-UiaSafetyState $entry.element
  if ($state.password) { throw 'SENSITIVE_SURFACE|Password controls cannot be captured' }
  try { $child = $walker.GetFirstChild($entry.element) }
  catch { throw 'TARGET_UNAVAILABLE|The password safety scan is unavailable' }
  if ($null -ne $child -and $entry.depth -ge 24) { throw 'SCAN_LIMIT|The password safety scan is incomplete' }
  while ($null -ne $child) {
    if ($pending.Count + $visited -ge 512) { throw 'SCAN_LIMIT|The password safety scan is incomplete' }
    $pending.Push(@{ element = $child; depth = ($entry.depth + 1) })
    try { $child = $walker.GetNextSibling($child) }
    catch { throw 'TARGET_UNAVAILABLE|The password safety scan is unavailable' }
  }
}
$stateAfter = Get-UiaSafetyState $root
if ($stateAfter.password -or $stateAfter.offscreen -or $stateAfter.processId -ne $rootState.processId) {
  throw 'TARGET_CHANGED|The trusted application window changed'
}
$payload = @{ process_id = [uint32]$rootState.processId; window_handle = [uint64]$windows[0].ToInt64(); executable_path = $expected } | ConvertTo-Json -Compress
$bytes = [Text.Encoding]::UTF8.GetBytes($payload)
if ($bytes.Length -gt 8192) { throw 'SCAN_LIMIT|The target identity is too large' }
$stdout = [Console]::OpenStandardOutput()
$stdout.Write($bytes, 0, $bytes.Length)
$stdout.Flush()
"#;

#[cfg(windows)]
pub(crate) fn capture_target_probe(
    executable_path: &str,
    is_cancelled: &dyn Fn() -> bool,
    timeout: std::time::Duration,
) -> Result<crate::window_capture::CaptureTarget, DesktopAdapterError> {
    use std::process::Stdio;
    let script = format!("{UIA_SAFETY_HELPERS_SCRIPT}\n{CAPTURE_WINDOW_SELECTION_SCRIPT}\n{CAPTURE_WINDOW_SCRIPT}\n{CAPTURE_TARGET_SCRIPT}");
    let mut command = desktop_child_command(WindowsDesktopAdapter::system_powershell()?);
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .env("ANGELBOT_TARGET_EXE", executable_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output =
        crate::window_capture::run_bounded_child(&mut command, None, 8192, is_cancelled, timeout)?;
    if !output.status.success() {
        return Err(parse_uia_helper_error(&output.stderr));
    }
    crate::window_capture::parse_target(&output.stdout)
}

#[cfg(windows)]
const DRAFT_TARGET_SCRIPT: &str = r#"
$selector = $env:ANGELBOT_DRAFT_SELECTOR
$rootState = Get-UiaSafetyState $root
if ($rootState.offscreen -or $rootState.password) { throw 'TARGET_UNAVAILABLE|The trusted application window is not visible' }
$condition = New-Object Windows.Automation.AndCondition(
  (New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Edit)),
  (New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty, $selector))
)
$matches = $root.FindAll([Windows.Automation.TreeScope]::Descendants, $condition)
if ($matches.Count -eq 0) { throw 'TARGET_NOT_FOUND|No matching editable control was found' }
if ($matches.Count -gt 128) { throw 'SCAN_LIMIT|Too many matching edit controls were found' }
if ($matches.Count -ne 1) { throw 'TARGET_AMBIGUOUS|More than one matching editable control was found' }
$target = $matches.Item(0)
$targetState = Get-UiaSafetyState $target
if ($targetState.password) { throw 'SENSITIVE_SURFACE|Password controls cannot be automated' }
if (-not $targetState.enabled) { throw 'TARGET_UNAVAILABLE|The matching control is disabled' }
if ($targetState.offscreen) { throw 'TARGET_UNAVAILABLE|The matching control is not visible' }
if ($targetState.processId -ne $rootState.processId) { throw 'TARGET_UNAVAILABLE|The control belongs to another process' }
$pattern = $null
if (-not $target.TryGetCurrentPattern([Windows.Automation.ValuePattern]::Pattern, [ref]$pattern)) { throw 'UNSUPPORTED_CONTROL|The matching control does not support ValuePattern' }
if (Get-UiaValuePatternReadOnly $pattern) { throw 'TARGET_UNAVAILABLE|The matching control is read-only' }
"#;

#[cfg(windows)]
const DRAFT_PREFLIGHT_SCRIPT: &str = r#"
$runtimeId = Get-UiaRuntimeId $target
$windowTitle = [string]$root.Current.Name
if ($windowTitle.Length -gt 160 -or $windowTitle -match '\p{Cc}') { $windowTitle = $null }
$payload = @{
  windowTitle = $windowTitle
  controlName = [string]$target.Current.Name
  identity = @{
    processId = $rootState.processId
    windowHandle = [long]$windows[0].ToInt64()
    controlRuntimeId = $runtimeId
  }
} | ConvertTo-Json -Compress -Depth 5
$bytes = [Text.Encoding]::UTF8.GetBytes($payload)
if ($bytes.Length -gt 2048) { throw 'SCAN_LIMIT|Draft target preflight is too large' }
$stdout = [Console]::OpenStandardOutput()
$stdout.Write($bytes, 0, $bytes.Length)
$stdout.Flush()
"#;

#[cfg(windows)]
const TEXT_TARGET_SCRIPT: &str = r#"
$expectedProcessId = [int]$env:ANGELBOT_DRAFT_EXPECTED_PID
$expectedWindowHandle = [long]$env:ANGELBOT_DRAFT_EXPECTED_HWND
$expectedRuntimeId = [string]$env:ANGELBOT_DRAFT_EXPECTED_RUNTIME_ID
$fieldLabel = [string]$env:ANGELBOT_FIELD_LABEL
try {
  $currentProcess = Get-Process -Id $expectedProcessId -ErrorAction Stop
  $currentPath = [IO.Path]::GetFullPath($currentProcess.Path)
  $rootState = Get-UiaSafetyState $root
} catch { throw 'TARGET_CHANGED|The selected application is no longer available' }
if ($currentPath -ine $expected -or
    [long]$currentProcess.MainWindowHandle.ToInt64() -ne $expectedWindowHandle -or
    [long]$windows[0].ToInt64() -ne $expectedWindowHandle -or
    $rootState.processId -ne $expectedProcessId -or
    $rootState.offscreen -or $rootState.password) {
  throw 'TARGET_CHANGED|The selected application window has changed'
}
$condition = New-Object Windows.Automation.PropertyCondition(
  [Windows.Automation.AutomationElement]::ControlTypeProperty,
  [Windows.Automation.ControlType]::Edit
)
$edits = $root.FindAll([Windows.Automation.TreeScope]::Descendants, $condition)
if ($edits.Count -gt 256) { throw 'SCAN_LIMIT|Too many edit controls were found' }
$target = $null
for ($i = 0; $i -lt $edits.Count; $i++) {
  $candidate = $edits.Item($i)
  try { $runtimeId = Get-UiaRuntimeId $candidate }
  catch { continue }
  if (($runtimeId -join ',') -ceq $expectedRuntimeId) {
    if ($null -ne $target) { throw 'TARGET_AMBIGUOUS|Selected field identity is duplicated' }
    $target = $candidate
  }
}
if ($null -eq $target) { throw 'TARGET_CHANGED|The selected field is no longer available' }
try { $targetState = Get-UiaSafetyState $target }
catch { throw 'TARGET_CHANGED|The selected field is no longer available' }
if ($targetState.password) { throw 'SENSITIVE_SURFACE|Password controls cannot be automated' }
$currentLabel = $targetState.current.AutomationId
if ($targetState.offscreen -or -not $targetState.enabled -or
    $targetState.processId -ne $expectedProcessId -or $null -eq $currentLabel -or
    [string]$currentLabel -cne $fieldLabel) {
  throw 'TARGET_CHANGED|The selected field has changed'
}
$pattern = $null
if (-not $target.TryGetCurrentPattern([Windows.Automation.ValuePattern]::Pattern, [ref]$pattern)) {
  throw 'UNSUPPORTED_CONTROL|The selected field cannot be filled'
}
if (Get-UiaValuePatternReadOnly $pattern) { throw 'TARGET_UNAVAILABLE|The selected field is read-only' }
"#;

#[cfg(windows)]
const CONTROL_TARGET_SCRIPT: &str = r#"
$expectedProcessId = [int]$env:ANGELBOT_DRAFT_EXPECTED_PID
$expectedWindowHandle = [long]$env:ANGELBOT_DRAFT_EXPECTED_HWND
$expectedRuntimeId = [string]$env:ANGELBOT_DRAFT_EXPECTED_RUNTIME_ID
$controlLabel = [string]$env:ANGELBOT_CONTROL_LABEL
$controlOperation = [string]$env:ANGELBOT_CONTROL_OPERATION
$controlRole = [string]$env:ANGELBOT_CONTROL_ROLE
$operationSupported = switch ($controlOperation) {
  'invoke' { $controlRole -in @('button', 'hyperlink', 'menuItem') }
  'select' { $controlRole -in @('radioButton', 'tabItem') }
  'expand' { $controlRole -ceq 'menuItem' }
  'collapse' { $controlRole -ceq 'menuItem' }
  'scrollup' { $controlRole -in @('pane', 'document', 'list') }
  'scrolldown' { $controlRole -in @('pane', 'document', 'list') }
  default { $false }
}
if (-not $operationSupported) { throw 'UNSUPPORTED_CONTROL|The selected operation is unsupported for this role' }
$expectedRoleName = switch ([string]$env:ANGELBOT_CONTROL_ROLE) {
  'button' { 'ControlType.Button' }
  'hyperlink' { 'ControlType.Hyperlink' }
  'menuItem' { 'ControlType.MenuItem' }
  'radioButton' { 'ControlType.RadioButton' }
  'tabItem' { 'ControlType.TabItem' }
  'pane' { 'ControlType.Pane' }
  'document' { 'ControlType.Document' }
  'list' { 'ControlType.List' }
  default { throw 'UNSUPPORTED_CONTROL|The selected control role is unsupported' }
}
function Assert-UiaControlTarget($element) {
  try {
    $currentProcess = Get-Process -Id $expectedProcessId -ErrorAction Stop
    $currentPath = [IO.Path]::GetFullPath($currentProcess.Path)
    $rootState = Get-UiaSafetyState $root
    $targetState = Get-UiaSafetyState $element
    $runtimeId = Get-UiaRuntimeId $element
    $currentName = $targetState.current.Name
    $currentRole = $targetState.current.ControlType.ProgrammaticName
  } catch { throw 'TARGET_CHANGED|The selected control is no longer available' }
  if ($currentPath -ine $expected -or
      [long]$currentProcess.MainWindowHandle.ToInt64() -ne $expectedWindowHandle -or
      [long]$windows[0].ToInt64() -ne $expectedWindowHandle -or
      $rootState.processId -ne $expectedProcessId -or
      $rootState.password -or $rootState.offscreen -or
      $targetState.processId -ne $expectedProcessId -or
      $targetState.password -or $targetState.offscreen -or -not $targetState.enabled -or
      $null -eq $currentName -or [string]$currentName -cne $controlLabel -or
      $null -eq $currentRole -or [string]$currentRole -cne $expectedRoleName -or
      ($runtimeId -join ',') -cne $expectedRuntimeId) {
    throw 'TARGET_CHANGED|The selected control has changed'
  }
  $patternKind = switch ($controlOperation) {
    'invoke' { [Windows.Automation.InvokePattern]::Pattern }
    'select' { [Windows.Automation.SelectionItemPattern]::Pattern }
    'scrollup' { [Windows.Automation.ScrollPattern]::Pattern }
    'scrolldown' { [Windows.Automation.ScrollPattern]::Pattern }
    default { [Windows.Automation.ExpandCollapsePattern]::Pattern }
  }
  $operationPattern = $null
  try {
    $supported = $element.TryGetCurrentPattern($patternKind, [ref]$operationPattern)
  } catch { throw 'TARGET_CHANGED|The selected control pattern is unavailable' }
  if (-not $supported -or $null -eq $operationPattern) {
    throw 'UNSUPPORTED_CONTROL|The selected control does not support the required UI Automation pattern'
  }
  return $operationPattern
}
function Get-UiaControlOperationState($operationPattern) {
  if ($controlOperation -ceq 'invoke') { return 'invoke' }
  try {
    $patternCurrent = $operationPattern.Current
    if ($null -eq $patternCurrent) { throw 'Unavailable control state' }
    if ($controlOperation -in @('scrollup', 'scrolldown')) {
      $scrollable = $patternCurrent.VerticallyScrollable
      $position = $patternCurrent.VerticalScrollPercent
      if ($null -eq $scrollable -or $null -eq $position -or
          $position -isnot [double] -or [double]::IsNaN($position) -or
          [double]::IsInfinity($position) -or $position -lt 0 -or $position -gt 100) {
        throw 'Unavailable vertical scroll state'
      }
      if (-not $scrollable) { throw 'Unavailable vertical scroll capability' }
      return [double]$position
    }
    if ($controlOperation -ceq 'select') {
      $selected = $patternCurrent.IsSelected
      if ($null -eq $selected) { throw 'Unavailable selected state' }
      return [bool]$selected
    }
    $expandState = $patternCurrent.ExpandCollapseState
    if ($null -eq $expandState -or [string]$expandState -cnotin @('Collapsed', 'Expanded', 'PartiallyExpanded', 'LeafNode')) {
      throw 'Unavailable expand state'
    }
  } catch { throw 'TARGET_UNAVAILABLE|The selected control state is unavailable' }
  if ([string]$expandState -ceq 'LeafNode') { throw 'UNSUPPORTED_CONTROL|Leaf nodes cannot expand or collapse' }
  return [string]$expandState
}
# Locate only the observed RuntimeId, with the same bounded Control View walk.
$walker = [Windows.Automation.TreeWalker]::ControlViewWalker
$stack = New-Object 'System.Collections.Generic.Stack[object]'
$target = $null
$visited = 0
try { $first = $walker.GetFirstChild($root) }
catch { throw 'TARGET_CHANGED|The selected control tree is unavailable' }
if ($null -ne $first) { $stack.Push(@{ element = $first; depth = 1 }) }
while ($stack.Count -gt 0) {
  $entry = $stack.Pop()
  $candidate = $entry.element
  $visited++
  if ($visited -gt 256) { throw 'SCAN_LIMIT|The control tree scan limit was reached' }
  try {
    $candidateState = Get-UiaSafetyState $candidate
    $sibling = $walker.GetNextSibling($candidate)
    if ($null -ne $sibling) { $stack.Push(@{ element = $sibling; depth = $entry.depth }) }
    if ($candidateState.password -or $candidateState.offscreen) { continue }
    $runtimeId = Get-UiaRuntimeId $candidate
    if (($runtimeId -join ',') -ceq $expectedRuntimeId) {
      if ($null -ne $target) { throw 'TARGET_AMBIGUOUS|The selected control identity is duplicated' }
      $target = $candidate
    }
    $child = $walker.GetFirstChild($candidate)
  } catch { throw 'TARGET_CHANGED|The selected control tree is unavailable' }
  if ($null -ne $child) {
    if ($entry.depth -ge 12) { throw 'SCAN_LIMIT|The control tree depth limit was reached' }
    $stack.Push(@{ element = $child; depth = ($entry.depth + 1) })
  }
}
if ($null -eq $target) { throw 'TARGET_CHANGED|The selected control is no longer available' }
$pattern = Assert-UiaControlTarget $target
$preflightState = Get-UiaControlOperationState $pattern
"#;

#[cfg(windows)]
const CONTROL_OPERATION_SCRIPT: &str = r#"
# Refresh exact identity, Name, role, safety and the operation's pattern immediately
# before the effect. All state errors here occur before possible mutation.
$pattern = Assert-UiaControlTarget $target
$beforeState = Get-UiaControlOperationState $pattern
$desiredState = switch ($controlOperation) {
  'select' { $true }
  'expand' { 'Expanded' }
  'collapse' { 'Collapsed' }
  'scrollup' { [double]0 }
  'scrolldown' { [double]100 }
  default { $null }
}
$alreadyDesired = $controlOperation -cne 'invoke' -and $beforeState -ceq $desiredState
try {
  if (-not $alreadyDesired) {
    switch ($controlOperation) {
      'invoke' { $pattern.Invoke() }
      'select' { $pattern.Select() }
      'expand' { $pattern.Expand() }
      'collapse' { $pattern.Collapse() }
      'scrollup' { $pattern.Scroll([Windows.Automation.ScrollAmount]::NoAmount, [Windows.Automation.ScrollAmount]::SmallDecrement) }
      'scrolldown' { $pattern.Scroll([Windows.Automation.ScrollAmount]::NoAmount, [Windows.Automation.ScrollAmount]::SmallIncrement) }
    }
  }
  if ($controlOperation -ceq 'invoke') {
    $status = 'dispatched'
    $detail = 'Invoke was dispatched through Windows UI Automation; application outcome is not verified'
  } else {
    # Reacquire fresh pattern/state after the operation. An exact state is
    # evidence about this control only, never evidence of a completed task.
    # Providers may publish state after the call returns. Retry only fresh
    # read-only state mismatches, never the effect or identity/safety failures.
    $matched = $false
    for ($read = 0; $read -lt 11; $read++) {
      $freshPattern = Assert-UiaControlTarget $target
      $afterState = Get-UiaControlOperationState $freshPattern
      $matched = if ($controlOperation -ceq 'scrollup' -and -not $alreadyDesired) { $afterState -lt $beforeState }
                 elseif ($controlOperation -ceq 'scrolldown' -and -not $alreadyDesired) { $afterState -gt $beforeState }
                 else { $afterState -ceq $desiredState }
      if ($matched) { break }
      if ($read -lt 10) { Start-Sleep -Milliseconds 25 }
    }
    if (-not $matched) { throw 'State readback did not match the requested operation' }
    $status = 'verified'
    $detail = if ($controlOperation -in @('scrollup', 'scrolldown')) {
      'UI Automation vertical scroll direction or boundary verified; task outcome is not verified'
    } else { "UI Automation control $controlOperation state verified; task outcome is not verified" }
  }
  $json = @{ status = $status; action = $controlOperation; detail = $detail } | ConvertTo-Json -Compress
  $bytes = [Text.Encoding]::UTF8.GetBytes($json)
  $stdout = [Console]::OpenStandardOutput()
  $stdout.Write($bytes, 0, $bytes.Length)
  $stdout.Flush()
} catch { throw 'RESULT_UNKNOWN|The control may have changed; inspect the application before retrying' }
"#;

#[cfg(windows)]
const VERIFIED_SET_VALUE_SCRIPT: &str = r#"
$text = $env:ANGELBOT_DRAFT_TEXT
$expectedProcessId = [int]$env:ANGELBOT_DRAFT_EXPECTED_PID
$expectedWindowHandle = [long]$env:ANGELBOT_DRAFT_EXPECTED_HWND
$expectedRuntimeId = [string]$env:ANGELBOT_DRAFT_EXPECTED_RUNTIME_ID
# Recheck identity immediately before SetValue; a stale approval cannot be
# redirected to a different process, window or UIA element.
try {
  $currentProcess = Get-Process -Id $expectedProcessId -ErrorAction Stop
  $currentPath = [IO.Path]::GetFullPath($currentProcess.Path)
  $actualRuntimeId = Get-UiaRuntimeId $target
  $rootState = Get-UiaSafetyState $root
  $targetState = Get-UiaSafetyState $target
} catch { throw 'TARGET_CHANGED|The preflighted field is no longer available' }
if ($currentPath -ine $expected -or
    [long]$currentProcess.MainWindowHandle.ToInt64() -ne $expectedWindowHandle -or
    [long]$windows[0].ToInt64() -ne $expectedWindowHandle -or
    $rootState.processId -ne $expectedProcessId -or
    $targetState.processId -ne $expectedProcessId -or
    $rootState.password -or $rootState.offscreen -or
    $targetState.password -or $targetState.offscreen -or -not $targetState.enabled -or
    ($env:ANGELBOT_FIELD_LABEL -and [string]$targetState.current.AutomationId -cne [string]$env:ANGELBOT_FIELD_LABEL) -or
    ($actualRuntimeId -join ',') -cne $expectedRuntimeId) {
  throw 'TARGET_CHANGED|The preflighted field has changed'
}
if (Get-UiaValuePatternReadOnly $pattern) { throw 'TARGET_UNAVAILABLE|The selected field is read-only' }
try {
  $pattern.SetValue($text)
  $actual = $pattern.Current.Value
  if ($null -eq $actual -or -not [string]::Equals($actual, $text, [StringComparison]::Ordinal)) {
    throw 'VERIFICATION_FAILED|The field value did not match the requested text'
  }
} catch {
  throw 'RESULT_UNKNOWN|The field may have changed but verification was not completed'
}
@{ status = 'verified'; detail = 'Text was filled and verified through Windows UI Automation' } | ConvertTo-Json -Compress
"#;

#[cfg(windows)]
const OBSERVATION_SCRIPT: &str = r#"
try {
  $rootCurrent = $root.Current
  $rootOffscreen = $rootCurrent.IsOffscreen
  $rootPassword = $rootCurrent.IsPassword
  if ($null -eq $rootCurrent -or $null -eq $rootOffscreen -or $null -eq $rootPassword) {
    throw 'Unavailable root safety metadata'
  }
  $rootVisible = -not $rootOffscreen -and -not $rootPassword
}
catch { throw 'TARGET_UNAVAILABLE|The trusted application window cannot be inspected' }
if (-not $rootVisible) { throw 'TARGET_UNAVAILABLE|The trusted application window is not visible' }
$walker = [Windows.Automation.TreeWalker]::ControlViewWalker
$stack = New-Object 'System.Collections.Generic.Stack[object]'
$controls = New-Object 'System.Collections.Generic.List[object]'
$targets = New-Object 'System.Collections.Generic.List[object]'
$diagnostics = New-Object 'System.Collections.Generic.List[string]'
# Reserve the complete bounded diagnostic array so later provider failures
# cannot push an otherwise valid public payload over its output budget.
$budgetDiagnostics = @('controlLimit', 'outputLimit', 'nodeLimit', 'depthLimit', 'providerUnavailable')
$visited = 0
$truncated = $false
$metadataLimitReached = $false
function Mark-ObservationPartial([string]$reason) {
  $script:truncated = $true
  if (-not $diagnostics.Contains($reason)) { $diagnostics.Add($reason) }
}
$first = $null
try { $first = $walker.GetFirstChild($root) }
catch { Mark-ObservationPartial 'providerUnavailable' }
if ($null -ne $first) { $stack.Push(@{ element = $first; depth = 1 }) }
while ($stack.Count -gt 0) {
  $entry = $stack.Pop()
  $element = $entry.element
  $visited++
  if ($visited -gt 256) { Mark-ObservationPartial 'nodeLimit'; break }
  $sibling = $null
  try { $sibling = $walker.GetNextSibling($element) }
  catch { Mark-ObservationPartial 'providerUnavailable' }
  if ($null -ne $sibling) { $stack.Push(@{ element = $sibling; depth = $entry.depth }) }
  # A provider that cannot report visibility or password state is not safe to descend into.
  try {
    $current = $element.Current
    $isPassword = $current.IsPassword
    $isOffscreen = $current.IsOffscreen
    # PowerShell can return null for a failed .NET property getter rather
    # than entering catch. Check the safety metadata before using it.
    if ($null -eq $current -or $null -eq $isPassword -or $null -eq $isOffscreen) {
      Mark-ObservationPartial 'providerUnavailable'
      continue
    }
    if ($isPassword -or $isOffscreen) { continue }
  } catch { Mark-ObservationPartial 'providerUnavailable'; continue }
  try {
    $roleName = $current.ControlType.ProgrammaticName
    if ([string]::IsNullOrEmpty([string]$roleName)) { Mark-ObservationPartial 'providerUnavailable' }
  }
  catch { Mark-ObservationPartial 'providerUnavailable'; $roleName = '' }
  $role = switch ($roleName) {
    'ControlType.Button' { 'button' }
    'ControlType.CheckBox' { 'checkBox' }
    'ControlType.RadioButton' { 'radioButton' }
    'ControlType.ComboBox' { 'comboBox' }
    'ControlType.Edit' { 'edit' }
    'ControlType.ListItem' { 'listItem' }
    'ControlType.TabItem' { 'tabItem' }
    'ControlType.MenuItem' { 'menuItem' }
    'ControlType.Hyperlink' { 'hyperlink' }
    'ControlType.Pane' { 'pane' }
    'ControlType.Document' { 'document' }
    'ControlType.List' { 'list' }
    default { $null }
  }
  # Ordinary layout containers must not exhaust the actionable metadata budget.
  # Only retain a named, enabled container with a usable vertical ScrollPattern.
  $isScrollContainer = $role -in @('pane', 'document', 'list')
  if ($isScrollContainer) {
    try {
      $pattern = $null
      $scrollName = $current.Name
      if ($null -eq $scrollName -or $null -eq $current.IsEnabled) { throw 'Unavailable scroll metadata' }
      if (-not $current.IsEnabled -or $scrollName.Length -eq 0 -or $scrollName.Length -gt 80 -or
          $scrollName -cne $scrollName.Trim() -or $scrollName -match '\p{Cc}' -or
          -not $element.TryGetCurrentPattern([Windows.Automation.ScrollPattern]::Pattern, [ref]$pattern)) {
        $role = $null
      } else {
        $scrollCurrent = $pattern.Current
        if ($null -eq $scrollCurrent -or $null -eq $scrollCurrent.VerticallyScrollable) { throw 'Unavailable scroll state' }
        if (-not $scrollCurrent.VerticallyScrollable) { $role = $null }
        else {
          $position = $scrollCurrent.VerticalScrollPercent
          if ($null -eq $position -or $position -isnot [double] -or [double]::IsNaN($position) -or
              [double]::IsInfinity($position) -or $position -lt 0 -or $position -gt 100) { throw 'Unavailable scroll position' }
        }
      }
    } catch { Mark-ObservationPartial 'providerUnavailable'; $role = $null }
  }
  if ($null -ne $role -and -not $metadataLimitReached) {
    if ($controls.Count -ge 32) {
      Mark-ObservationPartial 'controlLimit'
      $metadataLimitReached = $true
    } else {
    $name = $null
    # Edit, ComboBox and ListItem names can mirror user data rather than labels.
    if ($role -in @('button', 'checkBox', 'radioButton', 'tabItem', 'menuItem', 'hyperlink', 'pane', 'document', 'list')) {
      try {
        $rawName = $current.Name
        if ($null -eq $rawName) { Mark-ObservationPartial 'providerUnavailable' }
        $candidate = [string]$rawName
        if ($candidate.Length -gt 0 -and $candidate.Length -le 80 -and
            $candidate -ceq $candidate.Trim() -and $candidate -notmatch '\p{Cc}') {
          $name = $candidate
        }
      } catch { Mark-ObservationPartial 'providerUnavailable' }
    }
    $automationId = $null
    try {
      $rawAutomationId = $current.AutomationId
      if ($null -eq $rawAutomationId) { Mark-ObservationPartial 'providerUnavailable' }
      $candidate = [string]$rawAutomationId
      if ($candidate.Length -gt 0 -and $candidate.Length -le 80 -and
          $candidate -ceq $candidate.Trim() -and $candidate -notmatch '\p{Cc}') {
        $automationId = $candidate
      }
    } catch { Mark-ObservationPartial 'providerUnavailable' }
    try {
      $rawEnabled = $current.IsEnabled
      if ($null -eq $rawEnabled) { Mark-ObservationPartial 'providerUnavailable' }
      $enabled = [bool]$rawEnabled
    }
    catch { Mark-ObservationPartial 'providerUnavailable'; $enabled = $false }
    $capabilities = @()
    if ($enabled) {
      if ($isScrollContainer) { $capabilities += 'scroll' }
      try {
        $pattern = $null
        if ($role -in @('button', 'hyperlink', 'menuItem') -and
            $element.TryGetCurrentPattern([Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) {
          $capabilities += 'invoke'
        }
      } catch { Mark-ObservationPartial 'providerUnavailable' }
      try {
        $pattern = $null
        if ($role -in @('checkBox', 'menuItem') -and
            $element.TryGetCurrentPattern([Windows.Automation.TogglePattern]::Pattern, [ref]$pattern)) {
          $capabilities += 'toggle'
        }
      } catch { Mark-ObservationPartial 'providerUnavailable' }
      try {
        $pattern = $null
        if ($role -in @('radioButton', 'listItem', 'tabItem') -and
            $element.TryGetCurrentPattern([Windows.Automation.SelectionItemPattern]::Pattern, [ref]$pattern)) {
          $capabilities += 'select'
        }
      } catch { Mark-ObservationPartial 'providerUnavailable' }
      try {
        $pattern = $null
        if ($role -in @('comboBox', 'menuItem') -and
            $element.TryGetCurrentPattern([Windows.Automation.ExpandCollapsePattern]::Pattern, [ref]$pattern)) {
          $capabilities += 'expandCollapse'
        }
      } catch { Mark-ObservationPartial 'providerUnavailable' }
      try {
        $pattern = $null
        if ($role -eq 'edit' -and
            $element.TryGetCurrentPattern([Windows.Automation.ValuePattern]::Pattern, [ref]$pattern)) {
          if (-not (Get-UiaValuePatternReadOnly $pattern)) { $capabilities += 'setValue' }
        }
      } catch { Mark-ObservationPartial 'providerUnavailable' }
    }
    $controls.Add(@{ depth = [int]$entry.depth; role = $role; name = $name; automationId = $automationId; enabled = $enabled; capabilities = @($capabilities) })
    $probe = @{ controls = @($controls.ToArray()); truncated = $false; diagnostics = $budgetDiagnostics } | ConvertTo-Json -Compress -Depth 5
    if ([Text.Encoding]::UTF8.GetByteCount($probe) -gt 3072) {
      $controls.RemoveAt($controls.Count - 1)
      Mark-ObservationPartial 'outputLimit'
      $metadataLimitReached = $true
    } elseif (($role -eq 'edit' -and $enabled -and $capabilities -contains 'setValue' -and $null -ne $automationId) -or
              ($role -in @('button', 'hyperlink', 'menuItem') -and $enabled -and $capabilities -contains 'invoke' -and $null -ne $name) -or
              ($role -in @('radioButton', 'tabItem') -and $enabled -and $capabilities -contains 'select' -and $null -ne $name) -or
              ($role -eq 'menuItem' -and $enabled -and $capabilities -contains 'expandCollapse' -and $null -ne $name) -or
              ($isScrollContainer -and $enabled -and $capabilities -contains 'scroll' -and $null -ne $name)) {
      # Field Value is deliberately not collected. The runtime identity
      # remains in the native adapter and never reaches the model.
      try {
        $runtimeId = Get-UiaRuntimeId $element
        $controlProcessId = $current.ProcessId
        $windowProcessId = $root.Current.ProcessId
        if ($null -eq $controlProcessId -or $null -eq $windowProcessId -or
            [int]$controlProcessId -le 0 -or [int]$windowProcessId -le 0) {
          Mark-ObservationPartial 'providerUnavailable'
        } elseif ([int]$controlProcessId -eq [int]$windowProcessId) {
          $targets.Add(@{
            index = [int]($controls.Count - 1)
            identity = @{
              processId = [int]$windowProcessId
              windowHandle = [long]$windows[0].ToInt64()
              controlRuntimeId = $runtimeId
            }
          })
        }
      } catch { Mark-ObservationPartial 'providerUnavailable' }
    }
    }
  }
  $child = $null
  try { $child = $walker.GetFirstChild($element) }
  catch { Mark-ObservationPartial 'providerUnavailable' }
  if ($null -ne $child) {
    if ($entry.depth -ge 12) { Mark-ObservationPartial 'depthLimit' }
    else { $stack.Push(@{ element = $child; depth = ($entry.depth + 1) }) }
  }
}
$json = @{ controls = @($controls.ToArray()); truncated = $truncated; diagnostics = @($diagnostics.ToArray()); targets = @($targets.ToArray()) } | ConvertTo-Json -Compress -Depth 7
$bytes = [Text.Encoding]::UTF8.GetBytes($json)
if ($bytes.Length -gt 8192) { throw 'SCAN_LIMIT|Desktop observation is too large' }
$stdout = [Console]::OpenStandardOutput()
$stdout.Write($bytes, 0, $bytes.Length)
$stdout.Flush()
"#;

#[cfg(windows)]
fn desktop_child_command(executable: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(executable);
    crate::child_process_env::apply_minimal_child_environment(&mut command);
    command
}

/// A killed UIA helper may already have set a field before confirmation was
/// received. Reap it, then report the outcome as unknown rather than failed.
#[cfg(windows)]
fn wait_for_draft_child(
    child: &mut std::process::Child,
    is_cancelled: &dyn Fn() -> bool,
    timeout: std::time::Duration,
) -> Result<(), DesktopAdapterError> {
    use wait_timeout::ChildExt;

    let deadline = std::time::Instant::now() + timeout;
    loop {
        if is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(DesktopAdapterError::new(
                "RESULT_UNKNOWN",
                "Draft action interrupted; the field may have changed. Inspect it before retrying",
            ));
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(DesktopAdapterError::new(
                "RESULT_UNKNOWN",
                "Draft action timed out; the field may have changed. Inspect it before retrying",
            ));
        }
        match child.wait_timeout(remaining.min(std::time::Duration::from_millis(100))) {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(DesktopAdapterError::new(
                    "RESULT_UNKNOWN",
                    "Draft helper status was lost; inspect the field before retrying",
                ));
            }
        }
    }
}

#[cfg(windows)]
fn wait_for_read_only_child(
    child: &mut std::process::Child,
    operation: &str,
) -> Result<(), DesktopAdapterError> {
    wait_for_read_only_child_with_control(child, operation, &|| false, DISCOVERY_TIMEOUT)
}

#[cfg(windows)]
fn wait_for_read_only_child_with_control(
    child: &mut std::process::Child,
    operation: &str,
    is_cancelled: &dyn Fn() -> bool,
    timeout: std::time::Duration,
) -> Result<(), DesktopAdapterError> {
    use wait_timeout::ChildExt;
    let deadline = std::time::Instant::now() + timeout.min(DISCOVERY_TIMEOUT);
    loop {
        if is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(DesktopAdapterError::new(
                "SCAN_CANCELLED",
                format!("{operation} was cancelled"),
            ));
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(DesktopAdapterError::new(
                "SCAN_TIMEOUT",
                format!("{operation} timed out"),
            ));
        }
        match child.wait_timeout(remaining.min(std::time::Duration::from_millis(50))) {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => continue,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(DesktopAdapterError::new(
                    "ADAPTER_FAILED",
                    format!("{operation} could not be completed"),
                ));
            }
        }
    }
}

#[cfg(windows)]
fn parse_uia_helper_error(stderr: &[u8]) -> DesktopAdapterError {
    let raw = String::from_utf8_lossy(stderr);
    for code in [
        "TARGET_NOT_FOUND",
        "TARGET_AMBIGUOUS",
        "TARGET_UNAVAILABLE",
        "TARGET_CHANGED",
        "SENSITIVE_SURFACE",
        "UNSUPPORTED_CONTROL",
        "VERIFICATION_FAILED",
        "SCAN_LIMIT",
    ] {
        if let Some(start) = raw.find(&format!("{code}|")) {
            let message = raw[start + code.len() + 1..]
                .lines()
                .next()
                .unwrap_or("Desktop action failed");
            return DesktopAdapterError::new(code, message);
        }
    }
    DesktopAdapterError::new(
        "ADAPTER_FAILED",
        "Windows UI Automation failed without a recognized result",
    )
}

#[cfg(windows)]
fn classify_draft_helper_result(
    success: bool,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<(String, String), DesktopAdapterError> {
    let unknown = || {
        DesktopAdapterError::new(
            "RESULT_UNKNOWN",
            "The draft field may have changed but verification was not completed; inspect it before retrying",
        )
    };
    if !success {
        let error = parse_uia_helper_error(stderr);
        // These are all thrown before SetValue. A mismatch in readback or an
        // unrecognized failure could occur after the target app accepted text.
        return if matches!(
            error.code.as_str(),
            "TARGET_NOT_FOUND"
                | "TARGET_AMBIGUOUS"
                | "TARGET_UNAVAILABLE"
                | "TARGET_CHANGED"
                | "SENSITIVE_SURFACE"
                | "UNSUPPORTED_CONTROL"
                | "SCAN_LIMIT"
        ) {
            Err(error)
        } else {
            Err(unknown())
        };
    }
    let value: serde_json::Value = serde_json::from_slice(stdout).map_err(|_| unknown())?;
    if value.get("status").and_then(|field| field.as_str()) != Some("verified") {
        return Err(unknown());
    }
    let detail = value
        .get("detail")
        .and_then(|field| field.as_str())
        .ok_or_else(unknown)?;
    Ok(("verified".to_string(), detail.to_string()))
}

#[cfg(windows)]
fn classify_control_helper_result(
    operation: DesktopControlOperation,
    success: bool,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<(String, String), DesktopAdapterError> {
    let unknown = || {
        DesktopAdapterError::new(
            "RESULT_UNKNOWN",
            "The control may have changed; inspect the application before retrying",
        )
    };
    if !success {
        if String::from_utf8_lossy(stderr).contains("RESULT_UNKNOWN|") {
            return Err(unknown());
        }
        let error = parse_uia_helper_error(stderr);
        return if error.code == "ADAPTER_FAILED" || error.code == "VERIFICATION_FAILED" {
            Err(unknown())
        } else {
            Err(error)
        };
    }
    if stdout.len() > 1024 {
        return Err(unknown());
    }
    let value: serde_json::Value = serde_json::from_slice(stdout).map_err(|_| unknown())?;
    let expected_status = if operation == DesktopControlOperation::Invoke {
        "dispatched"
    } else {
        "verified"
    };
    if value.get("status").and_then(|field| field.as_str()) != Some(expected_status)
        || value.get("action").and_then(|field| field.as_str()) != Some(operation.as_str())
    {
        return Err(unknown());
    }
    Ok((
        expected_status.into(),
        if operation == DesktopControlOperation::Invoke {
            "Invoke was dispatched through Windows UI Automation; application outcome is not verified".into()
        } else {
            format!(
                "UI Automation control {} state verified; task outcome is not verified",
                operation.as_str()
            )
        },
    ))
}

#[cfg(windows)]
impl WindowsDesktopAdapter {
    fn begin_uia_operation(&self) -> Result<std::sync::MutexGuard<'_, ()>, DesktopAdapterError> {
        self.uia_gate.try_lock().map_err(|error| match error {
            std::sync::TryLockError::WouldBlock => DesktopAdapterError::new(
                "TARGET_BUSY",
                "Another trusted desktop operation is in progress",
            ),
            std::sync::TryLockError::Poisoned(_) => DesktopAdapterError::new(
                "ADAPTER_FAILED",
                "The trusted desktop operation gate is unavailable",
            ),
        })
    }

    fn issue_field_refs(
        &self,
        app_id: &str,
        executable: &std::path::Path,
        truncated: bool,
        controls: &mut [DesktopObservedControl],
        targets: Vec<PrivateObservedTarget>,
    ) -> Result<(), DesktopAdapterError> {
        // A partial tree cannot establish that an AutomationId is unique in
        // the window. Preserve read-only metadata, but issue no write refs.
        if truncated {
            return Ok(());
        }
        let targets = targets
            .into_iter()
            .filter(|target| controls[target.index].role == "edit")
            .collect::<Vec<_>>();
        let mut labels = std::collections::HashMap::<String, usize>::new();
        for control in controls.iter().filter(|control| control.role == "edit") {
            if let Some(label) = control.automation_id.as_deref() {
                *labels.entry(label.to_string()).or_default() += 1;
            }
        }
        let now = std::time::Instant::now();
        let mut refs = self.field_refs.lock().map_err(|_| {
            DesktopAdapterError::new(
                "ADAPTER_FAILED",
                "Desktop field reference store is unavailable",
            )
        })?;
        refs.retain(|_, lease| lease.expires_at > now);
        if refs.len().saturating_add(targets.len()) > MAX_FIELD_REFS {
            return Err(DesktopAdapterError::new(
                "SCAN_LIMIT",
                "Too many active desktop field references",
            ));
        }
        let mut identities = std::collections::HashSet::new();
        for target in targets {
            let control = &mut controls[target.index];
            if control.role != "edit" {
                continue;
            }
            let Some(label) = control.automation_id.as_deref() else {
                continue;
            };
            if labels.get(label) != Some(&1)
                || !identities.insert(target.identity.control_runtime_id.clone())
            {
                continue;
            }
            let field_ref = format!("field_{}", uuid::Uuid::new_v4().simple());
            refs.insert(
                field_ref.clone(),
                FieldRefLease {
                    app_id: app_id.to_string(),
                    executable: executable.to_path_buf(),
                    control_label: label.to_string(),
                    identity: target.identity,
                    expires_at: now + FIELD_REF_TTL,
                },
            );
            control.field_ref = Some(field_ref);
        }
        Ok(())
    }

    fn resolve_field_ref(
        &self,
        app_id: &str,
        executable: &std::path::Path,
        field_ref: &str,
    ) -> Result<FieldRefLease, DesktopAdapterError> {
        if field_ref.len() != 38
            || !field_ref.starts_with("field_")
            || !field_ref[6..].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(DesktopAdapterError::new(
                "TARGET_CHANGED",
                "Text field reference is invalid",
            ));
        }
        let mut refs = self.field_refs.lock().map_err(|_| {
            DesktopAdapterError::new(
                "ADAPTER_FAILED",
                "Desktop field reference store is unavailable",
            )
        })?;
        let now = std::time::Instant::now();
        refs.retain(|_, lease| lease.expires_at > now);
        let lease = refs.get(field_ref).cloned().ok_or_else(|| {
            DesktopAdapterError::new("TARGET_CHANGED", "Text field reference expired")
        })?;
        if lease.app_id != app_id || lease.executable != executable {
            return Err(DesktopAdapterError::new(
                "TARGET_CHANGED",
                "Text field reference no longer matches the trusted application",
            ));
        }
        Ok(lease)
    }

    fn issue_control_refs(
        &self,
        app_id: &str,
        executable: &std::path::Path,
        truncated: bool,
        controls: &mut [DesktopObservedControl],
        targets: &[PrivateObservedTarget],
    ) -> Result<(), DesktopAdapterError> {
        // No actionable reference may escape any incomplete/provider-failed scan.
        if truncated {
            return Ok(());
        }
        let now = std::time::Instant::now();
        let mut refs = self.control_refs.lock().map_err(|_| {
            DesktopAdapterError::new(
                "ADAPTER_FAILED",
                "Desktop control reference store is unavailable",
            )
        })?;
        refs.retain(|_, lease| lease.expires_at > now);
        let eligible = targets
            .iter()
            .filter(|target| !supported_control_operations(&controls[target.index]).is_empty())
            .count();
        if refs.len().saturating_add(eligible) > MAX_FIELD_REFS {
            return Err(DesktopAdapterError::new(
                "SCAN_LIMIT",
                "Too many active desktop control references",
            ));
        }
        let mut identities = std::collections::HashMap::new();
        for target in targets {
            *identities
                .entry((
                    target.identity.process_id,
                    target.identity.window_handle,
                    target.identity.control_runtime_id.clone(),
                ))
                .or_insert(0usize) += 1;
        }
        for target in targets {
            let control = &mut controls[target.index];
            let supported_operations = supported_control_operations(control);
            if supported_operations.is_empty()
                || identities.get(&(
                    target.identity.process_id,
                    target.identity.window_handle,
                    target.identity.control_runtime_id.clone(),
                )) != Some(&1)
            {
                continue;
            }
            let control_ref = format!("control_{}", uuid::Uuid::new_v4().simple());
            let identity = DesktopDraftTargetIdentity {
                // The pending approval binds this exact lease, including Name
                // and role, not any later observation of the same RuntimeId.
                automation_id: Some(control_ref.clone()),
                ..target.identity.clone()
            };
            refs.insert(
                control_ref.clone(),
                ControlRefLease {
                    app_id: app_id.to_string(),
                    executable: executable.to_path_buf(),
                    control_label: control.name.clone().expect("eligible control has a Name"),
                    role: control.role.clone(),
                    supported_operations,
                    identity,
                    expires_at: now + FIELD_REF_TTL,
                },
            );
            control.control_ref = Some(control_ref);
        }
        Ok(())
    }

    fn resolve_control_ref(
        &self,
        app_id: &str,
        executable: &std::path::Path,
        control_ref: &str,
        consume_identity: Option<&DesktopDraftTargetIdentity>,
    ) -> Result<ControlRefLease, DesktopAdapterError> {
        if control_ref.len() != 40
            || !control_ref.starts_with("control_")
            || !control_ref[8..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(DesktopAdapterError::new(
                "TARGET_CHANGED",
                "Control reference is invalid",
            ));
        }
        let mut refs = self.control_refs.lock().map_err(|_| {
            DesktopAdapterError::new(
                "ADAPTER_FAILED",
                "Desktop control reference store is unavailable",
            )
        })?;
        let now = std::time::Instant::now();
        refs.retain(|_, lease| lease.expires_at > now);
        let lease = refs.get(control_ref).cloned().ok_or_else(|| {
            DesktopAdapterError::new(
                "TARGET_CHANGED",
                "Control reference expired or was already used",
            )
        })?;
        let identity_matches = consume_identity.is_none_or(|identity| {
            let mut observed_identity = identity.clone();
            observed_identity.control_operation = None;
            observed_identity == lease.identity
        });
        if lease.app_id != app_id || lease.executable != executable || !identity_matches {
            return Err(DesktopAdapterError::new(
                "TARGET_CHANGED",
                "Control reference no longer matches the confirmed target",
            ));
        }
        if let Some(identity) = consume_identity {
            if !identity
                .control_operation
                .is_some_and(|operation| lease.supported_operations.contains(&operation))
            {
                return Err(DesktopAdapterError::new(
                    "UNSUPPORTED_CONTROL",
                    "The requested control operation was not preflighted or observed",
                ));
            }
            // One attempt only, even if the helper times out or reports unknown.
            refs.remove(control_ref);
        }
        Ok(lease)
    }

    fn invalidate_application_refs(
        &self,
        app_id: &str,
        executable: &std::path::Path,
    ) -> Result<(), DesktopAdapterError> {
        let mut fields = self.field_refs.lock().map_err(|_| {
            DesktopAdapterError::new(
                "ADAPTER_FAILED",
                "Desktop field reference store is unavailable",
            )
        })?;
        let mut controls = self.control_refs.lock().map_err(|_| {
            DesktopAdapterError::new(
                "ADAPTER_FAILED",
                "Desktop control reference store is unavailable",
            )
        })?;
        fields.retain(|_, lease| lease.app_id != app_id || lease.executable != executable);
        controls.retain(|_, lease| lease.app_id != app_id || lease.executable != executable);
        Ok(())
    }

    fn process_comparison_path(path: &std::path::Path) -> String {
        let value = path.to_string_lossy();
        if let Some(unc_path) = value.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{}", unc_path);
        }
        if let Some(normal_path) = value.strip_prefix(r"\\?\") {
            return normal_path.to_string();
        }
        value.into_owned()
    }

    fn canonical_executable(path: &str) -> Result<std::path::PathBuf, DesktopAdapterError> {
        let path = std::fs::canonicalize(path).map_err(|e| {
            DesktopAdapterError::new(
                "TARGET_UNAVAILABLE",
                format!("Trusted executable is unavailable: {e}"),
            )
        })?;
        if !path.is_file()
            || path
                .extension()
                .and_then(|v| v.to_str())
                .map(|v| !v.eq_ignore_ascii_case("exe"))
                .unwrap_or(true)
        {
            return Err(DesktopAdapterError::new(
                "INVALID_TARGET",
                "Trusted target must be an existing .exe file",
            ));
        }
        Ok(path)
    }

    fn system_executable(relative_path: &str) -> Result<std::path::PathBuf, DesktopAdapterError> {
        let system_root = std::env::var_os("SystemRoot").ok_or_else(|| {
            DesktopAdapterError::new("ADAPTER_FAILED", "SystemRoot is unavailable")
        })?;
        let executable = std::path::PathBuf::from(system_root).join(relative_path);
        if !executable.is_absolute()
            || !executable.is_file()
            || executable
                .extension()
                .and_then(|value| value.to_str())
                .is_none_or(|extension| !extension.eq_ignore_ascii_case("exe"))
        {
            return Err(DesktopAdapterError::new(
                "ADAPTER_FAILED",
                "Required Windows system executable is unavailable",
            ));
        }
        Ok(executable)
    }

    fn system_powershell() -> Result<std::path::PathBuf, DesktopAdapterError> {
        Self::system_executable("System32/WindowsPowerShell/v1.0/powershell.exe")
    }

    fn system_explorer() -> Result<std::path::PathBuf, DesktopAdapterError> {
        Self::system_executable("explorer.exe")
    }

    fn inspect_draft_targets(
        &self,
        executable_path: &str,
    ) -> Result<Vec<DesktopDraftTargetCandidate>, DesktopAdapterError> {
        use std::process::Stdio;
        let _uia_operation = self.begin_uia_operation()?;

        let executable = Self::canonical_executable(executable_path)?;
        let process_executable = Self::process_comparison_path(&executable);
        const SCRIPT: &str = r#"
$condition = New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Edit)
$edits = $root.FindAll([Windows.Automation.TreeScope]::Descendants, $condition)
if ($edits.Count -gt 128) { throw 'SCAN_LIMIT|Too many edit controls were found' }
$allNames = @()
$eligible = @()
for ($i = 0; $i -lt $edits.Count; $i++) {
  try {
    $element = $edits.Item($i)
    $current = $element.Current
    $name = [string]$current.Name
    if ($name.Length -eq 0) { continue }
    $allNames += $name
    if ($name.Length -gt 160 -or $name -cne $name.Trim() -or $name -match '[\x00-\x1f\x7f]') { continue }
    if ($current.IsPassword -or -not $current.IsEnabled -or $current.IsOffscreen) { continue }
    $pattern = $null
    if (-not $element.TryGetCurrentPattern([Windows.Automation.ValuePattern]::Pattern, [ref]$pattern)) { continue }
    if ($pattern.Current.IsReadOnly) { continue }
    $automationId = [string]$current.AutomationId
    if ($automationId.Length -gt 160 -or $automationId -match '[\x00-\x1f\x7f]') { $automationId = '' }
    $eligible += @{ name = $name; automationId = $(if ($automationId.Length -gt 0) { $automationId } else { $null }) }
  } catch {}
}
$candidates = @()
foreach ($item in $eligible) {
  if (@($allNames | Where-Object { $_ -ieq $item.name }).Count -eq 1) { $candidates += $item }
}
if ($candidates.Count -gt 32) { throw 'SCAN_LIMIT|Too many eligible draft targets were found' }
$json = @{ candidates = @($candidates) } | ConvertTo-Json -Compress -Depth 5
$bytes = [Text.Encoding]::UTF8.GetBytes($json)
if ($bytes.Length -gt 3072) { throw 'SCAN_LIMIT|Draft target list is too large' }
$stdout = [Console]::OpenStandardOutput()
$stdout.Write($bytes, 0, $bytes.Length)
$stdout.Flush()
"#;
        let script = format!("{UIA_SAFETY_HELPERS_SCRIPT}\n{TRUSTED_WINDOW_SCRIPT}\n{SCRIPT}");
        let mut child = desktop_child_command(Self::system_powershell()?)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .env("ANGELBOT_TARGET_EXE", process_executable)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                DesktopAdapterError::new(
                    "ADAPTER_FAILED",
                    format!("Failed to start Windows UI Automation: {error}"),
                )
            })?;
        wait_for_read_only_child(&mut child, "Draft target inspection")?;
        let output = child.wait_with_output().map_err(|error| {
            DesktopAdapterError::new(
                "ADAPTER_FAILED",
                format!("Failed to collect UI Automation result: {error}"),
            )
        })?;
        if !output.status.success() {
            return Err(parse_uia_helper_error(&output.stderr));
        }
        if output.stdout.len() > 4096 {
            return Err(DesktopAdapterError::new(
                "SCAN_LIMIT",
                "Draft target list is too large",
            ));
        }
        parse_discovery_payload(&output.stdout)
    }

    fn preflight_draft_target(
        &self,
        app_id: &str,
        executable_path: &str,
        selector: &str,
        timeout: std::time::Duration,
    ) -> Result<DesktopDraftTargetPreview, DesktopAdapterError> {
        use std::process::Stdio;
        let _uia_operation = self.begin_uia_operation()?;

        if timeout.is_zero() {
            return Err(DesktopAdapterError::new(
                "SCAN_TIMEOUT",
                "Draft target preflight timed out",
            ));
        }
        if !valid_draft_selector(selector) {
            return Err(DesktopAdapterError::new(
                "INVALID_ARGUMENT",
                "Draft target selector is invalid",
            ));
        }
        let executable = Self::canonical_executable(executable_path)?;
        let process_executable = Self::process_comparison_path(&executable);
        let script =
            format!("{UIA_SAFETY_HELPERS_SCRIPT}\n{TRUSTED_WINDOW_SCRIPT}\n{DRAFT_TARGET_SCRIPT}\n{DRAFT_PREFLIGHT_SCRIPT}");
        let mut child = desktop_child_command(Self::system_powershell()?)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .env("ANGELBOT_TARGET_EXE", process_executable)
            .env("ANGELBOT_DRAFT_SELECTOR", selector)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                DesktopAdapterError::new(
                    "ADAPTER_FAILED",
                    format!("Failed to start Windows UI Automation: {error}"),
                )
            })?;
        wait_for_read_only_child_with_control(
            &mut child,
            "Draft target preflight",
            &|| false,
            timeout,
        )?;
        let output = child.wait_with_output().map_err(|error| {
            DesktopAdapterError::new(
                "ADAPTER_FAILED",
                format!("Failed to collect UI Automation result: {error}"),
            )
        })?;
        if !output.status.success() {
            return Err(parse_uia_helper_error(&output.stderr));
        }
        parse_draft_target_payload(app_id, selector, &output.stdout)
    }

    fn preflight_text_target(
        &self,
        app_id: &str,
        executable_path: &str,
        field_ref: &str,
        timeout: std::time::Duration,
    ) -> Result<DesktopTextTargetPreview, DesktopAdapterError> {
        use std::process::Stdio;
        let _uia_operation = self.begin_uia_operation()?;

        if timeout.is_zero() {
            return Err(DesktopAdapterError::new(
                "SCAN_TIMEOUT",
                "Text target preflight timed out",
            ));
        }
        let executable = Self::canonical_executable(executable_path)?;
        let lease = self.resolve_field_ref(app_id, &executable, field_ref)?;
        let script = format!(
            "{UIA_SAFETY_HELPERS_SCRIPT}\n{TRUSTED_WINDOW_SCRIPT}\n{TEXT_TARGET_SCRIPT}\n\
             $windowTitle = [string]$root.Current.Name\n\
             if ($windowTitle.Length -gt 160 -or $windowTitle -match '\\p{{Cc}}') {{ $windowTitle = $null }}\n\
             $json = @{{ windowTitle = $windowTitle; controlLabel = $fieldLabel }} | ConvertTo-Json -Compress\n\
             $bytes = [Text.Encoding]::UTF8.GetBytes($json)\n\
             if ($bytes.Length -gt 1024) {{ throw 'SCAN_LIMIT|Text target preflight is too large' }}\n\
             $stdout = [Console]::OpenStandardOutput()\n\
             $stdout.Write($bytes, 0, $bytes.Length)\n\
             $stdout.Flush()"
        );
        let expected_runtime_id = lease
            .identity
            .control_runtime_id
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let mut child = desktop_child_command(Self::system_powershell()?)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .env(
                "ANGELBOT_TARGET_EXE",
                Self::process_comparison_path(&executable),
            )
            .env(
                "ANGELBOT_DRAFT_EXPECTED_PID",
                lease.identity.process_id.to_string(),
            )
            .env(
                "ANGELBOT_DRAFT_EXPECTED_HWND",
                lease.identity.window_handle.to_string(),
            )
            .env("ANGELBOT_DRAFT_EXPECTED_RUNTIME_ID", expected_runtime_id)
            .env("ANGELBOT_FIELD_LABEL", &lease.control_label)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                DesktopAdapterError::new(
                    "ADAPTER_FAILED",
                    format!("Failed to start Windows UI Automation: {error}"),
                )
            })?;
        wait_for_read_only_child_with_control(
            &mut child,
            "Text target preflight",
            &|| false,
            timeout,
        )?;
        let output = child.wait_with_output().map_err(|error| {
            DesktopAdapterError::new(
                "ADAPTER_FAILED",
                format!("Failed to collect UI Automation result: {error}"),
            )
        })?;
        if !output.status.success() {
            return Err(parse_uia_helper_error(&output.stderr));
        }
        parse_text_target_payload(app_id, &lease, &output.stdout)
    }

    fn control_target_command(
        executable: &std::path::Path,
        lease: &ControlRefLease,
        operation: DesktopControlOperation,
        ending_script: &str,
    ) -> Result<std::process::Command, DesktopAdapterError> {
        use std::process::Stdio;
        let script = format!("{UIA_SAFETY_HELPERS_SCRIPT}\n{TRUSTED_WINDOW_SCRIPT}\n{CONTROL_TARGET_SCRIPT}\n{ending_script}");
        let runtime_id = lease
            .identity
            .control_runtime_id
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let mut command = desktop_child_command(Self::system_powershell()?);
        command
            .args([
                "-Mta",
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .env(
                "ANGELBOT_TARGET_EXE",
                Self::process_comparison_path(executable),
            )
            .env(
                "ANGELBOT_DRAFT_EXPECTED_PID",
                lease.identity.process_id.to_string(),
            )
            .env(
                "ANGELBOT_DRAFT_EXPECTED_HWND",
                lease.identity.window_handle.to_string(),
            )
            .env("ANGELBOT_DRAFT_EXPECTED_RUNTIME_ID", runtime_id)
            .env("ANGELBOT_CONTROL_LABEL", &lease.control_label)
            .env("ANGELBOT_CONTROL_ROLE", &lease.role)
            .env("ANGELBOT_CONTROL_OPERATION", operation.as_str())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok(command)
    }

    fn preflight_control_target(
        &self,
        app_id: &str,
        executable_path: &str,
        control_ref: &str,
        operation: DesktopControlOperation,
        timeout: std::time::Duration,
    ) -> Result<DesktopTextTargetPreview, DesktopAdapterError> {
        let _uia_operation = self.begin_uia_operation()?;
        if timeout.is_zero() {
            return Err(DesktopAdapterError::new(
                "SCAN_TIMEOUT",
                "Control target preflight timed out",
            ));
        }
        let executable = Self::canonical_executable(executable_path)?;
        let lease = self.resolve_control_ref(app_id, &executable, control_ref, None)?;
        if !lease.supported_operations.contains(&operation) {
            return Err(DesktopAdapterError::new(
                "UNSUPPORTED_CONTROL",
                "The requested operation was not observed on this control",
            ));
        }
        let ending_script = r#"
$windowTitle = [string]$root.Current.Name
if ($windowTitle.Length -gt 160 -or $windowTitle -match '\p{Cc}') { $windowTitle = $null }
$json = @{ windowTitle = $windowTitle; controlLabel = $controlLabel } | ConvertTo-Json -Compress
$bytes = [Text.Encoding]::UTF8.GetBytes($json)
if ($bytes.Length -gt 1024) { throw 'SCAN_LIMIT|Control target preflight is too large' }
$stdout = [Console]::OpenStandardOutput()
$stdout.Write($bytes, 0, $bytes.Length)
$stdout.Flush()
"#;
        let mut child =
            Self::control_target_command(&executable, &lease, operation, ending_script)?
                .spawn()
                .map_err(|error| {
                    DesktopAdapterError::new(
                        "ADAPTER_FAILED",
                        format!("Failed to start Windows UI Automation: {error}"),
                    )
                })?;
        wait_for_read_only_child_with_control(
            &mut child,
            "Control target preflight",
            &|| false,
            timeout,
        )?;
        let output = child.wait_with_output().map_err(|error| {
            DesktopAdapterError::new(
                "ADAPTER_FAILED",
                format!("Failed to collect UI Automation result: {error}"),
            )
        })?;
        if !output.status.success() {
            return Err(parse_uia_helper_error(&output.stderr));
        }
        // Do not return a preflight for a reference that expired while scanning.
        self.resolve_control_ref(app_id, &executable, control_ref, None)?;
        parse_control_target_payload(
            app_id,
            &lease.control_label,
            DesktopDraftTargetIdentity {
                control_operation: Some(operation),
                ..lease.identity
            },
            &output.stdout,
        )
    }

    fn operate_control(
        &self,
        app_id: &str,
        executable_path: &str,
        operation: DesktopControlOperation,
        expected_target: Option<&DesktopDraftTargetIdentity>,
        is_cancelled: &dyn Fn() -> bool,
        timeout: std::time::Duration,
    ) -> Result<DesktopActionResult, DesktopAdapterError> {
        let _uia_operation = self.begin_uia_operation()?;
        if is_cancelled() || timeout.is_zero() {
            return Err(DesktopAdapterError::new(
                "CANCELLED",
                "Control action was cancelled before starting",
            ));
        }
        let identity = expected_target
            .filter(|identity| valid_draft_identity(identity))
            .ok_or_else(|| {
                DesktopAdapterError::new(
                    "TARGET_CHANGED",
                    "Control target preflight is missing or no longer valid",
                )
            })?;
        let control_ref = identity.automation_id.as_deref().ok_or_else(|| {
            DesktopAdapterError::new("TARGET_CHANGED", "Control target reference is missing")
        })?;
        if identity.control_operation != Some(operation) {
            return Err(DesktopAdapterError::new(
                "TARGET_CHANGED",
                "The requested operation differs from its preflight",
            ));
        }
        let executable = Self::canonical_executable(executable_path)?;
        let lease = self.resolve_control_ref(app_id, &executable, control_ref, Some(identity))?;
        // Controls can change descendants, including text-field identities.
        self.invalidate_application_refs(app_id, &executable)?;
        let mut child =
            Self::control_target_command(&executable, &lease, operation, CONTROL_OPERATION_SCRIPT)?
                .spawn()
                .map_err(|error| {
                    DesktopAdapterError::new(
                        "ADAPTER_FAILED",
                        format!("Failed to start Windows UI Automation: {error}"),
                    )
                })?;
        let result = (|| {
            // As for SetValue, a killed helper may have crossed the effect seam.
            wait_for_draft_child(&mut child, is_cancelled, timeout.min(DRAFT_TIMEOUT)).map_err(|_| {
                DesktopAdapterError::new("RESULT_UNKNOWN", "Control action was interrupted or timed out; it may have changed. Inspect the application before retrying")
            })?;
            let output = child.wait_with_output().map_err(|error| {
                DesktopAdapterError::new("RESULT_UNKNOWN", format!("Control result could not be collected; inspect the application before retrying: {error}"))
            })?;
            let (status, detail) = classify_control_helper_result(
                operation,
                output.status.success(),
                &output.stdout,
                &output.stderr,
            )?;
            Ok(DesktopActionResult {
                adapter: self.name().into(),
                status,
                target: app_id.into(),
                detail,
                process_id: None,
                action: Some(operation),
            })
        })();
        // Also invalidate references from observations that raced the helper.
        self.invalidate_application_refs(app_id, &executable).map_err(|_| {
            DesktopAdapterError::new("RESULT_UNKNOWN", "Control references could not be invalidated after the action; inspect before retrying")
        })?;
        result
    }

    fn prepare_draft(
        &self,
        app_id: &str,
        executable_path: &str,
        selector: &str,
        text: &str,
        expected_target: Option<&DesktopDraftTargetIdentity>,
        is_cancelled: &dyn Fn() -> bool,
        timeout: std::time::Duration,
    ) -> Result<DesktopActionResult, DesktopAdapterError> {
        use std::process::Stdio;
        let _uia_operation = self.begin_uia_operation()?;

        if is_cancelled() || timeout.is_zero() {
            return Err(DesktopAdapterError::new(
                "CANCELLED",
                "Draft action was cancelled before starting",
            ));
        }

        if text.contains('\0') || text.len() > 20_000 {
            return Err(DesktopAdapterError::new(
                "INVALID_ARGUMENT",
                "Draft text must be at most 20,000 bytes and contain no NUL characters",
            ));
        }
        if !valid_draft_selector(selector) {
            return Err(DesktopAdapterError::new(
                "INVALID_ARGUMENT",
                "Draft target selector is invalid",
            ));
        }
        let expected_target = expected_target
            .filter(|identity| {
                valid_draft_identity(identity) && identity.control_operation.is_none()
            })
            .ok_or_else(|| {
                DesktopAdapterError::new(
                    "TARGET_CHANGED",
                    "Draft target preflight is missing or no longer valid",
                )
            })?;
        let executable = Self::canonical_executable(executable_path)?;
        // `std::fs::canonicalize` uses the Windows extended-length prefix
        // (`\\?\`) while `Get-Process.Path` returns an ordinary DOS/UNC path.
        // Compare equivalent representations so a trusted running process can
        // actually be found without weakening the executable identity check.
        let process_executable = Self::process_comparison_path(&executable);
        // Fixed script: model-provided values are passed only as environment
        // variables, never interpolated into executable code.
        let script =
            format!("{UIA_SAFETY_HELPERS_SCRIPT}\n{TRUSTED_WINDOW_SCRIPT}\n{DRAFT_TARGET_SCRIPT}\n{VERIFIED_SET_VALUE_SCRIPT}");
        let expected_runtime_id = expected_target
            .control_runtime_id
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let mut command = desktop_child_command(Self::system_powershell()?);
        let mut child = command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .env("ANGELBOT_TARGET_EXE", process_executable)
            .env("ANGELBOT_DRAFT_SELECTOR", selector)
            .env("ANGELBOT_DRAFT_TEXT", text)
            .env(
                "ANGELBOT_DRAFT_EXPECTED_PID",
                expected_target.process_id.to_string(),
            )
            .env(
                "ANGELBOT_DRAFT_EXPECTED_HWND",
                expected_target.window_handle.to_string(),
            )
            .env("ANGELBOT_DRAFT_EXPECTED_RUNTIME_ID", expected_runtime_id)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                DesktopAdapterError::new(
                    "ADAPTER_FAILED",
                    format!("Failed to start Windows UI Automation: {e}"),
                )
            })?;
        wait_for_draft_child(&mut child, is_cancelled, timeout.min(DRAFT_TIMEOUT))?;
        let output = child.wait_with_output().map_err(|e| {
            DesktopAdapterError::new(
                "RESULT_UNKNOWN",
                format!(
                    "Draft result could not be collected; inspect the field before retrying: {e}"
                ),
            )
        })?;
        let (status, detail) =
            classify_draft_helper_result(output.status.success(), &output.stdout, &output.stderr)?;
        Ok(DesktopActionResult {
            adapter: self.name().to_string(),
            status,
            target: app_id.to_string(),
            detail,
            process_id: None,
            action: None,
        })
    }

    fn set_text(
        &self,
        app_id: &str,
        executable_path: &str,
        text: &str,
        expected_target: Option<&DesktopDraftTargetIdentity>,
        is_cancelled: &dyn Fn() -> bool,
        timeout: std::time::Duration,
    ) -> Result<DesktopActionResult, DesktopAdapterError> {
        use std::process::Stdio;
        let _uia_operation = self.begin_uia_operation()?;

        if is_cancelled() || timeout.is_zero() {
            return Err(DesktopAdapterError::new(
                "CANCELLED",
                "Text action was cancelled before starting",
            ));
        }
        if text.is_empty() || text.contains('\0') || text.len() > 20_000 {
            return Err(DesktopAdapterError::new(
                "INVALID_ARGUMENT",
                "Text must contain 1 to 20,000 bytes and no NUL characters",
            ));
        }
        let expected_target = expected_target
            .filter(|identity| {
                valid_draft_identity(identity) && identity.control_operation.is_none()
            })
            .ok_or_else(|| {
                DesktopAdapterError::new(
                    "TARGET_CHANGED",
                    "Text target preflight is missing or no longer valid",
                )
            })?;
        let field_label = expected_target
            .automation_id
            .as_deref()
            .filter(|label| valid_observation_label(label))
            .ok_or_else(|| {
                DesktopAdapterError::new(
                    "TARGET_CHANGED",
                    "Text target label is missing or no longer valid",
                )
            })?;
        let executable = Self::canonical_executable(executable_path)?;
        let script =
            format!("{UIA_SAFETY_HELPERS_SCRIPT}\n{TRUSTED_WINDOW_SCRIPT}\n{TEXT_TARGET_SCRIPT}\n{VERIFIED_SET_VALUE_SCRIPT}");
        let expected_runtime_id = expected_target
            .control_runtime_id
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let mut child = desktop_child_command(Self::system_powershell()?)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .env(
                "ANGELBOT_TARGET_EXE",
                Self::process_comparison_path(&executable),
            )
            .env("ANGELBOT_DRAFT_TEXT", text)
            .env(
                "ANGELBOT_DRAFT_EXPECTED_PID",
                expected_target.process_id.to_string(),
            )
            .env(
                "ANGELBOT_DRAFT_EXPECTED_HWND",
                expected_target.window_handle.to_string(),
            )
            .env("ANGELBOT_DRAFT_EXPECTED_RUNTIME_ID", expected_runtime_id)
            .env("ANGELBOT_FIELD_LABEL", field_label)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                DesktopAdapterError::new(
                    "ADAPTER_FAILED",
                    format!("Failed to start Windows UI Automation: {error}"),
                )
            })?;
        wait_for_draft_child(&mut child, is_cancelled, timeout.min(DRAFT_TIMEOUT))?;
        let output = child.wait_with_output().map_err(|error| {
            DesktopAdapterError::new(
                "RESULT_UNKNOWN",
                format!("Text result could not be collected; inspect the field before retrying: {error}"),
            )
        })?;
        let (status, detail) =
            classify_draft_helper_result(output.status.success(), &output.stdout, &output.stderr)?;
        Ok(DesktopActionResult {
            adapter: self.name().to_string(),
            status,
            target: app_id.to_string(),
            detail,
            process_id: None,
            action: None,
        })
    }
}

#[cfg(windows)]
impl DesktopAdapter for WindowsDesktopAdapter {
    fn name(&self) -> &str {
        "windows-native-uia"
    }

    fn inspect_draft_targets_for_settings(
        &self,
        app_id: &str,
        executable_path: &str,
    ) -> Result<DesktopDraftDiscovery, DesktopAdapterError> {
        let candidates = self.inspect_draft_targets(executable_path)?;
        Ok(DesktopDraftDiscovery {
            app_id: app_id.to_string(),
            candidates,
        })
    }

    fn preflight_draft(
        &self,
        app_id: &str,
        executable_path: &str,
        selector: &str,
        timeout: std::time::Duration,
    ) -> Result<DesktopDraftTargetPreview, DesktopAdapterError> {
        self.preflight_draft_target(app_id, executable_path, selector, timeout)
    }

    fn preflight_text_ref(
        &self,
        app_id: &str,
        executable_path: &str,
        field_ref: &str,
        timeout: std::time::Duration,
    ) -> Result<DesktopTextTargetPreview, DesktopAdapterError> {
        self.preflight_text_target(app_id, executable_path, field_ref, timeout)
    }

    fn preflight_control_ref(
        &self,
        app_id: &str,
        executable_path: &str,
        control_ref: &str,
        operation: DesktopControlOperation,
        timeout: std::time::Duration,
    ) -> Result<DesktopTextTargetPreview, DesktopAdapterError> {
        self.preflight_control_target(app_id, executable_path, control_ref, operation, timeout)
    }

    fn observe_trusted_window(
        &self,
        app_id: &str,
        executable_path: &str,
    ) -> Result<DesktopWindowObservation, DesktopAdapterError> {
        self.observe_trusted_window_with_control(
            app_id,
            executable_path,
            &|| false,
            DISCOVERY_TIMEOUT,
        )
    }

    fn observe_trusted_window_with_control(
        &self,
        app_id: &str,
        executable_path: &str,
        is_cancelled: &dyn Fn() -> bool,
        timeout: std::time::Duration,
    ) -> Result<DesktopWindowObservation, DesktopAdapterError> {
        use std::process::Stdio;
        let _uia_operation = self.begin_uia_operation()?;

        if is_cancelled() || timeout.is_zero() {
            return Err(DesktopAdapterError::new(
                "SCAN_CANCELLED",
                "Desktop observation was cancelled",
            ));
        }
        let executable = Self::canonical_executable(executable_path)?;
        let process_executable = Self::process_comparison_path(&executable);
        let script =
            format!("{UIA_SAFETY_HELPERS_SCRIPT}\n{TRUSTED_WINDOW_SCRIPT}\n{OBSERVATION_SCRIPT}");
        let mut child = desktop_child_command(Self::system_powershell()?)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .env("ANGELBOT_TARGET_EXE", process_executable)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                DesktopAdapterError::new(
                    "ADAPTER_FAILED",
                    format!("Failed to start Windows UI Automation: {error}"),
                )
            })?;
        wait_for_read_only_child_with_control(
            &mut child,
            "Desktop observation",
            is_cancelled,
            timeout,
        )?;
        let output = child.wait_with_output().map_err(|error| {
            DesktopAdapterError::new(
                "ADAPTER_FAILED",
                format!("Failed to collect UI Automation result: {error}"),
            )
        })?;
        if !output.status.success() {
            return Err(parse_uia_helper_error(&output.stderr));
        }
        if is_cancelled() {
            return Err(DesktopAdapterError::new(
                "SCAN_CANCELLED",
                "Desktop observation was cancelled",
            ));
        }
        let (mut payload, targets) = parse_private_observation_payload(&output.stdout)?;
        self.issue_field_refs(
            app_id,
            &executable,
            payload.truncated,
            &mut payload.controls,
            targets.clone(),
        )?;
        self.issue_control_refs(
            app_id,
            &executable,
            payload.truncated,
            &mut payload.controls,
            &targets,
        )?;
        Ok(DesktopWindowObservation {
            app_id: app_id.to_string(),
            controls: payload.controls,
            truncated: payload.truncated,
            diagnostics: payload.diagnostics,
        })
    }

    fn capture_trusted_window_with_control(
        &self,
        _app_id: &str,
        executable_path: &str,
        is_cancelled: &dyn Fn() -> bool,
        timeout: std::time::Duration,
    ) -> Result<DesktopWindowImage, DesktopAdapterError> {
        let _uia_operation = self.begin_uia_operation()?;
        if is_cancelled() {
            return Err(crate::window_capture::error("SCAN_CANCELLED"));
        }
        let started = std::time::Instant::now();
        let timeout = timeout.min(DISCOVERY_TIMEOUT);
        if timeout.is_zero() {
            return Err(crate::window_capture::error("SCAN_TIMEOUT"));
        }
        let executable = Self::canonical_executable(executable_path)?;
        let comparison = Self::process_comparison_path(&executable);
        let target = capture_target_probe(
            &comparison,
            is_cancelled,
            timeout.saturating_sub(started.elapsed()),
        )?;
        let image = crate::window_capture::capture_with_control(
            &target,
            is_cancelled,
            timeout.saturating_sub(started.elapsed()),
        )?;
        if is_cancelled() {
            return Err(crate::window_capture::error("SCAN_CANCELLED"));
        }
        // No control lease is issued. Successful capture is only an observation.
        Ok(image)
    }

    fn execute(&self, action: &DesktopAction) -> Result<DesktopActionResult, DesktopAdapterError> {
        match action {
            DesktopAction::OpenApp {
                app_id,
                executable_path,
                ..
            } => {
                let executable = Self::canonical_executable(executable_path)?;
                let mut command = desktop_child_command(executable);
                let child = command.spawn().map_err(|e| {
                    DesktopAdapterError::new(
                        "LAUNCH_FAILED",
                        format!("Failed to launch trusted application: {e}"),
                    )
                })?;
                Ok(DesktopActionResult {
                    adapter: self.name().into(),
                    status: "dispatched".into(),
                    target: app_id.clone(),
                    detail: "Trusted application launch was dispatched".into(),
                    process_id: Some(child.id()),
                    action: None,
                })
            }
            DesktopAction::OpenSettings { page, uri } => {
                let mut command = desktop_child_command(Self::system_explorer()?);
                let child = command.arg(uri).spawn().map_err(|e| {
                    DesktopAdapterError::new(
                        "LAUNCH_FAILED",
                        format!("Failed to open Windows Settings: {e}"),
                    )
                })?;
                Ok(DesktopActionResult {
                    adapter: self.name().into(),
                    status: "dispatched".into(),
                    target: page.clone(),
                    detail: "Windows Settings launch was dispatched".into(),
                    process_id: Some(child.id()),
                    action: None,
                })
            }
            DesktopAction::RevealPath { path, is_directory } => {
                let canonical = std::fs::canonicalize(path).map_err(|error| {
                    DesktopAdapterError::new(
                        "TARGET_UNAVAILABLE",
                        format!("Workspace item is unavailable: {error}"),
                    )
                })?;
                let explorer_path = Self::process_comparison_path(&canonical);
                let mut command = desktop_child_command(Self::system_explorer()?);
                if *is_directory {
                    command.arg(&explorer_path);
                } else {
                    command.arg(format!("/select,{explorer_path}"));
                }
                let child = command.spawn().map_err(|error| {
                    DesktopAdapterError::new(
                        "LAUNCH_FAILED",
                        format!("Failed to reveal workspace item: {error}"),
                    )
                })?;
                Ok(DesktopActionResult {
                    adapter: self.name().into(),
                    status: "dispatched".into(),
                    target: path.clone(),
                    detail: "Windows File Explorer reveal was dispatched".into(),
                    process_id: Some(child.id()),
                    action: None,
                })
            }
            DesktopAction::PrepareDraft {
                app_id,
                executable_path,
                selector,
                text,
                expected_target,
                ..
            } => self.prepare_draft(
                app_id,
                executable_path,
                selector,
                text,
                expected_target.as_ref(),
                &|| false,
                DRAFT_TIMEOUT,
            ),
            DesktopAction::SetText {
                app_id,
                executable_path,
                text,
                expected_target,
                ..
            } => self.set_text(
                app_id,
                executable_path,
                text,
                expected_target.as_ref(),
                &|| false,
                DRAFT_TIMEOUT,
            ),
            DesktopAction::OperateControl {
                app_id,
                executable_path,
                expected_target,
                operation,
                ..
            } => self.operate_control(
                app_id,
                executable_path,
                *operation,
                expected_target.as_ref(),
                &|| false,
                DRAFT_TIMEOUT,
            ),
        }
    }

    fn execute_with_control(
        &self,
        action: &DesktopAction,
        is_cancelled: &dyn Fn() -> bool,
        timeout: std::time::Duration,
    ) -> Result<DesktopActionResult, DesktopAdapterError> {
        match action {
            DesktopAction::PrepareDraft {
                app_id,
                executable_path,
                selector,
                text,
                expected_target,
                ..
            } => self.prepare_draft(
                app_id,
                executable_path,
                selector,
                text,
                expected_target.as_ref(),
                is_cancelled,
                timeout,
            ),
            DesktopAction::SetText {
                app_id,
                executable_path,
                text,
                expected_target,
                ..
            } => self.set_text(
                app_id,
                executable_path,
                text,
                expected_target.as_ref(),
                is_cancelled,
                timeout,
            ),
            DesktopAction::OperateControl {
                app_id,
                executable_path,
                expected_target,
                operation,
                ..
            } => self.operate_control(
                app_id,
                executable_path,
                *operation,
                expected_target.as_ref(),
                is_cancelled,
                timeout,
            ),
            _ => self.execute(action),
        }
    }
}

pub fn create_runtime_adapter() -> Arc<dyn DesktopAdapter> {
    #[cfg(windows)]
    {
        Arc::new(WindowsDesktopAdapter::default())
    }
    #[cfg(not(windows))]
    {
        Arc::new(UnsupportedDesktopAdapter)
    }
}

/// Debug HTTP stays deterministic by default. A developer must opt in
/// explicitly for an isolated Windows smoke test.
pub fn create_dev_adapter() -> Arc<dyn DesktopAdapter> {
    if std::env::var("ANGELBOT_DEV_DESKTOP_CONTROL").as_deref() == Ok("1") {
        create_runtime_adapter()
    } else {
        create_mock_adapter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    const FIXTURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

    #[cfg(windows)]
    fn run_fixture_commands(
        mut commands: Vec<std::process::Command>,
    ) -> Vec<crate::window_capture::BoundedChildOutput> {
        // Overlap slow PowerShell/CLR startup, keeping each fixture process-local.
        // The canonical runner serializes Rust tests, so batches do not overlap.
        let mut outputs = Vec::with_capacity(commands.len());
        for batch in commands.chunks_mut(4) {
            std::thread::scope(|scope| {
                let workers: Vec<_> = batch
                    .iter_mut()
                    .map(|command| {
                        scope.spawn(move || {
                            command
                                .stdin(std::process::Stdio::null())
                                .stdout(std::process::Stdio::piped())
                                .stderr(std::process::Stdio::piped());
                            crate::window_capture::run_bounded_child(
                                command,
                                None,
                                8192,
                                &|| false,
                                FIXTURE_TIMEOUT,
                            )
                            .expect("synthetic PowerShell fixture failed")
                        })
                    })
                    .collect();
                outputs.extend(workers.into_iter().map(|worker| worker.join().unwrap()));
            });
        }
        assert_eq!(outputs.len(), commands.len());
        outputs
    }

    #[cfg(windows)]
    #[test]
    fn capture_window_selector_enumerates_hwnds_without_changing_control_observation() {
        assert!(CAPTURE_WINDOW_SCRIPT.contains("EnumWindows("));
        assert!(CAPTURE_WINDOW_SCRIPT.contains("IsWindowVisible(hwnd)"));
        assert!(CAPTURE_WINDOW_SCRIPT.contains("GetWindow(hwnd, 4) == IntPtr.Zero"));
        assert!(CAPTURE_WINDOW_SCRIPT.contains("GetWindowThreadProcessId"));
        assert!(!CAPTURE_WINDOW_SCRIPT.contains("MainWindowHandle"));
        assert!(!CAPTURE_WINDOW_SCRIPT.contains("MainWindowTitle"));
        assert!(TRUSTED_WINDOW_SCRIPT.contains("MainWindowHandle"));
        assert!(CAPTURE_WINDOW_SELECTION_SCRIPT.contains("$matches.Count -ne 1"));
        assert!(!CAPTURE_WINDOW_SELECTION_SCRIPT.contains("Select-Object -First"));
    }

    #[cfg(windows)]
    #[test]
    fn capture_window_selector_rejects_same_pid_multiple_hwnds_using_synthetic_candidates() {
        let fixture = r#"
$ErrorActionPreference = 'Stop'
$trusted = 'C:\fixture\trusted.exe'
$first = @{ visible = $true; ownerless = $true; processId = 42; windowHandle = 100; executablePath = $trusted }
$second = @{ visible = $true; ownerless = $true; processId = 42; windowHandle = 200; executablePath = $trusted }
$other = @{ visible = $true; ownerless = $true; processId = 99; windowHandle = 300; executablePath = 'C:\fixture\other.exe' }
$hidden = @{ visible = $false; ownerless = $true; processId = 42; windowHandle = 400; executablePath = $trusted }
$owned = @{ visible = $true; ownerless = $false; processId = 42; windowHandle = 500; executablePath = $trusted }
function Test-Selection($candidates) {
  try {
    $selected = Select-CaptureWindow -candidates $candidates -expected $trusted
    return [long]$selected.windowHandle
  } catch { return $_.Exception.Message.Split('|')[0] }
}
@{
  sameProcess = Test-Selection @($first, $second, $other)
  unique = Test-Selection @($first, $other, $hidden, $owned)
  otherOnly = Test-Selection @($other, $hidden, $owned)
} | ConvertTo-Json -Compress
"#;
        let script = format!("{CAPTURE_WINDOW_SELECTION_SCRIPT}\n{fixture}");
        let mut command =
            desktop_child_command(WindowsDesktopAdapter::system_powershell().unwrap());
        command
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let output = crate::window_capture::run_bounded_child(
            &mut command,
            None,
            8192,
            &|| false,
            FIXTURE_TIMEOUT,
        )
        .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["sameProcess"], "TARGET_AMBIGUOUS");
        assert_eq!(result["unique"], 100);
        assert_eq!(result["otherOnly"], "TARGET_NOT_FOUND");
    }

    #[test]
    fn observation_payload_is_bounded_and_never_accepts_values() {
        let payload = parse_observation_payload(
            br#"{"controls":[{"depth":1,"role":"button","name":"Send","automationId":"sendButton","enabled":true,"capabilities":["invoke"]},{"depth":2,"role":"edit","enabled":true,"capabilities":["setValue"]}],"truncated":true}"#,
        )
        .unwrap();
        assert_eq!(payload.controls.len(), 2);
        assert_eq!(payload.controls[0].depth, 1);
        assert_eq!(payload.controls[1].depth, 2);
        assert_eq!(payload.controls[0].name.as_deref(), Some("Send"));
        assert_eq!(payload.controls[1].name, None);
        assert!(payload.truncated);
        for raw in [
            br#"{"controls":[{"depth":1,"role":"edit","name":"private text","enabled":true,"capabilities":[]}],"truncated":false}"#.as_slice(),
            br#"{"controls":[{"depth":1,"role":"button","value":"private text","enabled":true,"capabilities":[]}],"truncated":false}"#.as_slice(),
            br#"{"controls":[{"depth":1,"role":"button","isPassword":true,"enabled":true,"capabilities":[]}],"truncated":false}"#.as_slice(),
            br#"{"controls":[{"depth":1,"role":"button","name":"hidden\ntext","enabled":true,"capabilities":[]}],"truncated":false}"#.as_slice(),
            br#"{"controls":[{"depth":1,"role":"button","enabled":false,"capabilities":["invoke"]}],"truncated":false}"#.as_slice(),
            br#"{"controls":[{"depth":1,"role":"button","enabled":true,"capabilities":["setValue"]}],"truncated":false}"#.as_slice(),
            br#"{"controls":[{"depth":1,"role":"button","enabled":true,"capabilities":["invoke","invoke"]}],"truncated":false}"#.as_slice(),
            br#"{"controls":[{"depth":1,"role":"unknown","enabled":true,"capabilities":[]}],"truncated":false}"#.as_slice(),
            br#"{"controls":[{"depth":1,"role":"button","automationId":" bad ","enabled":true,"capabilities":[]}],"truncated":false}"#.as_slice(),
            br#"{"controls":[{"depth":0,"role":"button","enabled":true,"capabilities":[]}],"truncated":false}"#.as_slice(),
            br#"{"controls":[]}"#.as_slice(),
        ] {
            assert!(parse_observation_payload(raw).is_err());
        }
        let excessive = serde_json::json!({
            "controls": (0..=MAX_OBSERVED_CONTROLS)
                .map(|_| serde_json::json!({"depth": 1, "role": "button", "enabled": true, "capabilities": []}))
                .collect::<Vec<_>>(),
            "truncated": true
        });
        assert_eq!(
            parse_observation_payload(excessive.to_string().as_bytes())
                .unwrap_err()
                .code,
            "SCAN_LIMIT"
        );
        assert_eq!(
            parse_observation_payload(&vec![b' '; MAX_OBSERVATION_BYTES + 1])
                .unwrap_err()
                .code,
            "SCAN_LIMIT"
        );
    }

    #[test]
    fn observation_diagnostics_are_bounded_unique_and_require_partial_results() {
        let reasons = [
            DesktopObservationDiagnostic::ControlLimit,
            DesktopObservationDiagnostic::OutputLimit,
            DesktopObservationDiagnostic::NodeLimit,
            DesktopObservationDiagnostic::DepthLimit,
            DesktopObservationDiagnostic::ProviderUnavailable,
        ];
        let raw = serde_json::json!({
            "controls": [],
            "truncated": true,
            "diagnostics": reasons,
        });
        let payload = parse_observation_payload(raw.to_string().as_bytes()).unwrap();
        assert_eq!(payload.diagnostics, reasons);
        let public = serde_json::to_value(&payload).unwrap();
        assert_eq!(
            public["diagnostics"],
            serde_json::json!([
                "controlLimit",
                "outputLimit",
                "nodeLimit",
                "depthLimit",
                "providerUnavailable"
            ])
        );
        for invalid in [
            serde_json::json!({"controls": [], "truncated": false, "diagnostics": ["providerUnavailable"]}),
            serde_json::json!({"controls": [], "truncated": true, "diagnostics": ["providerUnavailable", "providerUnavailable"]}),
            serde_json::json!({"controls": [], "truncated": true, "diagnostics": ["private provider error"]}),
            serde_json::json!({"controls": [], "truncated": true, "diagnostics": vec!["nodeLimit"; MAX_OBSERVATION_DIAGNOSTICS + 1]}),
        ] {
            assert_eq!(
                parse_observation_payload(invalid.to_string().as_bytes())
                    .unwrap_err()
                    .code,
                "ADAPTER_FAILED"
            );
        }
        let legacy = parse_observation_payload(br#"{"controls":[],"truncated":false}"#).unwrap();
        assert!(legacy.diagnostics.is_empty());
        assert!(serde_json::to_value(legacy)
            .unwrap()
            .get("diagnostics")
            .is_none());
    }

    #[test]
    fn mock_observation_exposes_only_an_opaque_text_field_reference() {
        let adapter = MockDesktopAdapter::default();
        let snapshot = adapter
            .observe_trusted_window("trusted-chat", r"C:\trusted\chat.exe")
            .unwrap();
        assert_eq!(snapshot.app_id, "trusted-chat");
        assert_eq!(snapshot.controls.len(), 5);
        assert_eq!(
            snapshot.controls[0].field_ref.as_deref(),
            Some("mock-field-ref")
        );
        assert_eq!(snapshot.controls[0].name, None);
        assert_eq!(snapshot.controls[0].control_ref, None);
        assert_eq!(
            snapshot.controls[1].control_ref.as_deref(),
            Some("mock-control-ref")
        );
        assert_eq!(snapshot.controls[1].field_ref, None);
        assert!(!snapshot.truncated);
        assert!(snapshot.diagnostics.is_empty());
        let public = serde_json::to_string(&snapshot).unwrap();
        assert!(public.contains("mock-field-ref"));
        assert!(!public.contains("processId"));
        assert!(!public.contains("runtimeId"));
        assert!(adapter.actions().is_empty());
    }

    #[test]
    fn mock_text_ref_requires_exact_reference_and_backend_target() {
        let adapter = MockDesktopAdapter::default();
        adapter.observe_trusted_window("chat", "chat.exe").unwrap();
        assert_eq!(
            adapter
                .preflight_text_ref(
                    "chat",
                    "chat.exe",
                    "wrong",
                    std::time::Duration::from_secs(1)
                )
                .unwrap_err()
                .code,
            "TARGET_CHANGED"
        );
        let preview = adapter
            .preflight_text_ref(
                "chat",
                "chat.exe",
                "mock-field-ref",
                std::time::Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(preview.app_id, "chat");
        assert_eq!(preview.control_label, "mockField");
        assert_eq!(preview.identity.automation_id.as_deref(), Some("mockField"));
        let mut action = DesktopAction::SetText {
            app_id: "chat".into(),
            display_name: "Chat".into(),
            executable_path: "chat.exe".into(),
            text: "hello".into(),
            expected_target: None,
        };
        assert_eq!(adapter.execute(&action).unwrap_err().code, "TARGET_CHANGED");
        if let DesktopAction::SetText {
            expected_target, ..
        } = &mut action
        {
            let mut unlabeled = preview.identity.clone();
            unlabeled.automation_id = None;
            *expected_target = Some(unlabeled);
        }
        assert_eq!(adapter.execute(&action).unwrap_err().code, "TARGET_CHANGED");
        if let DesktopAction::SetText {
            expected_target, ..
        } = &mut action
        {
            *expected_target = Some(preview.identity);
        }
        assert_eq!(adapter.execute(&action).unwrap().status, "verified");
        assert_eq!(adapter.actions(), vec![action]);
    }

    #[test]
    fn mock_invoke_requires_observation_confirmation_and_one_attempt() {
        let adapter = MockDesktopAdapter::default();
        let timeout = std::time::Duration::from_secs(1);
        assert_eq!(
            adapter
                .preflight_control_ref(
                    "app",
                    "app.exe",
                    "mock-control-ref",
                    DesktopControlOperation::Invoke,
                    timeout
                )
                .unwrap_err()
                .code,
            "TARGET_CHANGED"
        );
        adapter.observe_trusted_window("app", "app.exe").unwrap();
        for (app, exe, reference) in [
            ("other", "app.exe", "mock-control-ref"),
            ("app", "other.exe", "mock-control-ref"),
            ("app", "app.exe", "mock-field-ref"),
        ] {
            assert_eq!(
                adapter
                    .preflight_control_ref(
                        app,
                        exe,
                        reference,
                        DesktopControlOperation::Invoke,
                        timeout
                    )
                    .unwrap_err()
                    .code,
                "TARGET_CHANGED"
            );
        }
        let preview = adapter
            .preflight_control_ref(
                "app",
                "app.exe",
                "mock-control-ref",
                DesktopControlOperation::Invoke,
                timeout,
            )
            .unwrap();
        assert_eq!(preview.control_label, "mockButton");
        let mut action = DesktopAction::OperateControl {
            app_id: "app".into(),
            display_name: "Application".into(),
            executable_path: "app.exe".into(),
            operation: DesktopControlOperation::Invoke,
            expected_target: None,
        };
        assert_eq!(adapter.execute(&action).unwrap_err().code, "TARGET_CHANGED");
        if let DesktopAction::OperateControl {
            expected_target, ..
        } = &mut action
        {
            let mut changed = preview.identity.clone();
            changed.control_runtime_id = vec![3];
            *expected_target = Some(changed);
        }
        assert_eq!(adapter.execute(&action).unwrap_err().code, "TARGET_CHANGED");
        if let DesktopAction::OperateControl {
            expected_target, ..
        } = &mut action
        {
            *expected_target = Some(preview.identity);
        }
        assert_eq!(
            adapter
                .execute_with_control(&action, &|| true, timeout)
                .unwrap_err()
                .code,
            "CANCELLED"
        );
        assert!(adapter.actions().is_empty());
        let result = adapter.execute(&action).unwrap();
        assert_eq!(result.status, "dispatched");
        assert!(result.detail.contains("not verified"));
        assert_eq!(adapter.execute(&action).unwrap_err().code, "TARGET_CHANGED");
        assert_eq!(
            adapter
                .preflight_control_ref(
                    "app",
                    "app.exe",
                    "mock-control-ref",
                    DesktopControlOperation::Invoke,
                    timeout
                )
                .unwrap_err()
                .code,
            "TARGET_CHANGED"
        );
        assert_eq!(adapter.actions(), vec![action]);
    }

    #[cfg(windows)]
    #[test]
    fn invoke_refs_bind_exact_observed_role_name_and_single_use_identity() {
        let raw = serde_json::json!({
            "controls": [
                {"depth":1,"role":"button","name":"Apply","enabled":true,"capabilities":["invoke"]},
                {"depth":1,"role":"hyperlink","name":"Details","enabled":true,"capabilities":["invoke"]},
                {"depth":1,"role":"menuItem","name":"Refresh","enabled":true,"capabilities":["invoke"]},
                {"depth":1,"role":"button","name":"Disabled","enabled":false,"capabilities":[]},
                {"depth":1,"role":"button","enabled":true,"capabilities":["invoke"]},
                {"depth":1,"role":"button","name":"No pattern","enabled":true,"capabilities":[]},
                {"depth":1,"role":"checkBox","name":"Other role","enabled":true,"capabilities":["toggle"]},
            ],
            "truncated": false,
            "targets": (0..3).map(|index| serde_json::json!({"index":index,"identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[index as i32 + 1]}})).collect::<Vec<_>>()
        });
        let (mut payload, targets) =
            parse_private_observation_payload(raw.to_string().as_bytes()).unwrap();
        let adapter = WindowsDesktopAdapter::default();
        let exe = std::path::Path::new(r"C:\trusted\app.exe");
        adapter
            .issue_field_refs("app", exe, false, &mut payload.controls, targets.clone())
            .unwrap();
        adapter
            .issue_control_refs("app", exe, false, &mut payload.controls, &targets)
            .unwrap();
        assert!(payload
            .controls
            .iter()
            .all(|control| control.field_ref.is_none()));
        for (index, expected_role) in ["button", "hyperlink", "menuItem"].iter().enumerate() {
            let reference = payload.controls[index].control_ref.as_deref().unwrap();
            assert_eq!(reference.len(), 40);
            let lease = adapter
                .resolve_control_ref("app", exe, reference, None)
                .unwrap();
            assert_eq!(&lease.role, expected_role);
            assert_eq!(
                Some(&lease.control_label),
                payload.controls[index].name.as_ref()
            );
            assert_eq!(lease.identity.automation_id.as_deref(), Some(reference));
            assert_eq!(
                adapter
                    .resolve_control_ref("other", exe, reference, None)
                    .unwrap_err()
                    .code,
                "TARGET_CHANGED"
            );
            assert_eq!(
                adapter
                    .resolve_control_ref("app", std::path::Path::new("other.exe"), reference, None)
                    .unwrap_err()
                    .code,
                "TARGET_CHANGED"
            );
            let mut changed = lease.identity.clone();
            changed.control_operation = Some(DesktopControlOperation::Invoke);
            changed.window_handle += 1;
            assert_eq!(
                adapter
                    .resolve_control_ref("app", exe, reference, Some(&changed))
                    .unwrap_err()
                    .code,
                "TARGET_CHANGED"
            );
            let attested = DesktopDraftTargetIdentity {
                control_operation: Some(DesktopControlOperation::Invoke),
                ..lease.identity
            };
            adapter
                .resolve_control_ref("app", exe, reference, Some(&attested))
                .unwrap();
            assert_eq!(
                adapter
                    .resolve_control_ref("app", exe, reference, None)
                    .unwrap_err()
                    .code,
                "TARGET_CHANGED"
            );
        }
        assert!(payload.controls[3..]
            .iter()
            .all(|control| control.control_ref.is_none()));
        for index in 0..7 {
            let mut invalid = raw.clone();
            invalid["targets"] = serde_json::json!([{"index":index,"identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[1]}}]);
            if index >= 3 {
                assert!(parse_private_observation_payload(invalid.to_string().as_bytes()).is_err());
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn partial_observations_duplicate_identity_and_expired_leases_cannot_invoke() {
        let adapter = WindowsDesktopAdapter::default();
        let exe = std::path::Path::new("fixture.exe");
        for reason in [
            "controlLimit",
            "outputLimit",
            "nodeLimit",
            "depthLimit",
            "providerUnavailable",
        ] {
            let raw = serde_json::json!({"controls":[{"depth":1,"role":"button","name":"Apply","enabled":true,"capabilities":["invoke"]}],"truncated":true,"diagnostics":[reason],"targets":[{"index":0,"identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[1]}}]});
            let (mut payload, targets) =
                parse_private_observation_payload(raw.to_string().as_bytes()).unwrap();
            adapter
                .issue_control_refs(
                    "app",
                    exe,
                    payload.truncated,
                    &mut payload.controls,
                    &targets,
                )
                .unwrap();
            assert!(payload.controls[0].control_ref.is_none(), "{reason}");
        }
        assert!(adapter.control_refs.lock().unwrap().is_empty());
        let raw = br#"{"controls":[{"depth":1,"role":"button","name":"Apply","enabled":true,"capabilities":["invoke"]},{"depth":1,"role":"menuItem","name":"Apply","enabled":true,"capabilities":["invoke"]}],"truncated":false,"targets":[{"index":0,"identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[1]}},{"index":1,"identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[1]}}]}"#;
        let (mut payload, mut targets) = parse_private_observation_payload(raw).unwrap();
        adapter
            .issue_control_refs("app", exe, false, &mut payload.controls, &targets)
            .unwrap();
        assert!(payload
            .controls
            .iter()
            .all(|control| control.control_ref.is_none()));
        targets.pop();
        adapter
            .issue_control_refs("app", exe, false, &mut payload.controls, &targets)
            .unwrap();
        let reference = payload.controls[0].control_ref.clone().unwrap();
        adapter
            .control_refs
            .lock()
            .unwrap()
            .get_mut(&reference)
            .unwrap()
            .expires_at = std::time::Instant::now() - std::time::Duration::from_secs(1);
        assert_eq!(
            adapter
                .resolve_control_ref("app", exe, &reference, None)
                .unwrap_err()
                .code,
            "TARGET_CHANGED"
        );
        assert!(adapter.control_refs.lock().unwrap().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn native_uia_gate_rejects_cross_workspace_overlap_before_provider_or_ref_consumption() {
        let adapter = Arc::new(WindowsDesktopAdapter::default());
        let exe = std::path::Path::new("fixture.exe");
        let raw = br#"{"controls":[{"depth":1,"role":"edit","automationId":"field","enabled":true,"capabilities":["setValue"]},{"depth":1,"role":"menuItem","name":"Menu","enabled":true,"capabilities":["invoke","expandCollapse"]}],"truncated":false,"targets":[{"index":0,"identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[1]}},{"index":1,"identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[2]}}]}"#;
        let (mut payload, targets) = parse_private_observation_payload(raw).unwrap();
        adapter
            .issue_field_refs("app", exe, false, &mut payload.controls, targets.clone())
            .unwrap();
        adapter
            .issue_control_refs("app", exe, false, &mut payload.controls, &targets)
            .unwrap();
        let field_ref = payload.controls[0].field_ref.clone().unwrap();
        let control_ref = payload.controls[1].control_ref.clone().unwrap();
        let field_lease = adapter.resolve_field_ref("app", exe, &field_ref).unwrap();
        let control_lease = adapter
            .resolve_control_ref("app", exe, &control_ref, None)
            .unwrap();
        let gate_ready = Arc::new(std::sync::Barrier::new(2));
        let gate_release = Arc::new(std::sync::Barrier::new(2));
        let holder = {
            let adapter = adapter.clone();
            let ready = gate_ready.clone();
            let release = gate_release.clone();
            std::thread::spawn(move || {
                let _gate = adapter.begin_uia_operation().unwrap();
                ready.wait();
                release.wait();
            })
        };
        gate_ready.wait();
        let timeout = std::time::Duration::from_secs(1);
        let busy = |error: DesktopAdapterError| assert_eq!(error.code, "TARGET_BUSY");
        // These enter through the adapter's public seam. The nonexistent path
        // can never be inspected because overlap is rejected first.
        let settings_adapter: &dyn DesktopAdapter = adapter.as_ref();
        busy(
            settings_adapter
                .inspect_draft_targets_for_settings("app", "fixture.exe")
                .unwrap_err(),
        );
        busy(
            adapter
                .observe_trusted_window("app", "fixture.exe")
                .unwrap_err(),
        );
        busy(
            adapter
                .observe_trusted_window_with_control("app", "fixture.exe", &|| false, timeout)
                .unwrap_err(),
        );
        busy(
            adapter
                .capture_trusted_window_with_control("app", "fixture.exe", &|| false, timeout)
                .unwrap_err(),
        );
        busy(
            adapter
                .preflight_draft("app", "fixture.exe", "Message", timeout)
                .unwrap_err(),
        );
        busy(
            adapter
                .preflight_text_ref("app", "fixture.exe", &field_ref, timeout)
                .unwrap_err(),
        );
        busy(
            adapter
                .preflight_control_ref(
                    "app",
                    "fixture.exe",
                    &control_ref,
                    DesktopControlOperation::Invoke,
                    timeout,
                )
                .unwrap_err(),
        );
        let actions = [
            DesktopAction::PrepareDraft {
                app_id: "app".into(),
                display_name: "App".into(),
                executable_path: "fixture.exe".into(),
                selector: "Message".into(),
                text: "hello".into(),
                expected_target: Some(field_lease.identity.clone()),
            },
            DesktopAction::SetText {
                app_id: "app".into(),
                display_name: "App".into(),
                executable_path: "fixture.exe".into(),
                text: "hello".into(),
                expected_target: Some(DesktopDraftTargetIdentity {
                    automation_id: Some("field".into()),
                    ..field_lease.identity.clone()
                }),
            },
            DesktopAction::OperateControl {
                app_id: "app".into(),
                display_name: "App".into(),
                executable_path: "fixture.exe".into(),
                operation: DesktopControlOperation::Invoke,
                expected_target: Some(DesktopDraftTargetIdentity {
                    control_operation: Some(DesktopControlOperation::Invoke),
                    ..control_lease.identity.clone()
                }),
            },
        ];
        for action in actions {
            busy(adapter.execute(&action).unwrap_err());
            busy(
                adapter
                    .execute_with_control(&action, &|| false, timeout)
                    .unwrap_err(),
            );
        }
        // Contention must not consume either pending selector or invalidate the
        // original observation. No PowerShell helper is launched by this test.
        assert_eq!(
            adapter
                .resolve_field_ref("app", exe, &field_ref)
                .unwrap()
                .identity,
            field_lease.identity
        );
        assert_eq!(
            adapter
                .resolve_control_ref("app", exe, &control_ref, None)
                .unwrap()
                .identity,
            control_lease.identity
        );
        gate_release.wait();
        holder.join().unwrap();
        assert!(adapter.begin_uia_operation().is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn poisoned_native_uia_gate_fails_closed() {
        let adapter = WindowsDesktopAdapter::default();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _gate = adapter.begin_uia_operation().unwrap();
            panic!("deterministic gate poison");
        }));
        assert!(panic.is_err());
        assert_eq!(
            adapter.begin_uia_operation().unwrap_err().code,
            "ADAPTER_FAILED"
        );
        assert_eq!(
            adapter
                .observe_trusted_window("app", "fixture.exe")
                .unwrap_err()
                .code,
            "ADAPTER_FAILED"
        );
    }

    #[cfg(windows)]
    #[test]
    fn control_operations_are_observed_attested_and_invalidate_app_descendants() {
        for (role, capabilities, expected) in [
            (
                "radioButton",
                vec!["select"],
                vec![DesktopControlOperation::Select],
            ),
            (
                "tabItem",
                vec!["select"],
                vec![DesktopControlOperation::Select],
            ),
            (
                "menuItem",
                vec!["invoke", "expandCollapse"],
                vec![
                    DesktopControlOperation::Invoke,
                    DesktopControlOperation::Expand,
                    DesktopControlOperation::Collapse,
                ],
            ),
            ("listItem", vec!["select"], vec![]),
            ("comboBox", vec!["expandCollapse"], vec![]),
            (
                "pane",
                vec!["scroll"],
                vec![
                    DesktopControlOperation::ScrollUp,
                    DesktopControlOperation::ScrollDown,
                ],
            ),
            (
                "document",
                vec!["scroll"],
                vec![
                    DesktopControlOperation::ScrollUp,
                    DesktopControlOperation::ScrollDown,
                ],
            ),
            (
                "list",
                vec!["scroll"],
                vec![
                    DesktopControlOperation::ScrollUp,
                    DesktopControlOperation::ScrollDown,
                ],
            ),
            ("pane", vec![], vec![]),
        ] {
            let control = DesktopObservedControl {
                depth: 1,
                role: role.into(),
                name: Some("Label".into()),
                automation_id: None,
                enabled: true,
                capabilities: capabilities.into_iter().map(str::to_string).collect(),
                field_ref: None,
                control_ref: None,
            };
            assert_eq!(supported_control_operations(&control), expected, "{role}");
        }
        let raw = br#"{"controls":[{"depth":1,"role":"edit","automationId":"field","enabled":true,"capabilities":["setValue"]},{"depth":1,"role":"menuItem","name":"Menu","enabled":true,"capabilities":["invoke","expandCollapse"]}],"truncated":false,"targets":[{"index":0,"identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[1]}},{"index":1,"identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[2]}}]}"#;
        let (mut payload, targets) = parse_private_observation_payload(raw).unwrap();
        let adapter = WindowsDesktopAdapter::default();
        let exe = std::path::Path::new("fixture.exe");
        adapter
            .issue_field_refs("app", exe, false, &mut payload.controls, targets.clone())
            .unwrap();
        adapter
            .issue_control_refs("app", exe, false, &mut payload.controls, &targets)
            .unwrap();
        let field_ref = payload.controls[0].field_ref.clone().unwrap();
        let control_ref = payload.controls[1].control_ref.clone().unwrap();
        let lease = adapter
            .resolve_control_ref("app", exe, &control_ref, None)
            .unwrap();
        assert_eq!(lease.identity.control_operation, None);
        let mut unattested = lease.identity.clone();
        assert_eq!(
            adapter
                .resolve_control_ref("app", exe, &control_ref, Some(&unattested))
                .unwrap_err()
                .code,
            "UNSUPPORTED_CONTROL"
        );
        unattested.control_operation = Some(DesktopControlOperation::Select);
        assert_eq!(
            adapter
                .resolve_control_ref("app", exe, &control_ref, Some(&unattested))
                .unwrap_err()
                .code,
            "UNSUPPORTED_CONTROL"
        );
        adapter.invalidate_application_refs("other", exe).unwrap();
        adapter.resolve_field_ref("app", exe, &field_ref).unwrap();
        adapter
            .resolve_control_ref("app", exe, &control_ref, None)
            .unwrap();
        adapter.invalidate_application_refs("app", exe).unwrap();
        assert_eq!(
            adapter
                .resolve_field_ref("app", exe, &field_ref)
                .unwrap_err()
                .code,
            "TARGET_CHANGED"
        );
        assert_eq!(
            adapter
                .resolve_control_ref("app", exe, &control_ref, None)
                .unwrap_err()
                .code,
            "TARGET_CHANGED"
        );
        let mut invalid: serde_json::Value = serde_json::from_slice(raw).unwrap();
        invalid["targets"][1]["identity"]["controlOperation"] = serde_json::json!("invoke");
        assert!(parse_private_observation_payload(invalid.to_string().as_bytes()).is_err());
    }

    #[test]
    fn mock_control_operations_bind_action_and_report_only_narrow_control_state() {
        let adapter = MockDesktopAdapter::default();
        let timeout = std::time::Duration::from_secs(1);
        for (operation, reference) in [
            (DesktopControlOperation::Select, "mock-select-ref"),
            (DesktopControlOperation::Expand, "mock-expand-ref"),
            (DesktopControlOperation::Collapse, "mock-expand-ref"),
            (DesktopControlOperation::ScrollUp, "mock-scroll-ref"),
            (DesktopControlOperation::ScrollDown, "mock-scroll-ref"),
        ] {
            assert_eq!(serde_json::to_value(operation).unwrap(), operation.as_str());
            adapter.observe_trusted_window("app", "app.exe").unwrap();
            let preview = adapter
                .preflight_control_ref("app", "app.exe", reference, operation, timeout)
                .unwrap();
            assert_eq!(preview.identity.control_operation, Some(operation));
            let wrong_operation = if operation == DesktopControlOperation::Expand {
                DesktopControlOperation::Collapse
            } else {
                DesktopControlOperation::Expand
            };
            let mut action = DesktopAction::OperateControl {
                app_id: "app".into(),
                display_name: "App".into(),
                executable_path: "app.exe".into(),
                operation: wrong_operation,
                expected_target: Some(preview.identity),
            };
            assert!(matches!(
                adapter.execute(&action).unwrap_err().code.as_str(),
                "TARGET_CHANGED" | "UNSUPPORTED_CONTROL"
            ));
            if let DesktopAction::OperateControl {
                operation: actual, ..
            } = &mut action
            {
                *actual = operation;
            }
            let result = adapter.execute(&action).unwrap();
            assert_eq!(result.status, "verified");
            assert_eq!(result.action, Some(operation));
            assert!(result.detail.contains("task outcome is not verified"));
            assert_eq!(
                adapter
                    .preflight_text_ref("app", "app.exe", "mock-field-ref", timeout)
                    .unwrap_err()
                    .code,
                "TARGET_CHANGED"
            );
            assert_eq!(
                adapter
                    .preflight_control_ref("app", "app.exe", reference, operation, timeout)
                    .unwrap_err()
                    .code,
                "TARGET_CHANGED"
            );
        }
        assert!(serde_json::from_str::<DesktopControlOperation>("\"toggle\"").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn invoke_helper_distinguishes_pre_dispatch_failures_and_uncertain_effects() {
        for stderr in [
            b"RESULT_UNKNOWN|Invoke provider failure".as_slice(),
            b"Unhandled UIA failure".as_slice(),
            b"RESULT_UNKNOWN|TARGET_CHANGED|after invoke".as_slice(),
        ] {
            assert_eq!(
                classify_control_helper_result(DesktopControlOperation::Invoke, false, b"", stderr)
                    .unwrap_err()
                    .code,
                "RESULT_UNKNOWN"
            );
        }
        assert_eq!(
            classify_control_helper_result(
                DesktopControlOperation::Invoke,
                false,
                b"",
                b"TARGET_CHANGED|Name changed"
            )
            .unwrap_err()
            .code,
            "TARGET_CHANGED"
        );
        for stdout in [
            b"not-json".as_slice(),
            br#"{"status":"verified"}"#.as_slice(),
            br#"{"status":"failed"}"#.as_slice(),
        ] {
            assert_eq!(
                classify_control_helper_result(DesktopControlOperation::Invoke, true, stdout, b"")
                    .unwrap_err()
                    .code,
                "RESULT_UNKNOWN"
            );
        }
        let result = classify_control_helper_result(
            DesktopControlOperation::Invoke,
            true,
            br#"{"status":"dispatched","action":"invoke"}"#,
            b"",
        )
        .unwrap();
        assert_eq!(result.0, "dispatched");
        assert!(result.1.contains("not verified"));
        for operation in [
            DesktopControlOperation::Select,
            DesktopControlOperation::Expand,
            DesktopControlOperation::Collapse,
            DesktopControlOperation::ScrollUp,
            DesktopControlOperation::ScrollDown,
        ] {
            let raw = serde_json::json!({"status":"verified","action":operation});
            let result =
                classify_control_helper_result(operation, true, raw.to_string().as_bytes(), b"")
                    .unwrap();
            assert_eq!(result.0, "verified");
            assert!(result.1.contains("task outcome is not verified"));
            assert_eq!(
                classify_control_helper_result(
                    operation,
                    true,
                    br#"{"status":"verified","action":"invoke"}"#,
                    b""
                )
                .unwrap_err()
                .code,
                "RESULT_UNKNOWN"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn private_observation_issues_refs_only_for_unique_labeled_edits() {
        let raw = br#"{"controls":[{"depth":1,"role":"edit","automationId":"subject","enabled":true,"capabilities":["setValue"]},{"depth":1,"role":"edit","automationId":"same","enabled":true,"capabilities":["setValue"]},{"depth":1,"role":"edit","automationId":"same","enabled":true,"capabilities":["setValue"]},{"depth":1,"role":"edit","enabled":true,"capabilities":["setValue"]}],"truncated":false,"targets":[{"index":0,"identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[1]}},{"index":1,"identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[2]}},{"index":2,"identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[3]}}]}"#;
        let (mut public, targets) = parse_private_observation_payload(raw).unwrap();
        let adapter = WindowsDesktopAdapter::default();
        let exe = std::path::Path::new(r"C:\trusted\app.exe");
        let (mut partial, partial_targets) = parse_private_observation_payload(raw).unwrap();
        partial.truncated = true;
        adapter
            .issue_field_refs(
                "app",
                exe,
                partial.truncated,
                &mut partial.controls,
                partial_targets,
            )
            .unwrap();
        assert!(partial
            .controls
            .iter()
            .all(|control| control.field_ref.is_none()));
        adapter
            .issue_field_refs("app", exe, false, &mut public.controls, targets)
            .unwrap();
        let issued = public.controls[0].field_ref.clone().unwrap();
        assert!(issued.starts_with("field_"));
        assert!(public.controls[1..]
            .iter()
            .all(|control| control.field_ref.is_none()));
        assert_eq!(
            adapter
                .resolve_field_ref("other", exe, &issued)
                .unwrap_err()
                .code,
            "TARGET_CHANGED"
        );
        assert_eq!(
            adapter
                .resolve_field_ref("app", exe, &issued)
                .unwrap()
                .control_label,
            "subject",
            "a mismatched attempt must not invalidate the real app's selector"
        );
    }

    #[cfg(windows)]
    #[test]
    fn every_partial_observation_reason_suppresses_text_references() {
        let adapter = WindowsDesktopAdapter::default();
        let exe = std::path::Path::new(r"C:\trusted\app.exe");
        for reason in [
            "controlLimit",
            "outputLimit",
            "nodeLimit",
            "depthLimit",
            "providerUnavailable",
        ] {
            let raw = serde_json::json!({
                "controls": [{
                    "depth": 1,
                    "role": "edit",
                    "automationId": "subject",
                    "enabled": true,
                    "capabilities": ["setValue"],
                }],
                "truncated": true,
                "diagnostics": [reason],
                "targets": [{
                    "index": 0,
                    "identity": {"processId": 42, "windowHandle": 1234, "controlRuntimeId": [1]},
                }],
            });
            let (mut partial, targets) =
                parse_private_observation_payload(raw.to_string().as_bytes()).unwrap();
            assert_eq!(partial.diagnostics.len(), 1);
            adapter
                .issue_field_refs(
                    "app",
                    exe,
                    partial.truncated,
                    &mut partial.controls,
                    targets,
                )
                .unwrap();
            assert!(partial.controls[0].field_ref.is_none(), "reason: {reason}");
        }
        assert!(adapter.field_refs.lock().unwrap().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn observation_script_uses_bounded_read_only_traversal() {
        assert!(OBSERVATION_SCRIPT.contains("$visited -gt 256"));
        assert!(OBSERVATION_SCRIPT.contains("$entry.depth -ge 12"));
        assert!(OBSERVATION_SCRIPT.contains("$bytes.Length -gt 8192"));
        assert!(OBSERVATION_SCRIPT.contains("$targets"));
        assert!(OBSERVATION_SCRIPT.contains("$controls.Count -ge 32"));
        assert!(OBSERVATION_SCRIPT.contains("$script:truncated = $true"));
        assert!(OBSERVATION_SCRIPT.contains("$budgetDiagnostics"));
        for reason in ["controlLimit", "outputLimit", "nodeLimit", "depthLimit"] {
            assert!(OBSERVATION_SCRIPT.contains(&format!("Mark-ObservationPartial '{reason}'")));
        }
        assert!(OBSERVATION_SCRIPT
            .contains("catch { Mark-ObservationPartial 'providerUnavailable'; continue }"));
        assert!(OBSERVATION_SCRIPT
            .contains("catch { Mark-ObservationPartial 'providerUnavailable'; $roleName = '' }"));
        assert!(OBSERVATION_SCRIPT.contains("$isPassword -or $isOffscreen"));
        assert!(OBSERVATION_SCRIPT
            .contains("$null -eq $current -or $null -eq $isPassword -or $null -eq $isOffscreen"));
        assert!(!OBSERVATION_SCRIPT.contains("FindAll"));
        assert!(!OBSERVATION_SCRIPT.contains("TextPattern"));
        assert!(!OBSERVATION_SCRIPT.contains(".Current.Value"));
        assert!(!OBSERVATION_SCRIPT.contains("SetValue("));
    }

    #[cfg(windows)]
    const OBSERVATION_TEST_PROVIDER_FIXTURE: &str = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -TypeDefinition @'
public class ObservationTestControlType {
  public string ProgrammaticName = "ControlType.Button";
}
public class ObservationTestValueInfo {
  private ObservationTestElement element;
  public ObservationTestValueInfo(ObservationTestElement element) { this.element = element; }
  public bool IsReadOnly {
    get {
      if (element.Failure == "valueReadOnly") { throw new System.Exception("private provider details"); }
      return element.Failure == "readOnly";
    }
  }
  public string Value {
    get {
      if (element.Failure == "readBackNull") { return null; }
      if (element.Failure == "readBackCase") { return element.Value.ToUpperInvariant(); }
      return element.Value;
    }
  }
}
public class ObservationTestValuePattern {
  private ObservationTestElement element;
  public ObservationTestValuePattern(ObservationTestElement element) { this.element = element; }
  public ObservationTestValueInfo Current {
    get {
      if (element.Failure == "valueCurrent") { throw new System.Exception("private provider details"); }
      return new ObservationTestValueInfo(element);
    }
  }
  public void SetValue(string value) { element.Written = true; element.Value = value; }
}
public class ObservationTestInvokePattern {
  private ObservationTestElement element;
  public ObservationTestInvokePattern(ObservationTestElement element) { this.element = element; }
  public void Invoke() {
    element.Invoked = true;
    if (element.Failure == "invokeThrows") { throw new System.Exception("private provider details"); }
  }
}
public class ObservationTestControlPatternInfo {
  private ObservationTestElement element;
  public ObservationTestControlPatternInfo(ObservationTestElement element) { this.element = element; }
  public bool IsSelected {
    get {
      if (element.Failure == "stateUnavailable") { throw new System.Exception("private provider details"); }
      if (element.Failure == "delayedState" && element.Applied && ++element.StateReadCount <= 2) { return element.PreviousSelected; }
      return element.Failure == "wrongState" && element.Applied ? false : element.Selected;
    }
  }
  public string ExpandCollapseState {
    get {
      if (element.Failure == "stateUnavailable") { throw new System.Exception("private provider details"); }
      if (element.Failure == "leaf") { return "LeafNode"; }
      if (element.Failure == "wrongState" && element.Applied) { return "PartiallyExpanded"; }
      if (element.Failure == "delayedState" && element.Applied && ++element.StateReadCount <= 2) { return element.PreviousExpandState; }
      return element.ExpandState;
    }
  }
  public bool VerticallyScrollable {
    get { return element.Failure != "nonScrollable"; }
  }
  public double VerticalScrollPercent {
    get {
      if (element.Failure == "stateUnavailable") { throw new System.Exception("private provider details"); }
      if (element.Failure == "invalidPosition") { return double.NaN; }
      if (element.Failure == "wrongState" && element.Applied) { return element.PreviousScrollPosition; }
      if (element.Failure == "delayedState" && element.Applied && ++element.StateReadCount <= 2) { return element.PreviousScrollPosition; }
      return element.ScrollPosition;
    }
  }
}
public class ObservationTestControlPattern {
  private ObservationTestElement element;
  public ObservationTestControlPattern(ObservationTestElement element) { this.element = element; }
  public ObservationTestControlPatternInfo Current {
    get {
      if (element.Failure == "postStateUnavailable" && element.Applied) { throw new System.Exception("private provider details"); }
      return new ObservationTestControlPatternInfo(element);
    }
  }
  private void Applied() {
    element.MutationCount++;
    element.PreviousSelected = element.Selected;
    element.PreviousExpandState = element.ExpandState;
    element.PreviousScrollPosition = element.ScrollPosition;
    element.Applied = true;
    if (element.Failure == "mutationThrows") { throw new System.Exception("private provider details"); }
  }
  public void Select() { Applied(); element.Selected = true; }
  public void Expand() { Applied(); element.ExpandState = "Expanded"; }
  public void Collapse() { Applied(); element.ExpandState = "Collapsed"; }
  public void Scroll(object horizontal, object vertical) {
    if (horizontal.ToString() != "NoAmount") { throw new System.Exception("Unexpected horizontal scroll"); }
    string amount = vertical.ToString();
    if (amount != "SmallIncrement" && amount != "SmallDecrement") { throw new System.Exception("Unexpected scroll amount"); }
    Applied();
    double change = amount == "SmallIncrement" ? 10 : -10;
    if (element.Failure == "wrongDirection") { change = -change; }
    element.ScrollPosition += change;
  }
}
public class ObservationTestInfo {
  private ObservationTestElement element;
  public ObservationTestInfo(ObservationTestElement element) { this.element = element; }
  public bool IsOffscreen {
    get {
      if (element.Failure == "visibility") { throw new System.Exception("private provider details"); }
      return element.Failure == "hidden";
    }
  }
  public bool IsPassword {
    get {
      if (element.Failure == "password") { throw new System.Exception("private provider details"); }
      return element.Failure == "sensitive";
    }
  }
  public bool IsEnabled {
    get {
      if (element.Failure == "enabled") { throw new System.Exception("private provider details"); }
      return element.Failure != "disabled" && (element.Editable || element.Invokable);
    }
  }
  public string Name {
    get {
      if (element.Failure == "postNameChanged" && element.Applied) { return "changed"; }
      if (element.Failure == "name") { throw new System.Exception("private provider details"); }
      if (element.Failure == "emptyName") { return ""; }
      return element.Identifier;
    }
  }
  public string AutomationId {
    get {
      if (element.Failure == "automationId") { throw new System.Exception("private provider details"); }
      if (element.Failure == "emptyAutomationId") { return ""; }
      return element.Identifier;
    }
  }
  public ObservationTestControlType ControlType {
    get {
      if (element.Failure == "role") { throw new System.Exception("private provider details"); }
      return new ObservationTestControlType() { ProgrammaticName = element.Failure == "changedRole" ? "ControlType.CheckBox" : (element.RoleOverride ?? (element.Editable ? "ControlType.Edit" : "ControlType.Button")) };
    }
  }
  public int ProcessId {
    get {
      if (element.Failure == "processId") { throw new System.Exception("private provider details"); }
      return element.Failure == "differentProcess" ? 43 : 42;
    }
  }
}
public class ObservationTestElement {
  public string Failure;
  public string Identifier;
  public bool Editable;
  public bool Written;
  public bool Invokable;
  public bool Invoked;
  public bool Applied;
  public bool Selected;
  public bool PreviousSelected;
  public string PreviousExpandState;
  public int StateReadCount;
  public int MutationCount;
  public double ScrollPosition = 50;
  public double PreviousScrollPosition;
  public string ControlOperation;
  public string RoleOverride;
  public string ExpandState = "Collapsed";
  public string Value;
  public ObservationTestElement FirstChild;
  public ObservationTestElement NextSibling;
  public ObservationTestInfo Current {
    get {
      if (Failure == "current") { throw new System.Exception("private provider details"); }
      return new ObservationTestInfo(this);
    }
  }
  public bool TryGetCurrentPattern(object patternId, out object pattern) {
    if (ControlOperation != null) {
      pattern = new ObservationTestControlPattern(this);
      return Failure != "missingPattern";
    }
    if (Invokable) {
      if (Failure == "invokePattern") { throw new System.Exception("private provider details"); }
      pattern = new ObservationTestInvokePattern(this);
      return Failure != "invokeUnavailable";
    }
    pattern = new ObservationTestValuePattern(this);
    return Editable;
  }
  public int[] GetRuntimeId() {
    if (Failure == "runtimeNull") { return null; }
    if (Failure == "runtimeEmpty") { return new int[0]; }
    if (Failure == "runtimeError") { throw new System.Exception("private provider details"); }
    if (Failure == "runtimeNegative") { return new int[] { 7, -2 }; }
    if (Failure == "differentRuntime") { return new int[] { 2 }; }
    return new int[] { 1 };
  }
}
public class ObservationTestWalker {
  public ObservationTestElement GetFirstChild(ObservationTestElement element) {
    if (element.Failure == "child") { throw new System.Exception("private provider details"); }
    return element.FirstChild;
  }
  public ObservationTestElement GetNextSibling(ObservationTestElement element) {
    if (element.Failure == "sibling") { throw new System.Exception("private provider details"); }
    return element.NextSibling;
  }
}
'@
$root = New-Object ObservationTestElement
$firstControl = New-Object ObservationTestElement
$firstControl.Identifier = 'first'
$laterControl = New-Object ObservationTestElement
$laterControl.Identifier = 'later'
$root.FirstChild = $firstControl
$firstControl.NextSibling = $laterControl
$windows = New-Object System.Collections.Generic.List[System.IntPtr]
$windows.Add([System.IntPtr]::new(1234))
$failure = $env:ANGELBOT_TEST_FAILURE
if ($failure -in @('valueCurrent', 'valueReadOnly', 'readOnly', 'processId', 'rootProcessId', 'runtimeNull', 'runtimeEmpty', 'runtimeError', 'runtimeNegative', 'healthyEdit')) {
  $firstControl.Editable = $true
}
if ($failure -eq 'rootChild') { $root.Failure = 'child' }
elseif ($failure -eq 'rootCurrent') { $root.Failure = 'current' }
elseif ($failure -eq 'rootVisibility') { $root.Failure = 'visibility' }
elseif ($failure -eq 'rootPassword') { $root.Failure = 'password' }
elseif ($failure -eq 'rootProcessId') { $root.Failure = 'processId' }
else { $firstControl.Failure = $failure }
"#;

    #[cfg(windows)]
    #[test]
    fn observation_provider_failures_return_partial_metadata_without_write_refs() {
        // Execute the same PowerShell traversal against a deterministic provider
        // double. No desktop windows, model credentials, or user data are needed.
        let fixture = OBSERVATION_TEST_PROVIDER_FIXTURE;
        let observation = OBSERVATION_SCRIPT.replace(
            "$walker = [Windows.Automation.TreeWalker]::ControlViewWalker",
            "$walker = New-Object ObservationTestWalker",
        );
        let script = format!("{fixture}\n{UIA_SAFETY_HELPERS_SCRIPT}\n{observation}");
        let cases = [
            ("current", 1),
            ("role", 1),
            ("visibility", 1),
            ("password", 1),
            ("name", 2),
            ("automationId", 2),
            ("enabled", 2),
            ("child", 2),
            ("sibling", 1),
            ("rootChild", 0),
            ("rootCurrent", 0),
            ("rootVisibility", 0),
            ("rootPassword", 0),
            ("valueCurrent", 2),
            ("valueReadOnly", 2),
            ("processId", 2),
            ("rootProcessId", 2),
            ("runtimeNull", 2),
            ("runtimeEmpty", 2),
            ("runtimeError", 2),
            ("emptyName", 2),
            ("emptyAutomationId", 2),
            ("readOnly", 2),
            ("runtimeNegative", 2),
            ("healthyEdit", 2),
            ("none", 2),
        ];
        let commands = cases
            .iter()
            .map(|(failure, _)| {
                let mut command =
                    desktop_child_command(WindowsDesktopAdapter::system_powershell().unwrap());
                command
                    .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                    .env("ANGELBOT_TEST_FAILURE", failure);
                command
            })
            .collect();
        for ((failure, expected_controls), output) in
            cases.into_iter().zip(run_fixture_commands(commands))
        {
            if matches!(failure, "rootCurrent" | "rootVisibility" | "rootPassword") {
                assert!(!output.status.success(), "fixture {failure}");
                assert_eq!(
                    parse_uia_helper_error(&output.stderr).code,
                    "TARGET_UNAVAILABLE",
                    "fixture {failure}"
                );
                continue;
            }
            assert!(
                output.status.success(),
                "fixture {failure}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let (mut payload, targets) = parse_private_observation_payload(&output.stdout)
                .unwrap_or_else(|error| panic!("fixture {failure}: {error}"));
            assert_eq!(
                payload.controls.len(),
                expected_controls,
                "fixture {failure}"
            );
            let complete = matches!(
                failure,
                "none"
                    | "emptyName"
                    | "emptyAutomationId"
                    | "readOnly"
                    | "runtimeNegative"
                    | "healthyEdit"
            );
            assert_eq!(payload.truncated, !complete, "fixture {failure}");
            assert_eq!(
                payload.diagnostics,
                if complete {
                    vec![]
                } else {
                    vec![DesktopObservationDiagnostic::ProviderUnavailable]
                },
                "fixture {failure}"
            );
            if matches!(failure, "valueCurrent" | "valueReadOnly" | "readOnly") {
                assert!(
                    payload.controls[0].capabilities.is_empty(),
                    "fixture {failure}"
                );
                assert!(targets.is_empty(), "fixture {failure}");
            }
            if matches!(failure, "current" | "role" | "child") {
                assert!(payload
                    .controls
                    .iter()
                    .any(|control| control.automation_id.as_deref() == Some("later")));
            }
            let adapter = WindowsDesktopAdapter::default();
            adapter
                .issue_field_refs(
                    "fixture",
                    std::path::Path::new("fixture.exe"),
                    payload.truncated,
                    &mut payload.controls,
                    targets,
                )
                .unwrap();
            if matches!(failure, "healthyEdit" | "runtimeNegative") {
                assert!(payload.controls[0].field_ref.is_some(), "fixture {failure}");
            } else {
                assert!(
                    payload
                        .controls
                        .iter()
                        .all(|control| control.field_ref.is_none()),
                    "fixture {failure}"
                );
            }
            assert!(!String::from_utf8_lossy(&output.stdout).contains("private provider details"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn scroll_observation_retains_only_named_usable_containers_without_reading_values() {
        let observation = OBSERVATION_SCRIPT.replace(
            "$walker = [Windows.Automation.TreeWalker]::ControlViewWalker",
            "$walker = New-Object ObservationTestWalker",
        );
        let setup = r#"
$firstControl.Invokable = $true
$firstControl.ControlOperation = 'scrolldown'
$firstControl.RoleOverride = $env:ANGELBOT_TEST_ROLE
$firstControl.NextSibling = $null
"#;
        let script = format!("{OBSERVATION_TEST_PROVIDER_FIXTURE}\n{UIA_SAFETY_HELPERS_SCRIPT}\n{setup}\n{observation}");
        let cases = [
            ("Pane", "none"),
            ("Document", "none"),
            ("List", "none"),
            ("Pane", "nonScrollable"),
            ("Document", "missingPattern"),
            ("List", "invalidPosition"),
            ("Pane", "emptyName"),
            ("Pane", "disabled"),
        ];
        let commands = cases
            .iter()
            .map(|(role, failure)| {
                let mut command =
                    desktop_child_command(WindowsDesktopAdapter::system_powershell().unwrap());
                command
                    .args(["-Mta", "-NoProfile", "-NonInteractive", "-Command", &script])
                    .env("ANGELBOT_TEST_FAILURE", failure)
                    .env("ANGELBOT_TEST_ROLE", format!("ControlType.{role}"));
                command
            })
            .collect();
        for ((role, failure), output) in cases.into_iter().zip(run_fixture_commands(commands)) {
            assert!(
                output.status.success(),
                "{role}/{failure}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let (mut payload, targets) = parse_private_observation_payload(&output.stdout).unwrap();
            assert_eq!(
                payload.controls.len(),
                usize::from(failure == "none"),
                "{role}/{failure}"
            );
            assert_eq!(
                payload.truncated,
                failure == "invalidPosition",
                "{role}/{failure}"
            );
            let adapter = WindowsDesktopAdapter::default();
            let exe = std::path::Path::new("fixture.exe");
            adapter
                .issue_control_refs(
                    "app",
                    exe,
                    payload.truncated,
                    &mut payload.controls,
                    &targets,
                )
                .unwrap();
            if failure == "none" {
                let control = &payload.controls[0];
                assert_eq!(control.capabilities, vec!["scroll"]);
                assert!(control.control_ref.is_some());
                assert!(control.field_ref.is_none());
            } else {
                assert!(targets.is_empty());
            }
            let raw = String::from_utf8_lossy(&output.stdout);
            assert!(!raw.contains("private provider details"));
            assert!(!raw.contains("\"value\""));
        }
    }

    #[cfg(windows)]
    #[test]
    fn verified_text_write_rejects_unavailable_metadata_and_case_changed_readback() {
        let setup = r#"
$firstControl.Editable = $true
$target = $firstControl
$pattern = New-Object ObservationTestValuePattern -ArgumentList $target
$expected = 'C:\trusted\fixture.exe'
function Get-Process {
  param([int]$Id)
  return [pscustomobject]@{ Path = 'C:\trusted\fixture.exe'; MainWindowHandle = [System.IntPtr]::new(1234) }
}
"#;
        let script = format!(
            "{OBSERVATION_TEST_PROVIDER_FIXTURE}\n{UIA_SAFETY_HELPERS_SCRIPT}\n{setup}\ntry {{\n{VERIFIED_SET_VALUE_SCRIPT}\n}} catch {{ @{{ code = ($_.Exception.Message -split '\\|')[0]; written = $firstControl.Written }} | ConvertTo-Json -Compress }}"
        );
        let failures = [
            "current",
            "password",
            "visibility",
            "enabled",
            "processId",
            "rootCurrent",
            "rootPassword",
            "rootVisibility",
            "rootProcessId",
            "automationId",
            "runtimeNull",
            "runtimeEmpty",
            "runtimeError",
            "valueCurrent",
            "valueReadOnly",
            "readOnly",
            "readBackCase",
            "readBackNull",
            "none",
        ];
        let commands = failures
            .iter()
            .map(|failure| {
                let mut command =
                    desktop_child_command(WindowsDesktopAdapter::system_powershell().unwrap());
                command
                    .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                    .env("ANGELBOT_TEST_FAILURE", failure)
                    .env("ANGELBOT_DRAFT_EXPECTED_PID", "42")
                    .env("ANGELBOT_DRAFT_EXPECTED_HWND", "1234")
                    .env("ANGELBOT_DRAFT_EXPECTED_RUNTIME_ID", "1")
                    .env("ANGELBOT_FIELD_LABEL", "first")
                    .env("ANGELBOT_DRAFT_TEXT", "Exact Case");
                command
            })
            .collect();
        for (failure, output) in failures.into_iter().zip(run_fixture_commands(commands)) {
            assert!(
                output.status.success(),
                "fixture {failure}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let result: serde_json::Value = serde_json::from_slice(&output.stdout)
                .unwrap_or_else(|error| panic!("fixture {failure}: {error}"));
            if failure == "none" {
                assert_eq!(result["status"], "verified");
            } else if matches!(failure, "readBackCase" | "readBackNull") {
                assert_eq!(result["code"], "RESULT_UNKNOWN", "fixture {failure}");
                assert_eq!(result["written"], true, "fixture {failure}");
            } else {
                assert!(
                    matches!(
                        result["code"].as_str(),
                        Some("TARGET_CHANGED" | "TARGET_UNAVAILABLE")
                    ),
                    "fixture {failure}: {result}"
                );
                assert_eq!(result["written"], false, "fixture {failure}");
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn invoke_immediately_rechecks_observed_identity_name_role_safety_and_pattern() {
        let setup = r#"
$firstControl.Editable = $false
$firstControl.Invokable = $true
$firstControl.Failure = 'none'
$firstControl.NextSibling = $null
$root.Failure = 'none'
$expected = 'C:\trusted\fixture.exe'
function Get-Process {
  param([int]$Id)
  return [pscustomobject]@{ Path = 'C:\trusted\fixture.exe'; MainWindowHandle = [System.IntPtr]::new(1234) }
}
"#;
        let target_script = CONTROL_TARGET_SCRIPT.replace(
            "$walker = [Windows.Automation.TreeWalker]::ControlViewWalker",
            "$walker = New-Object ObservationTestWalker",
        );
        let mutate = r#"
if ($failure -eq 'renamed') { $firstControl.Identifier = 'renamed' }
elseif ($failure -eq 'rootHidden') { $root.Failure = 'hidden' }
elseif ($failure -eq 'rootSensitive') { $root.Failure = 'sensitive' }
else { $firstControl.Failure = $failure }
"#;
        let script = format!(
            "{OBSERVATION_TEST_PROVIDER_FIXTURE}\n{UIA_SAFETY_HELPERS_SCRIPT}\n{setup}\ntry {{\n{target_script}\n{mutate}\n{CONTROL_OPERATION_SCRIPT}\n}} catch {{ @{{ code = ($_.Exception.Message -split '\\|')[0]; invoked = $firstControl.Invoked }} | ConvertTo-Json -Compress }}"
        );
        let failures = [
            "renamed",
            "changedRole",
            "disabled",
            "hidden",
            "sensitive",
            "differentProcess",
            "differentRuntime",
            "runtimeNull",
            "runtimeEmpty",
            "runtimeError",
            "current",
            "password",
            "visibility",
            "enabled",
            "processId",
            "role",
            "name",
            "rootHidden",
            "rootSensitive",
            "invokeUnavailable",
            "invokePattern",
            "invokeThrows",
            "none",
        ];
        let commands = failures
            .iter()
            .map(|failure| {
                let mut command =
                    desktop_child_command(WindowsDesktopAdapter::system_powershell().unwrap());
                command
                    .args(["-Mta", "-NoProfile", "-NonInteractive", "-Command", &script])
                    .env("ANGELBOT_TEST_FAILURE", failure)
                    .env("ANGELBOT_DRAFT_EXPECTED_PID", "42")
                    .env("ANGELBOT_DRAFT_EXPECTED_HWND", "1234")
                    .env("ANGELBOT_DRAFT_EXPECTED_RUNTIME_ID", "1")
                    .env("ANGELBOT_CONTROL_LABEL", "first")
                    .env("ANGELBOT_CONTROL_ROLE", "button")
                    .env("ANGELBOT_CONTROL_OPERATION", "invoke");
                command
            })
            .collect();
        for (failure, output) in failures.into_iter().zip(run_fixture_commands(commands)) {
            assert!(
                output.status.success(),
                "fixture {failure}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let result: serde_json::Value =
                serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
                    panic!(
                        "fixture {failure}: {error}; {}",
                        String::from_utf8_lossy(&output.stdout)
                    )
                });
            if failure == "none" {
                assert_eq!(result["status"], "dispatched");
                assert!(result["detail"].as_str().unwrap().contains("not verified"));
            } else if failure == "invokeThrows" {
                assert_eq!(result["code"], "RESULT_UNKNOWN");
                assert_eq!(result["invoked"], true);
            } else {
                assert!(
                    matches!(
                        result["code"].as_str(),
                        Some("TARGET_CHANGED" | "UNSUPPORTED_CONTROL")
                    ),
                    "fixture {failure}: {result}"
                );
                assert_eq!(result["invoked"], false, "fixture {failure}");
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn shared_control_helper_verifies_exact_state_and_classifies_post_action_failures() {
        let setup = r#"
$firstControl.Editable = $false
$firstControl.Invokable = $true
$firstControl.ControlOperation = $env:ANGELBOT_CONTROL_OPERATION
$firstControl.RoleOverride = $(if ($firstControl.ControlOperation -ceq 'select') { 'ControlType.RadioButton' } elseif ($firstControl.ControlOperation -in @('scrollup', 'scrolldown')) { 'ControlType.Pane' } else { 'ControlType.MenuItem' })
$firstControl.ExpandState = $(if ($firstControl.ControlOperation -ceq 'collapse') { 'Expanded' } else { 'Collapsed' })
$firstControl.Failure = 'none'
$firstControl.NextSibling = $null
$root.Failure = 'none'
$expected = 'C:\trusted\fixture.exe'
function Get-Process {
  param([int]$Id)
  return [pscustomobject]@{ Path = 'C:\trusted\fixture.exe'; MainWindowHandle = [System.IntPtr]::new(1234) }
}
"#;
        let mutate = r#"
$firstControl.Failure = $failure
if ($failure -ceq 'noop') {
  $firstControl.Selected = $true
  $firstControl.ExpandState = $(if ($controlOperation -ceq 'expand') { 'Expanded' } else { 'Collapsed' })
  $firstControl.ScrollPosition = $(if ($controlOperation -ceq 'scrollup') { 0 } else { 100 })
}
"#;
        let target_script = CONTROL_TARGET_SCRIPT.replace(
            "$walker = [Windows.Automation.TreeWalker]::ControlViewWalker",
            "$walker = New-Object ObservationTestWalker",
        );
        let operation_script = CONTROL_OPERATION_SCRIPT.replace(
            "status = $status; action = $controlOperation; detail = $detail",
            "status = $status; action = $controlOperation; detail = $detail; applied = $firstControl.Applied; mutationCount = $firstControl.MutationCount",
        );
        let script = format!(
            "{OBSERVATION_TEST_PROVIDER_FIXTURE}\n{UIA_SAFETY_HELPERS_SCRIPT}\n{setup}\ntry {{\n{target_script}\n{mutate}\n{operation_script}\n}} catch {{ @{{ code = ($_.Exception.Message -split '\\|')[0]; applied = $firstControl.Applied; mutationCount = $firstControl.MutationCount }} | ConvertTo-Json -Compress }}"
        );
        let mut cases = Vec::new();
        let mut commands = Vec::new();
        for operation in [
            DesktopControlOperation::Select,
            DesktopControlOperation::Expand,
            DesktopControlOperation::Collapse,
            DesktopControlOperation::ScrollUp,
            DesktopControlOperation::ScrollDown,
        ] {
            for failure in [
                "none",
                "delayedState",
                "noop",
                "wrongState",
                "mutationThrows",
                "postStateUnavailable",
                "postNameChanged",
                "missingPattern",
                "stateUnavailable",
                "leaf",
                "invalidPosition",
                "nonScrollable",
                "wrongDirection",
            ] {
                let scrolling = matches!(
                    operation,
                    DesktopControlOperation::ScrollUp | DesktopControlOperation::ScrollDown
                );
                if (failure == "leaf"
                    && !matches!(
                        operation,
                        DesktopControlOperation::Expand | DesktopControlOperation::Collapse
                    ))
                    || (matches!(
                        failure,
                        "invalidPosition" | "nonScrollable" | "wrongDirection"
                    ) && !scrolling)
                {
                    continue;
                }
                let mut command =
                    desktop_child_command(WindowsDesktopAdapter::system_powershell().unwrap());
                command
                    .args(["-Mta", "-NoProfile", "-NonInteractive", "-Command", &script])
                    .env("ANGELBOT_TEST_FAILURE", failure)
                    .env("ANGELBOT_DRAFT_EXPECTED_PID", "42")
                    .env("ANGELBOT_DRAFT_EXPECTED_HWND", "1234")
                    .env("ANGELBOT_DRAFT_EXPECTED_RUNTIME_ID", "1")
                    .env("ANGELBOT_CONTROL_LABEL", "first")
                    .env(
                        "ANGELBOT_CONTROL_ROLE",
                        if operation == DesktopControlOperation::Select {
                            "radioButton"
                        } else if scrolling {
                            "pane"
                        } else {
                            "menuItem"
                        },
                    )
                    .env("ANGELBOT_CONTROL_OPERATION", operation.as_str());
                cases.push((operation, failure));
                commands.push(command);
            }
        }
        for ((operation, failure), output) in cases.into_iter().zip(run_fixture_commands(commands))
        {
            assert!(
                output.status.success(),
                "{operation:?}/{failure}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let result: serde_json::Value =
                serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
                    panic!(
                        "{operation:?}/{failure}: {error}; {}",
                        String::from_utf8_lossy(&output.stdout)
                    )
                });
            if matches!(failure, "none" | "delayedState" | "noop") {
                assert_eq!(result["status"], "verified", "{operation:?}/{failure}");
                assert_eq!(result["action"], operation.as_str());
                assert_eq!(
                    result["applied"],
                    failure != "noop",
                    "{operation:?}/{failure}"
                );
            } else if matches!(
                failure,
                "missingPattern"
                    | "stateUnavailable"
                    | "leaf"
                    | "invalidPosition"
                    | "nonScrollable"
            ) {
                assert!(
                    matches!(
                        result["code"].as_str(),
                        Some("UNSUPPORTED_CONTROL" | "TARGET_UNAVAILABLE")
                    ),
                    "{operation:?}/{failure}: {result}"
                );
                assert_eq!(result["applied"], false, "{operation:?}/{failure}");
            } else {
                assert_eq!(
                    result["code"], "RESULT_UNKNOWN",
                    "{operation:?}/{failure}: {result}"
                );
                assert_eq!(result["applied"], true, "{operation:?}/{failure}");
            }
            assert_eq!(
                result["mutationCount"],
                u32::from(!matches!(
                    failure,
                    "noop"
                        | "missingPattern"
                        | "stateUnavailable"
                        | "leaf"
                        | "invalidPosition"
                        | "nonScrollable"
                )),
                "the effect must never repeat: {operation:?}/{failure}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn generic_text_helper_scripts_parse_in_windows_powershell() {
        let script = format!(
            "{UIA_SAFETY_HELPERS_SCRIPT}\n{TRUSTED_WINDOW_SCRIPT}\n{DRAFT_TARGET_SCRIPT}\n{DRAFT_PREFLIGHT_SCRIPT}\n{TEXT_TARGET_SCRIPT}\n{VERIFIED_SET_VALUE_SCRIPT}\n{CONTROL_TARGET_SCRIPT}\n{CONTROL_OPERATION_SCRIPT}\n{OBSERVATION_SCRIPT}"
        );
        let parser = "$tokens=$null; $errors=$null; [System.Management.Automation.Language.Parser]::ParseInput($env:ANGELBOT_TEST_SCRIPT, [ref]$tokens, [ref]$errors) | Out-Null; if ($errors.Count -gt 0) { $errors | ForEach-Object { Write-Output $_.Message }; exit 1 }";
        let output = desktop_child_command(WindowsDesktopAdapter::system_powershell().unwrap())
            .args(["-NoProfile", "-NonInteractive", "-Command", parser])
            .env("ANGELBOT_TEST_SCRIPT", script)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "PowerShell syntax errors: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    #[test]
    fn discovery_payload_exposes_only_bounded_unique_names() {
        let candidates = parse_discovery_payload(
            br#"{"candidates":[{"name":"Message","automationId":"messageBox"}]}"#,
        )
        .unwrap();
        assert_eq!(candidates[0].name, "Message");
        assert_eq!(candidates[0].automation_id.as_deref(), Some("messageBox"));

        assert!(parse_discovery_payload(
            br#"{"candidates":[{"name":"Message","value":"private draft"}]}"#
        )
        .is_err());
        assert!(parse_discovery_payload(
            br#"{"candidates":[{"name":"Message"},{"name":"message"}]}"#
        )
        .is_err());
        assert!(parse_discovery_payload(br#"{"candidates":[{"name":"\nSensitive"}]}"#).is_err());
        let excessive = serde_json::json!({
            "candidates": (0..33)
                .map(|index| serde_json::json!({"name": format!("Field {index}")}))
                .collect::<Vec<_>>()
        });
        assert_eq!(
            parse_discovery_payload(excessive.to_string().as_bytes())
                .unwrap_err()
                .code,
            "SCAN_LIMIT"
        );
    }

    #[test]
    fn draft_preflight_accepts_only_the_selected_control_and_bounded_identity() {
        let payload = br#"{"windowTitle":"Chat","controlName":"Message","identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[7,2,-1]}}"#;
        let preview = parse_draft_target_payload("chat", "Message", payload).unwrap();
        assert_eq!(preview.app_id, "chat");
        assert_eq!(preview.window_title.as_deref(), Some("Chat"));
        assert_eq!(preview.identity.control_runtime_id, vec![7, 2, -1]);

        assert!(parse_draft_target_payload("chat", "Other", payload).is_err());
        assert!(parse_draft_target_payload("chat", "\nMessage", payload).is_err());
        assert!(parse_draft_target_payload(
            "chat",
            "Message",
            br#"{"windowTitle":"Chat","controlName":"Message","value":"private","identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[7]}}"#
        )
        .is_err());
        assert!(parse_draft_target_payload(
            "chat",
            "Message",
            br#"{"windowTitle":"Chat","controlName":"Message","identity":{"processId":0,"windowHandle":1234,"controlRuntimeId":[7]}}"#
        )
        .is_err());
        assert!(parse_draft_target_payload(
            "chat",
            "Message",
            br#"{"windowTitle":"Chat","controlName":"Message","identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[]}}"#
        )
        .is_err());
    }

    #[cfg(windows)]
    #[test]
    fn draft_post_write_errors_are_never_retryable_failures() {
        for stderr in [
            b"VERIFICATION_FAILED|Readback differed".as_slice(),
            b"Unhandled UIA provider failure".as_slice(),
        ] {
            assert_eq!(
                classify_draft_helper_result(false, b"", stderr)
                    .unwrap_err()
                    .code,
                "RESULT_UNKNOWN"
            );
        }
        assert_eq!(
            classify_draft_helper_result(true, b"not-json", b"")
                .unwrap_err()
                .code,
            "RESULT_UNKNOWN"
        );
        assert_eq!(
            classify_draft_helper_result(true, br#"{"status":"dispatched"}"#, b"")
                .unwrap_err()
                .code,
            "RESULT_UNKNOWN"
        );
        assert_eq!(
            classify_draft_helper_result(false, b"", b"TARGET_NOT_FOUND|No control")
                .unwrap_err()
                .code,
            "TARGET_NOT_FOUND"
        );
        assert_eq!(
            classify_draft_helper_result(false, b"", b"TARGET_CHANGED|Window changed")
                .unwrap_err()
                .code,
            "TARGET_CHANGED"
        );
        assert_eq!(
            classify_draft_helper_result(true, br#"{"status":"verified","detail":"checked"}"#, b"")
                .unwrap(),
            ("verified".to_string(), "checked".to_string())
        );
    }

    #[cfg(windows)]
    #[test]
    fn process_comparison_path_removes_extended_windows_prefix() {
        assert_eq!(
            WindowsDesktopAdapter::process_comparison_path(std::path::Path::new(
                r"\\?\D:\Projects\AngelBot\target.exe"
            )),
            r"D:\Projects\AngelBot\target.exe"
        );
        assert_eq!(
            WindowsDesktopAdapter::process_comparison_path(std::path::Path::new(
                r"\\?\UNC\server\share\target.exe"
            )),
            r"\\server\share\target.exe"
        );
    }

    #[cfg(windows)]
    #[test]
    fn desktop_launchers_resolve_system_executables_without_path_search() {
        let explorer = WindowsDesktopAdapter::system_explorer().unwrap();
        let powershell = WindowsDesktopAdapter::system_powershell().unwrap();
        let system_root = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap());
        assert_eq!(explorer, system_root.join("explorer.exe"));
        assert_eq!(
            powershell,
            system_root.join("System32/WindowsPowerShell/v1.0/powershell.exe")
        );
        assert!(explorer.is_absolute());
        assert_eq!(
            explorer
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_ascii_lowercase(),
            "explorer.exe"
        );
        assert!(powershell.is_absolute());
        assert_eq!(
            powershell
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_ascii_lowercase(),
            "powershell.exe"
        );
    }

    #[cfg(windows)]
    #[test]
    fn every_desktop_launcher_uses_only_bounded_environment() {
        let command = desktop_child_command(WindowsDesktopAdapter::system_explorer().unwrap());
        let allowed = [
            "PATH",
            "PATHEXT",
            "SYSTEMROOT",
            "WINDIR",
            "COMSPEC",
            "TEMP",
            "TMP",
            "APPDATA",
            "LOCALAPPDATA",
            "USERPROFILE",
        ];
        for (key, _) in command.get_envs() {
            let key = key.to_string_lossy();
            assert!(
                allowed.iter().any(|value| value.eq_ignore_ascii_case(&key)),
                "desktop child inherited unexpected variable {key}"
            );
        }
        assert!(!command
            .get_envs()
            .any(|(key, _)| key == "ANGELBOT_MODEL_TEST_SECRET"));

        let system_root = std::env::var_os("SystemRoot").unwrap();
        let powershell = std::path::PathBuf::from(system_root)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut child_command = desktop_child_command(powershell);
        child_command.env("ANGELBOT_MODEL_TEST_SECRET", "must-not-leak");
        crate::child_process_env::apply_minimal_child_environment(&mut child_command);
        let output = child_command
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "if ($env:ANGELBOT_MODEL_TEST_SECRET) { exit 77 } else { exit 0 }",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "desktop child saw model sentinel");
    }

    #[cfg(windows)]
    #[test]
    fn interrupted_draft_helper_is_killed_and_reaped_promptly() {
        use std::process::Stdio;
        use std::sync::atomic::{AtomicBool, Ordering};

        let system_root = std::env::var_os("SystemRoot").unwrap();
        let powershell = std::path::PathBuf::from(system_root)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut child = desktop_child_command(powershell)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&stopped);
        let cancel = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            signal.store(true, Ordering::Release);
        });
        let start = std::time::Instant::now();
        let error = wait_for_draft_child(
            &mut child,
            &|| stopped.load(Ordering::Acquire),
            DRAFT_TIMEOUT,
        )
        .unwrap_err();
        cancel.join().unwrap();
        assert_eq!(error.code, "RESULT_UNKNOWN");
        assert!(start.elapsed() < std::time::Duration::from_secs(3));
        assert!(child.try_wait().unwrap().is_some(), "child was not reaped");
    }

    #[cfg(windows)]
    #[test]
    fn cancelled_read_only_observation_reaps_its_helper() {
        use std::process::Stdio;
        use std::sync::atomic::{AtomicBool, Ordering};

        let powershell = WindowsDesktopAdapter::system_powershell().unwrap();
        let mut child = desktop_child_command(powershell)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&stopped);
        let cancel = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            signal.store(true, Ordering::Release);
        });
        let start = std::time::Instant::now();
        let error = wait_for_read_only_child_with_control(
            &mut child,
            "Desktop observation",
            &|| stopped.load(Ordering::Acquire),
            DISCOVERY_TIMEOUT,
        )
        .unwrap_err();
        cancel.join().unwrap();
        assert_eq!(error.code, "SCAN_CANCELLED");
        assert!(start.elapsed() < std::time::Duration::from_secs(3));
        assert!(child.try_wait().unwrap().is_some(), "child was not reaped");
    }

    #[cfg(windows)]
    #[test]
    fn timed_out_draft_helper_is_killed_and_reaped() {
        use std::process::Stdio;

        let system_root = std::env::var_os("SystemRoot").unwrap();
        let powershell = std::path::PathBuf::from(system_root)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut child = desktop_child_command(powershell)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let error =
            wait_for_draft_child(&mut child, &|| false, std::time::Duration::from_millis(100))
                .unwrap_err();
        assert_eq!(error.code, "RESULT_UNKNOWN");
        assert!(child.try_wait().unwrap().is_some(), "child was not reaped");
    }

    #[test]
    fn mock_adapter_records_and_verifies_draft() {
        let adapter = MockDesktopAdapter::default();
        let preflight = adapter
            .preflight_draft(
                "chat",
                "chat.exe",
                "Message",
                std::time::Duration::from_secs(1),
            )
            .unwrap();
        assert!(adapter.actions().is_empty(), "preflight must be read-only");
        let action = DesktopAction::PrepareDraft {
            app_id: "chat".into(),
            display_name: "Chat".into(),
            executable_path: "chat.exe".into(),
            selector: "Message".into(),
            text: "hello".into(),
            expected_target: Some(preflight.identity),
        };
        let result = adapter.execute(&action).unwrap();
        assert_eq!(result.status, "verified");
        assert_eq!(adapter.actions(), vec![action]);
    }

    #[cfg(windows)]
    #[test]
    fn windows_draft_write_requires_preflight_identity() {
        let action = DesktopAction::PrepareDraft {
            app_id: "chat".into(),
            display_name: "Chat".into(),
            executable_path: "does-not-need-to-exist.exe".into(),
            selector: "Message".into(),
            text: "hello".into(),
            expected_target: None,
        };
        let error = WindowsDesktopAdapter::default()
            .execute(&action)
            .unwrap_err();
        assert_eq!(error.code, "TARGET_CHANGED");
    }

    #[cfg(windows)]
    #[test]
    fn windows_generic_text_write_requires_attested_automation_id() {
        let action = DesktopAction::SetText {
            app_id: "chat".into(),
            display_name: "Chat".into(),
            executable_path: "does-not-need-to-exist.exe".into(),
            text: "hello".into(),
            expected_target: Some(DesktopDraftTargetIdentity {
                process_id: 1,
                window_handle: 1,
                control_runtime_id: vec![1],
                automation_id: None,
                control_operation: None,
            }),
        };
        let error = WindowsDesktopAdapter::default()
            .execute(&action)
            .unwrap_err();
        assert_eq!(error.code, "TARGET_CHANGED");
    }

    #[test]
    fn mock_adapter_records_workspace_reveal() {
        let adapter = MockDesktopAdapter::default();
        let action = DesktopAction::RevealPath {
            path: r"D:\Projects\AngelBot\README.md".into(),
            is_directory: false,
        };
        let result = adapter.execute(&action).unwrap();
        assert_eq!(result.status, "dispatched");
        assert_eq!(adapter.actions(), vec![action]);
    }
}

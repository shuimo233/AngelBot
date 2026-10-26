//! Bounded desktop automation adapters.
//!
//! The agent never supplies an executable path, URI, selector strategy, or
//! script. Those values come from validated local configuration and strict
//! allowlists before reaching this interface.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

mod image_click;
pub use image_click::{
    DesktopImageClickDisplay, DesktopImageClickPreview, DesktopImageClickRequest,
    DesktopImageClickTarget, DesktopImageReference,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DesktopControlOperation {
    Invoke,
    Select,
    Choose,
    Check,
    Uncheck,
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
            Self::Choose => "choose",
            Self::Check => "check",
            Self::Uncheck => "uncheck",
            Self::Expand => "expand",
            Self::Collapse => "collapse",
            Self::ScrollUp => "scrollup",
            Self::ScrollDown => "scrolldown",
        }
    }
}

/// One bounded control request, shared by live preview and execution.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DesktopControlRequest {
    pub app_id: String,
    pub control_ref: String,
    pub action: DesktopControlOperation,
    pub option: Option<String>,
}

impl DesktopControlRequest {
    pub(crate) fn validate(&self) -> Result<(), DesktopAdapterError> {
        if self.control_ref.is_empty() || self.control_ref.len() > 128 {
            return Err(DesktopAdapterError::new(
                "INVALID_ARGUMENT",
                "Control reference is invalid",
            ));
        }
        validate_control_option(self.action, self.option.as_deref())
    }
}

fn validate_control_option(
    operation: DesktopControlOperation,
    option: Option<&str>,
) -> Result<(), DesktopAdapterError> {
    let valid = if operation == DesktopControlOperation::Choose {
        option.is_some_and(|value| {
            !value.trim().is_empty()
                && valid_observation_label(value)
                && value.encode_utf16().count() <= 80
        })
    } else {
        option.is_none()
    };
    if !valid {
        return Err(DesktopAdapterError::new(
            "INVALID_ARGUMENT",
            "Only choose accepts a required exact option label of at most 80 characters",
        ));
    }
    Ok(())
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
        /// Exact option bound to the pending request and its single-use preview.
        option: Option<String>,
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
    /// Only emitted after the exact approved ComboBox selection and collapse
    /// were verified. Echoes approved data, not a hidden Name/Value read.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub choice: Option<DesktopChoiceReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DesktopChoiceReceipt {
    pub control_name: String,
    pub option: String,
    pub expanded: bool,
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
    /// Opaque generic field/control lease key. The configured named-draft path
    /// leaves it absent; this is never model-supplied.
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
    field_ref: &str,
    lease: &FieldRefLease,
    raw: &[u8],
) -> Result<DesktopTextTargetPreview, DesktopAdapterError> {
    let mut preview = parse_control_target_payload(
        app_id,
        &lease.control_label,
        DesktopDraftTargetIdentity {
            automation_id: Some(field_ref.to_string()),
            ..lease.identity.clone()
        },
        raw,
    )?;
    if let Some(scope) = &lease.scope {
        preview.control_label = format!("{} / {}", scope.label(), preview.control_label);
    }
    Ok(preview)
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopObservedControl {
    /// Control View depth relative to the selected window/region; a selected
    /// region's own actionable root is depth 1.
    pub depth: u8,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation_id: Option<String>,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    /// Read-only provider state at observation time, not business completion or
    /// authority to act. Missing state is unknown, never a default value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<DesktopControlState>,
    /// Ephemeral selection hint, not an authorization grant or UIA identity.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub field_ref: Option<String>,
    /// Ephemeral selection hint for an attested control, not an authorization grant.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub control_ref: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopControlState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toggle: Option<DesktopToggleState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    /// Visible SelectionItem container kind, not a selector or action grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_container_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expansion: Option<DesktopExpansionState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertical_scroll_percent: Option<f64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DesktopToggleState {
    Off,
    On,
    Indeterminate,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DesktopExpansionState {
    Collapsed,
    Expanded,
    PartiallyExpanded,
    LeafNode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopWindowObservation {
    pub app_id: String,
    pub controls: Vec<DesktopObservedControl>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<DesktopObservedRegion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region_ref: Option<String>,
    /// True when a budget or provider access failure left the observation partial.
    pub truncated: bool,
    /// Bounded reasons for a partial observation; never provider error text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<DesktopObservationDiagnostic>,
}

/// Read-only hint for a bounded section of this same trusted application window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopObservedRegion {
    pub role: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region_ref: Option<String>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    regions: Vec<DesktopObservedRegion>,
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
    regions: Vec<DesktopObservedRegion>,
    #[serde(default)]
    region_targets: Vec<PrivateObservedRegion>,
    #[serde(default)]
    diagnostics: Vec<DesktopObservationDiagnostic>,
}

#[cfg(windows)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PrivateObservedRegion {
    index: usize,
    scope: RegionScope,
}

#[cfg(windows)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegionScope {
    identity: DesktopDraftTargetIdentity,
    role: String,
    name: String,
    /// Absolute Control View sibling indices, including filtered siblings.
    path: Vec<u16>,
}

#[cfg(windows)]
impl RegionScope {
    fn label(&self) -> &str {
        if self.name.is_empty() {
            &self.role
        } else {
            &self.name
        }
    }
}

const MAX_OBSERVED_CONTROLS: usize = 32;
const MAX_OBSERVATION_BYTES: usize = 3072;
const MAX_OBSERVATION_DIAGNOSTICS: usize = 5;
const MAX_OBSERVED_REGIONS: usize = 8;
#[cfg(windows)]
const MAX_PRIVATE_OBSERVATION_BYTES: usize = 8192;

#[cfg(windows)]
fn parse_private_observation_payload(
    raw: &[u8],
) -> Result<
    (
        ObservationPayload,
        Vec<PrivateObservedTarget>,
        Vec<PrivateObservedRegion>,
    ),
    DesktopAdapterError,
> {
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
        regions: private.regions,
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
    let mut region_indices = std::collections::HashSet::new();
    if private.region_targets.len() > public.regions.len() {
        return Err(DesktopAdapterError::new(
            "ADAPTER_FAILED",
            "Invalid desktop regions",
        ));
    }
    for target in &private.region_targets {
        let region = public.regions.get(target.index).ok_or_else(|| {
            DesktopAdapterError::new("ADAPTER_FAILED", "Invalid desktop region index")
        })?;
        if !region_indices.insert(target.index)
            || target.scope.role != region.role
            || target.scope.name != region.name
            || !valid_region_scope(&target.scope)
        {
            return Err(DesktopAdapterError::new(
                "ADAPTER_FAILED",
                "Invalid desktop region identity",
            ));
        }
    }
    Ok((public, private.targets, private.region_targets))
}

#[cfg(windows)]
fn valid_region_scope(scope: &RegionScope) -> bool {
    valid_draft_identity(&scope.identity)
        && scope.identity.automation_id.is_none()
        && scope.identity.control_operation.is_none()
        && matches!(scope.role.as_str(), "pane" | "document" | "list" | "group")
        && (scope.name.is_empty() || valid_observation_label(&scope.name))
        && (1..=12).contains(&scope.path.len())
        && scope
            .path
            .iter()
            .map(|index| usize::from(*index) + 1)
            .sum::<usize>()
            < 256
}

#[cfg(windows)]
fn region_scope_environment(scope: Option<&RegionScope>) -> Result<String, DesktopAdapterError> {
    scope
        .map(serde_json::to_string)
        .transpose()
        .map(|value| value.unwrap_or_default())
        .map_err(|_| {
            DesktopAdapterError::new("ADAPTER_FAILED", "Region identity could not be encoded")
        })
}

#[cfg(windows)]
fn fit_observation_projection(
    app_id: &str,
    region_ref: Option<&str>,
    payload: &mut ObservationPayload,
) -> Result<(), DesktopAdapterError> {
    let projected_size = |payload: &ObservationPayload, action_refs: bool| {
        let mut controls = payload.controls.clone();
        if action_refs {
            for control in &mut controls {
                if control.role == "edit"
                    && control.enabled
                    && control.automation_id.is_some()
                    && control.capabilities.iter().any(|value| value == "setValue")
                {
                    control.field_ref = Some(format!("field_{}", "0".repeat(32)));
                }
                if !supported_control_operations(control).is_empty() {
                    control.control_ref = Some(format!("control_{}", "0".repeat(32)));
                }
            }
        }
        let mut regions = payload.regions.clone();
        for region in &mut regions {
            region.region_ref = Some(format!("region_{}", "0".repeat(32)));
        }
        serde_json::to_vec(&DesktopWindowObservation {
            app_id: app_id.to_string(),
            controls,
            regions,
            region_ref: region_ref.map(str::to_string),
            truncated: payload.truncated,
            // Native scanning is complete. Reference issuance below performs
            // no provider reads and either succeeds or returns an error, so
            // only diagnostics already present belong to this final budget.
            diagnostics: payload.diagnostics.clone(),
        })
        .map(|bytes| bytes.len())
        .map_err(|_| {
            DesktopAdapterError::new("ADAPTER_FAILED", "Desktop observation could not be encoded")
        })
    };
    if projected_size(payload, !payload.truncated)? > MAX_OBSERVATION_BYTES {
        payload.truncated = true;
        if !payload
            .diagnostics
            .contains(&DesktopObservationDiagnostic::OutputLimit)
        {
            payload
                .diagnostics
                .push(DesktopObservationDiagnostic::OutputLimit);
        }
        while projected_size(payload, false)? > MAX_OBSERVATION_BYTES {
            if payload.controls.pop().is_none() {
                return Err(DesktopAdapterError::new(
                    "SCAN_LIMIT",
                    "Desktop region metadata exceeds its output budget",
                ));
            }
        }
    }
    Ok(())
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
    if payload.regions.len() > MAX_OBSERVED_REGIONS
        || payload.regions.iter().any(|region| {
            !matches!(region.role.as_str(), "pane" | "document" | "list" | "group")
                || (!region.name.is_empty() && !valid_observation_label(&region.name))
                || region.region_ref.is_some()
        })
    {
        return Err(DesktopAdapterError::new(
            "ADAPTER_FAILED",
            "Invalid desktop region metadata",
        ));
    }
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
                    | "text"
            )
            || control.name.as_ref().is_some_and(|name| {
                !matches!(
                    control.role.as_str(),
                    "button"
                        | "checkBox"
                        | "radioButton"
                        | "comboBox"
                        | "listItem"
                        | "tabItem"
                        | "menuItem"
                        | "hyperlink"
                        | "pane"
                        | "document"
                        | "list"
                        | "text"
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
                    | ("comboBox", "expandCollapse" | "selectOption")
                    | ("edit", "setValue")
                    | ("menuItem", "invoke" | "toggle" | "expandCollapse")
                    | ("pane" | "document" | "list", "scroll")
            );
            if !supported || !capabilities.insert(capability.as_str()) {
                return Err(DesktopAdapterError::new(
                    "ADAPTER_FAILED",
                    "Windows UI Automation returned invalid control capabilities",
                ));
            }
        }
        if let Some(state) = &control.state {
            if *state == DesktopControlState::default()
                || (state.toggle.is_some() && !capabilities.contains("toggle"))
                || (state.selected.is_some() && !capabilities.contains("select"))
                || state
                    .selection_container_role
                    .as_deref()
                    .is_some_and(|role| {
                        control.role != "listItem"
                            || !capabilities.contains("select")
                            || state.selected.is_none()
                            || !matches!(role, "list" | "comboBox")
                    })
                || (state.expansion.is_some() && !capabilities.contains("expandCollapse"))
                || state.vertical_scroll_percent.is_some_and(|value| {
                    !capabilities.contains("scroll")
                        || !value.is_finite()
                        || !(0.0..=100.0).contains(&value)
                })
            {
                return Err(DesktopAdapterError::new(
                    "ADAPTER_FAILED",
                    "Windows UI Automation returned invalid control state",
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
            &["radioButton", "tabItem", "listItem"][..],
            "select",
            DesktopControlOperation::Select,
        ),
        (
            &["comboBox"][..],
            "selectOption",
            DesktopControlOperation::Choose,
        ),
        (&["checkBox"][..], "toggle", DesktopControlOperation::Check),
        (
            &["checkBox"][..],
            "toggle",
            DesktopControlOperation::Uncheck,
        ),
        (
            &["menuItem", "comboBox"][..],
            "expandCollapse",
            DesktopControlOperation::Expand,
        ),
        (
            &["menuItem", "comboBox"][..],
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
    image_ref: Option<DesktopImageReference>,
}

impl DesktopWindowImage {
    pub(crate) fn from_png_bytes(png_bytes: Vec<u8>) -> Self {
        Self {
            png_bytes,
            image_ref: None,
        }
    }

    pub(crate) fn png_bytes(&self) -> &[u8] {
        &self.png_bytes
    }

    pub(crate) fn image_ref(&self) -> Option<&DesktopImageReference> {
        self.image_ref.as_ref()
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
    fn preflight_image_click(
        &self,
        _request: &DesktopImageClickRequest,
        _executable_path: &str,
        _timeout: std::time::Duration,
    ) -> Result<DesktopImageClickPreview, DesktopAdapterError> {
        Err(DesktopAdapterError::new(
            "UNSUPPORTED_PLATFORM",
            "Visual click preview is unavailable",
        ))
    }
    fn click_image_with_control(
        &self,
        _request: &DesktopImageClickRequest,
        _executable_path: &str,
        _target: &DesktopImageClickTarget,
        _is_cancelled: &dyn Fn() -> bool,
        _timeout: std::time::Duration,
    ) -> Result<DesktopActionResult, DesktopAdapterError> {
        Err(DesktopAdapterError::new(
            "UNSUPPORTED_PLATFORM",
            "Visual click is unavailable",
        ))
    }
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

    /// `allow_action_labels` is host-derived from the current interaction
    /// capability, never a model argument; it permits short ComboBox/ListItem Names.
    fn observe_trusted_window_with_control(
        &self,
        app_id: &str,
        executable_path: &str,
        region_ref: Option<&str>,
        _allow_action_labels: bool,
        is_cancelled: &dyn Fn() -> bool,
        _timeout: std::time::Duration,
    ) -> Result<DesktopWindowObservation, DesktopAdapterError> {
        if region_ref.is_some() {
            return Err(DesktopAdapterError::new(
                "UNSUPPORTED_CONTROL",
                "Scoped observation is unavailable through this adapter",
            ));
        }
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
    #[cfg(test)]
    next_execution_error: Mutex<Option<DesktopAdapterError>>,
    observed_apps: Mutex<std::collections::HashSet<(String, String)>>,
    observed_regions: Mutex<std::collections::HashSet<(String, String)>>,
    available_control_apps: Mutex<std::collections::HashSet<(String, String)>>,
}

impl MockDesktopAdapter {
    #[cfg(test)]
    pub(crate) fn fail_next_action_before_dispatch(&self, error: DesktopAdapterError) {
        *self.next_execution_error.lock().unwrap() = Some(error);
    }

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
            "mock-choice-ref" => Some(("mockChoice", 7, &[DesktopControlOperation::Choose])),
            "mock-toggle-ref" => Some((
                "Test checkbox",
                6,
                &[
                    DesktopControlOperation::Check,
                    DesktopControlOperation::Uncheck,
                ],
            )),
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
        #[cfg(test)]
        if let Some(error) = self.next_execution_error.lock().unwrap().take() {
            return Err(error);
        }
        if let DesktopAction::SetText {
            expected_target, ..
        } = action
        {
            if expected_target
                .as_ref()
                .and_then(|target| target.automation_id.as_deref())
                != Some("mock-field-ref")
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
            option,
            ..
        } = action
        {
            validate_control_option(*operation, option.as_deref())?;
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
                ("verified", app_id, "Mock field replacement verified")
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
                operation: DesktopControlOperation::Choose,
                ..
            } => ("verified", app_id, "Mock dropdown selection and collapsed state verified; task outcome is not verified"),
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
                operation: DesktopControlOperation::Check | DesktopControlOperation::Uncheck,
                ..
            } => (
                "verified",
                app_id,
                "Mock control requested checkbox state verified; task outcome is not verified",
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
            choice: match action {
                DesktopAction::OperateControl {
                    operation: DesktopControlOperation::Choose,
                    option: Some(option),
                    ..
                } => Some(DesktopChoiceReceipt {
                    control_name: "mockChoice".into(),
                    option: option.clone(),
                    expanded: false,
                }),
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
            control_label: if self
                .observed_regions
                .lock()
                .map_err(|_| {
                    DesktopAdapterError::new("ADAPTER_FAILED", "Mock adapter lock is poisoned")
                })?
                .contains(&(app_id.to_string(), executable_path.to_string()))
            {
                "Mock region / mockField".into()
            } else {
                "mockField".into()
            },
            identity: DesktopDraftTargetIdentity {
                process_id: 1,
                window_handle: 1,
                control_runtime_id: vec![1],
                automation_id: Some("mock-field-ref".into()),
                control_operation: None,
            },
        })
    }

    fn observe_trusted_window(
        &self,
        app_id: &str,
        executable_path: &str,
    ) -> Result<DesktopWindowObservation, DesktopAdapterError> {
        self.observed_regions
            .lock()
            .map_err(|_| {
                DesktopAdapterError::new("ADAPTER_FAILED", "Mock adapter lock is poisoned")
            })?
            .remove(&(app_id.to_string(), executable_path.to_string()));
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
        let mut toggle = DesktopToggleState::Off;
        let mut selected = false;
        let mut expansion = DesktopExpansionState::Collapsed;
        let mut scroll: f64 = 50.0;
        for action in self
            .actions
            .lock()
            .map_err(|_| {
                DesktopAdapterError::new("ADAPTER_FAILED", "Mock adapter lock is poisoned")
            })?
            .iter()
        {
            if let DesktopAction::OperateControl {
                app_id: target_app,
                executable_path: target_exe,
                operation,
                ..
            } = action
            {
                if target_app != app_id || target_exe != executable_path {
                    continue;
                }
                match operation {
                    DesktopControlOperation::Check => toggle = DesktopToggleState::On,
                    DesktopControlOperation::Uncheck => toggle = DesktopToggleState::Off,
                    DesktopControlOperation::Select => selected = true,
                    DesktopControlOperation::Choose => {}
                    DesktopControlOperation::Expand => expansion = DesktopExpansionState::Expanded,
                    DesktopControlOperation::Collapse => {
                        expansion = DesktopExpansionState::Collapsed
                    }
                    DesktopControlOperation::ScrollUp => scroll = (scroll - 10.0).max(0.0),
                    DesktopControlOperation::ScrollDown => scroll = (scroll + 10.0).min(100.0),
                    DesktopControlOperation::Invoke => {}
                }
            }
        }
        Ok(DesktopWindowObservation {
            app_id: app_id.to_string(),
            regions: vec![DesktopObservedRegion {
                role: "group".into(),
                name: "Mock region".into(),
                region_ref: Some("mock-region-ref".into()),
            }],
            region_ref: None,
            controls: vec![
                DesktopObservedControl {
                    depth: 1,
                    role: "edit".into(),
                    name: None,
                    automation_id: Some("mockField".into()),
                    enabled: true,
                    capabilities: vec!["setValue".into()],
                    state: None,
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
                    state: None,
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
                    state: Some(DesktopControlState {
                        selected: Some(selected),
                        ..Default::default()
                    }),
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
                    state: Some(DesktopControlState {
                        expansion: Some(expansion),
                        ..Default::default()
                    }),
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
                    state: Some(DesktopControlState {
                        vertical_scroll_percent: Some(scroll),
                        ..Default::default()
                    }),
                    field_ref: None,
                    control_ref: Some("mock-scroll-ref".into()),
                },
                DesktopObservedControl {
                    depth: 1,
                    role: "checkBox".into(),
                    name: Some("Test checkbox".into()),
                    automation_id: None,
                    enabled: true,
                    capabilities: vec!["toggle".into()],
                    state: Some(DesktopControlState {
                        toggle: Some(toggle),
                        ..Default::default()
                    }),
                    field_ref: None,
                    control_ref: Some("mock-toggle-ref".into()),
                },
                DesktopObservedControl {
                    depth: 1,
                    role: "comboBox".into(),
                    name: Some("mockChoice".into()),
                    automation_id: None,
                    enabled: true,
                    capabilities: vec!["selectOption".into()],
                    state: None,
                    field_ref: None,
                    control_ref: Some("mock-choice-ref".into()),
                },
            ],
            truncated: false,
            diagnostics: vec![],
        })
    }

    fn observe_trusted_window_with_control(
        &self,
        app_id: &str,
        executable_path: &str,
        region_ref: Option<&str>,
        _allow_action_labels: bool,
        is_cancelled: &dyn Fn() -> bool,
        timeout: std::time::Duration,
    ) -> Result<DesktopWindowObservation, DesktopAdapterError> {
        if is_cancelled() || timeout.is_zero() {
            return Err(DesktopAdapterError::new(
                "SCAN_CANCELLED",
                "Desktop observation was cancelled",
            ));
        }
        if region_ref.is_some_and(|reference| reference != "mock-region-ref") {
            return Err(DesktopAdapterError::new(
                "TARGET_CHANGED",
                "Region reference is unavailable",
            ));
        }
        let mut observation = self.observe_trusted_window(app_id, executable_path)?;
        if let Some(reference) = region_ref {
            self.observed_regions
                .lock()
                .map_err(|_| {
                    DesktopAdapterError::new("ADAPTER_FAILED", "Mock adapter lock is poisoned")
                })?
                .insert((app_id.to_string(), executable_path.to_string()));
            observation
                .controls
                .retain(|control| matches!(control.role.as_str(), "edit" | "checkBox" | "pane"));
            observation.regions.clear();
            observation.region_ref = Some(reference.to_string());
        }
        Ok(observation)
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
            control_label: if self
                .observed_regions
                .lock()
                .map_err(|_| {
                    DesktopAdapterError::new("ADAPTER_FAILED", "Mock adapter lock is poisoned")
                })?
                .contains(&(app_id.to_string(), executable_path.to_string()))
            {
                format!("Mock region / {label}")
            } else {
                label.into()
            },
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
    scope: Option<RegionScope>,
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
    scope: Option<RegionScope>,
    expires_at: std::time::Instant,
}

#[cfg(windows)]
#[derive(Debug, Clone)]
struct RegionRefLease {
    app_id: String,
    executable: std::path::PathBuf,
    scope: RegionScope,
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
    region_refs: Mutex<std::collections::HashMap<String, RegionRefLease>>,
    image_refs: image_click::ImageReferenceStore,
}

#[cfg(windows)]
const FIELD_REF_TTL: std::time::Duration = std::time::Duration::from_secs(300);
#[cfg(windows)]
const MAX_FIELD_REFS: usize = 128;

#[cfg(windows)]
const DRAFT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);

#[cfg(windows)]
pub(crate) const DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

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
$script:uiaNodeCount = 0
$script:uiaScope = $null
if ($env:ANGELBOT_REGION_SCOPE) {
  try { $script:uiaScope = $env:ANGELBOT_REGION_SCOPE | ConvertFrom-Json -ErrorAction Stop }
  catch { throw 'TARGET_CHANGED|The selected region identity is invalid' }
}
function Use-UiaNodeBudget {
  $script:uiaNodeCount++
  if ($script:uiaNodeCount -gt 256) { throw 'SCAN_LIMIT|The shared control traversal limit was reached' }
}
function Get-UiaScopeWalker {
  if ($null -ne $script:uiaWalker) { return $script:uiaWalker }
  return [Windows.Automation.TreeWalker]::ControlViewWalker
}
function Assert-UiaScopeNode($element, [int]$processId) {
  Use-UiaNodeBudget
  $state = Get-UiaSafetyState $element
  if ($state.password -or $state.offscreen -or $state.processId -ne $processId) {
    throw 'TARGET_CHANGED|The selected region path is no longer safe'
  }
  return $state
}
function Get-UiaScopeRoot {
  if ($null -eq $script:uiaScope) { return $root }
  try {
    $scope = $script:uiaScope
    $scopeProcessId = [int]$scope.identity.processId
    $process = Get-Process -Id $scopeProcessId -ErrorAction Stop
    if ([IO.Path]::GetFullPath($process.Path) -ine $expected -or
        (Get-UiaWindowProcessId ([IntPtr]::new([long]$scope.identity.windowHandle))) -ne $scopeProcessId -or
        [long]$windows[0].ToInt64() -ne [long]$scope.identity.windowHandle) { throw 'Window changed' }
    $null = Assert-UiaScopeNode $root $scopeProcessId
    $scopeWalker = Get-UiaScopeWalker
    $element = $root
    foreach ($siblingIndex in $scope.path) {
      $element = $scopeWalker.GetFirstChild($element)
      if ($null -eq $element) { throw 'Path changed' }
      for ($sibling = 0; $sibling -lt [int]$siblingIndex; $sibling++) {
        # Skipped siblings are navigation only, never inspected or descended.
        Use-UiaNodeBudget
        $element = $scopeWalker.GetNextSibling($element)
        if ($null -eq $element) { throw 'Path changed' }
      }
      $null = Assert-UiaScopeNode $element $scopeProcessId
    }
    $state = Get-UiaSafetyState $element
    $expectedRole = switch ([string]$scope.role) { 'pane' {'ControlType.Pane'} 'document' {'ControlType.Document'} 'list' {'ControlType.List'} 'group' {'ControlType.Group'} default {throw 'Role changed'} }
    if ([string]$state.current.Name -cne [string]$scope.name -or
        [string]$state.current.ControlType.ProgrammaticName -cne $expectedRole -or
        ((Get-UiaRuntimeId $element) -join ',') -cne ($scope.identity.controlRuntimeId -join ',')) { throw 'Region changed' }
    return $element
  } catch {
    if ($_.Exception.Message -like 'SCAN_LIMIT|*') { throw }
    throw 'TARGET_CHANGED|The selected region path or identity has changed'
  }
}
function Assert-UiaTargetInScope($element) {
  if ($null -eq $script:uiaScope) { return }
  $scopeRoot = Get-UiaScopeRoot
  $scopeRuntime = (Get-UiaRuntimeId $scopeRoot) -join ','
  $scopeWalker = Get-UiaScopeWalker
  for ($ancestor = 0; $ancestor -le 12 -and $null -ne $element; $ancestor++) {
    $null = Assert-UiaScopeNode $element ([int]$script:uiaScope.identity.processId)
    if (((Get-UiaRuntimeId $element) -join ',') -ceq $scopeRuntime) { return }
    $element = $scopeWalker.GetParent($element)
  }
  throw 'TARGET_CHANGED|The selected control is no longer inside its observed region'
}
function Find-UiaScopedRuntimeTarget([string]$expectedRuntimeId, $walker) {
  $script:uiaWalker = $walker
  $scopeRoot = Get-UiaScopeRoot
  $pending = New-Object 'System.Collections.Generic.Stack[object]'
  if ($null -ne $script:uiaScope) { $pending.Push(@{ element = $scopeRoot; depth = 1; sibling = $false }) }
  else {
    try { $first = $walker.GetFirstChild($scopeRoot) }
    catch { throw 'TARGET_CHANGED|The selected control tree is unavailable' }
    if ($null -ne $first) { $pending.Push(@{ element = $first; depth = 1; sibling = $true }) }
  }
  $found = $null
  while ($pending.Count -gt 0) {
    $entry = $pending.Pop()
    $candidate = $entry.element
    Use-UiaNodeBudget
    try {
      $state = Get-UiaSafetyState $candidate
      if ($entry.sibling) {
        $sibling = $walker.GetNextSibling($candidate)
        if ($null -ne $sibling) { $pending.Push(@{ element = $sibling; depth = $entry.depth; sibling = $true }) }
      }
      if ($state.password -or $state.offscreen) { continue }
      if (((Get-UiaRuntimeId $candidate) -join ',') -ceq $expectedRuntimeId) {
        if ($null -ne $found) { throw 'TARGET_AMBIGUOUS|The selected control identity is duplicated' }
        $found = $candidate
      }
      $child = $walker.GetFirstChild($candidate)
    } catch { throw 'TARGET_CHANGED|The selected control tree is unavailable' }
    if ($null -ne $child) {
      if ($entry.depth -ge 12) { throw 'SCAN_LIMIT|The selected control tree depth limit was reached' }
      $pending.Push(@{ element = $child; depth = ($entry.depth + 1); sibling = $true })
    }
  }
  if ($null -eq $found) { throw 'TARGET_CHANGED|The selected control is no longer available' }
  return $found
}
"#;

#[cfg(windows)]
const TRUSTED_WINDOW_SCRIPT: &str = concat!(
    include_str!("desktop_control/window_identity.ps1"),
    r#"
$expected = [IO.Path]::GetFullPath($env:ANGELBOT_TARGET_EXE)
$readiness = [Diagnostics.Stopwatch]::StartNew()
while ($true) {
  $windows = @(Get-UiaControlWindows $expected)
  if ($windows.Count -gt 0) { break }
  $remaining = [int]$script:uiaWindowReadinessWaitMs - $readiness.ElapsedMilliseconds
  if ($remaining -le 0) { throw 'TARGET_NOT_FOUND|The trusted application has no eligible control window' }
  Start-Sleep -Milliseconds ([Math]::Min(100, $remaining))
}
if ($windows.Count -ne 1) { throw 'TARGET_AMBIGUOUS|The trusted application has multiple eligible control windows' }
$root = [Windows.Automation.AutomationElement]::FromHandle($windows[0])
if ($null -eq $root) { throw 'TARGET_UNAVAILABLE|The trusted application window is unavailable' }
$windowProcessId = Get-UiaWindowProcessId $windows[0]
$windowProcess = Get-Process -Id $windowProcessId -ErrorAction Stop
if ([IO.Path]::GetFullPath($windowProcess.Path) -ine $expected -or
    (Get-UiaSafetyState $root).processId -ne $windowProcessId) {
  throw 'TARGET_CHANGED|The trusted control window identity changed'
}
"#
);

#[cfg(windows)]
fn control_observation_script(has_region_scope: bool) -> String {
    // Only a new root observation may wait for a just-launched app's first
    // window. Leased targets and every effect retain strict one-shot selection.
    let readiness = if has_region_scope {
        ""
    } else {
        "$script:uiaWindowReadinessWaitMs = 1500"
    };
    format!(
        "{UIA_SAFETY_HELPERS_SCRIPT}\n{readiness}\n{TRUSTED_WINDOW_SCRIPT}\n{OBSERVATION_SCRIPT}"
    )
}

/// Capture remains limited to top-level HWNDs. Controls can additionally use
/// the exact trusted process's own HWND inside a separate Windows app host.
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
const CAPTURE_WINDOW_SCRIPT: &str = concat!(
    include_str!("desktop_control/window_identity.ps1"),
    r#"
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
if ([AngelBotCaptureWindows]::IsIconic($windows[0])) { throw 'TARGET_MINIMIZED|Restore the trusted window before requesting observation again' }
$root = [Windows.Automation.AutomationElement]::FromHandle($windows[0])
if ($null -eq $root) { throw 'TARGET_UNAVAILABLE|The trusted application window is unavailable' }
if ((Get-UiaSafetyState $root).processId -ne $selected.processId) { throw 'TARGET_CHANGED|The trusted window identity changed' }
"#
);

/// Existing UIA safety metadata, bounded walk. No field values or names exit.
#[cfg(windows)]
const CAPTURE_TARGET_SCRIPT: &str = r#"
$rootState = Get-UiaSafetyState $root
if ([AngelBotCaptureWindows]::IsIconic($windows[0])) { throw 'TARGET_MINIMIZED|Restore the trusted window before requesting observation again' }
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
if ([AngelBotCaptureWindows]::IsIconic($windows[0])) { throw 'TARGET_MINIMIZED|Restore the trusted window before requesting observation again' }
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
    #[cfg(feature = "desktop-e2e")]
    let script = script
        .replace("$selected = Select-CaptureWindow", "[Console]::Error.WriteLine('[DEBUG-capture] enumeration-done')\n$selected = Select-CaptureWindow")
        .replace("$pending.Push(@{ element = $root; depth = 0 })", "[Console]::Error.WriteLine('[DEBUG-capture] scan-start')\n$pending.Push(@{ element = $root; depth = 0 })")
        .replace("$stateAfter = Get-UiaSafetyState $root", "[Console]::Error.WriteLine('[DEBUG-capture] scan-done')\n$stateAfter = Get-UiaSafetyState $root");
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
    (Get-UiaWindowProcessId ([IntPtr]::new($expectedWindowHandle))) -ne $expectedProcessId -or
    [long]$windows[0].ToInt64() -ne $expectedWindowHandle -or
    $rootState.processId -ne $expectedProcessId -or
    $rootState.offscreen -or $rootState.password) {
  throw 'TARGET_CHANGED|The selected application window has changed'
}
$walker = [Windows.Automation.TreeWalker]::ControlViewWalker
$target = Find-UiaScopedRuntimeTarget $expectedRuntimeId $walker
Assert-UiaTargetInScope $target
try { $targetState = Get-UiaSafetyState $target }
catch { throw 'TARGET_CHANGED|The selected field is no longer available' }
if ($targetState.password) { throw 'SENSITIVE_SURFACE|Password controls cannot be automated' }
$currentLabel = $targetState.current.AutomationId
if ($targetState.offscreen -or -not $targetState.enabled -or
    $targetState.processId -ne $expectedProcessId -or $null -eq $currentLabel -or
    [string]$currentLabel -cne $fieldLabel -or
    [string]$targetState.current.ControlType.ProgrammaticName -cne 'ControlType.Edit') {
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
  'select' { $controlRole -in @('radioButton', 'tabItem', 'listItem') }
  'choose' { $controlRole -ceq 'comboBox' }
  'check' { $controlRole -ceq 'checkBox' }
  'uncheck' { $controlRole -ceq 'checkBox' }
  'expand' { $controlRole -in @('menuItem', 'comboBox') }
  'collapse' { $controlRole -in @('menuItem', 'comboBox') }
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
  'checkBox' { 'ControlType.CheckBox' }
  'tabItem' { 'ControlType.TabItem' }
  'listItem' { 'ControlType.ListItem' }
  'comboBox' { 'ControlType.ComboBox' }
  'pane' { 'ControlType.Pane' }
  'document' { 'ControlType.Document' }
  'list' { 'ControlType.List' }
  default { throw 'UNSUPPORTED_CONTROL|The selected control role is unsupported' }
}
function Assert-UiaControlTarget($element) {
  Assert-UiaTargetInScope $element
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
      (Get-UiaWindowProcessId ([IntPtr]::new($expectedWindowHandle))) -ne $expectedProcessId -or
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
    'check' { [Windows.Automation.TogglePattern]::Pattern }
    'uncheck' { [Windows.Automation.TogglePattern]::Pattern }
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
      if ($selected -isnot [bool]) { throw 'Unavailable selected state' }
      return $selected
    }
    if ($controlOperation -in @('check', 'uncheck')) {
      $toggleState = $patternCurrent.ToggleState
      if ($null -eq $toggleState -or [string]$toggleState -cnotin @('On', 'Off', 'Indeterminate')) {
        throw 'Unavailable checkbox state'
      }
    } else {
      $expandState = $patternCurrent.ExpandCollapseState
      if ($null -eq $expandState -or [string]$expandState -cnotin @('Collapsed', 'Expanded', 'PartiallyExpanded', 'LeafNode')) {
        throw 'Unavailable expand state'
      }
    }
  } catch { throw 'TARGET_UNAVAILABLE|The selected control state is unavailable' }
  if ($controlOperation -in @('check', 'uncheck')) {
    # TogglePattern cycles states, it does not set one. Refuse tri-state input
    # rather than applying a second effect under the same confirmation.
    if ([string]$toggleState -ceq 'Indeterminate') { throw 'UNSUPPORTED_CONTROL|Indeterminate checkboxes cannot be automated' }
    return [string]$toggleState
  }
  if ([string]$expandState -ceq 'LeafNode') { throw 'UNSUPPORTED_CONTROL|Leaf nodes cannot expand or collapse' }
  return [string]$expandState
}
# Locate only the observed RuntimeId, with the same bounded Control View walk.
$walker = [Windows.Automation.TreeWalker]::ControlViewWalker
$target = Find-UiaScopedRuntimeTarget $expectedRuntimeId $walker
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
  'check' { 'On' }
  'uncheck' { 'Off' }
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
      'check' { $pattern.Toggle() }
      'uncheck' { $pattern.Toggle() }
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
    # unavailable state reads/mismatches, never the effect or identity/safety failures.
    for ($read = 0; $read -lt 11; $read++) {
      $freshPattern = Assert-UiaControlTarget $target
      $matched = $false
      try {
        $afterState = Get-UiaControlOperationState $freshPattern
        $matched = if ($controlOperation -ceq 'scrollup' -and -not $alreadyDesired) { $afterState -lt $beforeState }
                   elseif ($controlOperation -ceq 'scrolldown' -and -not $alreadyDesired) { $afterState -gt $beforeState }
                   else { $afterState -ceq $desiredState }
      } catch {
        if (-not $_.Exception.Message.StartsWith('TARGET_UNAVAILABLE|', [StringComparison]::Ordinal)) { throw }
      }
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
const CHOOSE_OPTION_SCRIPT: &str = include_str!("desktop_control/choose_option.ps1");

#[cfg(windows)]
const VERIFIED_SET_VALUE_SCRIPT: &str = r#"
# Windows can omit an empty environment variable; clearing must still pass
# an exact empty string to SetValue and compare it ordinally during readback.
$text = [string]$env:ANGELBOT_DRAFT_TEXT
$expectedProcessId = [int]$env:ANGELBOT_DRAFT_EXPECTED_PID
$expectedWindowHandle = [long]$env:ANGELBOT_DRAFT_EXPECTED_HWND
$expectedRuntimeId = [string]$env:ANGELBOT_DRAFT_EXPECTED_RUNTIME_ID
# Recheck identity immediately before SetValue; a stale approval cannot be
# redirected to a different process, window or UIA element.
try {
  Assert-UiaTargetInScope $target
  $currentProcess = Get-Process -Id $expectedProcessId -ErrorAction Stop
  $currentPath = [IO.Path]::GetFullPath($currentProcess.Path)
  $actualRuntimeId = Get-UiaRuntimeId $target
  $rootState = Get-UiaSafetyState $root
  $targetState = Get-UiaSafetyState $target
} catch { throw 'TARGET_CHANGED|The preflighted field is no longer available' }
if ($currentPath -ine $expected -or
    (Get-UiaWindowProcessId ([IntPtr]::new($expectedWindowHandle))) -ne $expectedProcessId -or
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
  Assert-UiaTargetInScope $target
  $currentProcess = Get-Process -Id $expectedProcessId -ErrorAction Stop
  $freshRootState = Get-UiaSafetyState $root
  $freshTargetState = Get-UiaSafetyState $target
  if ([IO.Path]::GetFullPath($currentProcess.Path) -ine $expected -or
      (Get-UiaWindowProcessId ([IntPtr]::new($expectedWindowHandle))) -ne $expectedProcessId -or
      [long]$windows[0].ToInt64() -ne $expectedWindowHandle -or
      $freshRootState.processId -ne $expectedProcessId -or $freshRootState.password -or $freshRootState.offscreen -or
      $freshTargetState.processId -ne $expectedProcessId -or $freshTargetState.password -or $freshTargetState.offscreen -or -not $freshTargetState.enabled -or
      [string]$freshTargetState.current.ControlType.ProgrammaticName -cne 'ControlType.Edit' -or
      ((Get-UiaRuntimeId $target) -join ',') -cne $expectedRuntimeId -or
      ($env:ANGELBOT_FIELD_LABEL -and [string]$freshTargetState.current.AutomationId -cne [string]$env:ANGELBOT_FIELD_LABEL)) {
    throw 'Readback field identity changed'
  }
  $freshPattern = $null
  if (-not $target.TryGetCurrentPattern([Windows.Automation.ValuePattern]::Pattern, [ref]$freshPattern) -or
      $null -eq $freshPattern -or (Get-UiaValuePatternReadOnly $freshPattern)) { throw 'Readback field pattern changed' }
  $actual = $freshPattern.Current.Value
  if ($null -eq $actual -or -not [string]::Equals($actual, $text, [StringComparison]::Ordinal)) {
    throw 'VERIFICATION_FAILED|The field value did not match the requested text'
  }
} catch {
  throw 'RESULT_UNKNOWN|The field may have changed but verification was not completed'
}
@{ status = 'verified'; detail = 'Field content was replaced and verified through Windows UI Automation; task outcome is not verified' } | ConvertTo-Json -Compress
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
$script:uiaWalker = $walker
$scopeRoot = Get-UiaScopeRoot
$stack = New-Object 'System.Collections.Generic.Stack[object]'
$controls = New-Object 'System.Collections.Generic.List[object]'
$targets = New-Object 'System.Collections.Generic.List[object]'
$regions = New-Object 'System.Collections.Generic.List[object]'
$regionTargets = New-Object 'System.Collections.Generic.List[object]'
$diagnostics = New-Object 'System.Collections.Generic.List[string]'
# Reserve the complete bounded diagnostic array so later provider failures
# cannot push an otherwise valid public payload over its output budget.
$budgetDiagnostics = @('controlLimit', 'outputLimit', 'nodeLimit', 'depthLimit', 'providerUnavailable')
$truncated = $false
$metadataLimitReached = $false
function Mark-ObservationPartial([string]$reason) {
  $script:truncated = $true
  if (-not $diagnostics.Contains($reason)) { $diagnostics.Add($reason) }
}
function Test-ObservationStaticText($element) {
  # A Text child of an editor/choice field can mirror its private value.
  # Check every ancestor without reading any ancestor's Name or text patterns.
  $rootId = (Get-UiaRuntimeId $root) -join ','
  $scopeId = $script:uiaScope.identity.controlRuntimeId -join ','
  $insideScope = $false
  for ($ancestor = 0; $ancestor -le 24 -and $null -ne $element; $ancestor++) {
    $state = Assert-UiaScopeNode $element ([int]$script:uiaScope.identity.processId)
    $ancestorRole = [string]$state.current.ControlType.ProgrammaticName
    if ([string]::IsNullOrEmpty($ancestorRole)) { throw 'TARGET_UNAVAILABLE|Ancestor role is unavailable' }
    if ($ancestor -eq 0 -and $ancestorRole -cne 'ControlType.Text') { return $false }
    if ($ancestorRole -in @('ControlType.Edit', 'ControlType.ComboBox', 'ControlType.ListItem')) { return $false }
    $runtimeId = (Get-UiaRuntimeId $element) -join ','
    if ($runtimeId -ceq $scopeId) { $insideScope = $true }
    if ($runtimeId -ceq $rootId -and $insideScope) { return $true }
    $element = $walker.GetParent($element)
  }
  throw 'TARGET_CHANGED|Static text is outside the observed window'
}
if ($null -ne $script:uiaScope) {
  $stack.Push(@{ element = $scopeRoot; depth = 1; path = [int[]]@($script:uiaScope.path); self = $true })
} else {
  $first = $null
  try { $first = $walker.GetFirstChild($scopeRoot) }
  catch { Mark-ObservationPartial 'providerUnavailable' }
  if ($null -ne $first) { $stack.Push(@{ element = $first; depth = 1; path = [int[]]@(0); self = $false }) }
}
while ($stack.Count -gt 0) {
  $entry = $stack.Pop()
  $element = $entry.element
  try { Use-UiaNodeBudget } catch { Mark-ObservationPartial 'nodeLimit'; break }
  $sibling = $null
  try { if (-not $entry.self) { $sibling = $walker.GetNextSibling($element) } }
  catch { Mark-ObservationPartial 'providerUnavailable' }
  if ($null -ne $sibling) {
    $siblingPath = [int[]]@($entry.path)
    $siblingPath[-1]++
    $stack.Push(@{ element = $sibling; depth = $entry.depth; path = $siblingPath; self = $false })
  }
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
  $regionRole = switch ($roleName) {
    'ControlType.Pane' { 'pane' } 'ControlType.Document' { 'document' }
    'ControlType.List' { 'list' } 'ControlType.Group' { 'group' } default { $null }
  }
  if ($null -ne $regionRole -and -not $entry.self -and $regions.Count -lt 8 -and $entry.path.Count -le 12 -and
      (($entry.path | Measure-Object -Sum).Sum + $entry.path.Count) -lt 256) {
    try {
      $regionName = $current.Name
      if ($null -ne $regionName -and $regionName.Length -le 80 -and
          $regionName -ceq $regionName.Trim() -and $regionName -notmatch '\p{Cc}') {
        $regionProcessId = $current.ProcessId
        $windowProcessId = $root.Current.ProcessId
        if ($null -eq $regionProcessId -or $null -eq $windowProcessId) { throw 'Unavailable region process' }
        if ([int]$regionProcessId -gt 0 -and [int]$regionProcessId -eq [int]$windowProcessId) {
          $regionRuntimeId = Get-UiaRuntimeId $element
          $regions.Add(@{ role = $regionRole; name = [string]$regionName })
          $regionTargets.Add(@{ index = [int]($regions.Count - 1); scope = @{
            role = $regionRole; name = [string]$regionName; path = [int[]]@($entry.path)
            identity = @{ processId = [int]$windowProcessId; windowHandle = [long]$windows[0].ToInt64(); controlRuntimeId = $regionRuntimeId }
          } })
          $regionProbe = @{ controls = @($controls.ToArray()); regions = @($regions.ToArray()); truncated = $true; diagnostics = $budgetDiagnostics } | ConvertTo-Json -Compress -Depth 5
          while ([Text.Encoding]::UTF8.GetByteCount($regionProbe) -gt 3072 -and $controls.Count -gt 0) {
            $controls.RemoveAt($controls.Count - 1)
            Mark-ObservationPartial 'outputLimit'
            $metadataLimitReached = $true
            $regionProbe = @{ controls = @($controls.ToArray()); regions = @($regions.ToArray()); truncated = $true; diagnostics = $budgetDiagnostics } | ConvertTo-Json -Compress -Depth 5
          }
        }
      }
    } catch { Mark-ObservationPartial 'providerUnavailable' }
  }
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
    'ControlType.Text' { if ($null -ne $script:uiaScope) { 'text' } }
    default { $null }
  }
  if ($role -eq 'text') {
    try { if (-not (Test-ObservationStaticText $element)) { $role = $null } }
    catch {
      if ($_.Exception.Message -like 'SCAN_LIMIT|*') { Mark-ObservationPartial 'nodeLimit' }
      else { Mark-ObservationPartial 'providerUnavailable' }
      $role = $null
    }
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
    # ComboBox/ListItem names can mirror user data. Only the host's explicit
    # interaction capability permits their short action labels, never Value.
    # Edit names stay hidden independently of that capability.
    if ($role -in @('button', 'checkBox', 'radioButton', 'tabItem', 'menuItem', 'hyperlink', 'pane', 'document', 'list') -or
        ($env:ANGELBOT_ACTION_LABELS -ceq '1' -and $role -in @('comboBox', 'listItem')) -or $role -eq 'text') {
      try {
        $rawName = $current.Name
        if ($null -eq $rawName) { Mark-ObservationPartial 'providerUnavailable' }
        $candidate = [string]$rawName
        if ($candidate.Length -gt 0 -and $candidate.Length -le 80 -and
            $candidate -ceq $candidate.Trim() -and $candidate -notmatch '\p{Cc}') {
          $name = $candidate
        } elseif ($role -eq 'text' -and $candidate.Length -gt 0) {
          Mark-ObservationPartial 'providerUnavailable'
        }
      } catch { Mark-ObservationPartial 'providerUnavailable' }
    }
    # Do not return an empty or shortened business result as readable text.
    if ($role -eq 'text' -and $null -eq $name) { continue }
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
    $state = @{}
    if ($enabled) {
      if ($isScrollContainer) { $capabilities += 'scroll'; $state.verticalScrollPercent = $position }
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
          $toggle = $pattern.Current.ToggleState
          if ($null -eq $toggle) { throw 'Unavailable toggle state' }
          $state.toggle = switch -CaseSensitive ($toggle.ToString()) {
            'Off' { 'off' }; 'On' { 'on' }; 'Indeterminate' { 'indeterminate' }
            default { throw 'Unavailable toggle state' }
          }
        }
      } catch { Mark-ObservationPartial 'providerUnavailable' }
      try {
        $pattern = $null
        if ($role -in @('radioButton', 'listItem', 'tabItem') -and
            $element.TryGetCurrentPattern([Windows.Automation.SelectionItemPattern]::Pattern, [ref]$pattern)) {
          $capabilities += 'select'
          $selected = $pattern.Current.IsSelected
          if ($selected -isnot [bool]) { throw 'Unavailable selection state' }
          $state.selected = $selected
          # Container type disambiguates same-name items without reading a
          # parent Name, hidden option or field Value/Text. It grants no action.
          if ($env:ANGELBOT_ACTION_LABELS -ceq '1' -and $role -ceq 'listItem') {
            $container = $pattern.Current.SelectionContainer
            if ($null -ne $container) {
              $containerState = Get-UiaSafetyState $container
              if ($containerState.processId -eq $windowProcessId -and -not $containerState.offscreen -and -not $containerState.password) {
                switch -CaseSensitive ($containerState.current.ControlType.ProgrammaticName) {
                  'ControlType.List' { $state.selectionContainerRole = 'list' }
                  'ControlType.ComboBox' { $state.selectionContainerRole = 'comboBox' }
                }
              }
            }
          }
        }
      } catch { Mark-ObservationPartial 'providerUnavailable' }
      try {
        $pattern = $null
        if ($role -in @('comboBox', 'menuItem') -and
            $element.TryGetCurrentPattern([Windows.Automation.ExpandCollapsePattern]::Pattern, [ref]$pattern)) {
          $capabilities += 'expandCollapse'
          $expansion = $pattern.Current.ExpandCollapseState
          if ($null -eq $expansion) { throw 'Unavailable expansion state' }
          $state.expansion = switch -CaseSensitive ($expansion.ToString()) {
            'Collapsed' { 'collapsed' }; 'Expanded' { 'expanded' }
            'PartiallyExpanded' { 'partiallyExpanded' }; 'LeafNode' { 'leafNode' }
            default { throw 'Unavailable expansion state' }
          }
        }
      } catch { Mark-ObservationPartial 'providerUnavailable' }
      try {
        $pattern = $null
        if ($role -ceq 'comboBox' -and $capabilities -contains 'expandCollapse' -and
            $element.TryGetCurrentPattern([Windows.Automation.SelectionPattern]::Pattern, [ref]$pattern)) {
          $multiple = $pattern.Current.CanSelectMultiple
          if ($multiple -isnot [bool]) { throw 'Unavailable selection mode' }
          if (-not $multiple) { $capabilities += 'selectOption' }
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
    # Match the Rust projection: absent optional metadata costs no payload
    # space. Null placeholders must not exhaust the unchanged metadata budget.
    $control = @{ depth = [int]$entry.depth; role = $role; enabled = $enabled }
    if ($null -ne $name) { $control.name = $name }
    if ($null -ne $automationId) { $control.automationId = $automationId }
    if ($capabilities.Count -gt 0) { $control.capabilities = @($capabilities) }
    if ($state.Count -gt 0) { $control.state = $state }
    $controls.Add($control)
    $probe = @{ controls = @($controls.ToArray()); regions = @($regions.ToArray()); truncated = $false; diagnostics = $budgetDiagnostics } | ConvertTo-Json -Compress -Depth 5
    if ([Text.Encoding]::UTF8.GetByteCount($probe) -gt 3072) {
      $controls.RemoveAt($controls.Count - 1)
      Mark-ObservationPartial 'outputLimit'
      $metadataLimitReached = $true
    } elseif (($role -eq 'edit' -and $enabled -and $capabilities -contains 'setValue' -and $null -ne $automationId) -or
              ($role -in @('button', 'hyperlink', 'menuItem') -and $enabled -and $capabilities -contains 'invoke' -and $null -ne $name) -or
              ($role -in @('radioButton', 'tabItem', 'listItem') -and $enabled -and $capabilities -contains 'select' -and $null -ne $name) -or
              ($role -eq 'checkBox' -and $enabled -and $capabilities -contains 'toggle' -and $null -ne $name) -or
              ($role -in @('menuItem', 'comboBox') -and $enabled -and $capabilities -contains 'expandCollapse' -and $null -ne $name) -or
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
    else { $stack.Push(@{ element = $child; depth = ($entry.depth + 1); path = [int[]](@($entry.path) + @(0)); self = $false }) }
  }
}
$json = @{ controls = @($controls.ToArray()); regions = @($regions.ToArray()); truncated = $truncated; diagnostics = @($diagnostics.ToArray()); targets = @($targets.ToArray() | Where-Object { $_.index -lt $controls.Count }); region_targets = @($regionTargets.ToArray()) } | ConvertTo-Json -Compress -Depth 8
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
        "TARGET_MINIMIZED",
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
        // TARGET_BUSY is a pre-dispatch refusal, never a post-action result.
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
        scope: Option<&RegionScope>,
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
                    scope: scope.cloned(),
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
        scope: Option<&RegionScope>,
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
                    scope: scope.cloned(),
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
        let mut regions = self.region_refs.lock().map_err(|_| {
            DesktopAdapterError::new(
                "ADAPTER_FAILED",
                "Desktop region reference store is unavailable",
            )
        })?;
        fields.retain(|_, lease| lease.app_id != app_id || lease.executable != executable);
        controls.retain(|_, lease| lease.app_id != app_id || lease.executable != executable);
        regions.retain(|_, lease| lease.app_id != app_id || lease.executable != executable);
        self.image_refs.revoke(app_id)?;
        Ok(())
    }

    fn issue_region_refs(
        &self,
        app_id: &str,
        executable: &std::path::Path,
        regions: &mut [DesktopObservedRegion],
        targets: &[PrivateObservedRegion],
    ) -> Result<(), DesktopAdapterError> {
        let now = std::time::Instant::now();
        let mut refs = self.region_refs.lock().map_err(|_| {
            DesktopAdapterError::new(
                "ADAPTER_FAILED",
                "Desktop region reference store is unavailable",
            )
        })?;
        refs.retain(|_, lease| lease.expires_at > now);
        if refs.len().saturating_add(targets.len()) > MAX_FIELD_REFS {
            return Err(DesktopAdapterError::new(
                "SCAN_LIMIT",
                "Too many active desktop region references",
            ));
        }
        let mut identities = std::collections::HashMap::new();
        for target in targets {
            *identities
                .entry((
                    target.scope.identity.process_id,
                    target.scope.identity.window_handle,
                    target.scope.identity.control_runtime_id.clone(),
                ))
                .or_insert(0usize) += 1;
        }
        for target in targets {
            if identities.get(&(
                target.scope.identity.process_id,
                target.scope.identity.window_handle,
                target.scope.identity.control_runtime_id.clone(),
            )) != Some(&1)
            {
                continue;
            }
            let reference = format!("region_{}", uuid::Uuid::new_v4().simple());
            refs.insert(
                reference.clone(),
                RegionRefLease {
                    app_id: app_id.to_string(),
                    executable: executable.to_path_buf(),
                    scope: target.scope.clone(),
                    expires_at: now + FIELD_REF_TTL,
                },
            );
            regions[target.index].region_ref = Some(reference);
        }
        Ok(())
    }

    fn resolve_region_ref(
        &self,
        app_id: &str,
        executable: &std::path::Path,
        reference: &str,
    ) -> Result<RegionScope, DesktopAdapterError> {
        if reference.len() != 39
            || !reference.starts_with("region_")
            || !reference[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(DesktopAdapterError::new(
                "TARGET_CHANGED",
                "Region reference is invalid",
            ));
        }
        let mut refs = self.region_refs.lock().map_err(|_| {
            DesktopAdapterError::new(
                "ADAPTER_FAILED",
                "Desktop region reference store is unavailable",
            )
        })?;
        let now = std::time::Instant::now();
        refs.retain(|_, lease| lease.expires_at > now);
        let lease = refs.get(reference).ok_or_else(|| {
            DesktopAdapterError::new("TARGET_CHANGED", "Region reference expired or changed")
        })?;
        if lease.app_id != app_id || lease.executable != executable {
            return Err(DesktopAdapterError::new(
                "TARGET_CHANGED",
                "Region reference belongs to another trusted application",
            ));
        }
        Ok(lease.scope.clone())
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
            .env(
                "ANGELBOT_REGION_SCOPE",
                region_scope_environment(lease.scope.as_ref())?,
            )
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
        self.resolve_field_ref(app_id, &executable, field_ref)?;
        parse_text_target_payload(app_id, field_ref, &lease, &output.stdout)
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
            .env(
                "ANGELBOT_REGION_SCOPE",
                region_scope_environment(lease.scope.as_ref())?,
            )
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
        let mut preview = parse_control_target_payload(
            app_id,
            &lease.control_label,
            DesktopDraftTargetIdentity {
                control_operation: Some(operation),
                ..lease.identity
            },
            &output.stdout,
        )?;
        if let Some(scope) = &lease.scope {
            preview.control_label = format!("{} / {}", scope.label(), preview.control_label);
        }
        Ok(preview)
    }

    fn operate_control(
        &self,
        app_id: &str,
        executable_path: &str,
        operation: DesktopControlOperation,
        option: Option<&str>,
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
        validate_control_option(operation, option)?;
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
        let ending_script = if operation == DesktopControlOperation::Choose {
            CHOOSE_OPTION_SCRIPT
        } else {
            CONTROL_OPERATION_SCRIPT
        };
        let mut child =
            Self::control_target_command(&executable, &lease, operation, ending_script)?
                .env("ANGELBOT_CHOICE_OPTION", option.unwrap_or_default())
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
                choice: (operation == DesktopControlOperation::Choose).then(|| {
                    DesktopChoiceReceipt {
                        control_name: lease.control_label.clone(),
                        option: option.unwrap_or_default().into(),
                        expanded: false,
                    }
                }),
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
            choice: None,
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
        if text.contains('\0') || text.len() > 20_000 {
            return Err(DesktopAdapterError::new(
                "INVALID_ARGUMENT",
                "Text must contain at most 20,000 bytes and no NUL characters",
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
        let field_ref = expected_target.automation_id.as_deref().ok_or_else(|| {
            DesktopAdapterError::new(
                "TARGET_CHANGED",
                "Text target reference is missing or no longer valid",
            )
        })?;
        let executable = Self::canonical_executable(executable_path)?;
        let lease = self.resolve_field_ref(app_id, &executable, field_ref)?;
        let mut observed_identity = expected_target.clone();
        observed_identity.automation_id = None;
        if observed_identity != lease.identity {
            return Err(DesktopAdapterError::new(
                "TARGET_CHANGED",
                "Text target differs from its observed reference",
            ));
        }
        self.invalidate_application_refs(app_id, &executable)?;
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
            .env("ANGELBOT_FIELD_LABEL", &lease.control_label)
            .env(
                "ANGELBOT_REGION_SCOPE",
                region_scope_environment(lease.scope.as_ref())?,
            )
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
            choice: None,
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
            None,
            false,
            &|| false,
            DISCOVERY_TIMEOUT,
        )
    }

    fn observe_trusted_window_with_control(
        &self,
        app_id: &str,
        executable_path: &str,
        region_ref: Option<&str>,
        allow_action_labels: bool,
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
        let started = std::time::Instant::now();
        let executable = Self::canonical_executable(executable_path)?;
        let scope = region_ref
            .map(|reference| self.resolve_region_ref(app_id, &executable, reference))
            .transpose()?;
        let process_executable = Self::process_comparison_path(&executable);
        let script = control_observation_script(scope.is_some());
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
            .env(
                "ANGELBOT_ACTION_LABELS",
                if allow_action_labels { "1" } else { "0" },
            )
            .env(
                "ANGELBOT_REGION_SCOPE",
                region_scope_environment(scope.as_ref())?,
            )
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
            timeout.saturating_sub(started.elapsed()),
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
        let (mut payload, mut targets, region_targets) =
            parse_private_observation_payload(&output.stdout)?;
        if scope.is_none()
            && payload
                .controls
                .iter()
                .any(|control| control.role == "text")
        {
            return Err(DesktopAdapterError::new(
                "ADAPTER_FAILED",
                "Static text requires an explicitly selected region",
            ));
        }
        if let Some(scope) = &scope {
            if region_targets.iter().any(|target| {
                target.scope.path.len() <= scope.path.len()
                    || !target.scope.path.starts_with(&scope.path)
                    || target.scope.identity.process_id != scope.identity.process_id
                    || target.scope.identity.window_handle != scope.identity.window_handle
            }) {
                return Err(DesktopAdapterError::new(
                    "ADAPTER_FAILED",
                    "Nested region escaped its selected scope",
                ));
            }
        }
        fit_observation_projection(app_id, region_ref, &mut payload)?;
        targets.retain(|target| target.index < payload.controls.len());
        self.issue_field_refs(
            app_id,
            &executable,
            payload.truncated,
            &mut payload.controls,
            targets.clone(),
            scope.as_ref(),
        )?;
        self.issue_control_refs(
            app_id,
            &executable,
            payload.truncated,
            &mut payload.controls,
            &targets,
            scope.as_ref(),
        )?;
        self.issue_region_refs(app_id, &executable, &mut payload.regions, &region_targets)?;
        Ok(DesktopWindowObservation {
            app_id: app_id.to_string(),
            controls: payload.controls,
            regions: payload.regions,
            region_ref: region_ref.map(str::to_string),
            truncated: payload.truncated,
            diagnostics: payload.diagnostics,
        })
    }

    fn capture_trusted_window_with_control(
        &self,
        app_id: &str,
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
        #[cfg(feature = "desktop-e2e")]
        eprintln!("[DEBUG-capture] initial-probe-start");
        let target = capture_target_probe(
            &comparison,
            is_cancelled,
            timeout.saturating_sub(started.elapsed()),
        )?;
        // Pixel-to-screen mapping is optional metadata, not a prerequisite for
        // read-only observation. A missing mapping returns imageRef=null and
        // cannot enter the independent visual-click confirmation path.
        let bounds = image_click::window_bounds(&target).ok();
        #[cfg(feature = "desktop-e2e")]
        eprintln!(
            "[DEBUG-capture] helper-start elapsed_ms={}",
            started.elapsed().as_millis()
        );
        let mut image = crate::window_capture::capture_with_control(
            &target,
            is_cancelled,
            timeout.saturating_sub(started.elapsed()),
        )?;
        if is_cancelled() {
            return Err(crate::window_capture::error("SCAN_CANCELLED"));
        }
        // A reference identifies an observation, never input authority. The
        // separate click requires a live image preview and one-use approval.
        image.image_ref = match bounds {
            Some(bounds) if image_click::window_bounds(&target).ok() == Some(bounds) => self
                .image_refs
                .issue(app_id, target, bounds, image.png_bytes())?,
            _ => None,
        };
        Ok(image)
    }

    fn preflight_image_click(
        &self,
        request: &DesktopImageClickRequest,
        executable_path: &str,
        timeout: std::time::Duration,
    ) -> Result<DesktopImageClickPreview, DesktopAdapterError> {
        let _operation = self.begin_uia_operation()?;
        let executable = Self::canonical_executable(executable_path)?;
        let target = self
            .image_refs
            .resolve(request, &Self::process_comparison_path(&executable))?;
        let image = image_click::recheck(&target, &|| false, timeout)?;
        use base64::Engine;
        Ok(DesktopImageClickPreview {
            image: DesktopImageClickDisplay {
                image_data_url: format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(image.png_bytes())
                ),
                width: target.width(),
                height: target.height(),
                x: request.x,
                y: request.y,
            },
            target,
        })
    }

    fn click_image_with_control(
        &self,
        request: &DesktopImageClickRequest,
        executable_path: &str,
        target: &DesktopImageClickTarget,
        is_cancelled: &dyn Fn() -> bool,
        timeout: std::time::Duration,
    ) -> Result<DesktopActionResult, DesktopAdapterError> {
        let _operation = self.begin_uia_operation()?;
        let executable = Self::canonical_executable(executable_path)?;
        let current = self
            .image_refs
            .resolve(request, &Self::process_comparison_path(&executable))?;
        if !target.same_observation(&current) {
            return Err(crate::window_capture::error("TARGET_CHANGED"));
        }
        self.invalidate_application_refs(&request.app_id, &executable)?;
        image_click::click(target, is_cancelled, timeout)?;
        Ok(DesktopActionResult {
            adapter: self.name().into(), status: "dispatched".into(), target: request.app_id.clone(),
            detail: "One approved left click was dispatched; observe fresh state to verify the business result".into(),
            process_id: None, action: None, choice: None,
        })
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
                    choice: None,
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
                    choice: None,
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
                    choice: None,
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
                option,
                ..
            } => self.operate_control(
                app_id,
                executable_path,
                *operation,
                option.as_deref(),
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
                option,
                ..
            } => self.operate_control(
                app_id,
                executable_path,
                *operation,
                option.as_deref(),
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
        script: Option<&str>,
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
                            // Large synthetic providers must not hit Windows'
                            // command-line limit. Reuse the bounded child's
                            // stdin writer; no script file or user data is used.
                            if script.is_some() {
                                command.args(["-Mta", "-NoProfile", "-NonInteractive", "-Command",
                                    "[Console]::InputEncoding = New-Object Text.UTF8Encoding($false); $ErrorActionPreference = 'Stop'; . ([scriptblock]::Create([Console]::In.ReadToEnd()))"]);
                            }
                            command
                                .stdin(if script.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() })
                                .stdout(std::process::Stdio::piped())
                                .stderr(std::process::Stdio::piped());
                            crate::window_capture::run_bounded_child(
                                command,
                                script.map(|value| value.as_bytes().to_vec()),
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
    fn window_selection_uses_exact_hwnd_ownership_without_main_window_heuristics() {
        assert!(CAPTURE_WINDOW_SCRIPT.contains("EnumWindows("));
        assert!(CAPTURE_WINDOW_SCRIPT.contains("IsWindowVisible(hwnd)"));
        assert!(CAPTURE_WINDOW_SCRIPT.contains("GetWindow(hwnd, 4) == IntPtr.Zero"));
        assert!(CAPTURE_WINDOW_SCRIPT.contains("GetWindowThreadProcessId"));
        assert!(!CAPTURE_WINDOW_SCRIPT.contains("MainWindowHandle"));
        assert!(!CAPTURE_WINDOW_SCRIPT.contains("MainWindowTitle"));
        assert!(TRUSTED_WINDOW_SCRIPT.contains("EligibleControlWindows"));
        for script in [
            UIA_SAFETY_HELPERS_SCRIPT,
            TRUSTED_WINDOW_SCRIPT,
            TEXT_TARGET_SCRIPT,
            CONTROL_TARGET_SCRIPT,
            VERIFIED_SET_VALUE_SCRIPT,
        ] {
            assert!(!script.contains("MainWindowHandle"));
        }
        assert!(CAPTURE_WINDOW_SELECTION_SCRIPT.contains("$matches.Count -ne 1"));
        assert!(!CAPTURE_WINDOW_SELECTION_SCRIPT.contains("Select-Object -First"));
    }

    #[cfg(windows)]
    #[test]
    fn dense_region_projection_preserves_static_text_and_action_reference_budget() {
        let control = |value| serde_json::from_value::<DesktopObservedControl>(value).unwrap();
        let mut controls = vec![
            control(
                serde_json::json!({"depth":1,"role":"text","name":"Fixture region A","enabled":true,"capabilities":[]}),
            ),
            control(
                serde_json::json!({"depth":2,"role":"edit","automationId":"regionField","enabled":true,"capabilities":["setValue"]}),
            ),
            control(
                serde_json::json!({"depth":2,"role":"checkBox","name":"Region checkbox","automationId":"regionCheckbox","enabled":true,"capabilities":["toggle"],"state":{"toggle":"off"}}),
            ),
            control(
                serde_json::json!({"depth":3,"role":"text","name":"Region checkbox","enabled":true,"capabilities":[]}),
            ),
        ];
        for index in 1..=10 {
            controls.push(control(serde_json::json!({"depth":2,"role":"button","name":format!("Region button {index}"),"automationId":format!("regionButton{}",index-1),"enabled":true,"capabilities":["invoke"]})));
            controls.push(control(serde_json::json!({"depth":3,"role":"text","name":format!("Action {index}"),"enabled":true,"capabilities":[]})));
        }
        let original = controls.clone();
        let mut payload = ObservationPayload {
            controls,
            regions: vec![],
            truncated: false,
            diagnostics: vec![],
        };
        let region_ref = format!("region_{}", "0".repeat(32));
        fit_observation_projection("draft-app", Some(&region_ref), &mut payload).unwrap();
        assert!(
            !payload.truncated,
            "a real dense region lost action references: {:?}",
            payload.diagnostics
        );
        assert_eq!(
            payload.controls, original,
            "formatting must not discard captions or controls"
        );
        for control in &mut payload.controls {
            if control.role == "edit" {
                control.field_ref = Some(format!("field_{}", "0".repeat(32)));
            } else if !supported_control_operations(control).is_empty() {
                control.control_ref = Some(format!("control_{}", "0".repeat(32)));
            }
        }
        let public = DesktopWindowObservation {
            app_id: "draft-app".into(),
            controls: payload.controls,
            regions: vec![],
            region_ref: Some(region_ref),
            truncated: false,
            diagnostics: payload.diagnostics,
        };
        assert_eq!(
            public
                .controls
                .iter()
                .filter(|control| control.role == "text")
                .count(),
            12
        );
        let bytes = serde_json::to_vec(&public).unwrap();
        assert!(bytes.len() <= MAX_OBSERVATION_BYTES);
        assert!(!String::from_utf8_lossy(&bytes).contains("\"capabilities\":[]"));
        let restored: DesktopWindowObservation = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            restored.controls, public.controls,
            "compact metadata must round-trip without data loss"
        );
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
$normalRoot = [AngelBotControlWindowCandidate]::new(100, 42, 0, $true)
$auxiliary = [AngelBotControlWindowCandidate]::new(200, 42, 0x08000080, $true)
$secondRoot = [AngelBotControlWindowCandidate]::new(300, 42, 0, $true)
$otherInstanceAux = [AngelBotControlWindowCandidate]::new(400, 99, 0x08000080, $true)
$interactiveTool = [AngelBotControlWindowCandidate]::new(500, 42, 0x00000080, $true)
$explicitApp = [AngelBotControlWindowCandidate]::new(600, 42, 0x08040080, $true)
$unknownStyle = [AngelBotControlWindowCandidate]::new(700, 42, 0, $false)
@{
  sameProcess = Test-Selection @($first, $second, $other)
  unique = Test-Selection @($first, $other, $hidden, $owned)
  otherOnly = Test-Selection @($other, $hidden, $owned)
  rootWithAux = @([AngelBotCaptureWindows]::SelectControlRoots(@($normalRoot, $auxiliary)))
  reverseOrder = @([AngelBotCaptureWindows]::SelectControlRoots(@($auxiliary, $normalRoot)))
  twoRoots = @([AngelBotCaptureWindows]::SelectControlRoots(@($normalRoot, $auxiliary, $secondRoot)))
  otherInstance = @([AngelBotCaptureWindows]::SelectControlRoots(@($normalRoot, $otherInstanceAux)))
  interactiveTool = @([AngelBotCaptureWindows]::SelectControlRoots(@($normalRoot, $interactiveTool)))
  explicitApp = @([AngelBotCaptureWindows]::SelectControlRoots(@($normalRoot, $explicitApp)))
  unknownStyle = @([AngelBotCaptureWindows]::SelectControlRoots(@($unknownStyle, $auxiliary)))
  auxOnly = @([AngelBotCaptureWindows]::SelectControlRoots(@($auxiliary, $otherInstanceAux)))
} | ConvertTo-Json -Compress
"#;
        let identity = include_str!("desktop_control/window_identity.ps1");
        let script = format!("{identity}\n{CAPTURE_WINDOW_SELECTION_SCRIPT}\n{fixture}");
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
        assert_eq!(result["rootWithAux"], serde_json::json!([100]));
        assert_eq!(result["reverseOrder"], serde_json::json!([100]));
        assert_eq!(result["twoRoots"], serde_json::json!([100, 300]));
        assert_eq!(result["otherInstance"], serde_json::json!([100, 400]));
        assert_eq!(result["interactiveTool"], serde_json::json!([100, 500]));
        assert_eq!(result["explicitApp"], serde_json::json!([100, 600]));
        assert_eq!(result["unknownStyle"], serde_json::json!([700, 200]));
        assert_eq!(result["auxOnly"], serde_json::json!([200, 400]));
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
        assert!(payload.controls[1].state.is_none());
        assert!(payload.truncated);
        let text = parse_observation_payload(
            br#"{"controls":[{"depth":1,"role":"text","name":"141","enabled":true,"capabilities":[]}],"truncated":false}"#,
        )
        .unwrap();
        assert_eq!(text.controls[0].name.as_deref(), Some("141"));
        #[cfg(windows)]
        assert!(supported_control_operations(&text.controls[0]).is_empty());
        for (role, capability, states) in [
            (
                "checkBox",
                "toggle",
                vec![
                    serde_json::json!({"toggle":"off"}),
                    serde_json::json!({"toggle":"on"}),
                    serde_json::json!({"toggle":"indeterminate"}),
                ],
            ),
            (
                "radioButton",
                "select",
                vec![
                    serde_json::json!({"selected":false}),
                    serde_json::json!({"selected":true}),
                ],
            ),
            (
                "tabItem",
                "select",
                vec![serde_json::json!({"selected":true})],
            ),
            (
                "listItem",
                "select",
                vec![
                    serde_json::json!({"selected":false}),
                    serde_json::json!({"selected":false,"selectionContainerRole":"list"}),
                    serde_json::json!({"selected":true,"selectionContainerRole":"comboBox"}),
                ],
            ),
            (
                "menuItem",
                "expandCollapse",
                vec![
                    serde_json::json!({"expansion":"expanded"}),
                    serde_json::json!({"expansion":"collapsed"}),
                    serde_json::json!({"expansion":"partiallyExpanded"}),
                    serde_json::json!({"expansion":"leafNode"}),
                ],
            ),
            (
                "comboBox",
                "expandCollapse",
                vec![serde_json::json!({"expansion":"collapsed"})],
            ),
            (
                "pane",
                "scroll",
                vec![
                    serde_json::json!({"verticalScrollPercent":0.0}),
                    serde_json::json!({"verticalScrollPercent":50.125}),
                    serde_json::json!({"verticalScrollPercent":100.0}),
                ],
            ),
        ] {
            for state in states {
                let raw = serde_json::json!({"controls":[{"depth":1,"role":role,"enabled":true,"capabilities":[capability],"state":state}],"truncated":false});
                let parsed = parse_observation_payload(raw.to_string().as_bytes()).unwrap();
                assert_eq!(
                    serde_json::to_value(parsed).unwrap()["controls"][0]["state"],
                    state
                );
            }
        }
        let state_base = serde_json::json!({"controls":[{"depth":1,"role":"checkBox","enabled":true,"capabilities":["toggle"]}],"truncated":false});
        for state in [
            serde_json::json!({}),
            serde_json::json!({"toggle":"unknown"}),
            serde_json::json!({"toggle":true}),
            serde_json::json!({"toggle":"On"}),
            serde_json::json!({"selected":false}),
            serde_json::json!({"expansion":"expanded"}),
            serde_json::json!({"verticalScrollPercent":0}),
            serde_json::json!({"value":"private"}),
            serde_json::json!({"toggle":"on","selected":true}),
            serde_json::json!({"toggle":"off","selectionContainerRole":"list"}),
        ] {
            let mut invalid = state_base.clone();
            invalid["controls"][0]["state"] = state;
            assert!(parse_observation_payload(invalid.to_string().as_bytes()).is_err());
        }
        for value in [
            serde_json::json!(-1),
            serde_json::json!(100.1),
            serde_json::json!("50"),
            serde_json::json!(true),
        ] {
            let raw = serde_json::json!({"controls":[{"depth":1,"role":"pane","enabled":true,"capabilities":["scroll"],"state":{"verticalScrollPercent":value}}],"truncated":false});
            assert!(parse_observation_payload(raw.to_string().as_bytes()).is_err());
        }
        let mut disabled = state_base.clone();
        disabled["controls"][0]["enabled"] = serde_json::json!(false);
        disabled["controls"][0]["capabilities"] = serde_json::json!([]);
        disabled["controls"][0]["state"] = serde_json::json!({"toggle":"off"});
        assert!(parse_observation_payload(disabled.to_string().as_bytes()).is_err());
        for raw in [
            br#"{"controls":[{"depth":1,"role":"text","name":"141","enabled":true,"capabilities":["invoke"]}],"truncated":false}"#.as_slice(),
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
        let regions = serde_json::json!({"controls": [], "truncated": false,
            "regions": [{"role":"group","name":"Section"}]});
        let parsed = parse_observation_payload(regions.to_string().as_bytes()).unwrap();
        assert_eq!(parsed.regions[0].name, "Section");
        let unnamed = serde_json::json!({"controls": [], "truncated": false,
            "regions": [{"role":"pane","name":""}]});
        assert!(parse_observation_payload(unnamed.to_string().as_bytes()).is_ok());
        for invalid_regions in [
            serde_json::json!([{"role":"button","name":"Section"}]),
            serde_json::json!([{"role":"group","name":" "}]),
            serde_json::json!([{"role":"group","name":"Section","regionRef":"injected"}]),
            serde_json::json!((0..=MAX_OBSERVED_REGIONS)
                .map(|_| serde_json::json!({"role":"group","name":"Section"}))
                .collect::<Vec<_>>()),
        ] {
            let mut invalid = regions.clone();
            invalid["regions"] = invalid_regions;
            assert!(parse_observation_payload(invalid.to_string().as_bytes()).is_err());
        }
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
        assert_eq!(snapshot.controls.len(), 7);
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
        assert_eq!(snapshot.controls[5].role, "checkBox");
        assert_eq!(snapshot.controls[5].name.as_deref(), Some("Test checkbox"));
        assert_eq!(snapshot.controls[5].capabilities, vec!["toggle"]);
        assert_eq!(snapshot.controls[6].role, "comboBox");
        assert_eq!(snapshot.controls[6].capabilities, vec!["selectOption"]);
        assert_eq!(
            snapshot.controls[6].control_ref.as_deref(),
            Some("mock-choice-ref")
        );
        assert_eq!(
            snapshot.controls[5].control_ref.as_deref(),
            Some("mock-toggle-ref")
        );
        assert!(!snapshot.truncated);
        assert!(snapshot.diagnostics.is_empty());
        let public = serde_json::to_string(&snapshot).unwrap();
        assert!(public.contains("mock-field-ref"));
        assert!(!public.contains("processId"));
        assert!(!public.contains("runtimeId"));
        assert!(adapter.actions().is_empty());
        assert_eq!(
            snapshot.regions[0].region_ref.as_deref(),
            Some("mock-region-ref")
        );
        let scoped = adapter
            .observe_trusted_window_with_control(
                "trusted-chat",
                r"C:\trusted\chat.exe",
                Some("mock-region-ref"),
                false,
                &|| false,
                std::time::Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(scoped.region_ref.as_deref(), Some("mock-region-ref"));
        assert_eq!(scoped.controls.len(), 3);
        assert!(scoped.regions.is_empty());
        assert_eq!(
            adapter
                .observe_trusted_window_with_control(
                    "trusted-chat",
                    r"C:\trusted\chat.exe",
                    Some("unknown"),
                    false,
                    &|| false,
                    std::time::Duration::from_secs(1)
                )
                .unwrap_err()
                .code,
            "TARGET_CHANGED"
        );
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
        assert_eq!(
            preview.identity.automation_id.as_deref(),
            Some("mock-field-ref")
        );
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
            option: None,
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
    fn control_refs_bind_exact_observed_role_name_and_single_use_identity() {
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
            "targets": ([0, 1, 2, 6]).into_iter().map(|index| serde_json::json!({"index":index,"identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[index + 1]}})).collect::<Vec<_>>()
        });
        let (mut payload, targets, _) =
            parse_private_observation_payload(raw.to_string().as_bytes()).unwrap();
        let adapter = WindowsDesktopAdapter::default();
        let exe = std::path::Path::new(r"C:\trusted\app.exe");
        adapter
            .issue_field_refs(
                "app",
                exe,
                false,
                &mut payload.controls,
                targets.clone(),
                None,
            )
            .unwrap();
        adapter
            .issue_control_refs("app", exe, false, &mut payload.controls, &targets, None)
            .unwrap();
        assert!(payload
            .controls
            .iter()
            .all(|control| control.field_ref.is_none()));
        for (index, expected_role, operation) in [
            (0, "button", DesktopControlOperation::Invoke),
            (1, "hyperlink", DesktopControlOperation::Invoke),
            (2, "menuItem", DesktopControlOperation::Invoke),
            (6, "checkBox", DesktopControlOperation::Check),
        ] {
            let reference = payload.controls[index].control_ref.as_deref().unwrap();
            assert_eq!(reference.len(), 40);
            let lease = adapter
                .resolve_control_ref("app", exe, reference, None)
                .unwrap();
            assert_eq!(lease.role, expected_role);
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
            changed.control_operation = Some(operation);
            changed.window_handle += 1;
            assert_eq!(
                adapter
                    .resolve_control_ref("app", exe, reference, Some(&changed))
                    .unwrap_err()
                    .code,
                "TARGET_CHANGED"
            );
            let attested = DesktopDraftTargetIdentity {
                control_operation: Some(operation),
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
        assert!(payload.controls[3..6]
            .iter()
            .all(|control| control.control_ref.is_none()));
        for index in 0..7 {
            let mut invalid = raw.clone();
            invalid["targets"] = serde_json::json!([{"index":index,"identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[1]}}]);
            if (3..6).contains(&index) {
                assert!(parse_private_observation_payload(invalid.to_string().as_bytes()).is_err());
            } else {
                assert!(parse_private_observation_payload(invalid.to_string().as_bytes()).is_ok());
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
            let (mut payload, targets, _) =
                parse_private_observation_payload(raw.to_string().as_bytes()).unwrap();
            adapter
                .issue_control_refs(
                    "app",
                    exe,
                    payload.truncated,
                    &mut payload.controls,
                    &targets,
                    None,
                )
                .unwrap();
            assert!(payload.controls[0].control_ref.is_none(), "{reason}");
        }
        assert!(adapter.control_refs.lock().unwrap().is_empty());
        let raw = br#"{"controls":[{"depth":1,"role":"button","name":"Apply","enabled":true,"capabilities":["invoke"]},{"depth":1,"role":"menuItem","name":"Apply","enabled":true,"capabilities":["invoke"]}],"truncated":false,"targets":[{"index":0,"identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[1]}},{"index":1,"identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[1]}}]}"#;
        let (mut payload, mut targets, _) = parse_private_observation_payload(raw).unwrap();
        adapter
            .issue_control_refs("app", exe, false, &mut payload.controls, &targets, None)
            .unwrap();
        assert!(payload
            .controls
            .iter()
            .all(|control| control.control_ref.is_none()));
        targets.pop();
        adapter
            .issue_control_refs("app", exe, false, &mut payload.controls, &targets, None)
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
        let (mut payload, targets, _) = parse_private_observation_payload(raw).unwrap();
        adapter
            .issue_field_refs(
                "app",
                exe,
                false,
                &mut payload.controls,
                targets.clone(),
                None,
            )
            .unwrap();
        adapter
            .issue_control_refs("app", exe, false, &mut payload.controls, &targets, None)
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
                .observe_trusted_window_with_control(
                    "app",
                    "fixture.exe",
                    None,
                    false,
                    &|| false,
                    timeout,
                )
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
                option: None,
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
                "checkBox",
                vec!["toggle"],
                vec![
                    DesktopControlOperation::Check,
                    DesktopControlOperation::Uncheck,
                ],
            ),
            ("checkBox", vec![], vec![]),
            ("menuItem", vec!["toggle"], vec![]),
            (
                "menuItem",
                vec!["invoke", "expandCollapse"],
                vec![
                    DesktopControlOperation::Invoke,
                    DesktopControlOperation::Expand,
                    DesktopControlOperation::Collapse,
                ],
            ),
            (
                "listItem",
                vec!["select"],
                vec![DesktopControlOperation::Select],
            ),
            (
                "comboBox",
                vec!["expandCollapse"],
                vec![
                    DesktopControlOperation::Expand,
                    DesktopControlOperation::Collapse,
                ],
            ),
            (
                "comboBox",
                vec!["expandCollapse", "selectOption"],
                vec![
                    DesktopControlOperation::Choose,
                    DesktopControlOperation::Expand,
                    DesktopControlOperation::Collapse,
                ],
            ),
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
                state: None,
                field_ref: None,
                control_ref: None,
            };
            assert_eq!(supported_control_operations(&control), expected, "{role}");
            for unavailable in [
                DesktopObservedControl {
                    enabled: false,
                    ..control.clone()
                },
                DesktopObservedControl {
                    name: None,
                    ..control.clone()
                },
                DesktopObservedControl {
                    name: Some(" ".into()),
                    ..control.clone()
                },
            ] {
                assert!(supported_control_operations(&unavailable).is_empty());
            }
        }
        let raw = br#"{"controls":[{"depth":1,"role":"edit","automationId":"field","enabled":true,"capabilities":["setValue"]},{"depth":1,"role":"menuItem","name":"Menu","enabled":true,"capabilities":["invoke","expandCollapse"]}],"truncated":false,"targets":[{"index":0,"identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[1]}},{"index":1,"identity":{"processId":42,"windowHandle":1234,"controlRuntimeId":[2]}}]}"#;
        let (mut payload, targets, _) = parse_private_observation_payload(raw).unwrap();
        let adapter = WindowsDesktopAdapter::default();
        let exe = std::path::Path::new("fixture.exe");
        adapter
            .issue_field_refs(
                "app",
                exe,
                false,
                &mut payload.controls,
                targets.clone(),
                None,
            )
            .unwrap();
        adapter
            .issue_control_refs("app", exe, false, &mut payload.controls, &targets, None)
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

        let scope = RegionScope {
            identity: DesktopDraftTargetIdentity {
                process_id: 42,
                window_handle: 1234,
                control_runtime_id: vec![9],
                automation_id: None,
                control_operation: None,
            },
            role: "group".into(),
            name: "Region".into(),
            path: vec![2],
        };
        let mut scoped_raw: serde_json::Value = serde_json::from_slice(raw).unwrap();
        scoped_raw["regions"] = serde_json::json!([{"role":"group","name":"Region"}]);
        scoped_raw["region_targets"] = serde_json::json!([{"index":0,"scope":scope}]);
        let (mut scoped, targets, region_targets) =
            parse_private_observation_payload(scoped_raw.to_string().as_bytes()).unwrap();
        adapter
            .issue_region_refs("app", exe, &mut scoped.regions, &region_targets)
            .unwrap();
        let region_ref = scoped.regions[0].region_ref.clone().unwrap();
        assert_eq!(
            adapter.resolve_region_ref("app", exe, &region_ref).unwrap(),
            scope
        );
        assert_eq!(
            adapter
                .resolve_region_ref("other", exe, &region_ref)
                .unwrap_err()
                .code,
            "TARGET_CHANGED"
        );
        assert_eq!(
            adapter
                .resolve_region_ref("app", std::path::Path::new("other.exe"), &region_ref)
                .unwrap_err()
                .code,
            "TARGET_CHANGED"
        );
        adapter
            .issue_field_refs(
                "app",
                exe,
                false,
                &mut scoped.controls,
                targets.clone(),
                Some(&scope),
            )
            .unwrap();
        adapter
            .issue_control_refs(
                "app",
                exe,
                false,
                &mut scoped.controls,
                &targets,
                Some(&scope),
            )
            .unwrap();
        let field_ref = scoped.controls[0].field_ref.clone().unwrap();
        let control_ref = scoped.controls[1].control_ref.clone().unwrap();
        let field = adapter.resolve_field_ref("app", exe, &field_ref).unwrap();
        assert_eq!(field.scope.as_ref(), Some(&scope));
        assert_eq!(
            adapter
                .resolve_control_ref("app", exe, &control_ref, None)
                .unwrap()
                .scope
                .as_ref(),
            Some(&scope)
        );
        let preview_raw =
            serde_json::json!({"windowTitle":"Fixture", "controlLabel":field.control_label});
        let preview = parse_text_target_payload(
            "app",
            &field_ref,
            &field,
            preview_raw.to_string().as_bytes(),
        )
        .unwrap();
        assert_eq!(
            preview.control_label,
            format!("Region / {}", field.control_label)
        );
        assert_eq!(
            preview.identity.automation_id.as_deref(),
            Some(field_ref.as_str())
        );
        adapter
            .region_refs
            .lock()
            .unwrap()
            .get_mut(&region_ref)
            .unwrap()
            .expires_at = std::time::Instant::now();
        assert_eq!(
            adapter
                .resolve_region_ref("app", exe, &region_ref)
                .unwrap_err()
                .code,
            "TARGET_CHANGED"
        );
        assert_eq!(
            adapter
                .resolve_field_ref("app", exe, &field_ref)
                .unwrap()
                .scope
                .as_ref(),
            Some(&scope),
            "action leases own their scope, not a live region lookup"
        );
        adapter.invalidate_application_refs("app", exe).unwrap();
        assert!(adapter.region_refs.lock().unwrap().is_empty());
        assert!(adapter.resolve_field_ref("app", exe, &field_ref).is_err());
        assert!(adapter
            .resolve_control_ref("app", exe, &control_ref, None)
            .is_err());
        for path in [vec![], vec![0; 13], vec![256]] {
            let mut invalid = scoped_raw.clone();
            invalid["region_targets"][0]["scope"]["path"] = serde_json::json!(path);
            assert!(parse_private_observation_payload(invalid.to_string().as_bytes()).is_err());
        }
        let mut budget = ObservationPayload {
            controls: vec![scoped.controls[0].clone(); 32],
            regions: (0..MAX_OBSERVED_REGIONS)
                .map(|_| DesktopObservedRegion {
                    role: "group".into(),
                    name: "区".repeat(80),
                    region_ref: None,
                })
                .collect(),
            truncated: false,
            diagnostics: vec![],
        };
        for control in &mut budget.controls {
            control.field_ref = None;
            control.control_ref = None;
        }
        fit_observation_projection(
            "app",
            Some("region_00000000000000000000000000000000"),
            &mut budget,
        )
        .unwrap();
        assert!(budget.truncated);
        assert!(budget
            .diagnostics
            .contains(&DesktopObservationDiagnostic::OutputLimit));
        assert_eq!(
            budget.regions.len(),
            MAX_OBSERVED_REGIONS,
            "readonly region hints survive control output exhaustion"
        );
        assert!(budget
            .controls
            .iter()
            .all(|control| control.field_ref.is_none() && control.control_ref.is_none()));
        let public = serde_json::to_value(DesktopWindowObservation {
            app_id: "app".into(),
            controls: budget.controls,
            regions: budget.regions,
            region_ref: Some("region_00000000000000000000000000000000".into()),
            truncated: budget.truncated,
            diagnostics: budget.diagnostics,
        })
        .unwrap();
        assert!(serde_json::to_vec(&public).unwrap().len() <= MAX_OBSERVATION_BYTES);
        assert!(!public.to_string().contains("processId"));
    }

    #[test]
    fn mock_control_operations_bind_action_and_report_only_narrow_control_state() {
        let adapter = MockDesktopAdapter::default();
        let timeout = std::time::Duration::from_secs(1);
        for (operation, reference) in [
            (DesktopControlOperation::Select, "mock-select-ref"),
            (DesktopControlOperation::Check, "mock-toggle-ref"),
            (DesktopControlOperation::Uncheck, "mock-toggle-ref"),
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
            let wrong_operation = match operation {
                DesktopControlOperation::Expand => DesktopControlOperation::Collapse,
                DesktopControlOperation::Check => DesktopControlOperation::Uncheck,
                DesktopControlOperation::Uncheck => DesktopControlOperation::Check,
                _ => DesktopControlOperation::Expand,
            };
            let mut action = DesktopAction::OperateControl {
                app_id: "app".into(),
                display_name: "App".into(),
                executable_path: "app.exe".into(),
                operation: wrong_operation,
                option: None,
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
            let observation = adapter.observe_trusted_window("app", "app.exe").unwrap();
            let state = observation
                .controls
                .iter()
                .find(|control| control.control_ref.as_deref() == Some(reference))
                .unwrap()
                .state
                .as_ref()
                .unwrap();
            match operation {
                DesktopControlOperation::Select => assert_eq!(state.selected, Some(true)),
                DesktopControlOperation::Check => {
                    assert_eq!(state.toggle, Some(DesktopToggleState::On))
                }
                DesktopControlOperation::Uncheck => {
                    assert_eq!(state.toggle, Some(DesktopToggleState::Off))
                }
                DesktopControlOperation::Expand => {
                    assert_eq!(state.expansion, Some(DesktopExpansionState::Expanded))
                }
                DesktopControlOperation::Collapse => {
                    assert_eq!(state.expansion, Some(DesktopExpansionState::Collapsed))
                }
                DesktopControlOperation::ScrollUp => {
                    assert_eq!(state.vertical_scroll_percent, Some(40.0))
                }
                DesktopControlOperation::ScrollDown => {
                    assert_eq!(state.vertical_scroll_percent, Some(50.0))
                }
                DesktopControlOperation::Invoke | DesktopControlOperation::Choose => unreachable!(),
            }
            let other = adapter.observe_trusted_window("other", "app.exe").unwrap();
            assert_eq!(
                other.controls[5].state.as_ref().unwrap().toggle,
                Some(DesktopToggleState::Off)
            );
            assert_eq!(
                other.controls[2].state.as_ref().unwrap().selected,
                Some(false)
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
            DesktopControlOperation::Check,
            DesktopControlOperation::Uncheck,
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
        let (mut public, targets, _) = parse_private_observation_payload(raw).unwrap();
        let adapter = WindowsDesktopAdapter::default();
        let exe = std::path::Path::new(r"C:\trusted\app.exe");
        let (mut partial, partial_targets, _) = parse_private_observation_payload(raw).unwrap();
        partial.truncated = true;
        adapter
            .issue_field_refs(
                "app",
                exe,
                partial.truncated,
                &mut partial.controls,
                partial_targets,
                None,
            )
            .unwrap();
        assert!(partial
            .controls
            .iter()
            .all(|control| control.field_ref.is_none()));
        adapter
            .issue_field_refs("app", exe, false, &mut public.controls, targets, None)
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
            let (mut partial, targets, _) =
                parse_private_observation_payload(raw.to_string().as_bytes()).unwrap();
            assert_eq!(partial.diagnostics.len(), 1);
            adapter
                .issue_field_refs(
                    "app",
                    exe,
                    partial.truncated,
                    &mut partial.controls,
                    targets,
                    None,
                )
                .unwrap();
            assert!(partial.controls[0].field_ref.is_none(), "reason: {reason}");
        }
        assert!(adapter.field_refs.lock().unwrap().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn observation_script_uses_bounded_read_only_traversal() {
        assert!(OBSERVATION_SCRIPT.contains("Use-UiaNodeBudget"));
        assert!(UIA_SAFETY_HELPERS_SCRIPT.contains("$script:uiaNodeCount -gt 256"));
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

        let root_readiness = "$script:uiaWindowReadinessWaitMs = 1500";
        assert!(control_observation_script(false).contains(root_readiness));
        assert!(!control_observation_script(true).contains(root_readiness));
        for strict_script in [
            TRUSTED_WINDOW_SCRIPT,
            DRAFT_TARGET_SCRIPT,
            TEXT_TARGET_SCRIPT,
            CONTROL_TARGET_SCRIPT,
            CONTROL_OPERATION_SCRIPT,
            VERIFIED_SET_VALUE_SCRIPT,
            CAPTURE_WINDOW_SCRIPT,
            CAPTURE_TARGET_SCRIPT,
        ] {
            assert!(!strict_script.contains(root_readiness));
        }

        // Fake only clock/process enumeration and the owned provider. Selection
        // and readiness use the same production script, without real windows.
        let setup = r#"
$fixtureRoot = $root
$script:readinessClock = [pscustomobject]@{ ElapsedMilliseconds = 0 }
$script:processReads = 0
$script:sleeps = 0
$mode = $env:ANGELBOT_TEST_READINESS
function Start-Sleep {
  param([int]$Milliseconds)
  $script:sleeps++
  $script:readinessClock.ElapsedMilliseconds += $Milliseconds
}
function Get-UiaControlWindows {
  param([string]$expected)
  $script:processReads++
  if ($mode -eq 'provider') { throw 'TARGET_UNAVAILABLE|private provider details' }
  if ($mode -eq 'missing' -or ($mode -eq 'delayed' -and $script:processReads -le 2) -or
      ($mode -in @('ambiguous', 'unsafe') -and $script:processReads -eq 1)) { return }
  [IntPtr]::new(1234)
  if ($mode -eq 'ambiguous') {
    [IntPtr]::new(5678)
  }
}
function Get-Process { param([int]$Id); return [pscustomobject]@{ Path = $env:ANGELBOT_TARGET_EXE; MainWindowHandle = [IntPtr]::Zero } }
if ($mode -eq 'unsafe') { $root.Failure = 'password' }
"#;
        let cases = [
            ("immediate", false, "ready", 1, 0, 0),
            ("delayed", false, "ready", 3, 2, 200),
            ("ambiguous", false, "TARGET_AMBIGUOUS", 2, 1, 100),
            ("missing", false, "TARGET_NOT_FOUND", 16, 15, 1500),
            ("delayed", true, "TARGET_NOT_FOUND", 1, 0, 0),
            ("unsafe", false, "TARGET_UNAVAILABLE", 2, 1, 100),
            ("provider", false, "TARGET_UNAVAILABLE", 1, 0, 0),
        ];
        let commands = cases
            .iter()
            .map(|(mode, scoped, ..)| {
                let observation = control_observation_script(*scoped)
                    .replace(include_str!("desktop_control/window_identity.ps1"), "")
                    .replace("[Diagnostics.Stopwatch]::StartNew()", "$script:readinessClock")
                    .replace(
                        "[Windows.Automation.AutomationElement]::FromHandle($windows[0])",
                        "$fixtureRoot",
                    )
                    .replace(OBSERVATION_SCRIPT, "$null = Get-UiaSafetyState $root");
                let script = format!(
                    "{OBSERVATION_TEST_PROVIDER_FIXTURE}\n{setup}\n$code = 'ready'; try {{\n{observation}\n}} catch {{ $code = $_.Exception.Message.Split('|')[0] }}\n@{{ code = $code; processReads = $script:processReads; sleeps = $script:sleeps; elapsed = $script:readinessClock.ElapsedMilliseconds; mutationCount = $firstControl.MutationCount; invoked = $firstControl.Invoked; written = $firstControl.Written }} | ConvertTo-Json -Compress"
                );
                let mut command =
                    desktop_child_command(WindowsDesktopAdapter::system_powershell().unwrap());
                command
                    .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                    .env("ANGELBOT_TARGET_EXE", "C:\\fixture\\trusted.exe")
                    .env("ANGELBOT_TEST_FAILURE", "none")
                    .env("ANGELBOT_TEST_READINESS", mode);
                command
            })
            .collect();
        for ((mode, scoped, code, reads, sleeps, elapsed), output) in
            cases.into_iter().zip(run_fixture_commands(commands, None))
        {
            assert!(
                output.status.success(),
                "readiness {mode}/{scoped}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stderr.is_empty(), "readiness {mode}/{scoped}");
            let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(result["code"], code, "readiness {mode}/{scoped}");
            assert_eq!(result["processReads"], reads, "readiness {mode}/{scoped}");
            assert_eq!(result["sleeps"], sleeps, "readiness {mode}/{scoped}");
            assert_eq!(result["elapsed"], elapsed, "readiness {mode}/{scoped}");
            assert_eq!(result["mutationCount"], 0, "readiness {mode}/{scoped}");
            assert_eq!(result["invoked"], false, "readiness {mode}/{scoped}");
            assert_eq!(result["written"], false, "readiness {mode}/{scoped}");
            assert!(!String::from_utf8_lossy(&output.stdout).contains("private provider details"));
        }
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
  public void SetValue(string value) {
    element.MutationCount++;
    element.Written = true; element.Value = value;
    if (element.Failure == "postFieldIdentity") { element.RuntimeId++; }
    if (element.Failure == "postFieldLabel") { element.Identifier = "changed"; }
    if (element.Failure == "postFieldScopeName") { element.Parent.Identifier = "Changed region"; }
    if (element.Failure == "postFieldScopeMove") { element.Parent = element.Parent.Parent; }
  }
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
  public ObservationTestElement SelectionContainer { get { return element.Parent; } }
  public object CanSelectMultiple {
    get {
      if (element.Failure == "selectionModeUnavailable") { throw new System.Exception("private provider details"); }
      if (element.Failure == "selectionModeNull") { return null; }
      return element.Failure == "multipleSelection";
    }
  }
  public object IsSelected {
    get {
      if (element.Failure == "stateUnavailable") { throw new System.Exception("private provider details"); }
      if (element.Failure == "stateNull") { return null; }
      if (element.Failure == "invalidState" || element.Failure == "initialInvalidState" ||
          (element.Failure == "postSelectedInvalid" && element.Applied)) { return "false"; }
      if (element.Failure == "delayedState" && element.Applied && ++element.StateReadCount <= 2) { return element.PreviousSelected; }
      return element.Failure == "wrongState" && element.Applied ? false : element.Selected;
    }
  }
  public string ExpandCollapseState {
    get {
      if (element.Failure == "stateUnavailable") { throw new System.Exception("private provider details"); }
      if (element.Failure == "stateNull") { return null; }
      if (element.Failure == "invalidState") { return "Unknown"; }
      if (element.Failure == "leaf") { return "LeafNode"; }
      if (element.Failure == "wrongState" && element.Applied) { return "PartiallyExpanded"; }
      if (element.Failure == "delayedState" && element.Applied && ++element.StateReadCount <= 2) { return element.PreviousExpandState; }
      return element.ExpandState;
    }
  }
  public string ToggleState {
    get {
      if (element.Failure == "stateUnavailable") { throw new System.Exception("private provider details"); }
      if (element.Failure == "stateNull" || element.Failure == "initialStateNull") { return null; }
      if (element.Failure == "invalidState" || element.Failure == "initialInvalidState") { return "Unknown"; }
      if (element.Failure == "indeterminate" || element.Failure == "initialIndeterminate" || (element.Failure == "postIndeterminate" && element.Applied)) { return "Indeterminate"; }
      if (element.Applied && (element.Failure == "wrongState" || (element.Failure == "delayedState" && ++element.StateReadCount <= 2))) { return element.PreviousToggleState; }
      return element.ToggleState;
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
      // A control can publish a transient unavailable state before its stale
      // value and then the requested value. Keep the existing delayed-readback
      // scenarios exercising both kinds of lag, without adding fixture runs.
      if (element.Failure == "delayedState" && element.Applied && element.StateReadCount == 0) {
        element.StateReadCount++;
        throw new System.Exception("private provider details");
      }
      return new ObservationTestControlPatternInfo(element);
    }
  }
  private void Applied() {
    element.MutationCount++;
    element.PreviousSelected = element.Selected;
    element.PreviousExpandState = element.ExpandState;
    element.PreviousToggleState = element.ToggleState;
    element.PreviousScrollPosition = element.ScrollPosition;
    element.Applied = true;
    if (element.Failure == "postScopeName") { element.Parent.Identifier = "Changed region"; }
    if (element.Failure == "postScopeMoved") { element.Parent = element.Parent.Parent; }
    if (element.Failure == "mutationThrows") { throw new System.Exception("private provider details"); }
  }
  public void Select() { Applied(); element.Selected = true; }
  public void Expand() { Applied(); element.ExpandState = "Expanded"; }
  public void Collapse() { Applied(); element.ExpandState = "Collapsed"; }
  public void Toggle() { Applied(); element.ToggleState = element.ToggleState == "On" ? "Off" : "On"; }
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
  public string PreviousToggleState;
  public int StateReadCount;
  public int MutationCount;
  public double ScrollPosition = 50;
  public double PreviousScrollPosition;
  public string ControlOperation;
  public string RoleOverride;
  public string ExpandState = "Collapsed";
  public string ToggleState = "Off";
  public string Value;
  public int RuntimeId = 1;
  public ObservationTestElement Parent;
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
    if (Failure == "postRuntimeChanged" && Applied) { return new int[] { 2 }; }
    return new int[] { RuntimeId };
  }
}
public class ObservationTestWalker {
  public ObservationTestElement GetParent(ObservationTestElement element) { return element.Parent; }
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
function Get-UiaWindowProcessId([IntPtr]$handle) {
  if ($handle.ToInt64() -ne 1234) { return 0 }
  if ($firstControl.Failure -eq 'windowOwnerChanged' -or
      ($failure -eq 'postWindowOwnerChanged' -and $firstControl.Written)) { return 99 }
  return 42
}
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
                command.env("ANGELBOT_TEST_FAILURE", failure);
                command
            })
            .collect();
        for ((failure, expected_controls), output) in cases
            .into_iter()
            .zip(run_fixture_commands(commands, Some(&script)))
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
            let (mut payload, targets, _) = parse_private_observation_payload(&output.stdout)
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
                    None,
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
    fn static_text_observation_requires_a_safe_selected_region_and_never_issues_actions() {
        let observation = OBSERVATION_SCRIPT.replace(
            "$walker = [Windows.Automation.TreeWalker]::ControlViewWalker",
            "$walker = New-Object ObservationTestWalker",
        );
        let setup = r#"
$firstControl.RoleOverride = 'ControlType.Text'
$firstControl.Identifier = '141'
$firstControl.NextSibling = $null
$root.RuntimeId = 99
$region = New-Object ObservationTestElement
$region.Identifier = 'Display'
$region.RoleOverride = 'ControlType.Group'
$region.RuntimeId = 10
$region.Parent = $root
$region.FirstChild = $firstControl
$firstControl.Parent = $region
$root.FirstChild = $region
if ($failure -eq 'longName') { $firstControl.Identifier = 'x' * 81 }
if ($failure -eq 'controlName') { $firstControl.Identifier = "private`ntext" }
if ($failure -in @('editAncestor', 'choiceAncestor', 'ancestorRoleUnavailable')) {
  $edit = New-Object ObservationTestElement
  $edit.Identifier = 'ordinaryField'
  $edit.RoleOverride = 'ControlType.Edit'
  if ($failure -eq 'choiceAncestor') { $edit.RoleOverride = 'ControlType.ComboBox' }
  if ($failure -eq 'ancestorRoleUnavailable') { $edit.Failure = 'role' }
  $edit.Parent = $region
  $edit.FirstChild = $firstControl
  $firstControl.Parent = $edit
  $region.FirstChild = $edit
}
if ($failure -eq 'outsideRegion') { $firstControl.Parent = $root }
if ($failure -in @('unnamedRegion', 'unnamedDiscovery')) { $region.Identifier = '' }
if ($failure -notin @('unscoped', 'unnamedDiscovery')) {
  $script:uiaScope = @{ role = 'group'; name = 'Display'; path = @(0); identity = @{ processId = 42; windowHandle = 1234; controlRuntimeId = @(10) } }
  $script:uiaScope.name = $region.Identifier
}
$expected = 'C:\trusted\fixture.exe'
function Get-Process { param([int]$Id) return [pscustomobject]@{ Path = 'C:\trusted\fixture.exe' } }
"#;
        let script = format!("{OBSERVATION_TEST_PROVIDER_FIXTURE}\n{UIA_SAFETY_HELPERS_SCRIPT}\n{setup}\n{observation}");
        let cases = [
            "none",
            "unscoped",
            "unnamedRegion",
            "unnamedDiscovery",
            "hidden",
            "sensitive",
            "differentProcess",
            "editAncestor",
            "choiceAncestor",
            "ancestorRoleUnavailable",
            "outsideRegion",
            "longName",
            "controlName",
        ];
        let commands = cases
            .iter()
            .map(|failure| {
                let mut command =
                    desktop_child_command(WindowsDesktopAdapter::system_powershell().unwrap());
                command
                    .env("ANGELBOT_TEST_FAILURE", failure)
                    .env("ANGELBOT_ACTION_LABELS", "0");
                command
            })
            .collect();
        for (failure, output) in cases
            .into_iter()
            .zip(run_fixture_commands(commands, Some(&script)))
        {
            assert!(
                output.status.success(),
                "{failure}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let (payload, targets, _) = parse_private_observation_payload(&output.stdout).unwrap();
            let text: Vec<_> = payload
                .controls
                .iter()
                .filter(|c| c.role == "text")
                .collect();
            assert_eq!(
                text.len(),
                usize::from(matches!(failure, "none" | "unnamedRegion")),
                "{failure}"
            );
            if failure == "unnamedDiscovery" {
                assert_eq!(payload.regions.len(), 1);
                assert_eq!(payload.regions[0].name, "");
                assert!(payload.controls.is_empty());
            }
            for control in text {
                assert_eq!(control.name.as_deref(), Some("141"));
                assert!(control.capabilities.is_empty());
                assert!(control.state.is_none());
                assert!(control.field_ref.is_none() && control.control_ref.is_none());
                assert!(supported_control_operations(control).is_empty());
            }
            assert!(targets.is_empty(), "{failure}");
            assert_eq!(
                payload.truncated,
                matches!(
                    failure,
                    "differentProcess"
                        | "outsideRegion"
                        | "ancestorRoleUnavailable"
                        | "longName"
                        | "controlName"
                ),
                "{failure}"
            );
            assert!(!String::from_utf8_lossy(&output.stdout).contains("private provider details"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn control_observation_reads_bounded_states_without_mutating_or_reading_values() {
        let observation = OBSERVATION_SCRIPT.replace(
            "$walker = [Windows.Automation.TreeWalker]::ControlViewWalker",
            "$walker = New-Object ObservationTestWalker",
        );
        let setup = r#"
$firstControl.Invokable = $true
$firstControl.ControlOperation = $env:ANGELBOT_TEST_OPERATION
$firstControl.RoleOverride = $env:ANGELBOT_TEST_ROLE
$firstControl.NextSibling = $null
$firstControl.Selected = $failure -eq 'selected'
if ($failure -eq 'on') { $firstControl.ToggleState = 'On' }
if ($failure -eq 'expanded') { $firstControl.ExpandState = 'Expanded' }
if ($failure -eq 'partial') { $firstControl.ExpandState = 'PartiallyExpanded' }
if ($env:ANGELBOT_TEST_ROLE -ceq 'ControlType.ListItem' -and $failure -eq 'labelsAllowed') {
  $root.RoleOverride = 'ControlType.List'
  $firstControl.Parent = $root
  $windowProcessId = $root.Current.ProcessId
}
"#;
        let script = format!("{OBSERVATION_TEST_PROVIDER_FIXTURE}\n{UIA_SAFETY_HELPERS_SCRIPT}\n{setup}\n{observation}\nif ($firstControl.MutationCount -ne 0 -or $firstControl.Written -or $firstControl.Invoked) {{ throw 'Read-only observation mutated a control' }}");
        let cases = [
            ("Pane", "scrolldown", "none", false),
            ("Document", "scrolldown", "none", false),
            ("List", "scrolldown", "none", false),
            ("Pane", "scrolldown", "nonScrollable", false),
            ("Document", "scrolldown", "missingPattern", false),
            ("List", "scrolldown", "invalidPosition", true),
            ("Pane", "scrolldown", "emptyName", false),
            ("Pane", "scrolldown", "disabled", false),
            ("CheckBox", "check", "none", false),
            ("CheckBox", "check", "on", false),
            ("CheckBox", "check", "indeterminate", false),
            ("CheckBox", "check", "stateUnavailable", true),
            ("CheckBox", "check", "stateNull", true),
            ("CheckBox", "check", "invalidState", true),
            ("CheckBox", "check", "hidden", false),
            ("CheckBox", "check", "sensitive", false),
            ("RadioButton", "select", "none", false),
            ("RadioButton", "select", "selected", false),
            ("RadioButton", "select", "stateNull", true),
            ("RadioButton", "select", "invalidState", true),
            ("TabItem", "select", "selected", false),
            ("ListItem", "select", "none", false),
            ("ListItem", "select", "labelsAllowed", false),
            ("MenuItem", "expand", "none", false),
            ("MenuItem", "expand", "expanded", false),
            ("MenuItem", "expand", "partial", false),
            ("MenuItem", "expand", "leaf", false),
            ("MenuItem", "expand", "stateUnavailable", true),
            ("ComboBox", "expand", "none", false),
            ("ComboBox", "expand", "labelsAllowed", false),
            ("ComboBox", "expand", "multipleSelection", false),
            ("ComboBox", "expand", "selectionModeNull", true),
            ("ComboBox", "expand", "selectionModeUnavailable", true),
            ("ComboBox", "expand", "stateNull", true),
            ("ComboBox", "expand", "invalidState", true),
        ];
        let commands = cases
            .iter()
            .map(|(role, operation, failure, _)| {
                let mut command =
                    desktop_child_command(WindowsDesktopAdapter::system_powershell().unwrap());
                command
                    .env("ANGELBOT_TEST_FAILURE", failure)
                    .env(
                        "ANGELBOT_ACTION_LABELS",
                        if *failure == "labelsAllowed" {
                            "1"
                        } else {
                            "0"
                        },
                    )
                    .env("ANGELBOT_TEST_OPERATION", operation)
                    .env("ANGELBOT_TEST_ROLE", format!("ControlType.{role}"));
                command
            })
            .collect();
        for ((role, operation, failure, partial), output) in cases
            .into_iter()
            .zip(run_fixture_commands(commands, Some(&script)))
        {
            assert!(
                output.status.success(),
                "{role}/{failure}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let (mut payload, targets, _) =
                parse_private_observation_payload(&output.stdout).unwrap();
            let retained = if operation == "scrolldown" {
                failure == "none"
            } else {
                !matches!(failure, "hidden" | "sensitive")
            };
            assert_eq!(
                payload.controls.len(),
                usize::from(retained),
                "{role}/{failure}"
            );
            assert_eq!(payload.truncated, partial, "{role}/{failure}");
            assert_eq!(
                payload
                    .diagnostics
                    .contains(&DesktopObservationDiagnostic::ProviderUnavailable),
                partial
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
                    None,
                )
                .unwrap();
            if retained && !partial {
                let control = &payload.controls[0];
                let state = serde_json::to_value(control.state.as_ref().unwrap()).unwrap();
                let (key, expected) = match operation {
                    "scrolldown" => ("verticalScrollPercent", serde_json::json!(50.0)),
                    "check" => (
                        "toggle",
                        serde_json::json!(match failure {
                            "on" => "on",
                            "indeterminate" => "indeterminate",
                            _ => "off",
                        }),
                    ),
                    "select" => ("selected", serde_json::json!(failure == "selected")),
                    "expand" => (
                        "expansion",
                        serde_json::json!(match failure {
                            "expanded" => "expanded",
                            "partial" => "partiallyExpanded",
                            "leaf" => "leafNode",
                            _ => "collapsed",
                        }),
                    ),
                    _ => unreachable!(),
                };
                assert_eq!(state[key], expected, "{role}/{failure}");
                if role == "ListItem" && failure == "labelsAllowed" {
                    assert_eq!(state["selectionContainerRole"], "list");
                } else {
                    assert!(state.get("selectionContainerRole").is_none());
                }
                if role == "ComboBox" {
                    assert_eq!(
                        control.capabilities.iter().any(|c| c == "selectOption"),
                        failure != "multipleSelection"
                    );
                }
                assert_eq!(
                    control.control_ref.is_some(),
                    !matches!(role, "ComboBox" | "ListItem") || failure == "labelsAllowed"
                );
                if matches!(role, "ComboBox" | "ListItem") {
                    assert_eq!(
                        control.name.as_deref(),
                        (failure == "labelsAllowed").then_some("first")
                    );
                }
                assert!(control.field_ref.is_none());
            } else if !retained {
                assert!(targets.is_empty());
            } else {
                if matches!(failure, "selectionModeNull" | "selectionModeUnavailable") {
                    assert_eq!(
                        payload.controls[0].state.as_ref().unwrap().expansion,
                        Some(DesktopExpansionState::Collapsed)
                    );
                    assert!(!payload.controls[0]
                        .capabilities
                        .iter()
                        .any(|c| c == "selectOption"));
                } else {
                    assert!(payload.controls[0].state.is_none(), "{role}/{failure}");
                }
                assert!(payload.controls[0].control_ref.is_none());
            }
            let raw = String::from_utf8_lossy(&output.stdout);
            assert!(!raw.contains("private provider details"));
            assert!(!raw.contains("\"value\""));
            assert!(!raw.contains("\"name\":null"));
            assert!(!raw.contains("\"automationId\":null"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn verified_text_write_rejects_unavailable_metadata_and_case_changed_readback() {
        let setup = r#"
$firstControl.Editable = $true
$target = $firstControl
$pattern = New-Object ObservationTestValuePattern -ArgumentList $target
$script:uiaWalker = New-Object ObservationTestWalker
if ($failure -in @('postFieldScopeName', 'postFieldScopeMove')) {
  $region = New-Object ObservationTestElement
  $region.Identifier = 'Region'
  $region.RoleOverride = 'ControlType.Group'
  $region.RuntimeId = 10
  $region.Invokable = $true
  $region.FirstChild = $firstControl
  $region.Parent = $root
  $firstControl.Parent = $region
  $root.FirstChild = $region
  $script:uiaScope = @{ role = 'group'; name = 'Region'; path = @(0); identity = @{ processId = 42; windowHandle = 1234; controlRuntimeId = @(10) } }
}
$expected = 'C:\trusted\fixture.exe'
function Get-Process {
  param([int]$Id)
  return [pscustomobject]@{ Path = 'C:\trusted\fixture.exe'; MainWindowHandle = [System.IntPtr]::Zero }
}
"#;
        let script = format!(
            "{OBSERVATION_TEST_PROVIDER_FIXTURE}\n{UIA_SAFETY_HELPERS_SCRIPT}\n{setup}\ntry {{\n$receipt = . {{ {VERIFIED_SET_VALUE_SCRIPT} }}\n@{{ status = ($receipt | ConvertFrom-Json).status; value = $firstControl.Value; written = $firstControl.Written; mutationCount = $firstControl.MutationCount }} | ConvertTo-Json -Compress\n}} catch {{ @{{ code = ($_.Exception.Message -split '\\|')[0]; written = $firstControl.Written; mutationCount = $firstControl.MutationCount }} | ConvertTo-Json -Compress }}"
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
            "windowOwnerChanged",
            "postWindowOwnerChanged",
            "postFieldIdentity",
            "postFieldLabel",
            "postFieldScopeName",
            "postFieldScopeMove",
            "none",
        ];
        let mut cases = failures
            .iter()
            .map(|failure| (*failure, Some("Exact Case")))
            .collect::<Vec<_>>();
        cases.extend([
            ("none", Some("")),
            ("none", None),
            ("readBackNull", None),
            ("postFieldIdentity", Some("")),
            ("readOnly", Some("")),
        ]);
        let commands = cases
            .iter()
            .map(|(failure, text)| {
                let mut command =
                    desktop_child_command(WindowsDesktopAdapter::system_powershell().unwrap());
                command
                    .env("ANGELBOT_TEST_FAILURE", failure)
                    .env("ANGELBOT_DRAFT_EXPECTED_PID", "42")
                    .env("ANGELBOT_DRAFT_EXPECTED_HWND", "1234")
                    .env("ANGELBOT_DRAFT_EXPECTED_RUNTIME_ID", "1")
                    .env("ANGELBOT_FIELD_LABEL", "first");
                if let Some(text) = text {
                    command.env("ANGELBOT_DRAFT_TEXT", text);
                } else {
                    command.env_remove("ANGELBOT_DRAFT_TEXT");
                }
                command
            })
            .collect();
        for ((failure, text), output) in cases
            .into_iter()
            .zip(run_fixture_commands(commands, Some(&script)))
        {
            assert!(
                output.status.success(),
                "fixture {failure}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let result: serde_json::Value = serde_json::from_slice(&output.stdout)
                .unwrap_or_else(|error| panic!("fixture {failure}: {error}"));
            if failure == "none" {
                assert_eq!(result["status"], "verified");
                assert_eq!(result["value"], text.unwrap_or(""));
                assert_eq!(result["written"], true);
                assert_eq!(result["mutationCount"], 1);
            } else if matches!(
                failure,
                "readBackCase"
                    | "readBackNull"
                    | "postWindowOwnerChanged"
                    | "postFieldIdentity"
                    | "postFieldLabel"
                    | "postFieldScopeName"
                    | "postFieldScopeMove"
            ) {
                assert_eq!(result["code"], "RESULT_UNKNOWN", "fixture {failure}");
                assert_eq!(result["written"], true, "fixture {failure}");
                assert_eq!(result["mutationCount"], 1, "fixture {failure}");
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
  return [pscustomobject]@{ Path = 'C:\trusted\fixture.exe'; MainWindowHandle = [System.IntPtr]::Zero }
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
            "windowOwnerChanged",
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
        for (failure, output) in failures
            .into_iter()
            .zip(run_fixture_commands(commands, Some(&script)))
        {
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
$firstControl.RoleOverride = $(if ($firstControl.ControlOperation -ceq 'select') { 'ControlType.RadioButton' } elseif ($firstControl.ControlOperation -in @('check', 'uncheck')) { 'ControlType.CheckBox' } elseif ($firstControl.ControlOperation -in @('scrollup', 'scrolldown')) { 'ControlType.Pane' } else { 'ControlType.MenuItem' })
if ($env:ANGELBOT_TEST_ROLE) { $firstControl.RoleOverride = $env:ANGELBOT_TEST_ROLE }
$firstControl.ExpandState = $(if ($firstControl.ControlOperation -ceq 'collapse') { 'Expanded' } else { 'Collapsed' })
$firstControl.ToggleState = $(if ($firstControl.ControlOperation -ceq 'uncheck') { 'On' } else { 'Off' })
$firstControl.Failure = $(if ($failure.StartsWith('initial')) { $failure } else { 'none' })
$firstControl.NextSibling = $null
$root.Failure = 'none'
$region = $null
if ($env:ANGELBOT_TEST_SCOPE -ceq 'true') {
  $region = New-Object ObservationTestElement
  $region.Identifier = 'Region'
  $region.RoleOverride = 'ControlType.Group'
  $region.RuntimeId = 10
  $region.Invokable = $true
  $region.FirstChild = $firstControl
  $region.Parent = $root
  $firstControl.Parent = $region
  $root.FirstChild = $region
  $script:uiaScope = @{ role = 'group'; name = 'Region'; path = @(0); identity = @{ processId = 42; windowHandle = 1234; controlRuntimeId = @(10) } }
  if ($failure -in @('scopePrecedingHidden', 'scopePrecedingSensitive')) {
    $excluded = New-Object ObservationTestElement
    $excluded.Failure = $(if ($failure -ceq 'scopePrecedingHidden') { 'hidden' } else { 'sensitive' })
    $excluded.NextSibling = $region
    $root.FirstChild = $excluded
    $script:uiaScope.path = @(1)
  }
}
$expected = 'C:\trusted\fixture.exe'
function Get-Process {
  param([int]$Id)
  return [pscustomobject]@{ Path = 'C:\trusted\fixture.exe'; MainWindowHandle = [System.IntPtr]::Zero }
}
"#;
        let mutate = r#"
$firstControl.Failure = $failure
if ($null -ne $region) {
  if ($failure -ceq 'scopedDelayed') { $firstControl.Failure = 'delayedState' }
  if ($failure -ceq 'scopeHidden') { $region.Failure = 'hidden' }
  if ($failure -ceq 'scopeSensitive') { $region.Failure = 'sensitive' }
  if ($failure -ceq 'scopeName') { $region.Identifier = 'Changed region' }
  if ($failure -ceq 'scopeRuntime') { $region.RuntimeId = 11 }
  if ($failure -ceq 'scopeOutside') { $firstControl.Parent = $root }
  if ($failure -ceq 'scopePathChanged') { $laterControl.NextSibling = $region; $root.FirstChild = $laterControl }
  if ($failure -ceq 'scopeBudget') { $script:uiaNodeCount = 255 }
  if ($failure -ceq 'postScopeBudget') { $script:uiaNodeCount = 250 }
}
if ($failure -ceq 'noop') {
  $firstControl.Selected = $true
  $firstControl.ExpandState = $(if ($controlOperation -ceq 'expand') { 'Expanded' } else { 'Collapsed' })
  $firstControl.ToggleState = $(if ($controlOperation -ceq 'check') { 'On' } else { 'Off' })
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
            DesktopControlOperation::Check,
            DesktopControlOperation::Uncheck,
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
                "postSelectedInvalid",
                "postNameChanged",
                "postRuntimeChanged",
                "missingPattern",
                "stateUnavailable",
                "initialIndeterminate",
                "initialStateNull",
                "initialInvalidState",
                "indeterminate",
                "stateNull",
                "invalidState",
                "postIndeterminate",
                "leaf",
                "invalidPosition",
                "nonScrollable",
                "wrongDirection",
            ] {
                let scrolling = matches!(
                    operation,
                    DesktopControlOperation::ScrollUp | DesktopControlOperation::ScrollDown
                );
                let checkbox = matches!(
                    operation,
                    DesktopControlOperation::Check | DesktopControlOperation::Uncheck
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
                    || (matches!(
                        failure,
                        "initialIndeterminate"
                            | "initialStateNull"
                            | "initialInvalidState"
                            | "indeterminate"
                            | "stateNull"
                            | "invalidState"
                            | "postIndeterminate"
                    ) && !checkbox
                        && !(operation == DesktopControlOperation::Select
                            && matches!(
                                failure,
                                "initialInvalidState" | "stateNull" | "invalidState"
                            )))
                    || (failure == "postSelectedInvalid"
                        && operation != DesktopControlOperation::Select)
                {
                    continue;
                }
                let mut command =
                    desktop_child_command(WindowsDesktopAdapter::system_powershell().unwrap());
                command
                    .env("ANGELBOT_TEST_FAILURE", failure)
                    .env("ANGELBOT_DRAFT_EXPECTED_PID", "42")
                    .env("ANGELBOT_DRAFT_EXPECTED_HWND", "1234")
                    .env("ANGELBOT_DRAFT_EXPECTED_RUNTIME_ID", "1")
                    .env("ANGELBOT_CONTROL_LABEL", "first")
                    .env(
                        "ANGELBOT_CONTROL_ROLE",
                        if operation == DesktopControlOperation::Select {
                            "radioButton"
                        } else if checkbox {
                            "checkBox"
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
        for failure in [
            "scopedNone",
            "scopedDelayed",
            "scopePrecedingHidden",
            "scopePrecedingSensitive",
            "scopeHidden",
            "scopeSensitive",
            "scopeName",
            "scopeRuntime",
            "scopeOutside",
            "scopePathChanged",
            "scopeBudget",
            "postScopeName",
            "postScopeMoved",
            "postScopeBudget",
        ] {
            let mut command =
                desktop_child_command(WindowsDesktopAdapter::system_powershell().unwrap());
            command
                .env("ANGELBOT_TEST_FAILURE", failure)
                .env("ANGELBOT_TEST_SCOPE", "true")
                .env("ANGELBOT_DRAFT_EXPECTED_PID", "42")
                .env("ANGELBOT_DRAFT_EXPECTED_HWND", "1234")
                .env("ANGELBOT_DRAFT_EXPECTED_RUNTIME_ID", "1")
                .env("ANGELBOT_CONTROL_LABEL", "first")
                .env("ANGELBOT_CONTROL_ROLE", "checkBox")
                .env("ANGELBOT_CONTROL_OPERATION", "check");
            cases.push((DesktopControlOperation::Check, failure));
            commands.push(command);
        }
        // The same production helper must enforce state, identity and no-op
        // semantics for newly admitted roles, not merely advertise a reference.
        for (role, operation) in [
            ("listItem", DesktopControlOperation::Select),
            ("comboBox", DesktopControlOperation::Expand),
            ("comboBox", DesktopControlOperation::Collapse),
        ] {
            for failure in ["none", "noop", "wrongState", "postNameChanged"] {
                let mut command =
                    desktop_child_command(WindowsDesktopAdapter::system_powershell().unwrap());
                command
                    .env("ANGELBOT_TEST_FAILURE", failure)
                    .env(
                        "ANGELBOT_TEST_ROLE",
                        if role == "listItem" {
                            "ControlType.ListItem"
                        } else {
                            "ControlType.ComboBox"
                        },
                    )
                    .env("ANGELBOT_DRAFT_EXPECTED_PID", "42")
                    .env("ANGELBOT_DRAFT_EXPECTED_HWND", "1234")
                    .env("ANGELBOT_DRAFT_EXPECTED_RUNTIME_ID", "1")
                    .env("ANGELBOT_CONTROL_LABEL", "first")
                    .env("ANGELBOT_CONTROL_ROLE", role)
                    .env("ANGELBOT_CONTROL_OPERATION", operation.as_str());
                cases.push((operation, failure));
                commands.push(command);
            }
        }
        for ((operation, failure), output) in cases
            .into_iter()
            .zip(run_fixture_commands(commands, Some(&script)))
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
            if matches!(
                failure,
                "none"
                    | "delayedState"
                    | "noop"
                    | "scopedNone"
                    | "scopedDelayed"
                    | "scopePrecedingHidden"
                    | "scopePrecedingSensitive"
            ) {
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
                    | "initialIndeterminate"
                    | "initialStateNull"
                    | "initialInvalidState"
                    | "indeterminate"
                    | "stateNull"
                    | "invalidState"
                    | "scopeHidden"
                    | "scopeSensitive"
                    | "scopeName"
                    | "scopeRuntime"
                    | "scopeOutside"
                    | "scopePathChanged"
                    | "scopeBudget"
                    | "leaf"
                    | "invalidPosition"
                    | "nonScrollable"
            ) {
                assert!(
                    matches!(
                        result["code"].as_str(),
                        Some(
                            "UNSUPPORTED_CONTROL"
                                | "TARGET_UNAVAILABLE"
                                | "TARGET_CHANGED"
                                | "SCAN_LIMIT"
                        )
                    ),
                    "{operation:?}/{failure}: {result}"
                );
                assert_eq!(result["applied"], false, "{operation:?}/{failure}");
                if failure.starts_with("scope") {
                    assert_eq!(
                        result["code"],
                        if failure == "scopeBudget" {
                            "SCAN_LIMIT"
                        } else {
                            "TARGET_CHANGED"
                        }
                    );
                }
                if matches!(failure, "initialIndeterminate" | "indeterminate") {
                    assert_eq!(result["code"], "UNSUPPORTED_CONTROL");
                } else if matches!(
                    failure,
                    "initialStateNull" | "initialInvalidState" | "stateNull" | "invalidState"
                ) {
                    assert_eq!(result["code"], "TARGET_UNAVAILABLE");
                }
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
                        | "initialIndeterminate"
                        | "initialStateNull"
                        | "initialInvalidState"
                        | "indeterminate"
                        | "stateNull"
                        | "invalidState"
                        | "scopeHidden"
                        | "scopeSensitive"
                        | "scopeName"
                        | "scopeRuntime"
                        | "scopeOutside"
                        | "scopePathChanged"
                        | "scopeBudget"
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
            "PROGRAMFILES",
            "PROGRAMFILES(X86)",
            "PROGRAMW6432",
        ];
        for (key, value) in command.get_envs() {
            assert_eq!(value, std::env::var_os(key).as_deref());
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

        let powershell = WindowsDesktopAdapter::system_powershell().unwrap();
        let script = format!(
            "function Get-Process {{}}\n{}",
            control_observation_script(false)
        );
        for cancelled in [true, false] {
            let mut child = desktop_child_command(powershell.clone())
                .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                .env("ANGELBOT_TARGET_EXE", "C:\\fixture\\not-launched.exe")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let start = std::time::Instant::now();
            let timeout = if cancelled {
                DISCOVERY_TIMEOUT
            } else {
                std::time::Duration::from_millis(150)
            };
            let error = wait_for_read_only_child_with_control(
                &mut child,
                "Desktop observation",
                &|| cancelled && start.elapsed() >= std::time::Duration::from_millis(150),
                timeout,
            )
            .unwrap_err();
            assert_eq!(
                error.code,
                if cancelled {
                    "SCAN_CANCELLED"
                } else {
                    "SCAN_TIMEOUT"
                }
            );
            assert!(start.elapsed() < std::time::Duration::from_secs(3));
            assert!(child.try_wait().unwrap().is_some(), "child was not reaped");
        }
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

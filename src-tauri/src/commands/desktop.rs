//! Trusted desktop application configuration.
//!
//! These commands only manage the local allowlist. Desktop side effects are
//! exposed to the agent through confirmation-aware tool handlers.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::command;

pub const CAPABILITY_LAUNCH: &str = "launch";
pub const CAPABILITY_DRAFT: &str = "draft";
/// Allows filling a dynamically selected ordinary text field. This is
/// separate from the fixed target authorized by the draft capability.
pub const CAPABILITY_FILL: &str = "fill";
/// Explicit opt-in to invoking observed controls. Existing trusted entries
/// never acquire this capability implicitly; every invocation is confirmed.
pub const CAPABILITY_INTERACT: &str = "interact";
/// Persisted compatibility marker for window observation scope, not a
/// separately configurable action capability in the current settings UI.
pub const CAPABILITY_OBSERVE: &str = "observe";
/// Consent-version marker: older structural observation grants never silently
/// expand to pixel capture. This is not a separate per-app settings toggle.
pub const CAPABILITY_OBSERVE_IMAGE: &str = "observeImage";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TrustedDesktopApp {
    pub id: String,
    pub display_name: String,
    pub executable_path: String,
    pub capabilities: Vec<String>,
    pub draft_selector: Option<String>,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TrustedDesktopAppStatus {
    pub id: String,
    pub status: String,
}

/// A settings-page confirmation for an existing app whose observation scope
/// predates the current trusted-app defaults. The path guards against an app
/// being replaced between display and confirmation.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopObservationScopeApp {
    pub id: String,
    pub executable_path: String,
}

pub const STATUS_AVAILABLE: &str = "available";
pub const STATUS_DISABLED: &str = "disabled";
pub const STATUS_EXECUTABLE_UNAVAILABLE: &str = "executableUnavailable";

pub fn trusted_desktop_app_status(app: &TrustedDesktopApp) -> &'static str {
    if !app.enabled {
        return STATUS_DISABLED;
    }
    let path = std::path::Path::new(&app.executable_path);
    if path.is_file()
        && path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("exe"))
    {
        STATUS_AVAILABLE
    } else {
        STATUS_EXECUTABLE_UNAVAILABLE
    }
}

fn validate_identifier(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err("Application ID must contain only letters, numbers, '_' or '-' and be at most 64 characters".to_string());
    }
    Ok(())
}

fn normalize_app(mut app: TrustedDesktopApp) -> Result<TrustedDesktopApp, String> {
    app.id = app.id.trim().to_string();
    validate_identifier(&app.id)?;
    app.display_name = app.display_name.trim().to_string();
    if app.display_name.is_empty() || app.display_name.chars().count() > 80 {
        return Err("Display name must contain 1 to 80 characters".to_string());
    }
    let canonical = std::fs::canonicalize(app.executable_path.trim())
        .map_err(|e| format!("Executable is unavailable: {e}"))?;
    if !canonical.is_file()
        || canonical
            .extension()
            .and_then(|v| v.to_str())
            .map(|v| !v.eq_ignore_ascii_case("exe"))
            .unwrap_or(true)
    {
        return Err("Executable path must point to an existing .exe file".to_string());
    }
    app.executable_path = canonical.to_string_lossy().to_string();

    if app.capabilities.iter().any(|v| {
        v != CAPABILITY_LAUNCH
            && v != CAPABILITY_DRAFT
            && v != CAPABILITY_FILL
            && v != CAPABILITY_INTERACT
            && v != CAPABILITY_OBSERVE
            && v != CAPABILITY_OBSERVE_IMAGE
    }) {
        return Err(
            "Capabilities must contain only desktop action capabilities or observation scope markers"
                .to_string(),
        );
    }
    // A new or explicitly re-saved trusted app enters observation scope. Old
    // stored rows without this marker remain outside that scope until confirmed.
    app.capabilities.push(CAPABILITY_OBSERVE.to_string());
    app.capabilities.push(CAPABILITY_OBSERVE_IMAGE.to_string());
    app.capabilities.sort();
    app.capabilities.dedup();
    app.draft_selector = app
        .draft_selector
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    if app.capabilities.iter().any(|v| v == CAPABILITY_DRAFT) {
        let selector = app.draft_selector.as_deref().ok_or_else(|| {
            "Draft capability requires an exact UI Automation control name".to_string()
        })?;
        if selector.chars().count() > 160 || selector.contains('\0') {
            return Err(
                "Draft control name must be at most 160 characters and contain no NUL characters"
                    .to_string(),
            );
        }
    } else {
        app.draft_selector = None;
    }
    Ok(app)
}

fn row_to_app(row: &rusqlite::Row<'_>) -> rusqlite::Result<TrustedDesktopApp> {
    let capabilities: String = row.get(3)?;
    let capabilities = serde_json::from_str(&capabilities).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(TrustedDesktopApp {
        id: row.get(0)?,
        display_name: row.get(1)?,
        executable_path: row.get(2)?,
        capabilities,
        draft_selector: row.get(4)?,
        enabled: row.get::<_, i64>(5)? != 0,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

pub fn get_trusted_desktop_app_impl(
    state: &crate::AppState,
    id: &str,
) -> Result<Option<TrustedDesktopApp>, String> {
    get_trusted_desktop_app_from_db(&state.db, id)
}

pub fn get_trusted_desktop_app_from_db(
    db: &std::sync::Arc<std::sync::Mutex<rusqlite::Connection>>,
    id: &str,
) -> Result<Option<TrustedDesktopApp>, String> {
    let conn = db.lock().map_err(|e| e.to_string())?;
    get_trusted_desktop_app_from_conn(&conn, id)
}

pub(crate) fn get_trusted_desktop_app_from_conn(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<Option<TrustedDesktopApp>, String> {
    validate_identifier(id)?;
    conn.query_row(
        "SELECT id, display_name, executable_path, capabilities, draft_selector, enabled, created_at, updated_at FROM desktop_trusted_apps WHERE id = ?1",
        params![id],
        row_to_app,
    ).optional().map_err(|e| e.to_string())
}

/// Explicit settings-page inspection of an already saved trusted app.
/// Disabled and launch-only entries are inspectable while the user sets up a
/// draft target; inspection neither grants draft capability nor saves it.
#[command]
pub async fn inspect_desktop_draft_targets(
    state: tauri::State<'_, crate::AppState>,
    app_id: String,
) -> Result<crate::desktop_control::DesktopDraftDiscovery, String> {
    let db = state.db.clone();
    let adapter = state.desktop_adapter.clone();
    tauri::async_runtime::spawn_blocking(move || {
        inspect_desktop_draft_targets_from_db(&db, adapter.as_ref(), &app_id)
    })
    .await
    .map_err(|error| format!("Draft target inspection task failed: {error}"))?
}

pub fn inspect_desktop_draft_targets_from_db(
    db: &std::sync::Arc<std::sync::Mutex<rusqlite::Connection>>,
    adapter: &dyn crate::desktop_control::DesktopAdapter,
    app_id: &str,
) -> Result<crate::desktop_control::DesktopDraftDiscovery, String> {
    let app = get_trusted_desktop_app_from_db(db, app_id)?.ok_or_else(|| {
        "Save the trusted application before inspecting its draft targets".to_string()
    })?;
    adapter
        .inspect_draft_targets_for_settings(&app.id, &app.executable_path)
        .map_err(|error| error.to_string())
}

#[command]
pub fn get_desktop_trusted_apps(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<TrustedDesktopApp>, String> {
    get_desktop_trusted_apps_impl(&state)
}

pub fn get_desktop_trusted_apps_impl(
    state: &crate::AppState,
) -> Result<Vec<TrustedDesktopApp>, String> {
    get_desktop_trusted_apps_from_db(&state.db)
}

pub fn get_desktop_trusted_apps_from_db(
    db: &std::sync::Arc<std::sync::Mutex<rusqlite::Connection>>,
) -> Result<Vec<TrustedDesktopApp>, String> {
    let conn = db.lock().map_err(|e| e.to_string())?;
    let mut statement = conn.prepare(
        "SELECT id, display_name, executable_path, capabilities, draft_selector, enabled, created_at, updated_at FROM desktop_trusted_apps ORDER BY display_name COLLATE NOCASE, id"
    ).map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], row_to_app)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[command]
pub fn get_desktop_trusted_app_statuses(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<TrustedDesktopAppStatus>, String> {
    get_desktop_trusted_app_statuses_impl(&state)
}

pub fn get_desktop_trusted_app_statuses_impl(
    state: &crate::AppState,
) -> Result<Vec<TrustedDesktopAppStatus>, String> {
    get_desktop_trusted_app_statuses_from_db(&state.db)
}

pub fn get_desktop_trusted_app_statuses_from_db(
    db: &std::sync::Arc<std::sync::Mutex<rusqlite::Connection>>,
) -> Result<Vec<TrustedDesktopAppStatus>, String> {
    Ok(get_desktop_trusted_apps_from_db(db)?
        .into_iter()
        .map(|app| TrustedDesktopAppStatus {
            id: app.id.clone(),
            status: trusted_desktop_app_status(&app).to_string(),
        })
        .collect())
}

#[command]
pub fn save_desktop_trusted_app(
    state: tauri::State<'_, crate::AppState>,
    app: TrustedDesktopApp,
) -> Result<TrustedDesktopApp, String> {
    save_desktop_trusted_app_impl(&state, app)
}

pub fn save_desktop_trusted_app_impl(
    state: &crate::AppState,
    app: TrustedDesktopApp,
) -> Result<TrustedDesktopApp, String> {
    let saved = save_desktop_trusted_app_to_db(&state.db, app)?;
    state.desktop_action_previews.revoke_for_app(&saved.id)?;
    Ok(saved)
}

// The existing timestamp also attests configuration freshness. Advance it
// even within one second or after a backwards clock adjustment; never let
// revoke -> restore make a pre-revocation approval match again.
fn next_app_revision(previous: i64, now: i64) -> Result<i64, String> {
    previous
        .checked_add(1)
        .map(|next| next.max(now))
        .ok_or_else(|| "Trusted application configuration revision exhausted".to_string())
}

fn save_desktop_trusted_app_to_db(
    db: &std::sync::Arc<std::sync::Mutex<rusqlite::Connection>>,
    app: TrustedDesktopApp,
) -> Result<TrustedDesktopApp, String> {
    let mut app = normalize_app(app)?;
    let now = chrono::Utc::now().timestamp();
    let conn = db.lock().map_err(|e| e.to_string())?;
    let existing: Option<(i64, String, i64)> = conn
        .query_row(
            "SELECT created_at, executable_path, updated_at FROM desktop_trusted_apps WHERE id = ?1",
            params![app.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if existing
        .as_ref()
        .is_some_and(|(_, path, _)| path != &app.executable_path)
        && app
            .capabilities
            .iter()
            .any(|value| value == CAPABILITY_FILL || value == CAPABILITY_INTERACT)
    {
        return Err(
            "Revoke the fill or interact capability before changing the trusted executable path"
                .to_string(),
        );
    }
    app.updated_at = existing
        .as_ref()
        .map(|(_, _, previous)| next_app_revision(*previous, now))
        .transpose()?
        .unwrap_or(now);
    app.created_at = existing.map(|(created_at, _, _)| created_at).unwrap_or(now);
    let capabilities = serde_json::to_string(&app.capabilities).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO desktop_trusted_apps (id, display_name, executable_path, capabilities, draft_selector, enabled, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET display_name = excluded.display_name, executable_path = excluded.executable_path,
            capabilities = excluded.capabilities, draft_selector = excluded.draft_selector, enabled = excluded.enabled,
            updated_at = excluded.updated_at",
        params![app.id, app.display_name, app.executable_path, capabilities, app.draft_selector, app.enabled as i64, app.created_at, app.updated_at],
    ).map_err(|e| {
        if e.to_string().contains("UNIQUE constraint failed") {
            "This executable is already trusted under another application ID".to_string()
        } else { e.to_string() }
    })?;
    Ok(app)
}

/// Upgrade only the exact legacy rows that the settings UI displayed and the
/// user explicitly confirmed. Validation and writes share one DB transaction.
#[command]
pub fn confirm_desktop_observation_scope(
    state: tauri::State<'_, crate::AppState>,
    apps: Vec<DesktopObservationScopeApp>,
) -> Result<Vec<TrustedDesktopApp>, String> {
    confirm_desktop_observation_scope_from_db(&state.db, &apps)
}

pub fn confirm_desktop_observation_scope_from_db(
    db: &std::sync::Arc<std::sync::Mutex<rusqlite::Connection>>,
    apps: &[DesktopObservationScopeApp],
) -> Result<Vec<TrustedDesktopApp>, String> {
    if apps.is_empty() {
        return Err(
            "Select at least one existing application to include in window observation scope"
                .to_string(),
        );
    }
    let mut seen = std::collections::HashSet::new();
    for app in apps {
        validate_identifier(&app.id)?;
        if !seen.insert(&app.id) {
            return Err(format!("Application '{}' appears more than once", app.id));
        }
    }

    let mut conn = db.lock().map_err(|e| e.to_string())?;
    let transaction = conn.transaction().map_err(|e| e.to_string())?;
    let mut confirmed = Vec::with_capacity(apps.len());
    for selection in apps {
        let app = transaction
            .query_row(
                "SELECT id, display_name, executable_path, capabilities, draft_selector, enabled, created_at, updated_at FROM desktop_trusted_apps WHERE id = ?1",
                params![selection.id],
                row_to_app,
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Trusted application '{}' no longer exists", selection.id))?;
        if app.executable_path != selection.executable_path {
            return Err(format!(
                "Trusted application '{}' changed since confirmation was requested",
                selection.id
            ));
        }
        confirmed.push(app);
    }

    let now = chrono::Utc::now().timestamp();
    for app in &mut confirmed {
        if [CAPABILITY_OBSERVE, CAPABILITY_OBSERVE_IMAGE]
            .iter()
            .any(|marker| !app.capabilities.iter().any(|value| value == marker))
        {
            app.capabilities.push(CAPABILITY_OBSERVE.to_string());
            app.capabilities.push(CAPABILITY_OBSERVE_IMAGE.to_string());
            app.capabilities.sort();
            app.capabilities.dedup();
            app.updated_at = next_app_revision(app.updated_at, now)?;
            let capabilities =
                serde_json::to_string(&app.capabilities).map_err(|e| e.to_string())?;
            transaction
                .execute(
                    "UPDATE desktop_trusted_apps SET capabilities = ?1, updated_at = ?2 WHERE id = ?3",
                    params![capabilities, app.updated_at, app.id],
                )
                .map_err(|e| e.to_string())?;
        }
    }
    transaction.commit().map_err(|e| e.to_string())?;
    Ok(confirmed)
}

#[command]
pub fn delete_desktop_trusted_app(
    state: tauri::State<'_, crate::AppState>,
    id: String,
) -> Result<(), String> {
    delete_desktop_trusted_app_impl(&state, &id)
}

pub fn delete_desktop_trusted_app_impl(state: &crate::AppState, id: &str) -> Result<(), String> {
    validate_identifier(id)?;
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    // Same DB -> preview order as preflight's final check + insertion: a
    // preview checked before deletion cannot be inserted after revocation.
    state.desktop_action_previews.revoke_for_app(id)?;
    let changed = conn
        .execute(
            "DELETE FROM desktop_trusted_apps WHERE id = ?1",
            params![id],
        )
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err("Trusted application not found".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> std::sync::Arc<std::sync::Mutex<rusqlite::Connection>> {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE desktop_trusted_apps (
                id TEXT PRIMARY KEY, display_name TEXT NOT NULL, executable_path TEXT NOT NULL,
                capabilities TEXT NOT NULL, draft_selector TEXT, enabled INTEGER NOT NULL,
                created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
            );",
            )
            .unwrap();
        std::sync::Arc::new(std::sync::Mutex::new(connection))
    }

    #[test]
    fn normalization_rejects_unbounded_capability() {
        let file = tempfile::Builder::new().suffix(".exe").tempfile().unwrap();
        let app = TrustedDesktopApp {
            id: "chat".into(),
            display_name: "Chat".into(),
            executable_path: file.path().to_string_lossy().into(),
            capabilities: vec!["shell".into()],
            draft_selector: None,
            enabled: true,
            created_at: 0,
            updated_at: 0,
        };
        assert!(normalize_app(app).unwrap_err().contains("Capabilities"));
    }

    #[test]
    fn draft_requires_exact_selector() {
        let file = tempfile::Builder::new().suffix(".exe").tempfile().unwrap();
        let app = TrustedDesktopApp {
            id: "chat".into(),
            display_name: "Chat".into(),
            executable_path: file.path().to_string_lossy().into(),
            capabilities: vec![CAPABILITY_DRAFT.into()],
            draft_selector: None,
            enabled: true,
            created_at: 0,
            updated_at: 0,
        };
        assert!(normalize_app(app).unwrap_err().contains("requires"));
    }

    #[test]
    fn saved_trusted_app_enters_observation_scope_without_draft_selector() {
        let file = tempfile::Builder::new().suffix(".exe").tempfile().unwrap();
        let app = TrustedDesktopApp {
            id: "notes".into(),
            display_name: "Notes".into(),
            executable_path: file.path().to_string_lossy().into(),
            capabilities: vec![],
            draft_selector: None,
            enabled: true,
            created_at: 0,
            updated_at: 0,
        };
        let db = test_db();
        let saved = save_desktop_trusted_app_to_db(&db, app).unwrap();
        assert_eq!(
            saved.capabilities,
            vec![CAPABILITY_OBSERVE, CAPABILITY_OBSERVE_IMAGE]
        );
        assert!(saved.draft_selector.is_none());
        assert_eq!(
            get_trusted_desktop_app_from_db(&db, "notes")
                .unwrap()
                .unwrap()
                .capabilities,
            vec![CAPABILITY_OBSERVE, CAPABILITY_OBSERVE_IMAGE]
        );
    }

    #[test]
    fn fill_is_independent_of_the_fixed_draft_target() {
        let file = tempfile::Builder::new().suffix(".exe").tempfile().unwrap();
        let db = test_db();
        let saved = save_desktop_trusted_app_to_db(
            &db,
            TrustedDesktopApp {
                id: "editor".into(),
                display_name: "Editor".into(),
                executable_path: file.path().to_string_lossy().into(),
                capabilities: vec![CAPABILITY_FILL.into()],
                draft_selector: None,
                enabled: true,
                created_at: 0,
                updated_at: 0,
            },
        )
        .unwrap();
        assert_eq!(
            saved.capabilities,
            vec![
                CAPABILITY_FILL,
                CAPABILITY_OBSERVE,
                CAPABILITY_OBSERVE_IMAGE
            ]
        );
        assert!(saved.draft_selector.is_none());

        let draft_only = normalize_app(TrustedDesktopApp {
            capabilities: vec![CAPABILITY_DRAFT.into()],
            draft_selector: Some("Message".into()),
            ..saved
        })
        .unwrap();
        assert!(!draft_only
            .capabilities
            .iter()
            .any(|value| value == CAPABILITY_FILL));
    }

    #[test]
    fn changing_executable_cannot_carry_dynamic_control_authority() {
        for capability in [CAPABILITY_FILL, CAPABILITY_INTERACT] {
            let first = tempfile::Builder::new().suffix(".exe").tempfile().unwrap();
            let second = tempfile::Builder::new().suffix(".exe").tempfile().unwrap();
            let db = test_db();
            let mut app = save_desktop_trusted_app_to_db(
                &db,
                TrustedDesktopApp {
                    id: "editor".into(),
                    display_name: "Editor".into(),
                    executable_path: first.path().to_string_lossy().into(),
                    capabilities: vec![capability.into()],
                    draft_selector: None,
                    enabled: true,
                    created_at: 0,
                    updated_at: 0,
                },
            )
            .unwrap();
            app.executable_path = second.path().to_string_lossy().into();
            assert!(save_desktop_trusted_app_to_db(&db, app.clone())
                .unwrap_err()
                .contains("Revoke the fill or interact capability"));
            assert_eq!(
                get_trusted_desktop_app_from_db(&db, "editor")
                    .unwrap()
                    .unwrap()
                    .executable_path,
                first.path().canonicalize().unwrap().to_string_lossy()
            );
            app.capabilities.retain(|value| value != capability);
            let replaced = save_desktop_trusted_app_to_db(&db, app).unwrap();
            assert!(!replaced
                .capabilities
                .iter()
                .any(|value| value == capability));
        }
    }

    #[test]
    fn legacy_observation_scope_confirmation_is_atomic_and_path_bound() {
        let first_executable = tempfile::Builder::new().suffix(".exe").tempfile().unwrap();
        let second_executable = tempfile::Builder::new().suffix(".exe").tempfile().unwrap();
        let first_path = first_executable.path().to_string_lossy().to_string();
        let second_path = second_executable.path().to_string_lossy().to_string();
        let db = test_db();
        {
            let conn = db.lock().unwrap();
            for (id, path) in [("first", &first_path), ("second", &second_path)] {
                conn.execute(
                    "INSERT INTO desktop_trusted_apps (id, display_name, executable_path, capabilities, draft_selector, enabled, created_at, updated_at) VALUES (?1, ?1, ?2, '[\"launch\"]', NULL, 1, 1, 1)",
                    params![id, path],
                ).unwrap();
            }
        }
        let first = DesktopObservationScopeApp {
            id: "first".into(),
            executable_path: first_path,
        };
        let wrong_second = DesktopObservationScopeApp {
            id: "second".into(),
            executable_path: "C:/replaced.exe".into(),
        };
        assert!(
            confirm_desktop_observation_scope_from_db(&db, &[first.clone(), wrong_second]).is_err()
        );
        assert_eq!(
            get_trusted_desktop_app_from_db(&db, "first")
                .unwrap()
                .unwrap()
                .capabilities,
            vec![CAPABILITY_LAUNCH]
        );

        let confirmed = confirm_desktop_observation_scope_from_db(&db, &[first.clone()]).unwrap();
        assert_eq!(
            confirmed[0].capabilities,
            vec![
                CAPABILITY_LAUNCH,
                CAPABILITY_OBSERVE,
                CAPABILITY_OBSERVE_IMAGE
            ]
        );
        assert_eq!(confirmed[0].created_at, 1);
        assert_eq!(
            get_trusted_desktop_app_from_db(&db, "second")
                .unwrap()
                .unwrap()
                .capabilities,
            vec![CAPABILITY_LAUNCH]
        );
        assert_eq!(
            confirm_desktop_observation_scope_from_db(&db, &[first]).unwrap()[0].capabilities,
            vec![
                CAPABILITY_LAUNCH,
                CAPABILITY_OBSERVE,
                CAPABILITY_OBSERVE_IMAGE
            ]
        );
    }

    #[test]
    fn legacy_app_is_not_upgraded_just_by_loading_it() {
        let db = test_db();
        db.lock().unwrap().execute(
            "INSERT INTO desktop_trusted_apps (id, display_name, executable_path, capabilities, draft_selector, enabled, created_at, updated_at) VALUES ('legacy', 'Legacy', 'C:/legacy.exe', '[\"launch\"]', NULL, 1, 1, 1)",
            [],
        ).unwrap();
        let app = get_trusted_desktop_app_from_db(&db, "legacy")
            .unwrap()
            .unwrap();
        assert_eq!(app.capabilities, vec![CAPABILITY_LAUNCH]);
        assert!(confirm_desktop_observation_scope_from_db(&db, &[]).is_err());
    }

    #[test]
    fn structural_scope_requires_explicit_image_upgrade_and_advances_revision_once() {
        let db = test_db();
        db.lock().unwrap().execute(
            "INSERT INTO desktop_trusted_apps (id, display_name, executable_path, capabilities, draft_selector, enabled, created_at, updated_at) VALUES ('legacy', 'Legacy', 'C:/legacy.exe', '[\"launch\",\"observe\"]', NULL, 1, 1, 1)",
            [],
        ).unwrap();
        let loaded = get_trusted_desktop_app_from_db(&db, "legacy")
            .unwrap()
            .unwrap();
        assert_eq!(
            loaded.capabilities,
            vec![CAPABILITY_LAUNCH, CAPABILITY_OBSERVE]
        );
        assert_eq!(loaded.updated_at, 1);
        let selection = DesktopObservationScopeApp {
            id: loaded.id,
            executable_path: loaded.executable_path,
        };
        let confirmed =
            confirm_desktop_observation_scope_from_db(&db, &[selection.clone()]).unwrap();
        assert_eq!(
            confirmed[0].capabilities,
            vec![
                CAPABILITY_LAUNCH,
                CAPABILITY_OBSERVE,
                CAPABILITY_OBSERVE_IMAGE
            ]
        );
        assert!(confirmed[0].updated_at > loaded.updated_at);
        let repeat = confirm_desktop_observation_scope_from_db(&db, &[selection]).unwrap();
        assert_eq!(repeat[0], confirmed[0]);
    }

    #[test]
    fn runtime_status_distinguishes_available_disabled_and_missing_apps() {
        let executable = tempfile::Builder::new().suffix(".exe").tempfile().unwrap();
        let mut app = TrustedDesktopApp {
            id: "chat".into(),
            display_name: "Chat".into(),
            executable_path: executable.path().to_string_lossy().into(),
            capabilities: vec![CAPABILITY_LAUNCH.into()],
            draft_selector: None,
            enabled: true,
            created_at: 0,
            updated_at: 0,
        };
        assert_eq!(trusted_desktop_app_status(&app), STATUS_AVAILABLE);

        app.enabled = false;
        assert_eq!(trusted_desktop_app_status(&app), STATUS_DISABLED);

        app.enabled = true;
        app.executable_path = executable
            .path()
            .with_extension("missing.exe")
            .to_string_lossy()
            .into();
        assert_eq!(
            trusted_desktop_app_status(&app),
            STATUS_EXECUTABLE_UNAVAILABLE
        );
    }

    #[test]
    fn target_inspection_requires_a_saved_app_id() {
        let db = std::sync::Arc::new(std::sync::Mutex::new(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        db.lock().unwrap().execute_batch(
            "CREATE TABLE desktop_trusted_apps (id TEXT, display_name TEXT, executable_path TEXT, capabilities TEXT, draft_selector TEXT, enabled INTEGER, created_at INTEGER, updated_at INTEGER);"
        ).unwrap();
        let adapter = crate::desktop_control::create_mock_adapter();
        let error =
            inspect_desktop_draft_targets_from_db(&db, adapter.as_ref(), "not-saved").unwrap_err();
        assert!(error.contains("Save the trusted"));
        assert!(inspect_desktop_draft_targets_from_db(&db, adapter.as_ref(), "../other").is_err());
    }

    #[test]
    fn settings_inspection_uses_the_supplied_adapter_without_granting_capabilities() {
        let db = test_db();
        db.lock().unwrap().execute(
            "INSERT INTO desktop_trusted_apps (id, display_name, executable_path, capabilities, draft_selector, enabled, created_at, updated_at) VALUES ('setup', 'Setup', 'fixture.exe', '[\"launch\"]', NULL, 0, 1, 1)",
            [],
        ).unwrap();
        let adapter = crate::desktop_control::create_mock_adapter();
        let discovery =
            inspect_desktop_draft_targets_from_db(&db, adapter.as_ref(), "setup").unwrap();
        assert_eq!(discovery.app_id, "setup");
        assert_eq!(
            discovery.candidates,
            vec![crate::desktop_control::DesktopDraftTargetCandidate {
                name: "Message".into(),
                automation_id: Some("mockField".into()),
            }]
        );
        let saved = get_trusted_desktop_app_from_db(&db, "setup")
            .unwrap()
            .unwrap();
        assert!(!saved.enabled);
        assert_eq!(saved.capabilities, vec![CAPABILITY_LAUNCH]);
        assert!(saved.draft_selector.is_none());
    }
}

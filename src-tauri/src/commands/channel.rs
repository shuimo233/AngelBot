use crate::AppState;
use chrono::Utc;
use rusqlite::params;
use serde::Serialize;
use tauri::State;
use uuid::Uuid;

#[derive(Serialize)]
pub struct ChannelConnection {
    pub id: String,
    #[serde(rename = "channelType")]
    pub channel_type: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub enabled: bool,
    #[serde(rename = "permissionSummary")]
    pub permission_summary: String,
    #[serde(rename = "proactiveReason")]
    pub proactive_reason: String,
    #[serde(rename = "proactiveDailyLimit")]
    pub proactive_daily_limit: i64,
    #[serde(rename = "credentialConfigured")]
    pub credential_configured: bool,
}
#[derive(Serialize)]
pub struct ChannelAuditEntry {
    pub id: String,
    pub action: String,
    pub details: String,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
}

#[tauri::command]
pub fn create_channel_connection(
    state: State<AppState>,
    channel_type: String,
    display_name: String,
    permission_summary: String,
) -> Result<ChannelConnection, String> {
    create_channel_connection_impl(&state, channel_type, display_name, permission_summary)
}
pub fn create_channel_connection_impl(
    state: &AppState,
    channel_type: String,
    display_name: String,
    permission_summary: String,
) -> Result<ChannelConnection, String> {
    let now = Utc::now().timestamp();
    let item = ChannelConnection {
        id: Uuid::new_v4().to_string(),
        channel_type,
        display_name,
        enabled: false,
        permission_summary,
        proactive_reason: String::new(),
        proactive_daily_limit: 0,
        credential_configured: false,
    };
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute("INSERT INTO channel_connections (id,channel_type,display_name,enabled,permission_summary,proactive_reason,proactive_daily_limit,created_at,updated_at) VALUES (?1,?2,?3,0,?4,'',0,?5,?5)",params![item.id,item.channel_type,item.display_name,item.permission_summary,now]).map_err(|e|e.to_string())?;
    conn.execute("INSERT INTO channel_audit_log (id,connection_id,action,created_at) VALUES (?1,?2,'registered',?3)",params![Uuid::new_v4().to_string(),item.id,now]).map_err(|e|e.to_string())?;
    Ok(item)
}

#[tauri::command]
pub fn get_channel_connections(state: State<AppState>) -> Result<Vec<ChannelConnection>, String> {
    get_channel_connections_impl(&state)
}
pub fn get_channel_connections_impl(state: &AppState) -> Result<Vec<ChannelConnection>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let mut stmt=conn.prepare("SELECT id,channel_type,display_name,enabled,permission_summary,proactive_reason,proactive_daily_limit,credential_configured FROM channel_connections WHERE removed_at IS NULL ORDER BY updated_at DESC").map_err(|e|e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ChannelConnection {
                id: r.get(0)?,
                channel_type: r.get(1)?,
                display_name: r.get(2)?,
                enabled: r.get::<_, i64>(3)? != 0,
                permission_summary: r.get(4)?,
                proactive_reason: r.get(5)?,
                proactive_daily_limit: r.get(6)?,
                credential_configured: r.get::<_, i64>(7)? != 0,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_channel_credential(
    app: tauri::AppHandle,
    state: State<AppState>,
    id: String,
    credential: String,
) -> Result<(), String> {
    crate::keychain::write_channel_credential(&app, &id, &credential)?;
    let now = Utc::now().timestamp();
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute("UPDATE channel_connections SET credential_configured=1,updated_at=?2 WHERE id=?1 AND removed_at IS NULL",params![id,now]).map_err(|e|e.to_string())?;
    conn.execute("INSERT INTO channel_audit_log (id,connection_id,action,details,created_at) VALUES (?1,?2,'credential_saved','stored in OS keychain',?3)",params![Uuid::new_v4().to_string(),id,now]).map_err(|e|e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn clear_channel_credential(
    app: tauri::AppHandle,
    state: State<AppState>,
    id: String,
) -> Result<(), String> {
    crate::keychain::delete_channel_credential(&app, &id)?;
    let now = Utc::now().timestamp();
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE channel_connections SET credential_configured=0,updated_at=?2 WHERE id=?1",
        params![id, now],
    )
    .map_err(|e| e.to_string())?;
    conn.execute("INSERT INTO channel_audit_log (id,connection_id,action,details,created_at) VALUES (?1,?2,'credential_cleared','removed from OS keychain',?3)",params![Uuid::new_v4().to_string(),id,now]).map_err(|e|e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn confirm_channel_message(
    state: State<AppState>,
    id: String,
    message: String,
    confirmed: bool,
) -> Result<(), String> {
    confirm_channel_message_impl(&state, &id, &message, confirmed)
}
pub fn confirm_channel_message_impl(
    state: &AppState,
    id: &str,
    message: &str,
    confirmed: bool,
) -> Result<(), String> {
    if !confirmed {
        return Err("External channel delivery requires desktop confirmation".to_string());
    }
    let now = Utc::now().timestamp();
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let enabled: bool = conn
        .query_row(
            "SELECT enabled FROM channel_connections WHERE id=?1 AND removed_at IS NULL",
            params![id],
            |r| Ok(r.get::<_, i64>(0)? != 0),
        )
        .map_err(|e| e.to_string())?;
    if !enabled {
        return Err("Channel connection is not enabled".to_string());
    }
    conn.execute("INSERT INTO channel_audit_log (id,connection_id,action,details,created_at) VALUES (?1,?2,'message_confirmed',?3,?4)",params![Uuid::new_v4().to_string(),id,format!("{} characters approved for delivery",message.chars().count()),now]).map_err(|e|e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn set_channel_proactive_policy(
    state: State<AppState>,
    id: String,
    reason: String,
    daily_limit: i64,
) -> Result<(), String> {
    set_channel_proactive_policy_impl(&state, &id, &reason, daily_limit)
}
pub fn set_channel_proactive_policy_impl(
    state: &AppState,
    id: &str,
    reason: &str,
    daily_limit: i64,
) -> Result<(), String> {
    let limit = daily_limit.clamp(0, 24);
    let now = Utc::now().timestamp();
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute("UPDATE channel_connections SET proactive_reason=?2,proactive_daily_limit=?3,updated_at=?4 WHERE id=?1",params![id,reason,limit,now]).map_err(|e|e.to_string())?;
    conn.execute("INSERT INTO channel_audit_log (id,connection_id,action,details,created_at) VALUES (?1,?2,'proactive_policy_updated',?3,?4)",params![Uuid::new_v4().to_string(),id,format!("daily_limit={limit}; reason={reason}"),now]).map_err(|e|e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn set_channel_enabled(
    state: State<AppState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    set_channel_enabled_impl(&state, &id, enabled)
}
pub fn set_channel_enabled_impl(state: &AppState, id: &str, enabled: bool) -> Result<(), String> {
    let now = Utc::now().timestamp();
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE channel_connections SET enabled=?2,updated_at=?3 WHERE id=?1",
        params![id, enabled, now],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO channel_audit_log (id,connection_id,action,created_at) VALUES (?1,?2,?3,?4)",
        params![
            Uuid::new_v4().to_string(),
            id,
            if enabled { "enabled" } else { "disabled" },
            now
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn revoke_channel_connection(state: State<AppState>, id: String) -> Result<(), String> {
    revoke_channel_connection_impl(&state, &id)
}
pub fn revoke_channel_connection_impl(state: &AppState, id: &str) -> Result<(), String> {
    let now = Utc::now().timestamp();
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE channel_connections SET enabled=0,removed_at=?2,updated_at=?2 WHERE id=?1",
        params![id, now],
    )
    .map_err(|e| e.to_string())?;
    conn.execute("INSERT INTO channel_audit_log (id,connection_id,action,details,created_at) VALUES (?1,?2,'revoked','connection removed by user',?3)",params![Uuid::new_v4().to_string(),id,now]).map_err(|e|e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_channel_audit(
    state: State<AppState>,
    connection_id: String,
) -> Result<Vec<ChannelAuditEntry>, String> {
    get_channel_audit_impl(&state, &connection_id)
}
pub fn get_channel_audit_impl(
    state: &AppState,
    connection_id: &str,
) -> Result<Vec<ChannelAuditEntry>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let mut stmt=conn.prepare("SELECT id,action,details,created_at FROM channel_audit_log WHERE connection_id=?1 ORDER BY created_at DESC").map_err(|e|e.to_string())?;
    let rows = stmt
        .query_map(params![connection_id], |r| {
            Ok(ChannelAuditEntry {
                id: r.get(0)?,
                action: r.get(1)?,
                details: r.get(2)?,
                created_at: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

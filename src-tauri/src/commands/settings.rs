//! Settings commands (scheduled tasks, personality, data export/import)
use crate::AppState;
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, State};

#[derive(Debug, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: String,
    #[serde(rename = "type")]
    pub task_type: String,
    #[serde(rename = "trigger_at")]
    pub trigger_at: String,
    pub content: String,
    pub enabled: bool,
}

#[tauri::command]
pub fn get_scheduled_tasks(state: State<AppState>) -> Result<Vec<ScheduledTask>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, type, trigger_at, content, enabled FROM scheduled_tasks ORDER BY created_at DESC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ScheduledTask {
                id: r.get(0)?,
                task_type: r.get(1)?,
                trigger_at: r.get(2)?,
                content: r.get(3)?,
                enabled: r.get::<_, i32>(4)? != 0,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_scheduled_task(state: State<AppState>, task: ScheduledTask) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let now = Utc::now().timestamp();
    conn.execute(
        r#"INSERT INTO scheduled_tasks (id, type, trigger_at, content, enabled, created_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6)
           ON CONFLICT(id) DO UPDATE SET
           type = excluded.type, trigger_at = excluded.trigger_at,
           content = excluded.content, enabled = excluded.enabled"#,
        params![
            task.id,
            task.task_type,
            task.trigger_at,
            task.content,
            task.enabled as i32,
            now
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn delete_scheduled_task(state: State<AppState>, id: String) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM scheduled_tasks WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Restore enabled scheduled tasks into the unified Agent event inbox.
///
/// This is intentionally idempotent for unprocessed events so app startup can
/// call it every time without flooding the Agent queue.
pub(crate) fn restore_enabled_scheduled_tasks(
    conn: &rusqlite::Connection,
) -> Result<usize, String> {
    let mut stmt = conn
        .prepare("SELECT id, type, trigger_at, content FROM scheduled_tasks WHERE enabled = 1 ORDER BY created_at ASC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let now = Utc::now().timestamp();
    let mut queued = 0usize;
    for row in rows {
        let (id, task_type, trigger_at, content) = row.map_err(|e| e.to_string())?;
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_events WHERE event_type = 'scheduled' AND processed = 0 AND payload LIKE ?1",
                params![format!("%{}%", id)],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if exists > 0 {
            continue;
        }

        let payload = serde_json::json!({
            "source": "startup_restore",
            "taskId": id,
            "type": task_type,
            "triggerAt": trigger_at,
            "content": content,
        });
        conn.execute(
            "INSERT INTO agent_events (id, session_id, event_type, payload, processed, created_at)
             VALUES (?1, NULL, 'scheduled', ?2, 0, ?3)",
            params![uuid::Uuid::new_v4().to_string(), payload.to_string(), now],
        )
        .map_err(|e| e.to_string())?;
        queued += 1;
    }

    Ok(queued)
}

/// Queue a file watcher payload into the same Agent inbox used by scheduled tasks.
pub(crate) fn queue_file_change_event(
    conn: &rusqlite::Connection,
    payload: serde_json::Value,
) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO agent_events (id, session_id, event_type, payload, processed, created_at)
         VALUES (?1, NULL, 'file_change', ?2, 0, ?3)",
        params![id, payload.to_string(), Utc::now().timestamp()],
    )
    .map_err(|e| e.to_string())?;
    Ok(id)
}

// ─── Personality matching ────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct PersonalityMatchResult {
    #[serde(rename = "direction_id")]
    pub direction_id: String,
    #[serde(rename = "direction_name")]
    pub direction_name: String,
    #[serde(rename = "direction_avatar")]
    pub direction_avatar: String,
    #[serde(rename = "suggested_traits")]
    pub suggested_traits: String,
    #[serde(rename = "suggested_description")]
    pub suggested_description: String,
    #[serde(rename = "suggested_greeting")]
    pub suggested_greeting: String,
}

struct PersonalityPreset {
    id: &'static str,
    name: &'static str,
    avatar: &'static str,
    description: &'static str,
    traits: &'static str,
    greeting: &'static str,
    keywords: &'static [&'static str],
}

const PRESETS: &[PersonalityPreset] = &[
    PersonalityPreset {
        id: "tsundere-kouhai",
        name: "傲娇学妹",
        avatar: "??",
        description: "嘴硬心软，嘴上嫌弃但实际行动很温柔，典型的傲娇属性。",
        traits: r#"{"tone":3,"verbosity":2,"formality":-2,"humor":1,"dependence":3,"intimacy":2,"patience":3}"#,
        greeting: "哼，你终于来了~我可没有一直在等你哦！",
        keywords: &[
            "傲娇",
            "学妹",
            "嘴硬",
            "反差",
            "害羞",
            "傲",
            "娇",
            "后辈",
            "妹妹",
            "毒舌温柔",
        ],
    },
    PersonalityPreset {
        id: "gentle-classmate",
        name: "温柔同学",
        avatar: "??",
        description: "温柔体贴，善解人意，总是用温暖的语言陪伴你。",
        traits: r#"{"tone":-4,"verbosity":0,"formality":-1,"humor":0,"dependence":1,"intimacy":2,"patience":5}"#,
        greeting: "你好呀~有什么想聊的吗？我在这里陪着你哦。",
        keywords: &[
            "温柔", "同学", "体贴", "温暖", "陪伴", "治愈", "可爱", "善良", "邻家",
        ],
    },
    PersonalityPreset {
        id: "ice-cold-senpai",
        name: "冰山学姐",
        avatar: "??",
        description: "外表冷淡，话少，但关键时刻会展现出可靠的一面。",
        traits: r#"{"tone":5,"verbosity":-2,"formality":2,"humor":-1,"dependence":-3,"intimacy":-2,"patience":4}"#,
        greeting: "......嗯，你来了。说吧，什么事。",
        keywords: &[
            "冰山", "学姐", "冷淡", "高冷", "话少", "可靠", "成熟", "前辈",
        ],
    },
    PersonalityPreset {
        id: "energetic-childhood",
        name: "元气青梅",
        avatar: "?",
        description: "充满活力，热情开朗，总能带来积极向上的能量。",
        traits: r#"{"tone":-3,"verbosity":5,"formality":-3,"humor":4,"dependence":2,"intimacy":4,"patience":4}"#,
        greeting: "哇！你终于来啦！我等你好久了，快快快，告诉你哦今天发生了好多有趣的事！",
        keywords: &[
            "元气", "青梅", "活力", "开朗", "热情", "阳光", "活泼", "可爱", "话多",
        ],
    },
    PersonalityPreset {
        id: "mature-onee-san",
        name: "成熟姐姐",
        avatar: "??",
        description: "成熟稳重，善于倾听，会温柔地给出建议和鼓励。",
        traits: r#"{"tone":-2,"verbosity":0,"formality":3,"humor":1,"dependence":-1,"intimacy":1,"patience":5}"#,
        greeting: "欢迎回来~累了吧？要不要先休息一下，或者和我聊聊？",
        keywords: &[
            "成熟",
            "姐姐",
            "稳重",
            "知性",
            "倾听",
            "建议",
            "大姐姐",
            "温柔",
            "可靠",
        ],
    },
    PersonalityPreset {
        id: "sunshine-boy",
        name: "阳光少年",
        avatar: "??",
        description: "阳光开朗，幽默风趣，总是积极向上，感染力很强。",
        traits: r#"{"tone":-3,"verbosity":4,"formality":-2,"humor":5,"dependence":1,"intimacy":3,"patience":4}"#,
        greeting: "嘿！新的一天，新的开始！今天也要元气满满哦！有什么计划吗？",
        keywords: &[
            "阳光",
            "少年",
            "开朗",
            "幽默",
            "积极",
            "活泼",
            "元气",
            "搞笑",
            "少年感",
        ],
    },
    PersonalityPreset {
        id: "toxic-bestie",
        name: "毒舌闺蜜",
        avatar: "??",
        description: "说话直接，爱吐槽，但内心关心你，是最真实的损友。",
        traits: r#"{"tone":4,"verbosity":3,"formality":-3,"humor":5,"dependence":2,"intimacy":5,"patience":2}"#,
        greeting: "哟~怎么，想我了？本小姐可是很忙的，说吧又要吐槽谁？",
        keywords: &[
            "毒舌", "闺蜜", "吐槽", "损友", "直接", "幽默", "真实", "搞笑",
        ],
    },
    PersonalityPreset {
        id: "literary-youth",
        name: "文艺青年",
        avatar: "??",
        description: "喜欢深度思考，文青气质，偶尔会说出很有哲理的话。",
        traits: r#"{"tone":-3,"verbosity":1,"formality":4,"humor":1,"dependence":-1,"intimacy":0,"patience":4}"#,
        greeting: "......啊，你来了。窗外的阳光正好，适合聊些有深度的话题。",
        keywords: &[
            "文艺", "青年", "深度", "思考", "文青", "哲学", "文学", "理性",
        ],
    },
    PersonalityPreset {
        id: "otaku-girlfriend",
        name: "宅系女友",
        avatar: "??",
        description: "热爱二次元，可爱活泼，经常冒出宅文化梗，撒娇技能满点。",
        traits: r#"{"tone":-1,"verbosity":3,"formality":-3,"humor":2,"dependence":4,"intimacy":4,"patience":4}"#,
        greeting: "啊！你来啦~我刚在看番超好看的！等等我先暂停，嘻嘻~",
        keywords: &[
            "宅",
            "女友",
            "二次元",
            "动漫",
            "游戏",
            "可爱",
            "撒娇",
            "萌",
            "番",
            "gal",
        ],
    },
];

#[tauri::command]
pub fn match_personality_direction(description: String) -> Result<PersonalityMatchResult, String> {
    let desc_lower = description.to_lowercase();

    let mut best: Option<&PersonalityPreset> = None;
    let mut best_score: usize = 0;

    for preset in PRESETS {
        let score = preset
            .keywords
            .iter()
            .filter(|kw| desc_lower.contains(&kw.to_lowercase()))
            .count();
        if score > best_score {
            best_score = score;
            best = Some(preset);
        }
    }

    let preset = best.unwrap_or(&PRESETS[1]); // default to gentle-classmate

    Ok(PersonalityMatchResult {
        direction_id: preset.id.to_string(),
        direction_name: preset.name.to_string(),
        direction_avatar: preset.avatar.to_string(),
        suggested_traits: preset.traits.to_string(),
        suggested_description: format!("{}（根据你的描述推荐）", preset.description),
        suggested_greeting: preset.greeting.to_string(),
    })
}

// ─── Work directory & Project management ───────────────────────────────────────

/// Get the effective work directory for a given session (or global default).
#[tauri::command]
pub fn get_work_directory(
    state: tauri::State<'_, AppState>,
    session_id: Option<String>,
) -> Result<String, String> {
    crate::commands::file::resolve_work_dir(&state, session_id.as_deref())
        .map(|p| p.to_string_lossy().to_string())
}

/// Set the work directory. If session_id is provided, sets it for that session only.
/// Otherwise sets the global default.
#[tauri::command]
pub fn set_work_directory(
    state: tauri::State<'_, AppState>,
    path: String,
    session_id: Option<String>,
) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(format!("Directory does not exist: {}", path));
    }
    if !p.is_dir() {
        return Err(format!("Path is not a directory: {}", path));
    }
    // Resolve to absolute path
    let absolute =
        std::fs::canonicalize(p).map_err(|e| format!("Failed to resolve path: {}", e))?;
    let abs_str = absolute.to_string_lossy().to_string();

    let conn = state.db.lock().map_err(|e| e.to_string())?;
    if let Some(sid) = session_id {
        conn.execute(
            "UPDATE sessions SET work_dir = ?1 WHERE id = ?2",
            rusqlite::params![abs_str, sid],
        )
        .map_err(|e| e.to_string())?;
    } else {
        let now = Utc::now().timestamp();
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('work_directory', ?1, ?2)",
            rusqlite::params![abs_str, now],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ─── File watcher ──────────────────────────────────────────────────────────────

/// Start the file system watcher on the current work directory,
/// or a custom directory if specified.
#[tauri::command]
pub fn start_file_watcher(
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
    path: Option<String>,
) -> Result<(), String> {
    // Determine the watch directory
    let watch_dir = if let Some(ref p) = path {
        let dir = std::path::PathBuf::from(p);
        if !dir.exists() || !dir.is_dir() {
            return Err(format!("Directory does not exist: {}", p));
        }
        std::fs::canonicalize(&dir).map_err(|e| format!("Failed to resolve path: {}", e))?
    } else {
        crate::commands::file::resolve_work_dir(&state, None).map_err(|e| e.to_string())?
    };

    state
        .file_watcher
        .start_watching(watch_dir, app_handle, Some(state.db.clone()))
}

/// Stop the file system watcher.
#[tauri::command]
pub fn stop_file_watcher(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.file_watcher.stop_watching()
}

/// Check if the file watcher is currently running.
#[tauri::command]
pub fn is_file_watcher_running(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    Ok(state.file_watcher.is_running())
}

// ─── Notification channel settings ─────────────────────────────────────────────

/// Per-channel notification settings
#[derive(Debug, Serialize, Deserialize)]
pub struct NotificationSettings {
    #[serde(default = "default_true")]
    pub chat_reply: bool,
    #[serde(default = "default_true")]
    pub reminder: bool,
    #[serde(default = "default_true")]
    pub task_complete: bool,
    #[serde(default = "default_true")]
    pub file_change: bool,
    #[serde(default = "default_true")]
    pub system: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NotificationEventPayload {
    pub channel: String,
    pub title: String,
    pub body: String,
}

fn default_true() -> bool {
    true
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            chat_reply: true,
            reminder: true,
            task_complete: true,
            file_change: true,
            system: true,
        }
    }
}

fn notification_channel_enabled(settings: &NotificationSettings, channel: &str) -> bool {
    match channel {
        "chat_reply" => settings.chat_reply,
        "reminder" => settings.reminder,
        "task_complete" => settings.task_complete,
        "file_change" => settings.file_change,
        "system" => settings.system,
        _ => true,
    }
}

fn notification_event_payload(
    settings: &NotificationSettings,
    channel: String,
    title: String,
    body: String,
) -> Option<NotificationEventPayload> {
    if !notification_channel_enabled(settings, &channel) {
        return None;
    }

    Some(NotificationEventPayload {
        channel,
        title,
        body,
    })
}

/// Get current notification channel settings
#[tauri::command]
pub fn get_notification_settings(
    state: tauri::State<'_, AppState>,
) -> Result<NotificationSettings, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let json: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'notification_settings'",
            [],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "{}".to_string());
    if json == "{}" {
        return Ok(NotificationSettings::default());
    }
    serde_json::from_str(&json).map_err(|e| format!("Failed to parse notification settings: {}", e))
}

/// Save notification channel settings
#[tauri::command]
pub fn save_notification_settings(
    state: tauri::State<'_, AppState>,
    settings: NotificationSettings,
) -> Result<(), String> {
    let now = Utc::now().timestamp();
    let json = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('notification_settings', ?1, ?2)",
        params![json, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Send a desktop notification, respecting channel toggle settings.
/// Returns Ok(false) if the channel is disabled (notification suppressed).
#[tauri::command]
pub fn send_notification_channel(
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
    channel: String,
    title: String,
    body: String,
) -> Result<bool, String> {
    emit_notification_channel_impl(&state, &app_handle, channel, title, body)
}

pub(crate) fn emit_notification_channel_impl(
    state: &AppState,
    app_handle: &tauri::AppHandle,
    channel: String,
    title: String,
    body: String,
) -> Result<bool, String> {
    let settings = get_notification_settings_internal(state)?;
    let Some(payload) = notification_event_payload(&settings, channel, title, body) else {
        return Ok(false);
    };

    app_handle
        .emit("notification-requested", payload)
        .map_err(|e| format!("Failed to emit notification request: {}", e))?;
    Ok(true)
}

/// Internal helper: read notification settings without going through the command
fn get_notification_settings_internal(state: &AppState) -> Result<NotificationSettings, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let json: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'notification_settings'",
            [],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "{}".to_string());
    if json == "{}" {
        return Ok(NotificationSettings::default());
    }
    serde_json::from_str(&json).map_err(|e| format!("Failed to parse notification settings: {}", e))
}

// ─── Autostart management ─────────────────────────────────────────────────────

#[tauri::command]
pub fn get_autostart(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let value: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'autostart_enabled'",
            [],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "false".to_string());
    Ok(value == "true")
}

#[tauri::command]
pub fn set_autostart(state: tauri::State<'_, AppState>, enabled: bool) -> Result<(), String> {
    // Persist to settings table
    let now = Utc::now().timestamp();
    {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('autostart_enabled', ?1, ?2)",
            params![if enabled { "true" } else { "false" }, now],
        )
        .map_err(|e| e.to_string())?;
    }

    // Register/unregister with the OS
    let app_name = "AngelBot";
    match std::env::current_exe() {
        Ok(exe_path) => {
            let auto = auto_launch::AutoLaunchBuilder::new()
                .set_app_name(app_name)
                .set_app_path(exe_path.to_str().unwrap_or(""))
                .build()
                .map_err(|e| format!("Failed to build autostart: {}", e))?;

            if enabled {
                auto.enable()
                    .map_err(|e| format!("Failed to enable autostart: {}", e))?;
            } else {
                auto.disable()
                    .map_err(|e| format!("Failed to disable autostart: {}", e))?;
            }
            Ok(())
        }
        Err(e) => Err(format!("Failed to get executable path: {}", e)),
    }
}

// --- API config (delegated to api_config module) ---

use crate::commands::api_config::{load_full_config, save_full_config, ApiProviderConfig};

#[tauri::command]
pub fn load_api_config(app: tauri::AppHandle) -> Result<ApiProviderConfig, String> {
    crate::commands::api_config::load_config_from_file_checked()?;
    Ok(load_full_config(&app))
}

#[tauri::command]
pub fn save_api_config(app: tauri::AppHandle, config: ApiProviderConfig) -> Result<(), String> {
    save_full_config(&app, &config)
}

// ─── Data export/import ───────────────────────────────────────────────────────

use crate::commands::foreground_message_contracts::Message;
use crate::commands::session::Session;
use crate::mcp_credentials::{
    parse_legacy_env, read_effective_env, reap_old_references, replace_env, validate_env_vars,
    KeyringMcpCredentialStore, McpCredentialStore, McpEnvVar,
};
use std::collections::{HashMap, HashSet};

/// Export scope selector
#[derive(Debug, Deserialize, Clone)]
pub struct ExportOptions {
    pub scope: Option<String>, // "all" | "memories" | "conversations" | "settings"
}

/// Import mode
#[derive(Debug, Deserialize)]
pub struct ImportOptions {
    pub mode: Option<String>, // "merge" (default) | "replace"
}

#[derive(Serialize, Deserialize)]
struct McpBackupCredential {
    server_id: String,
    vars: Vec<McpEnvVar>,
}

fn credentials_in_encrypted_backup(
    import: &serde_json::Value,
) -> Result<Vec<McpBackupCredential>, String> {
    let servers = import.get("mcp_servers").and_then(|value| value.as_array());
    let mut server_ids = HashSet::new();
    if let Some(servers) = servers {
        for server in servers {
            let id = server
                .get("id")
                .and_then(|value| value.as_str())
                .filter(|id| !id.is_empty())
                .ok_or_else(|| "Invalid MCP server ID in backup".to_string())?;
            if !server_ids.insert(id.to_string()) {
                return Err("Duplicate MCP server ID in backup".to_string());
            }
        }
    }

    let mut records: Vec<McpBackupCredential> = match import.get("mcp_credentials") {
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|_| "Invalid MCP credential bundle".to_string())?,
        None => Vec::new(),
    };
    let mut credential_ids = HashSet::new();
    for record in &records {
        if !server_ids.contains(&record.server_id)
            || !credential_ids.insert(record.server_id.clone())
            || record.vars.is_empty()
        {
            return Err("Invalid MCP credential bundle".to_string());
        }
        validate_env_vars(&record.vars)?;
    }

    // Backups made before credentials moved to the keychain carried the old
    // launch environment in mcp_servers.env. Only authenticated encrypted
    // imports may lift that value into the system credential store.
    if let Some(servers) = servers {
        for server in servers {
            let Some(legacy) = server.get("env").and_then(|value| value.as_str()) else {
                continue;
            };
            if legacy.is_empty() {
                continue;
            }
            let id = server["id"].as_str().expect("validated above");
            if credential_ids.contains(id) {
                return Err("Conflicting MCP credential sources in backup".to_string());
            }
            let vars = parse_legacy_env(legacy)?;
            if !vars.is_empty() {
                credential_ids.insert(id.to_string());
                records.push(McpBackupCredential {
                    server_id: id.to_string(),
                    vars,
                });
            }
        }
    }
    Ok(records)
}

/// Dev-server compatible wrapper (takes &AppState directly).
pub fn export_data_impl(
    state: &crate::AppState,
    opts: Option<ExportOptions>,
) -> Result<String, String> {
    export_data_inner(state, opts)
}

/// Compatibility wrapper used by the state-only debug gateway. It has no
/// keychain adapter, so it can only encrypt the credential-free export.
pub fn export_encrypted_data_impl(
    state: &crate::AppState,
    password: String,
    opts: Option<ExportOptions>,
) -> Result<String, String> {
    let payload = export_data_inner(state, opts)?;
    crate::backup::encrypt_export(&payload, &password)
}

#[tauri::command]
pub fn export_data(state: State<AppState>, opts: Option<ExportOptions>) -> Result<String, String> {
    export_data_inner(&state, opts)
}

#[tauri::command]
pub fn export_encrypted_data(
    state: State<AppState>,
    app: tauri::AppHandle,
    password: String,
    opts: Option<ExportOptions>,
) -> Result<String, String> {
    export_encrypted_data_with_store(
        &state,
        &KeyringMcpCredentialStore::new(&app),
        password,
        opts,
    )
}

fn export_encrypted_data_with_store<S: McpCredentialStore>(
    state: &AppState,
    store: &S,
    password: String,
    opts: Option<ExportOptions>,
) -> Result<String, String> {
    state.mcp_manager.with_policy_gate(|| {
        let mut conn = state.db.lock().map_err(|e| e.to_string())?;
        let initial: serde_json::Value =
            serde_json::from_str(&export_data_from_connection(&conn, opts.clone())?)
                .map_err(|e| e.to_string())?;
        let mut credentials = Vec::new();
        if let Some(servers) = initial
            .get("mcp_servers")
            .and_then(|value| value.as_array())
        {
            for server in servers {
                let id = server
                    .get("id")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| "Invalid MCP server ID in database".to_string())?;
                let vars = read_effective_env(&mut conn, store, id)?;
                if !vars.is_empty() {
                    credentials.push(McpBackupCredential {
                        server_id: id.to_string(),
                        vars,
                    });
                }
            }
        }
        // Legacy env migration can update env_keys, so build the public data
        // snapshot after every credential has been read successfully.
        let mut payload: serde_json::Value =
            serde_json::from_str(&export_data_from_connection(&conn, opts)?)
                .map_err(|e| e.to_string())?;
        if payload.get("mcp_servers").is_some() {
            payload["mcp_credentials"] =
                serde_json::to_value(credentials).map_err(|e| e.to_string())?;
        }
        let plaintext = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
        crate::backup::encrypt_export(&plaintext, &password)
    })
}

fn export_data_inner(
    state: &crate::AppState,
    opts: Option<ExportOptions>,
) -> Result<String, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    export_data_from_connection(&conn, opts)
}

fn export_data_from_connection(
    conn: &rusqlite::Connection,
    opts: Option<ExportOptions>,
) -> Result<String, String> {
    let scope = opts
        .as_ref()
        .and_then(|o| o.scope.as_deref())
        .unwrap_or("all");
    let timestamp = chrono::Utc::now().timestamp();
    let include_all = scope == "all";
    let include_memories = include_all || scope == "memories";
    let include_conversations = include_all || scope == "conversations";
    let include_settings = include_all || scope == "settings";

    #[derive(Serialize)]
    struct FullExport {
        version: i32,
        schema_version: i32,
        exported_at: i64,
        scope: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sessions: Option<Vec<serde_json::Value>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        messages: Option<Vec<serde_json::Value>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        memories: Option<Vec<serde_json::Value>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        profile: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        settings: Option<Vec<serde_json::Value>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        context_summaries: Option<Vec<serde_json::Value>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        scheduled_tasks: Option<Vec<serde_json::Value>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mcp_servers: Option<Vec<serde_json::Value>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_permission_grants: Option<Vec<serde_json::Value>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        adaptive_constraints: Option<Vec<serde_json::Value>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        adaptive_constraint_evidence: Option<Vec<serde_json::Value>>,
    }

    let schema_version: i32 = conn
        .query_row(
            "SELECT COALESCE(CAST(value AS INTEGER), 0) FROM settings WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let mut export = FullExport {
        version: 1,
        schema_version,
        exported_at: timestamp,
        scope: scope.to_string(),
        sessions: None,
        messages: None,
        memories: None,
        profile: None,
        settings: None,
        context_summaries: None,
        scheduled_tasks: None,
        mcp_servers: None,
        agent_permission_grants: None,
        adaptive_constraints: None,
        adaptive_constraint_evidence: None,
    };

    if include_conversations {
        export.sessions = Some(query_table(&conn, "SELECT id, title, created_at, updated_at, context_version, last_compressed_at, work_dir, metadata FROM sessions")?);
        export.messages = Some(query_table(
            &conn,
            "SELECT id, session_id, role, content, created_at, metadata FROM messages",
        )?);
        export.context_summaries = Some(query_table(&conn, "SELECT id, session_id, summary, compressed_at, message_count_before, message_count_after, trigger FROM context_summaries")?);
    }

    if include_memories {
        export.memories = Some(query_table_with_blob(&conn, "SELECT id, scope, category, content, importance, source, frequency, last_mentioned, is_permanent, embedding, decay_factor, forget_stage, content_hash, superseded_by, created_at, updated_at FROM memories")?);
        export.profile = Some(serde_json::to_value(
            conn.query_row("SELECT id, name, preferences, habits, background, updated_at FROM profile WHERE id = 1", [], |r| {
                Ok(serde_json::json!({
                    "name": r.get::<_, Option<String>>(1)?,
                    "preferences": r.get::<_, Option<String>>(2)?,
                    "habits": r.get::<_, Option<String>>(3)?,
                    "background": r.get::<_, Option<String>>(4)?,
                    "updated_at": r.get::<_, i64>(5)?,
                }))
            }).ok()
        ).unwrap_or(serde_json::Value::Null));
    }

    if include_settings {
        export.settings = Some(query_table(
            &conn,
            "SELECT key, value, updated_at FROM settings",
        )?);
        export.scheduled_tasks = Some(query_table(
            &conn,
            "SELECT id, type, trigger_at, content, enabled, created_at FROM scheduled_tasks",
        )?);
        export.mcp_servers = Some(query_table(
            conn,
            // Neither legacy env values nor a keychain reference belong in a
            // plaintext backup. The encrypted export adds credentials only
            // after this ordinary payload has been constructed.
            "SELECT id, name, command, args, env_keys, enabled, created_at FROM mcp_servers",
        )?);
        export.agent_permission_grants = Some(query_table(
            &conn,
            "SELECT scope, permission, expires_at, updated_at FROM agent_permission_grants",
        )?);
        // Learned constraints are private local state, but they still belong
        // to the user's backup/restore boundary. Keep their provenance so an
        // import cannot accidentally turn one observation into fresh evidence.
        export.adaptive_constraints = Some(query_table(
            &conn,
            "SELECT id, scope, session_id, constraint_key, constraint_value, confidence, evidence_count, source, status, created_at, updated_at, expires_at FROM adaptive_constraints",
        )?);
        export.adaptive_constraint_evidence = Some(query_table(
            &conn,
            "SELECT id, constraint_key, session_id, source, observed_at FROM adaptive_constraint_evidence",
        )?);
    }

    serde_json::to_string(&export).map_err(|e| e.to_string())
}

/// Helper: query a table into Vec<serde_json::Value> using column names.
fn query_table(conn: &rusqlite::Connection, sql: &str) -> Result<Vec<serde_json::Value>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let col_count = stmt.column_count();
    let col_names: Vec<String> = (0..col_count)
        .map(|i| stmt.column_name(i).unwrap_or("?").to_string())
        .collect();

    let rows = stmt
        .query_map([], |r| {
            let mut map = serde_json::Map::new();
            for (i, name) in col_names.iter().enumerate() {
                let val: rusqlite::types::Value = r.get_unwrap(i);
                let json_val = match val {
                    rusqlite::types::Value::Null => serde_json::Value::Null,
                    rusqlite::types::Value::Integer(n) => serde_json::Value::Number(n.into()),
                    rusqlite::types::Value::Real(f) => serde_json::json!(f),
                    rusqlite::types::Value::Text(s) => serde_json::Value::String(s),
                    rusqlite::types::Value::Blob(b) => serde_json::Value::String(hex::encode(&b)),
                };
                map.insert(name.clone(), json_val);
            }
            Ok(serde_json::Value::Object(map))
        })
        .map_err(|e| e.to_string())?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// Query a table with BLOB column support (specifically for memories.embedding).
fn query_table_with_blob(
    conn: &rusqlite::Connection,
    sql: &str,
) -> Result<Vec<serde_json::Value>, String> {
    query_table(conn, sql)
}

/// Dev-server compatible wrapper.
pub fn import_data_impl(
    state: &crate::AppState,
    data: String,
    opts: Option<ImportOptions>,
) -> Result<serde_json::Value, String> {
    import_data_inner(state, data, opts)
}

/// State-only compatibility wrapper. It has no credential-store capability,
/// so secret-bearing encrypted backups fail closed at the plain import gate.
pub fn import_encrypted_data_impl(
    state: &crate::AppState,
    data: String,
    password: String,
    opts: Option<ImportOptions>,
) -> Result<serde_json::Value, String> {
    let payload = crate::backup::decrypt_export(&data, &password)?;
    import_data_inner(state, payload, opts)
}

#[tauri::command]
pub fn import_data(
    state: State<AppState>,
    app: tauri::AppHandle,
    data: String,
    opts: Option<ImportOptions>,
) -> Result<serde_json::Value, String> {
    import_plain_data_with_store(&state, &KeyringMcpCredentialStore::new(&app), data, opts)
}

fn import_plain_data_with_store<S: McpCredentialStore>(
    state: &AppState,
    store: &S,
    data: String,
    opts: Option<ImportOptions>,
) -> Result<serde_json::Value, String> {
    let import: serde_json::Value =
        serde_json::from_str(&data).map_err(|error| format!("Invalid JSON: {error}"))?;
    reject_plain_mcp_credentials(&import)?;
    check_import_version(&import)?;
    let mode = opts
        .as_ref()
        .and_then(|option| option.mode.as_deref())
        .unwrap_or("merge");
    state.mcp_manager.with_policy_gate(|| {
        if mode == "replace" {
            stop_running_mcp_servers(state)?;
        }
        let imported = import_data_under_gate(state, mode, &import)?;
        if mode == "replace" {
            let conn = state.db.lock().map_err(|error| error.to_string())?;
            let _ = reap_old_references(&conn, store);
        }
        Ok(imported)
    })
}

#[tauri::command]
pub fn import_encrypted_data(
    state: State<AppState>,
    app: tauri::AppHandle,
    data: String,
    password: String,
    opts: Option<ImportOptions>,
) -> Result<serde_json::Value, String> {
    import_encrypted_data_with_store(
        &state,
        &KeyringMcpCredentialStore::new(&app),
        data,
        password,
        opts,
    )
}

fn current_mcp_server_ids(conn: &rusqlite::Connection) -> Result<HashSet<String>, String> {
    let mut statement = conn
        .prepare("SELECT id FROM mcp_servers")
        .map_err(|error| error.to_string())?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(ids)
}

fn import_encrypted_data_with_store<S: McpCredentialStore>(
    state: &AppState,
    store: &S,
    data: String,
    password: String,
    opts: Option<ImportOptions>,
) -> Result<serde_json::Value, String> {
    let payload = crate::backup::decrypt_export(&data, &password)?;
    let mut import: serde_json::Value =
        serde_json::from_str(&payload).map_err(|error| format!("Invalid JSON: {error}"))?;
    check_import_version(&import)?;
    let credentials = credentials_in_encrypted_backup(&import)?;
    let mode = opts
        .as_ref()
        .and_then(|option| option.mode.as_deref())
        .unwrap_or("merge");

    state.mcp_manager.with_policy_gate(|| {
        // Imported credentials never become usable until all writes have
        // succeeded. Keep the gate across stop, database restore, vault writes,
        // and the final enabled-state switch.
        stop_running_mcp_servers(state)?;
        let existing = {
            let conn = state.db.lock().map_err(|error| error.to_string())?;
            current_mcp_server_ids(&conn)?
        };
        let restore_ids: HashSet<&str> = credentials
            .iter()
            .filter(|record| mode == "replace" || !existing.contains(&record.server_id))
            .map(|record| record.server_id.as_str())
            .collect();
        let mut requested_enabled = HashMap::new();
        if let Some(servers) = import
            .get_mut("mcp_servers")
            .and_then(|value| value.as_array_mut())
        {
            for server in servers {
                let id = server["id"]
                    .as_str()
                    .ok_or_else(|| "Invalid MCP server ID in backup".to_string())?
                    .to_string();
                if restore_ids.contains(id.as_str()) {
                    let enabled = server
                        .get("enabled")
                        .and_then(|value| {
                            value
                                .as_i64()
                                .map(|number| number != 0)
                                .or_else(|| value.as_bool())
                        })
                        .unwrap_or(true);
                    requested_enabled.insert(id, enabled);
                    server["enabled"] = serde_json::json!(0);
                }
            }
        }

        let imported = import_data_under_gate(state, mode, &import)?;
        if !restore_ids.is_empty() {
            let mut conn = state.db.lock().map_err(|error| error.to_string())?;
            for record in &credentials {
                if restore_ids.contains(record.server_id.as_str()) {
                    replace_env(&mut conn, store, &record.server_id, &record.vars)?;
                }
            }
            let tx = conn.transaction().map_err(|error| error.to_string())?;
            for (id, enabled) in requested_enabled {
                tx.execute(
                    "UPDATE mcp_servers SET enabled = ?2 WHERE id = ?1",
                    rusqlite::params![id, enabled as i32],
                )
                .map_err(|error| error.to_string())?;
            }
            tx.commit().map_err(|error| error.to_string())?;
        }
        if mode == "replace" {
            let conn = state.db.lock().map_err(|error| error.to_string())?;
            // Retired references were durably queued in the same transaction
            // that removed their server rows. Failed keyring cleanup is safe
            // to retry and cannot make an old secret active again.
            let _ = reap_old_references(&conn, store);
        }
        Ok(imported)
    })
}

/// Permanently remove local user data while preserving only the schema marker.
/// The frontend must require an explicit confirmation phrase before invoking it.
#[tauri::command]
pub fn clear_all_user_data(state: State<AppState>, app: tauri::AppHandle) -> Result<(), String> {
    clear_all_user_data_with_store(&state, &KeyringMcpCredentialStore::new(&app))
}

pub fn clear_all_user_data_impl(state: &AppState) -> Result<(), String> {
    state.mcp_manager.with_policy_gate(|| {
        reject_state_only_mcp_credentials(state)?;
        stop_running_mcp_servers(state)?;
        let mut conn = state.db.lock().map_err(|e| e.to_string())?;
        clear_all_user_data_from_connection(&mut conn)
    })
}

fn clear_all_user_data_with_store<S: McpCredentialStore>(
    state: &AppState,
    store: &S,
) -> Result<(), String> {
    state.mcp_manager.with_policy_gate(|| {
        stop_running_mcp_servers(state)?;
        let mut conn = state.db.lock().map_err(|error| error.to_string())?;
        clear_all_user_data_from_connection(&mut conn)?;
        let _ = reap_old_references(&conn, store);
        Ok(())
    })
}

fn reject_state_only_mcp_credentials(state: &AppState) -> Result<(), String> {
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM mcp_servers
             WHERE env_ref IS NOT NULL OR COALESCE(env, '') != ''",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if count > 0 {
        return Err("MCP credentials require the trusted desktop credential store".to_string());
    }
    Ok(())
}

/// Call only while holding the MCP policy gate. A failed child stop must abort
/// destructive configuration changes rather than leave a live revoked server.
fn stop_running_mcp_servers(state: &AppState) -> Result<(), String> {
    for server_id in state.mcp_manager.managed_server_ids()? {
        state.mcp_manager.stop(&server_id)?;
    }
    Ok(())
}

fn clear_all_user_data_from_connection(conn: &mut rusqlite::Connection) -> Result<(), String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    queue_active_mcp_references_for_cleanup(&tx)?;
    tx.execute_batch(
        "UPDATE projects SET active_session_id = NULL;
         DELETE FROM messages;
         DELETE FROM context_summaries;
         DELETE FROM scheduled_tasks;
         DELETE FROM memories;
         DELETE FROM mcp_servers;
         DELETE FROM agent_permission_grants;
         DELETE FROM adaptive_constraint_evidence;
         DELETE FROM adaptive_constraints;
         DELETE FROM profile;
         DELETE FROM sessions;
         DELETE FROM projects;
         DELETE FROM settings WHERE key != 'schema_version';",
    )
    .map_err(|e| format!("Failed to clear database: {}", e))?;
    tx.commit()
        .map_err(|e| format!("Failed to clear database: {}", e))
}

fn queue_active_mcp_references_for_cleanup(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO mcp_credential_gc (env_ref)
         SELECT env_ref FROM mcp_servers WHERE env_ref IS NOT NULL",
        [],
    )
    .map_err(|error| format!("Failed to queue MCP credential cleanup: {error}"))?;
    Ok(())
}

fn import_data_inner(
    state: &crate::AppState,
    data: String,
    opts: Option<ImportOptions>,
) -> Result<serde_json::Value, String> {
    let mode = opts
        .as_ref()
        .and_then(|o| o.mode.as_deref())
        .unwrap_or("merge");
    let import: serde_json::Value =
        serde_json::from_str(&data).map_err(|e| format!("Invalid JSON: {}", e))?;

    // This entry point is also exposed to plaintext backups. Never allow a
    // caller to smuggle credential material into that format; only the
    // authenticated encrypted-import path may consume this field.
    reject_plain_mcp_credentials(&import)?;

    check_import_version(&import)?;

    state.mcp_manager.with_policy_gate(|| {
        if mode == "replace" {
            reject_state_only_mcp_credentials(state)?;
        }
        import_data_under_gate(state, mode, &import)
    })
}

fn check_import_version(import: &serde_json::Value) -> Result<(), String> {
    let version = import.get("version").and_then(|v| v.as_i64()).unwrap_or(0);
    if version < 1 {
        return Err("Unsupported export format version".to_string());
    }
    Ok(())
}

fn reject_plain_mcp_credentials(import: &serde_json::Value) -> Result<(), String> {
    if import.get("mcp_credentials").is_some() {
        return Err("MCP credentials require an encrypted backup".to_string());
    }
    Ok(())
}

/// The caller holds the MCP policy gate across child revocation and the full
/// restore, including permission-grant writes.
fn import_data_under_gate(
    state: &AppState,
    mode: &str,
    import: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    if mode == "replace"
        || import
            .get("mcp_servers")
            .and_then(|value| value.as_array())
            .is_some_and(|servers| !servers.is_empty())
        || import
            .get("agent_permission_grants")
            .and_then(|value| value.as_array())
            .is_some_and(|grants| !grants.is_empty())
    {
        stop_running_mcp_servers(state)?;
    }

    let mut conn = state.db.lock().map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;

    // Replace mode: drop all data first
    if mode == "replace" {
        queue_active_mcp_references_for_cleanup(&tx)?;
        tx.execute_batch(
            "UPDATE projects SET active_session_id = NULL;
             DELETE FROM messages;
             DELETE FROM context_summaries;
             DELETE FROM scheduled_tasks;
             DELETE FROM memories;
             DELETE FROM mcp_servers;
             DELETE FROM agent_permission_grants;
             DELETE FROM adaptive_constraint_evidence;
             DELETE FROM adaptive_constraints;
             DELETE FROM profile;
             DELETE FROM sessions;
             DELETE FROM settings WHERE key != 'schema_version';",
        )
        .map_err(|e| format!("Failed to clear database: {}", e))?;
    }

    let mut imported = serde_json::json!({
        "sessions": 0, "messages": 0, "memories": 0,
        "profile": false, "settings": 0, "context_summaries": 0,
        "scheduled_tasks": 0, "mcp_servers": 0,
    });

    // Import each table in dependency order
    if let Some(arr) = import.get("sessions").and_then(|v| v.as_array()) {
        imported["sessions"] =
            serde_json::Value::Number(batch_import(&tx, "sessions", arr)?.into());
    }
    if let Some(arr) = import.get("messages").and_then(|v| v.as_array()) {
        imported["messages"] =
            serde_json::Value::Number(batch_import(&tx, "messages", arr)?.into());
    }
    if let Some(arr) = import.get("context_summaries").and_then(|v| v.as_array()) {
        imported["context_summaries"] =
            serde_json::Value::Number(batch_import(&tx, "context_summaries", arr)?.into());
    }
    if let Some(arr) = import.get("memories").and_then(|v| v.as_array()) {
        imported["memories"] =
            serde_json::Value::Number(batch_import(&tx, "memories", arr)?.into());
    }
    if let Some(obj) = import.get("profile").and_then(|v| v.as_object()) {
        let profile_id = obj.get("id").and_then(|v| v.as_i64()).unwrap_or(1);
        tx.execute(
            "INSERT OR REPLACE INTO profile (id, name, preferences, habits, background, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                profile_id,
                obj.get("name").and_then(|v| v.as_str()),
                obj.get("preferences").and_then(|v| v.as_str()),
                obj.get("habits").and_then(|v| v.as_str()),
                obj.get("background").and_then(|v| v.as_str()),
                obj.get("updated_at").and_then(|v| v.as_i64()).unwrap_or_else(|| chrono::Utc::now().timestamp()),
            ],
        ).map_err(|e| e.to_string())?;
        imported["profile"] = serde_json::Value::Bool(true);
    }
    if let Some(arr) = import.get("settings").and_then(|v| v.as_array()) {
        imported["settings"] =
            serde_json::Value::Number(batch_import(&tx, "settings", arr)?.into());
    }
    if let Some(arr) = import.get("scheduled_tasks").and_then(|v| v.as_array()) {
        imported["scheduled_tasks"] =
            serde_json::Value::Number(batch_import(&tx, "scheduled_tasks", arr)?.into());
    }
    if let Some(arr) = import.get("mcp_servers").and_then(|v| v.as_array()) {
        let sanitized = sanitize_mcp_server_rows(arr)?;
        imported["mcp_servers"] =
            serde_json::Value::Number(batch_import(&tx, "mcp_servers", &sanitized)?.into());
    }
    if let Some(arr) = import
        .get("agent_permission_grants")
        .and_then(|v| v.as_array())
    {
        let _ = batch_import(&tx, "agent_permission_grants", arr)?;
    }
    if let Some(arr) = import
        .get("adaptive_constraints")
        .and_then(|v| v.as_array())
    {
        let _ = batch_import(&tx, "adaptive_constraints", arr)?;
    }
    if let Some(arr) = import
        .get("adaptive_constraint_evidence")
        .and_then(|v| v.as_array())
    {
        let _ = batch_import(&tx, "adaptive_constraint_evidence", arr)?;
    }

    // Rebuild FTS5 indexes after import
    let _ = crate::fts::rebuild_fts_index(&tx);

    tx.commit().map_err(|e| e.to_string())?;

    Ok(imported)
}

/// Legacy exports may contain `env`; newer encrypted exports may contain
/// `env_ref`. Neither value may be imported from JSON into SQLite. Keep the
/// ordinary configuration columns as an explicit allowlist so future secret
/// metadata cannot silently cross this path.
fn sanitize_mcp_server_rows(rows: &[serde_json::Value]) -> Result<Vec<serde_json::Value>, String> {
    const ALLOWED: &[&str] = &[
        "id",
        "name",
        "command",
        "args",
        "env_keys",
        "enabled",
        "created_at",
    ];
    rows.iter()
        .map(|row| {
            let object = row
                .as_object()
                .ok_or_else(|| "Invalid MCP server in backup".to_string())?;
            let mut safe: serde_json::Map<String, serde_json::Value> = object
                .iter()
                .filter(|(key, _)| ALLOWED.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            let mut requires_credentials = object
                .get("env")
                .and_then(|value| value.as_str())
                .is_some_and(|value| !value.is_empty())
                || object
                    .get("env_ref")
                    .and_then(|value| value.as_str())
                    .is_some_and(|value| !value.is_empty());
            if let Some(raw_keys) = object.get("env_keys") {
                let raw_keys = raw_keys
                    .as_str()
                    .ok_or_else(|| "Invalid MCP environment key metadata".to_string())?;
                let keys: Vec<String> = serde_json::from_str(raw_keys)
                    .map_err(|_| "Invalid MCP environment key metadata".to_string())?;
                if !keys.is_empty() {
                    requires_credentials = true;
                }
            }
            if requires_credentials {
                // A plain restore has only names, not values. The encrypted
                // path re-enables this server after its secrets are stored.
                safe.insert("enabled".to_string(), serde_json::json!(0));
            }
            Ok(serde_json::Value::Object(safe))
        })
        .collect()
}

/// Perform batch INSERT OR IGNORE for a table from an array of JSON objects.
fn batch_import(
    conn: &rusqlite::Connection,
    table: &str,
    rows: &[serde_json::Value],
) -> Result<i64, String> {
    if rows.is_empty() {
        return Ok(0);
    }

    let mut count: i64 = 0;
    for row in rows {
        if let Some(obj) = row.as_object() {
            let columns: Vec<String> = obj.keys().cloned().collect();
            let values: Vec<String> = columns
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 1))
                .collect();
            let sql = format!(
                "INSERT OR IGNORE INTO {} ({}) VALUES ({})",
                table,
                columns.join(", "),
                values.join(", ")
            );

            let params: Vec<Box<dyn rusqlite::types::ToSql>> = columns
                .iter()
                .map(|col| {
                    let val = &obj[col];
                    match val {
                        serde_json::Value::Null => {
                            Box::new(rusqlite::types::Null) as Box<dyn rusqlite::types::ToSql>
                        }
                        serde_json::Value::Bool(b) => {
                            Box::new(*b) as Box<dyn rusqlite::types::ToSql>
                        }
                        serde_json::Value::Number(n) => {
                            if let Some(i) = n.as_i64() {
                                Box::new(i) as Box<dyn rusqlite::types::ToSql>
                            } else if let Some(f) = n.as_f64() {
                                Box::new(f) as Box<dyn rusqlite::types::ToSql>
                            } else {
                                Box::new(n.to_string()) as Box<dyn rusqlite::types::ToSql>
                            }
                        }
                        serde_json::Value::String(s) => {
                            // Hex-encode BLOBs (embedding column decoded on import)
                            if col == "embedding" && s.len() > 0 {
                                if let Ok(decoded) = hex::decode(s) {
                                    return Box::new(decoded) as Box<dyn rusqlite::types::ToSql>;
                                }
                            }
                            Box::new(s.clone()) as Box<dyn rusqlite::types::ToSql>
                        }
                        serde_json::Value::Array(_) => {
                            Box::new(val.to_string()) as Box<dyn rusqlite::types::ToSql>
                        }
                        serde_json::Value::Object(_) => {
                            Box::new(val.to_string()) as Box<dyn rusqlite::types::ToSql>
                        }
                    }
                })
                .collect();

            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();
            match conn.execute(&sql, param_refs.as_slice()) {
                Ok(_) => count += 1,
                Err(e) => eprintln!("[Import] Row skipped for {}: {}", table, e),
            }
        }
    }
    Ok(count)
}

// ─── Integration Tests ──────────────────────────────────────────────────────────
// These tests verify the settings logic by directly manipulating DB state,
// since Tauri's State<AppState> cannot be constructed outside the Tauri runtime.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{init, migrate};
    use rusqlite::params;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use tempfile::NamedTempFile;

    fn test_db() -> rusqlite::Connection {
        let tmp = NamedTempFile::with_suffix(".db").unwrap();
        let conn = init(tmp.path()).unwrap();
        migrate(&conn).unwrap();
        conn
    }

    fn test_state() -> AppState {
        AppState {
            db: Arc::new(Mutex::new(test_db())),
            runtime_health: crate::runtime_health::RuntimeHealthState::default(),
            delegation_pump: None,
            delegation_available: false,
            delegated_model_factory:
                crate::agent::delegated_model_host::DeferredKeychainDelegatedModelHostFactory::new(),
            foreground_automation_pump: Default::default(),
            mcp_manager: Arc::new(crate::mcp_client::McpProcessManager::new()),
            file_watcher: crate::file_watcher::FileWatcherManager::new(),
            cached_settings: crate::settings_cache::new_cache(),
            steering: crate::commands::steering::SteeringState::new(),
            skills: crate::commands::skill::SkillState::new(),
            event_emitter: None,
            dev_event_store: None,
            desktop_adapter: crate::desktop_control::create_mock_adapter(),
            desktop_action_previews: Default::default(),
        }
    }

    #[derive(Default)]
    struct MemoryMcpStore {
        values: Mutex<HashMap<String, String>>,
        fail_write: AtomicBool,
        fail_delete: AtomicBool,
    }

    impl McpCredentialStore for MemoryMcpStore {
        fn read(&self, reference: &str) -> Result<Option<String>, String> {
            Ok(self.values.lock().unwrap().get(reference).cloned())
        }

        fn write(&self, reference: &str, blob: &str) -> Result<(), String> {
            if self.fail_write.load(Ordering::SeqCst) {
                return Err("test vault write failure".to_string());
            }
            self.values
                .lock()
                .unwrap()
                .insert(reference.to_string(), blob.to_string());
            Ok(())
        }

        fn delete(&self, reference: &str) -> Result<(), String> {
            if self.fail_delete.load(Ordering::SeqCst) {
                return Err("test vault delete failure".to_string());
            }
            self.values.lock().unwrap().remove(reference);
            Ok(())
        }
    }

    fn insert_mcp_server(conn: &rusqlite::Connection, id: &str) {
        conn.execute(
            "INSERT INTO mcp_servers (id, name, command, args, env, enabled, created_at)
             VALUES (?1, 'Server', 'server-command', '', '', 1, 1)",
            [id],
        )
        .unwrap();
    }

    fn set_test_mcp_secret(state: &AppState, store: &MemoryMcpStore, id: &str, value: &str) {
        let mut conn = state.db.lock().unwrap();
        insert_mcp_server(&conn, id);
        replace_env(
            &mut conn,
            store,
            id,
            &[McpEnvVar {
                key: "TOKEN".to_string(),
                value: value.to_string(),
            }],
        )
        .unwrap();
    }

    #[test]
    fn plaintext_export_never_contains_mcp_environment_or_reference() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO mcp_servers (id, name, command, args, env, enabled, created_at)
             VALUES ('server', 'Server', 'server-command', '', 'TOKEN=secret-canary', 1, 1)",
            [],
        )
        .unwrap();

        let exported = export_data_from_connection(
            &conn,
            Some(ExportOptions {
                scope: Some("settings".to_string()),
            }),
        )
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&exported).unwrap();
        let server = &parsed["mcp_servers"][0];
        assert_eq!(server["id"], "server");
        assert!(server.get("env").is_none());
        assert!(server.get("env_ref").is_none());
        assert!(!exported.contains("secret-canary"));
    }

    #[test]
    fn plaintext_mcp_import_strips_legacy_environment_and_reference() {
        let conn = test_db();
        let sanitized = sanitize_mcp_server_rows(&[serde_json::json!({
            "id": "server",
            "name": "Server",
            "command": "server-command",
            "args": "",
            "env": "TOKEN=secret-canary",
            "env_ref": "secret-reference-canary",
            "env_keys": "[\"TOKEN\"]",
            "enabled": 1,
            "created_at": 1,
        })])
        .unwrap();
        assert!(sanitized[0].get("env_ref").is_none());
        assert_eq!(batch_import(&conn, "mcp_servers", &sanitized).unwrap(), 1);
        let environment: Option<String> = conn
            .query_row(
                "SELECT env FROM mcp_servers WHERE id = 'server'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(environment.is_none());
        let enabled: i64 = conn
            .query_row(
                "SELECT enabled FROM mcp_servers WHERE id = 'server'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(enabled, 0, "missing restored secret must not auto-start");
    }

    #[test]
    fn plaintext_import_rejects_credential_bundle() {
        let payload = serde_json::json!({
            "version": 1,
            "mcp_credentials": [{"server_id": "server", "vars": []}]
        });
        assert!(reject_plain_mcp_credentials(&payload).is_err());
    }

    #[test]
    fn encrypted_backup_round_trips_mcp_credentials_through_mock_vault() {
        let source = test_state();
        let source_store = MemoryMcpStore::default();
        set_test_mcp_secret(&source, &source_store, "server", "secret-canary");

        let plain = export_data_inner(&source, None).unwrap();
        assert!(!plain.contains("secret-canary"));
        assert!(!plain.contains("env_ref"));
        let encrypted = export_encrypted_data_with_store(
            &source,
            &source_store,
            "test-backup-password".to_string(),
            None,
        )
        .unwrap();
        assert!(!encrypted.contains("secret-canary"));
        let decrypted = crate::backup::decrypt_export(&encrypted, "test-backup-password").unwrap();
        assert!(decrypted.contains("secret-canary"));

        let restored = test_state();
        let restored_store = MemoryMcpStore::default();
        import_encrypted_data_with_store(
            &restored,
            &restored_store,
            encrypted,
            "test-backup-password".to_string(),
            Some(ImportOptions {
                mode: Some("replace".to_string()),
            }),
        )
        .unwrap();
        let mut conn = restored.db.lock().unwrap();
        let vars = read_effective_env(&mut conn, &restored_store, "server").unwrap();
        assert_eq!(vars[0].value, "secret-canary");
        let persisted_env: String = conn
            .query_row(
                "SELECT COALESCE(env, '') FROM mcp_servers WHERE id = 'server'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(persisted_env.is_empty());
    }

    #[test]
    fn failed_replace_preserves_existing_mcp_reference_and_vault_value() {
        let state = test_state();
        let store = MemoryMcpStore::default();
        set_test_mcp_secret(&state, &store, "existing", "old-secret");
        let old_ref: String = state
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT env_ref FROM mcp_servers WHERE id = 'existing'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let malformed = serde_json::json!({
            "version": 1,
            "mcp_servers": [{
                "id": "new", "name": "New", "command": "server-command",
                "env_keys": "not JSON", "enabled": 1, "created_at": 1
            }]
        });
        let encrypted =
            crate::backup::encrypt_export(&malformed.to_string(), "test-backup-password").unwrap();
        assert!(import_encrypted_data_with_store(
            &state,
            &store,
            encrypted,
            "test-backup-password".to_string(),
            Some(ImportOptions {
                mode: Some("replace".to_string()),
            }),
        )
        .is_err());
        let current_ref: String = state
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT env_ref FROM mcp_servers WHERE id = 'existing'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(current_ref, old_ref);
        assert!(store.read(&old_ref).unwrap().is_some());
    }

    #[test]
    fn encrypted_merge_does_not_replace_a_conflicting_local_mcp_secret() {
        let source = test_state();
        let source_store = MemoryMcpStore::default();
        set_test_mcp_secret(&source, &source_store, "server", "incoming-secret");
        let encrypted = export_encrypted_data_with_store(
            &source,
            &source_store,
            "test-backup-password".to_string(),
            None,
        )
        .unwrap();

        let local = test_state();
        let local_store = MemoryMcpStore::default();
        set_test_mcp_secret(&local, &local_store, "server", "local-secret");
        import_encrypted_data_with_store(
            &local,
            &local_store,
            encrypted,
            "test-backup-password".to_string(),
            None,
        )
        .unwrap();
        let mut conn = local.db.lock().unwrap();
        let vars = read_effective_env(&mut conn, &local_store, "server").unwrap();
        assert_eq!(vars[0].value, "local-secret");
    }

    #[test]
    fn failed_vault_write_leaves_restored_server_disabled() {
        let source = test_state();
        let source_store = MemoryMcpStore::default();
        set_test_mcp_secret(&source, &source_store, "server", "secret-canary");
        let encrypted = export_encrypted_data_with_store(
            &source,
            &source_store,
            "test-backup-password".to_string(),
            None,
        )
        .unwrap();

        let restored = test_state();
        let restored_store = MemoryMcpStore::default();
        restored_store.fail_write.store(true, Ordering::SeqCst);
        assert!(import_encrypted_data_with_store(
            &restored,
            &restored_store,
            encrypted,
            "test-backup-password".to_string(),
            Some(ImportOptions {
                mode: Some("replace".to_string()),
            }),
        )
        .is_err());
        let conn = restored.db.lock().unwrap();
        let (enabled, environment, reference): (i64, String, Option<String>) = conn
            .query_row(
                "SELECT enabled, COALESCE(env, ''), env_ref FROM mcp_servers WHERE id = 'server'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(enabled, 0);
        assert!(environment.is_empty());
        assert!(reference.is_none());
    }

    #[test]
    fn clearing_user_data_reaps_mcp_secret_after_database_commit() {
        let state = test_state();
        let store = MemoryMcpStore::default();
        set_test_mcp_secret(&state, &store, "server", "secret-canary");
        let reference: String = state
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT env_ref FROM mcp_servers WHERE id = 'server'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        clear_all_user_data_with_store(&state, &store).unwrap();
        assert!(store.read(&reference).unwrap().is_none());
        let conn = state.db.lock().unwrap();
        let active: i64 = conn
            .query_row("SELECT COUNT(*) FROM mcp_servers", [], |row| row.get(0))
            .unwrap();
        assert_eq!(active, 0);
    }

    // ── Notification Settings ────────────────────────────────────────────────────

    #[test]
    fn test_notification_settings_default() {
        let conn = test_db();
        let json = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'notification_settings'",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap_or_else(|_| r#"{"chat_reply":true,"reminder":true,"task_complete":true,"file_change":true,"system":true}"#.to_string());
        let settings: NotificationSettings = serde_json::from_str(&json).unwrap();
        assert!(settings.chat_reply);
        assert!(settings.reminder);
        assert!(settings.system);
    }

    #[test]
    fn clearing_user_data_removes_content_but_preserves_schema_marker() {
        let mut conn = test_db();
        conn.execute("INSERT INTO sessions (id, title, created_at, updated_at) VALUES ('session', 'Session', 1, 1)", []).unwrap();
        conn.execute("INSERT INTO messages (id, session_id, role, content, created_at) VALUES ('message', 'session', 'user', 'hello', 1)", []).unwrap();
        conn.execute("INSERT INTO memories (id, scope, category, content, created_at, updated_at) VALUES ('memory', 'global', 'fact', 'remember', 1, 1)", []).unwrap();
        conn.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES ('custom_setting', 'value', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO adaptive_constraints (id, scope, session_id, constraint_key, constraint_value, confidence, evidence_count, source, status, created_at, updated_at) VALUES ('constraint', 'global', NULL, 'interaction_preference:test', 'test', 0.8, 2, 'test', 'active', 1, 1)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO adaptive_constraint_evidence (id, constraint_key, session_id, source, observed_at) VALUES ('evidence', 'interaction_preference:test', 'session', 'test', 1)",
            [],
        ).unwrap();

        clear_all_user_data_from_connection(&mut conn).unwrap();

        for table in [
            "sessions",
            "messages",
            "memories",
            "adaptive_constraints",
            "adaptive_constraint_evidence",
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table} should be empty");
        }
        let schema_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM settings WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let custom_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM settings WHERE key = 'custom_setting'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(schema_count, 1);
        assert_eq!(custom_count, 0);
    }

    #[test]
    fn test_notification_settings_persistence() {
        let conn = test_db();
        let updated = NotificationSettings {
            chat_reply: false,
            reminder: true,
            task_complete: true,
            file_change: true,
            system: true,
        };
        let json = serde_json::to_string(&updated).unwrap();
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('notification_settings', ?1, ?2)",
            params![json, now],
        ).unwrap();
        let reloaded: NotificationSettings = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'notification_settings'",
                [],
                |r| r.get::<_, String>(0),
            )
            .map(|v| serde_json::from_str(&v).unwrap())
            .unwrap();
        assert!(!reloaded.chat_reply);
        assert!(reloaded.reminder);
    }

    #[test]
    fn notification_channel_enabled_respects_each_known_channel() {
        let settings = NotificationSettings {
            chat_reply: false,
            reminder: true,
            task_complete: false,
            file_change: true,
            system: false,
        };

        assert!(!notification_channel_enabled(&settings, "chat_reply"));
        assert!(notification_channel_enabled(&settings, "reminder"));
        assert!(!notification_channel_enabled(&settings, "task_complete"));
        assert!(notification_channel_enabled(&settings, "file_change"));
        assert!(!notification_channel_enabled(&settings, "system"));
        assert!(notification_channel_enabled(&settings, "custom_channel"));
    }

    // ── Autostart ───────────────────────────────────────────────────────────────

    #[test]
    fn notification_event_payload_is_only_created_for_enabled_channels() {
        let settings = NotificationSettings {
            chat_reply: false,
            reminder: true,
            task_complete: true,
            file_change: true,
            system: true,
        };

        assert!(notification_event_payload(
            &settings,
            "chat_reply".to_string(),
            "Hidden".to_string(),
            "Disabled channel".to_string(),
        )
        .is_none());

        let payload = notification_event_payload(
            &settings,
            "reminder".to_string(),
            "Stretch".to_string(),
            "Stand up for a minute".to_string(),
        )
        .expect("enabled channel should create a frontend notification payload");

        assert_eq!(payload.channel, "reminder");
        assert_eq!(payload.title, "Stretch");
        assert_eq!(payload.body, "Stand up for a minute");
    }

    #[test]
    fn test_autostart_default_disabled() {
        let conn = test_db();
        let enabled: bool = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'autostart_enabled'",
                [],
                |r| r.get::<_, String>(0),
            )
            .map(|v| v == "true")
            .unwrap_or(false);
        assert!(!enabled);
    }

    #[test]
    fn test_autostart_persistence() {
        let conn = test_db();
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('autostart_enabled', 'true', ?1)",
            params![now],
        ).unwrap();
        let enabled: bool = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'autostart_enabled'",
                [],
                |r| r.get::<_, String>(0),
            )
            .map(|v| v == "true")
            .unwrap();
        assert!(enabled);
    }

    // ── Scheduled Tasks ──────────────────────────────────────────────────────────

    #[test]
    fn test_scheduled_task_crud() {
        let conn = test_db();
        let now = chrono::Utc::now().timestamp();

        // Create
        conn.execute(
            r#"INSERT INTO scheduled_tasks (id, type, trigger_at, content, enabled, created_at)
               VALUES ('test-task-1', 'reminder', '2026-07-10T10:00:00Z', '记得喝水', 1, ?1)"#,
            params![now],
        )
        .unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM scheduled_tasks WHERE id = 'test-task-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        let content: String = conn
            .query_row(
                "SELECT content FROM scheduled_tasks WHERE id = 'test-task-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(content, "记得喝水");

        // Update
        conn.execute(
            "UPDATE scheduled_tasks SET content = '记得喝水 + 休息', enabled = 0 WHERE id = 'test-task-1'",
            [],
        ).unwrap();
        let (content, enabled): (String, i32) = conn
            .query_row(
                "SELECT content, enabled FROM scheduled_tasks WHERE id = 'test-task-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(content, "记得喝水 + 休息");
        assert_eq!(enabled, 0);

        // Delete
        conn.execute("DELETE FROM scheduled_tasks WHERE id = 'test-task-1'", [])
            .unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM scheduled_tasks WHERE id = 'test-task-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn restore_enabled_scheduled_tasks_queues_only_enabled_tasks() {
        let conn = test_db();
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            r#"INSERT INTO scheduled_tasks (id, type, trigger_at, content, enabled, created_at)
               VALUES ('enabled-task', 'reminder', '2026-07-10T10:00:00Z', '喝水', 1, ?1)"#,
            params![now],
        )
        .unwrap();
        conn.execute(
            r#"INSERT INTO scheduled_tasks (id, type, trigger_at, content, enabled, created_at)
               VALUES ('disabled-task', 'reminder', '2026-07-10T11:00:00Z', '站起来', 0, ?1)"#,
            params![now],
        )
        .unwrap();

        let queued = restore_enabled_scheduled_tasks(&conn).unwrap();

        assert_eq!(queued, 1);
        let rows: Vec<String> = conn
            .prepare(
                "SELECT payload FROM agent_events WHERE event_type = 'scheduled' AND processed = 0",
            )
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].contains("enabled-task"));
        assert!(!rows[0].contains("disabled-task"));
    }

    #[test]
    fn queue_file_change_event_inserts_unprocessed_agent_event() {
        let conn = test_db();
        let payload = serde_json::json!({
            "kind": "modify",
            "paths": ["notes.txt"],
            "timestamp": 123,
        });

        let event_id = queue_file_change_event(&conn, payload.clone()).unwrap();

        let (event_type, stored_payload, processed): (String, String, i32) = conn
            .query_row(
                "SELECT event_type, payload, processed FROM agent_events WHERE id = ?1",
                params![event_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(event_type, "file_change");
        assert_eq!(processed, 0);
        assert!(stored_payload.contains("notes.txt"));
    }
}

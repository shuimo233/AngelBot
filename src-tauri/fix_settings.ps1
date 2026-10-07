$file = "d:\Projects\desktop\AngelBot\src-tauri\src\commands\settings.rs"
$bytes = [System.IO.File]::ReadAllBytes($file)
$text = [System.Text.Encoding]::UTF8.GetString($bytes)

# Find the API config section by looking for "API config persistence"
$apiIdx = $text.IndexOf("API config persistence")
Write-Output "API config idx: $apiIdx"

# Find Data export section
$dataIdx = $text.IndexOf("Data export/import")
Write-Output "Data export idx: $dataIdx"

if ($apiIdx -lt 0) {
    Write-Output "ERROR: API config section not found"
    exit 1
}

# If API config section exists, remove it (everything from apiIdx back to the previous `}` line to dataIdx)
if ($apiIdx -ge 0 -and $dataIdx -ge 0) {
    # Find the start of the API config block (look back for the closing `}` of set_autostart)
    $beforeApi = $text.Substring(0, $apiIdx)
    # Find the last `}` before the API config section
    $lastBrace = $beforeApi.LastIndexOf("}")
    Write-Output "Last brace before API config: $lastBrace"
    
    $after = $text.Substring($dataIdx)
    
    $replacement = @"
}

// --- API config (delegated to api_config module) ---

use crate::commands::api_config::{ApiProviderConfig, load_api_config_internal};

#[tauri::command]
pub fn load_api_config(state: State<AppState>) -> Result<Option<ApiProviderConfig>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    Ok(load_api_config_internal(&conn))
}

#[tauri::command]
pub fn save_api_config(
    state: State<AppState>,
    config: ApiProviderConfig,
) -> Result<(), String> {
    let now = Utc::now().timestamp();
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let entries: &[(&str, String)] = &[
        ("api_provider", config.provider),
        ("api_model", config.model),
        ("api_base_url", config.base_url),
        ("api_key", config.api_key),
        ("api_max_tokens", config.max_tokens.to_string()),
        ("api_temperature", config.temperature.to_string()),
    ];
    for (key, value) in entries {
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)",
            params![key, value, now],
        ).map_err(|e| e.to_string())?;
    }
    Ok(())
}

"@

    $new = $text.Substring(0, $lastBrace + 1) + $replacement + $after
    [System.IO.File]::WriteAllText($file, $new, [System.Text.Encoding]::UTF8)
    Write-Output "Done - replaced API config section"
} else {
    Write-Output "Nothing to replace"
}

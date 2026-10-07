//! User-triggered model connection actions. No credential ever crosses IPC.

use crate::commands::api_config::ApiProviderConfig;
use crate::llm::{chatgpt_auth, LlmResponse, ModelAuthMode, ProviderConfig};
use serde::Serialize;
use std::collections::HashSet;
use std::time::Duration;

#[tauri::command]
pub fn chatgpt_plan_status(
    app: tauri::AppHandle,
) -> Result<chatgpt_auth::ChatGptPlanStatus, String> {
    chatgpt_auth::status(&app)
}

#[tauri::command]
pub async fn chatgpt_plan_sign_in(
    app: tauri::AppHandle,
) -> Result<chatgpt_auth::ChatGptPlanStatus, String> {
    chatgpt_auth::sign_in(&app).await
}

#[tauri::command]
pub async fn chatgpt_plan_change_account(
    app: tauri::AppHandle,
) -> Result<chatgpt_auth::ChatGptPlanStatus, String> {
    chatgpt_auth::change_account(&app).await
}

#[tauri::command]
pub fn chatgpt_plan_cancel_sign_in() -> Result<(), String> {
    chatgpt_auth::cancel_sign_in()
}

#[tauri::command]
pub async fn chatgpt_plan_disconnect(
    app: tauri::AppHandle,
) -> Result<chatgpt_auth::ChatGptPlanStatus, String> {
    chatgpt_auth::disconnect(&app).await
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct AvailableModel {
    id: String,
    name: String,
}

fn parse_plan_models(value: serde_json::Value) -> Result<Vec<AvailableModel>, String> {
    let models = value
        .get("models")
        .and_then(serde_json::Value::as_array)
        .filter(|models| models.len() <= 512)
        .ok_or("ChatGPT model catalog is invalid")?;
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for model in models.iter().filter(|model| model["visibility"] == "list") {
        let id = model["slug"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 200)
            .ok_or("ChatGPT model identifier is invalid")?;
        let name = model["display_name"]
            .as_str()
            .filter(|name| !name.is_empty() && name.len() <= 300)
            .unwrap_or(id);
        if seen.insert(id.to_owned()) {
            result.push(AvailableModel {
                id: id.into(),
                name: name.into(),
            });
        }
    }
    Ok(result)
}

#[tauri::command]
pub async fn chatgpt_plan_models(app: tauri::AppHandle) -> Result<Vec<AvailableModel>, String> {
    let status = chatgpt_auth::status(&app)?;
    if !status.plan_enabled {
        return Err("ChatGPT plan access needs authorization".into());
    }
    let source = chatgpt_auth::credential_source(
        &app,
        status.credential_ref.as_deref().unwrap_or_default(),
    )?;
    let token = source.bearer_token().await?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| "Cannot initialize model catalog connection")?;
    let mut response = client
        .get("https://api.openai.com/v1/models")
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "Cannot load ChatGPT models; check your connection")?;
    if !response.status().is_success() {
        return Err(format!(
            "ChatGPT model catalog unavailable (HTTP {})",
            response.status().as_u16()
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "ChatGPT model catalog was interrupted")?
    {
        if body.len() + chunk.len() > 1024 * 1024 {
            return Err("ChatGPT model catalog exceeds limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    source.validate_session()?;
    parse_plan_models(
        serde_json::from_slice(&body).map_err(|_| "ChatGPT model catalog is invalid")?,
    )
}

#[tauri::command]
pub async fn test_model_connection(
    app: tauri::AppHandle,
    config: ApiProviderConfig,
) -> Result<bool, String> {
    let mut profile = ProviderConfig {
        provider: config.provider,
        model: config.model,
        base_url: config.base_url,
        api_key: config.api_key,
        max_tokens: 64,
        temperature: config.temperature,
        protocol: config.protocol,
        auth_mode: config.auth_mode,
        credential_ref: config.credential_ref,
    };
    if profile.auth_mode == ModelAuthMode::ApiKey && profile.api_key.trim().is_empty() {
        profile.api_key = crate::commands::api_config::api_key_from_environment(&profile.provider)
            .or_else(|| {
                crate::keychain::read_credentials(&app)
                    .ok()
                    .filter(|credentials| {
                        credentials.provider.is_empty() || credentials.provider == profile.provider
                    })
                    .map(|credentials| credentials.api_key)
            })
            .unwrap_or_default();
    }
    let provider = crate::llm::create_provider_for_app(&profile, Some(&app))?;
    let response = tokio::time::timeout(
        Duration::from_secs(60),
        provider.chat(
            &[crate::llm::Message {
                role: "user".into(),
                content: "Reply with OK only. Do not call tools.".into(),
                tool_calls: None,
                tool_call_id: None,
                tool_images: Vec::new(),
                protocol_state: None,
            }],
            None,
            None,
        ),
    )
    .await
    .map_err(|_| "Model connection test timed out")??;
    match response.into_parts().0 {
        LlmResponse::Text {
            stop_reason: crate::llm::StopReason::EndTurn,
            text,
        } if !text.trim().is_empty() => Ok(true),
        _ => Err("Model did not complete a valid text response".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_account_specific_and_preserves_server_order() {
        let catalog = parse_plan_models(serde_json::json!({"models": [
            {"slug":"second","display_name":"Second","visibility":"list"},
            {"slug":"hidden","visibility":"hidden"},
            {"slug":"first","display_name":"First","visibility":"list"},
            {"slug":"second","visibility":"list"}
        ]}))
        .unwrap();
        assert_eq!(
            catalog,
            vec![
                AvailableModel {
                    id: "second".into(),
                    name: "Second".into()
                },
                AvailableModel {
                    id: "first".into(),
                    name: "First".into()
                }
            ]
        );
        assert!(parse_plan_models(serde_json::json!({"data":[]})).is_err());
        assert!(parse_plan_models(serde_json::json!({"models":[{"visibility":"list"}]})).is_err());
    }
}

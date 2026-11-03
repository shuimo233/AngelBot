//! User-triggered model connection actions. No credential ever crosses IPC.

use crate::commands::api_config::ApiProviderConfig;
use crate::llm::{
    chatgpt_auth, LlmProvider, LlmResponse, Message, ModelAuthMode, ProviderConfig, StopReason,
    ToolSchema,
};
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

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum ToolRoundTrip {
    Verified,
    NotObserved,
    InvalidCall,
    InvalidResult,
    RequestFailed,
    TimedOut,
}

/// Returned only after a completed, nonempty text response. Probe details and
/// private protocol items never cross IPC or become saved capability grants.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConnectionReport {
    tool_round_trip: ToolRoundTrip,
}

async fn check_model_connection(
    provider: &dyn LlmProvider,
    budget: Duration,
) -> Result<ModelConnectionReport, String> {
    let deadline = tokio::time::Instant::now() + budget;
    let message = |role: &str, content: String| Message {
        role: role.into(),
        content,
        tool_calls: None,
        tool_call_id: None,
        tool_images: Vec::new(),
        protocol_state: None,
    };
    let response = tokio::time::timeout_at(
        deadline,
        provider.chat(
            &[message(
                "user",
                "Reply with OK only. Do not call tools.".into(),
            )],
            None,
            None,
        ),
    )
    .await
    .map_err(|_| "Model connection test timed out")??;
    if !matches!(response.into_parts().0,
        LlmResponse::Text { stop_reason: StopReason::EndTurn, text } if !text.trim().is_empty())
    {
        return Err("Model did not complete a valid text response".into());
    }

    // This is a synthetic tool, never a ToolRegistry dispatch. The only local
    // result is a fresh receipt created after a valid call, not in the prompt.
    let tools = [ToolSchema {
        name: "angelbot_connection_probe".into(),
        description: "Connection check only; returns a receipt without accessing any user data or performing any action.".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {"echo": {"type": "string", "enum": ["ping"]}},
            "required": ["echo"],
            "additionalProperties": false
        }),
    }];
    let mut messages = vec![message("user", "Connection check: call angelbot_connection_probe exactly once with echo set to ping. After its result, reply only with the receipt value it returns. Do not invent a receipt or call any other tool.".into())];
    let check_tools = async {
        let response = provider
            .chat(&messages, Some(&tools), None)
            .await
            .map_err(|_| ToolRoundTrip::RequestFailed)?;
        let (response, protocol_state) = response.into_parts();
        if protocol_state
            .as_ref()
            .is_some_and(|state| state.validate().is_err())
        {
            return Err(ToolRoundTrip::InvalidCall);
        }
        let (calls, text) = match response {
            LlmResponse::ToolCalls {
                calls,
                text,
                stop_reason: StopReason::ToolUse,
            } => (calls, text),
            LlmResponse::Text {
                text,
                stop_reason: StopReason::EndTurn,
            } if !text.trim().is_empty() => {
                return Err(ToolRoundTrip::NotObserved);
            }
            _ => return Err(ToolRoundTrip::InvalidCall),
        };
        if calls.len() != 1
            || calls[0].name != tools[0].name
            || calls[0].arguments != serde_json::json!({"echo": "ping"})
            || calls[0].id.trim().is_empty()
            || calls[0].id.len() > 256
            || calls[0].id.chars().any(char::is_control)
        {
            return Err(ToolRoundTrip::InvalidCall);
        }
        let receipt = uuid::Uuid::new_v4().simple().to_string();
        let call_id = calls[0].id.clone();
        messages.push(Message {
            tool_calls: Some(calls),
            protocol_state,
            ..message("assistant", text.unwrap_or_default())
        });
        messages.push(Message {
            tool_call_id: Some(call_id),
            ..message("tool", serde_json::json!({"receipt": receipt}).to_string())
        });
        let response = provider
            .chat(&messages, Some(&tools), None)
            .await
            .map_err(|_| ToolRoundTrip::RequestFailed)?;
        let (response, protocol_state) = response.into_parts();
        if protocol_state
            .as_ref()
            .is_some_and(|state| state.validate().is_err())
        {
            return Err(ToolRoundTrip::InvalidResult);
        }
        match response {
            LlmResponse::Text {
                text,
                stop_reason: StopReason::EndTurn,
            } if text.trim() == receipt => Ok(ToolRoundTrip::Verified),
            _ => Err(ToolRoundTrip::InvalidResult),
        }
    };
    let tool_round_trip = match tokio::time::timeout_at(deadline, check_tools).await {
        Ok(Ok(status)) | Ok(Err(status)) => status,
        Err(_) => ToolRoundTrip::TimedOut,
    };
    Ok(ModelConnectionReport { tool_round_trip })
}

#[tauri::command]
pub async fn test_model_connection(
    app: tauri::AppHandle,
    config: ApiProviderConfig,
) -> Result<ModelConnectionReport, String> {
    let mut profile = ProviderConfig {
        provider: config.provider,
        model: config.model,
        base_url: config.base_url,
        api_key: config.api_key,
        max_tokens: 1024,
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
    check_model_connection(provider.as_ref(), Duration::from_secs(60)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ModelProtocol, ProtocolContinuation, ThinkingSettings, ToolCall};
    use std::collections::VecDeque;
    use std::sync::Mutex;

    enum ProbeReply {
        Model(Result<LlmResponse, String>),
        Receipt,
        Pending,
    }

    struct ProbeProvider {
        replies: Mutex<VecDeque<ProbeReply>>,
        requests: Mutex<Vec<(Vec<Message>, Vec<ToolSchema>)>>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for ProbeProvider {
        fn name(&self) -> &str {
            "connection-test"
        }

        async fn chat(
            &self,
            messages: &[Message],
            tools: Option<&[ToolSchema]>,
            _: Option<&ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            self.requests
                .lock()
                .unwrap()
                .push((messages.to_vec(), tools.unwrap_or_default().to_vec()));
            let reply = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("no retry or extra model request");
            match reply {
                ProbeReply::Model(result) => result,
                ProbeReply::Pending => std::future::pending().await,
                ProbeReply::Receipt => {
                    let output: serde_json::Value =
                        serde_json::from_str(&messages.last().unwrap().content).unwrap();
                    Ok(LlmResponse::Text {
                        text: output["receipt"].as_str().unwrap().into(),
                        stop_reason: StopReason::EndTurn,
                    })
                }
            }
        }
    }

    fn text_reply(text: &str, stop_reason: StopReason) -> ProbeReply {
        ProbeReply::Model(Ok(LlmResponse::Text {
            text: text.into(),
            stop_reason,
        }))
    }

    fn probe_call() -> ToolCall {
        ToolCall {
            id: "probe-call".into(),
            name: "angelbot_connection_probe".into(),
            arguments: serde_json::json!({"echo":"ping"}),
        }
    }

    fn call_reply(calls: Vec<ToolCall>, stop_reason: StopReason) -> ProbeReply {
        ProbeReply::Model(Ok(LlmResponse::ToolCalls {
            calls,
            text: None,
            stop_reason,
        }))
    }

    #[tokio::test]
    async fn connection_probe_requires_a_complete_native_round_trip_without_real_tools() {
        let ok = || text_reply("OK", StopReason::EndTurn);
        let call = || call_reply(vec![probe_call()], StopReason::ToolUse);
        let mut cases = vec![
            (
                "verified",
                vec![ok(), call(), ProbeReply::Receipt],
                Some(ToolRoundTrip::Verified),
            ),
            (
                "no native call",
                vec![ok(), text_reply("I called the tool.", StopReason::EndTurn)],
                Some(ToolRoundTrip::NotObserved),
            ),
            (
                "pseudo call",
                vec![
                    ok(),
                    ProbeReply::Model(Ok(LlmResponse::ProtocolViolation {
                        raw_text: "private-provider-output".into(),
                        reason: "pseudo call".into(),
                    })),
                ],
                Some(ToolRoundTrip::InvalidCall),
            ),
            (
                "empty calls",
                vec![ok(), call_reply(vec![], StopReason::ToolUse)],
                Some(ToolRoundTrip::InvalidCall),
            ),
            (
                "multiple calls",
                vec![
                    ok(),
                    call_reply(vec![probe_call(), probe_call()], StopReason::ToolUse),
                ],
                Some(ToolRoundTrip::InvalidCall),
            ),
            (
                "truncated call",
                vec![ok(), call_reply(vec![probe_call()], StopReason::MaxTokens)],
                Some(ToolRoundTrip::InvalidCall),
            ),
            (
                "invented result",
                vec![ok(), call(), text_reply("OK", StopReason::EndTurn)],
                Some(ToolRoundTrip::InvalidResult),
            ),
            (
                "empty result",
                vec![ok(), call(), text_reply("", StopReason::EndTurn)],
                Some(ToolRoundTrip::InvalidResult),
            ),
            (
                "truncated result",
                vec![ok(), call(), text_reply("partial", StopReason::MaxTokens)],
                Some(ToolRoundTrip::InvalidResult),
            ),
            (
                "repeated call",
                vec![ok(), call(), call()],
                Some(ToolRoundTrip::InvalidResult),
            ),
            (
                "call request failure",
                vec![
                    ok(),
                    ProbeReply::Model(Err("private-provider-error".into())),
                ],
                Some(ToolRoundTrip::RequestFailed),
            ),
            (
                "result request failure",
                vec![
                    ok(),
                    call(),
                    ProbeReply::Model(Err("private-provider-error".into())),
                ],
                Some(ToolRoundTrip::RequestFailed),
            ),
            (
                "empty text connection",
                vec![text_reply(" ", StopReason::EndTurn)],
                None,
            ),
            (
                "truncated text connection",
                vec![text_reply("OK", StopReason::MaxTokens)],
                None,
            ),
            (
                "text connection error",
                vec![ProbeReply::Model(Err("service unavailable".into()))],
                None,
            ),
            ("text timeout", vec![ProbeReply::Pending], None),
            (
                "call timeout",
                vec![ok(), ProbeReply::Pending],
                Some(ToolRoundTrip::TimedOut),
            ),
            (
                "result timeout",
                vec![ok(), call(), ProbeReply::Pending],
                Some(ToolRoundTrip::TimedOut),
            ),
        ];
        for (name, changed) in [
            (
                "unknown tool",
                ToolCall {
                    name: "write_file".into(),
                    ..probe_call()
                },
            ),
            (
                "empty id",
                ToolCall {
                    id: "".into(),
                    ..probe_call()
                },
            ),
            (
                "control id",
                ToolCall {
                    id: "bad\0id".into(),
                    ..probe_call()
                },
            ),
            (
                "long id",
                ToolCall {
                    id: "x".repeat(257),
                    ..probe_call()
                },
            ),
            (
                "missing argument",
                ToolCall {
                    arguments: serde_json::json!({}),
                    ..probe_call()
                },
            ),
            (
                "wrong argument",
                ToolCall {
                    arguments: serde_json::json!({"echo":null}),
                    ..probe_call()
                },
            ),
            (
                "extra argument",
                ToolCall {
                    arguments: serde_json::json!({"echo":"ping","path":"private-path"}),
                    ..probe_call()
                },
            ),
        ] {
            cases.push((
                name,
                vec![ok(), call_reply(vec![changed], StopReason::ToolUse)],
                Some(ToolRoundTrip::InvalidCall),
            ));
        }
        let continuation = ProtocolContinuation {
            protocol: ModelProtocol::OpenaiResponses,
            model: "private-model".into(),
            credential_ref: "private-credential".into(),
            output_items: vec![
                serde_json::json!({"type":"reasoning","encrypted_content":"private-reasoning"}),
                serde_json::json!({"type":"function_call","call_id":"probe-call","name":"angelbot_connection_probe","arguments":"{\"echo\":\"ping\"}"}),
            ],
        };
        for (name, protocol, expected) in [
            (
                "private continuation",
                continuation.clone(),
                ToolRoundTrip::Verified,
            ),
            (
                "invalid continuation",
                ProtocolContinuation {
                    output_items: vec![],
                    ..continuation.clone()
                },
                ToolRoundTrip::InvalidCall,
            ),
        ] {
            let mut replies = vec![
                ok(),
                ProbeReply::Model(Ok(LlmResponse::WithProtocol {
                    response: Box::new(LlmResponse::ToolCalls {
                        calls: vec![probe_call()],
                        text: None,
                        stop_reason: StopReason::ToolUse,
                    }),
                    protocol,
                })),
            ];
            if expected == ToolRoundTrip::Verified {
                replies.push(ProbeReply::Receipt);
            }
            cases.push((name, replies, Some(expected)));
        }
        for (name, replies, expected) in cases {
            let count = replies.len();
            let timeout = replies
                .iter()
                .any(|reply| matches!(reply, ProbeReply::Pending));
            let provider = ProbeProvider {
                replies: Mutex::new(replies.into()),
                requests: Mutex::new(vec![]),
            };
            let result = check_model_connection(
                &provider,
                if timeout {
                    Duration::from_millis(1)
                } else {
                    Duration::from_secs(5)
                },
            )
            .await;
            match expected {
                Some(status) => {
                    let report = result.unwrap();
                    assert_eq!(report.tool_round_trip, status, "{name}");
                    let encoded = serde_json::to_string(&report).unwrap();
                    let wire_status = match status {
                        ToolRoundTrip::Verified => "verified",
                        ToolRoundTrip::NotObserved => "notObserved",
                        ToolRoundTrip::InvalidCall => "invalidCall",
                        ToolRoundTrip::InvalidResult => "invalidResult",
                        ToolRoundTrip::RequestFailed => "requestFailed",
                        ToolRoundTrip::TimedOut => "timedOut",
                    };
                    assert_eq!(
                        serde_json::from_str::<serde_json::Value>(&encoded).unwrap(),
                        serde_json::json!({"toolRoundTrip": wire_status}),
                        "{name}: exact IPC contract"
                    );
                    assert!(
                        !encoded.contains("private"),
                        "{name}: no provider output in IPC"
                    );
                }
                None => assert!(result.is_err(), "{name}: no false text success"),
            }
            let requests = provider.requests.lock().unwrap();
            assert_eq!(
                requests.len(),
                count,
                "{name}: bounded requests, no retries"
            );
            assert!(requests[0].1.is_empty());
            for (messages, tools) in requests.iter().skip(1) {
                assert_eq!(tools.len(), 1);
                assert_eq!(tools[0].name, "angelbot_connection_probe");
                assert_eq!(messages[0].role, "user");
                assert!(messages
                    .iter()
                    .all(|message| message.tool_images.is_empty()));
            }
            if let Some((messages, _)) = requests.get(2) {
                assert_eq!(messages.len(), 3);
                assert_eq!(messages[1].role, "assistant");
                assert_eq!(messages[1].tool_calls.as_ref().unwrap()[0].id, "probe-call");
                assert_eq!(messages[2].role, "tool");
                assert_eq!(messages[2].tool_call_id.as_deref(), Some("probe-call"));
                let receipt: serde_json::Value =
                    serde_json::from_str(&messages[2].content).unwrap();
                assert!(!messages[0]
                    .content
                    .contains(receipt["receipt"].as_str().unwrap()));
                if name == "private continuation" {
                    assert_eq!(
                        messages[1].protocol_state.as_ref().unwrap().output_items,
                        continuation.output_items
                    );
                }
            }
        }
    }

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

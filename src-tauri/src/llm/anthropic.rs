//! Anthropic provider — handles Anthropic's Messages API
//!
//! Key differences from OpenAI format:
//! - System message extracted to top-level `system` field
//! - Tool schemas use Anthropic-specific format with `input_schema`
//! - Tool calls returned as `content` blocks with `type: "tool_use"`
//! - Uses `x-api-key` header and `anthropic-version` header

use super::{
    transport, LlmFailure, LlmProvider, LlmResponse, Message, ProviderUsage, StopReason,
    ThinkingEffort, ThinkingProtocol, ThinkingSettings, ToolCall, ToolSchema,
};
use serde_json::Value;
use std::sync::{Arc, Mutex};

fn anthropic_effort(effort: ThinkingEffort) -> &'static str {
    match effort {
        ThinkingEffort::Low => "low",
        ThinkingEffort::Medium => "medium",
        ThinkingEffort::High => "high",
    }
}

pub struct AnthropicProvider {
    /// Arc<Mutex<...>> allows `refresh_api_key` and `set_model` without &mut self.
    api_key: Arc<Mutex<String>>,
    model: Arc<Mutex<String>>,
    max_tokens: u32,
    temperature: f64,
    endpoint: String,
    label: String,
    last_usage: Arc<Mutex<Option<ProviderUsage>>>,
}

impl AnthropicProvider {
    pub fn new(api_key: String, model: String, max_tokens: u32, temperature: f64) -> Self {
        Self {
            api_key: Arc::new(Mutex::new(api_key)),
            model: Arc::new(Mutex::new(model)),
            max_tokens,
            temperature,
            endpoint: "https://api.anthropic.com/v1/messages".into(),
            label: "anthropic".into(),
            last_usage: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_endpoint(mut self, base_url: String, label: String) -> Self {
        let base = base_url.trim_end_matches('/');
        self.endpoint = if base.ends_with("/messages") {
            base.to_owned()
        } else if base.ends_with("/v1") {
            format!("{base}/messages")
        } else {
            format!("{base}/v1/messages")
        };
        self.label = label;
        self
    }
}

#[async_trait::async_trait]
impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        &self.label
    }
    fn latest_usage(&self) -> Option<ProviderUsage> {
        *self.last_usage.lock().unwrap()
    }

    async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
    ) -> Result<LlmResponse, String> {
        self.chat_classified(messages, tools, thinking)
            .await
            .map_err(|failure| failure.diagnostic)
    }

    async fn chat_stream_classified(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
        _delta_sender: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<LlmResponse, LlmFailure> {
        self.chat_classified(messages, tools, thinking).await
    }

    async fn chat_classified(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
    ) -> Result<LlmResponse, LlmFailure> {
        *self.last_usage.lock().unwrap() = None;
        let body = self.build_request_body(messages, tools, thinking);

        let api_key = self.api_key.lock().unwrap().clone();
        let mut request = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(std::time::Duration::from_secs(30))
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .map_err(|_| "Cannot initialize Anthropic-compatible connection")?
            .post(&self.endpoint)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body);
        if !api_key.is_empty() {
            request = request.header("x-api-key", &api_key);
        }
        let resp = transport::send_model_request(request)
            .await
            .map_err(|error| transport::request_failure(error, "Anthropic Messages"))?;

        if !resp.status().is_success() {
            return Err(transport::api_response_failure(resp, "Anthropic Messages").await);
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|error| transport::response_read_failure(error, "Anthropic Messages"))?;
        let resp_json: Value = serde_json::from_slice(&bytes)
            .map_err(|_| "Anthropic Messages 响应 JSON 格式无效。")?;
        if let Some(failure) = transport::json_response_failure(&resp_json, "Anthropic Messages") {
            return Err(failure);
        }
        self.parse_response(resp_json).map_err(Into::into)
    }
}

impl AnthropicProvider {
    fn split_system_messages(messages: &[Message]) -> (Option<String>, Vec<&Message>) {
        let mut system = String::new();
        let mut remaining = Vec::new();
        for message in messages {
            if message.role != "system" {
                remaining.push(message);
            } else if !message.content.trim().is_empty() {
                if !system.is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(&message.content);
            }
        }
        ((!system.is_empty()).then_some(system), remaining)
    }

    /// The exact body sent to Messages. Keep the system segments outside the
    /// conversation, without changing user, assistant or tool order.
    fn build_request_body(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
    ) -> Value {
        let (system_prompt, user_messages) = Self::split_system_messages(messages);
        let anthropic_messages = self.build_anthropic_messages(&user_messages);

        let mut body = serde_json::json!({
            "model": &*self.model.lock().unwrap(),
            "messages": anthropic_messages,
            "max_tokens": self.max_tokens,
            "temperature": self.temperature,
        });

        // This adapter represents ordered system segments as one top-level string.
        if let Some(sys) = system_prompt {
            body["system"] = serde_json::json!(sys);
        }

        if let Some(thinking) = thinking {
            match thinking.protocol {
                ThinkingProtocol::AnthropicManual => {
                    if let Some(budget) = thinking.budget_tokens {
                        body["thinking"] = serde_json::json!({
                            "type": "enabled",
                            "budget_tokens": budget,
                        });
                        // Manual thinking is only compatible with the default
                        // temperature on the models AngelBot supports here.
                        body["temperature"] = serde_json::json!(1.0);
                    }
                }
                ThinkingProtocol::AnthropicAdaptive => {
                    body["thinking"] = serde_json::json!({ "type": "adaptive" });
                    body["output_config"] = serde_json::json!({
                        "effort": anthropic_effort(thinking.effort),
                    });
                }
                ThinkingProtocol::AnthropicEffortOnly => {
                    body["output_config"] = serde_json::json!({
                        "effort": anthropic_effort(thinking.effort),
                    });
                }
                ThinkingProtocol::DeepSeek | ThinkingProtocol::OpenAiResponses => {}
            }
        }

        // Convert tools to Anthropic format
        if let Some(tools) = tools {
            if !tools.is_empty() {
                let anthropic_tools: Vec<Value> = tools
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "name": t.name,
                            "description": t.description,
                            "input_schema": t.parameters,
                        })
                    })
                    .collect();
                body["tools"] = serde_json::json!(anthropic_tools);

                eprintln!(
                    "[Anthropic] Sending {} tools: {:?}",
                    tools.len(),
                    tools.iter().map(|t| &t.name).collect::<Vec<_>>()
                );
            }
        }
        body
    }

    /// Convert unified messages to Anthropic format
    fn build_anthropic_messages(&self, messages: &[&Message]) -> Vec<Value> {
        messages
            .iter()
            .map(|msg| {
                let role = match msg.role.as_str() {
                    "assistant" => "assistant",
                    _ => "user", // tool results also go as user with tool_result content blocks
                };

                let mut obj = serde_json::json!({
                    "role": role,
                });

                // Handle tool results: Anthropic uses content blocks with type "tool_result"
                if msg.role == "tool" {
                    obj["content"] = serde_json::json!([{
                        "type": "tool_result",
                        "tool_use_id": msg.tool_call_id.as_deref().unwrap_or(""),
                        "content": msg.text_for_unsupported_tool_images(),
                    }]);
                    return obj;
                }

                // Handle assistant with tool calls
                if msg.role == "assistant"
                    && msg.tool_calls.as_ref().map_or(false, |tc| !tc.is_empty())
                {
                    let mut content_blocks: Vec<Value> = Vec::new();

                    // Add text content if any
                    if !msg.content.is_empty() {
                        content_blocks.push(serde_json::json!({
                            "type": "text",
                            "text": msg.content,
                        }));
                    }

                    // Add tool_use blocks
                    if let Some(tool_calls) = &msg.tool_calls {
                        for tc in tool_calls {
                            content_blocks.push(serde_json::json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.name,
                                "input": tc.arguments,
                            }));
                        }
                    }

                    obj["content"] = serde_json::json!(content_blocks);
                    return obj;
                }

                // Plain text message
                obj["content"] = serde_json::json!(msg.content);
                obj
            })
            .collect()
    }

    /// Parse Anthropic's response format back to unified LlmResponse
    fn parse_response(&self, resp: Value) -> Result<LlmResponse, String> {
        *self.last_usage.lock().unwrap() = None;
        if let (Some(input), Some(output)) = (
            resp["usage"]["input_tokens"].as_u64(),
            resp["usage"]["output_tokens"].as_u64(),
        ) {
            *self.last_usage.lock().unwrap() = u32::try_from(input)
                .ok()
                .zip(u32::try_from(output).ok())
                .map(|(input, output)| {
                    ProviderUsage::from_anthropic_usage(input, output, &resp["usage"])
                });
        }
        let content = resp["content"].as_array().ok_or_else(|| {
            format!(
                "[Anthropic] Unexpected response format: {}",
                serde_json::to_string(&resp).unwrap_or_default()
            )
        })?;

        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut text_parts: Vec<String> = Vec::new();

        for block in content {
            match block["type"].as_str() {
                Some("tool_use") => {
                    tool_calls.push(ToolCall {
                        id: block["id"].as_str().unwrap_or("").to_string(),
                        name: block["name"].as_str().unwrap_or("").to_string(),
                        arguments: block["input"].clone(),
                    });
                }
                Some("text") => {
                    if let Some(text) = block["text"].as_str() {
                        if !text.is_empty() {
                            text_parts.push(text.to_string());
                        }
                    }
                }
                _ => {}
            }
        }

        if !tool_calls.is_empty() {
            let text = if text_parts.is_empty() {
                None
            } else {
                Some(text_parts.join("\n"))
            };
            // Issue #078: capture the Anthropic stop_reason.
            let stop_reason = StopReason::from_anthropic_stop_reason(
                resp.get("stop_reason").and_then(Value::as_str),
            );
            Ok(LlmResponse::ToolCalls {
                calls: tool_calls,
                text,
                stop_reason,
            })
        } else {
            // Issue #078: capture the Anthropic stop_reason.
            let stop_reason = StopReason::from_anthropic_stop_reason(
                resp.get("stop_reason").and_then(Value::as_str),
            );
            Ok(LlmResponse::Text {
                text: text_parts.join("\n"),
                stop_reason,
            })
        }
    }

    fn refresh_api_key(&self, api_key: &str) -> bool {
        let mut key = self.api_key.lock().unwrap();
        *key = api_key.to_string();
        true
    }

    fn set_model(&self, model: &str) -> bool {
        let mut m = self.model.lock().unwrap();
        *m = model.to_string();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_wire_prompt_contract_preserves_system_order_and_tool_transcript() {
        let provider = AnthropicProvider::new("fixture".into(), "fixture-model".into(), 64, 0.3);
        let message = |role: &str, content: &str| Message {
            role: role.into(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            protocol_state: None,
            tool_images: Vec::new(),
        };
        let mut call = message("assistant", "Checking");
        call.tool_calls = Some(vec![crate::llm::ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "fixture.txt"}),
        }]);
        let mut result = message("tool", "fixture contents");
        result.tool_call_id = Some("call-1".into());
        let body = provider.build_request_body(
            &[
                message("system", "Main identity"),
                message("user", "Read the fixture"),
                message("system", " \t\n"),
                call,
                message("system", "Pending attention"),
                result,
                message("system", "Ordered tail"),
            ],
            None,
            None,
        );
        assert_eq!(
            body["system"],
            "Main identity\n\nPending attention\n\nOrdered tail"
        );
        assert_eq!(
            body["messages"],
            serde_json::json!([
                {"role": "user", "content": "Read the fixture"},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "Checking"},
                    {"type": "tool_use", "id": "call-1", "name": "read_file",
                        "input": {"path": "fixture.txt"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call-1",
                        "content": "fixture contents"}
                ]}
            ])
        );
        let blank_only = provider.build_request_body(
            &[message("system", " \n "), message("user", "Hello")],
            None,
            None,
        );
        assert!(blank_only.get("system").is_none());
        assert_eq!(
            blank_only["messages"],
            serde_json::json!([
                {"role": "user", "content": "Hello"}
            ])
        );
    }
}

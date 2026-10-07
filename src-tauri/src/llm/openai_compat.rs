//! OpenAI-compatible provider — handles all providers with OpenAI-style APIs
//!
//! Covers: OpenAI, DeepSeek, Ollama, Groq, xAI, OpenRouter, Azure, Mistral, Custom
//! All share the same /chat/completions endpoint format.

use super::{
    LlmProvider, LlmResponse, Message, ProviderUsage, StopReason, ThinkingEffort, ThinkingProtocol,
    ThinkingSettings, ToolCall, ToolSchema,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub(crate) fn openai_compatible_request_headers(
    provider: &str,
    api_key: &str,
) -> Vec<(&'static str, String)> {
    let mut headers = Vec::new();

    if provider == "google" {
        headers.push((
            "x-goog-api-client",
            format!("angelbot-oai/{}", env!("CARGO_PKG_VERSION")),
        ));
    }

    if !api_key.trim().is_empty() {
        if provider == "azure" {
            headers.push(("api-key", api_key.to_string()));
        } else {
            headers.push(("Authorization", format!("Bearer {api_key}")));
        }
    }

    headers
}

/// A generic provider for any OpenAI-compatible API endpoint
pub struct OpenAiCompatibleProvider {
    /// Arc<Mutex<...>> allows `refresh_api_key` and `set_model` without &mut self.
    api_key: Arc<Mutex<String>>,
    model: Arc<Mutex<String>>,
    base_url: String,
    max_tokens: u32,
    temperature: f64,
    /// Provider label for logging
    label: String,
    last_usage: Arc<Mutex<Option<ProviderUsage>>>,
}

impl OpenAiCompatibleProvider {
    pub fn new(
        api_key: String,
        model: String,
        base_url: String,
        max_tokens: u32,
        temperature: f64,
        label: String,
    ) -> Self {
        Self {
            api_key: Arc::new(Mutex::new(api_key)),
            model: Arc::new(Mutex::new(model)),
            base_url,
            max_tokens,
            temperature,
            label,
            last_usage: Arc::new(Mutex::new(None)),
        }
    }

    /// Provider-specific transport headers for the shared OpenAI-compatible
    /// request path. Keeping this here ensures normal generation, streaming,
    /// and credential refresh all follow the same authentication contract.
    fn request_headers(&self) -> Vec<(&'static str, String)> {
        let api_key = self.api_key.lock().unwrap().clone();
        openai_compatible_request_headers(&self.label, &api_key)
    }

    fn apply_request_headers(
        &self,
        mut request: reqwest::RequestBuilder,
    ) -> reqwest::RequestBuilder {
        for (name, value) in self.request_headers() {
            request = request.header(name, value);
        }
        request
    }

    /// Create from a ProviderConfig
    pub fn from_config(
        api_key: &str,
        model: &str,
        base_url: &str,
        max_tokens: u32,
        temperature: f64,
        label: &str,
    ) -> Self {
        Self::new(
            api_key.to_string(),
            model.to_string(),
            base_url.to_string(),
            max_tokens,
            temperature,
            label.to_string(),
        )
    }
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
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
        let mut body = serde_json::json!({
            "model": &*self.model.lock().unwrap(),
            "messages": self.build_messages(messages),
            "max_tokens": self.max_tokens,
            "temperature": self.temperature,
        });

        // OpenAI-compatible APIs do not share a reasoning extension. Only
        // DeepSeek gets its documented `thinking` request fields.
        if let Some(t) = thinking {
            if t.protocol == ThinkingProtocol::DeepSeek && self.label == "deepseek" {
                match t.effort {
                    ThinkingEffort::Low => {
                        body["thinking"] = serde_json::json!({ "type": "disabled" });
                    }
                    ThinkingEffort::Medium => {
                        body["thinking"] = serde_json::json!({ "type": "enabled" });
                        body["reasoning_effort"] = serde_json::json!("high");
                    }
                    ThinkingEffort::High => {
                        body["thinking"] = serde_json::json!({ "type": "enabled" });
                        body["reasoning_effort"] = serde_json::json!("max");
                    }
                }
                // DeepSeek documents that temperature is ignored in thinking
                // mode; omit it so the request matches the native API.
                if t.effort != ThinkingEffort::Low {
                    body.as_object_mut()
                        .expect("completion body is an object")
                        .remove("temperature");
                }
            }
        }

        // Attach tools if provided
        if let Some(tools) = tools {
            let tool_schemas: Vec<_> = tools.iter().map(|t| t.to_openai_format()).collect();
            body["tools"] = serde_json::json!(tool_schemas);
            body["tool_choice"] = serde_json::json!("auto");

            eprintln!(
                "[{}] Sending {} tools: {:?}",
                self.label,
                tools.len(),
                tool_schemas
                    .iter()
                    .map(|v| v
                        .get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(|n| n.as_str())
                        .unwrap_or("?"))
                    .collect::<Vec<_>>()
            );
        }

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        // Validate entire request before sending
        self.validate_request(&body)?;

        // Serialize to JSON string and re-parse to catch any serialization edge cases.
        // This is the final validation before sending to the API.
        let body_str = serde_json::to_string(&body)
            .map_err(|e| format!("[{}] Failed to serialize request body: {}", self.label, e))?;

        // Parse it back and validate the messages array specifically
        let re_parsed: Value = serde_json::from_str(&body_str).map_err(|e| {
            format!(
                "[{}] JSON serialization round-trip failed: {}",
                self.label, e
            )
        })?;

        // Final check: verify every message's content is a string or null
        if let Some(msgs) = re_parsed.get("messages").and_then(|m| m.as_array()) {
            eprintln!(
                "[{}] Validating {} messages after round-trip",
                self.label,
                msgs.len()
            );
            for (i, msg) in msgs.iter().enumerate() {
                let content = msg.get("content");
                if let Some(c) = content {
                    if !c.is_string() && !c.is_null() {
                        // Log enough info to debug without panicking on Chinese chars
                        let content_type = if c.is_array() {
                            "array"
                        } else if c.is_object() {
                            "object/map"
                        } else if c.is_number() {
                            "number"
                        } else {
                            "unknown"
                        };
                        eprintln!(
                            "[{}] CRITICAL: messages[{}].content is {} (not string/null)",
                            self.label, i, content_type
                        );
                        return Err(format!(
                            "[{}] messages[{}].content is {} after serialization",
                            self.label, i, content_type
                        ));
                    }
                    let char_count = match c {
                        serde_json::Value::String(s) => s.chars().count(),
                        serde_json::Value::Null => 0,
                        _ => 0,
                    };
                    eprintln!("[{}] messages[{}] OK: {} chars", self.label, i, char_count);
                } else {
                    eprintln!("[{}] messages[{}] has no content field", self.label, i);
                }
            }
        }

        // DEBUG: log the full request body
        let body_pretty = serde_json::to_string_pretty(&body).unwrap_or_else(|_| body_str.clone());
        eprintln!("[{}] REQUEST BODY:\n{}", self.label, body_pretty);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| format!("[{}] Client build failed: {}", self.label, e))?;

        let req_builder = client
            .post(&url)
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body);

        let req_builder = self.apply_request_headers(req_builder);

        let resp = req_builder
            .send()
            .await
            .map_err(|e| format!("[{}] HTTP request failed: {}", self.label, e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("[{}] API error {}: {}", self.label, status, text));
        }

        let resp_json: Value = resp
            .json()
            .await
            .map_err(|e| format!("[{}] JSON parse error: {}", self.label, e))?;

        // DEBUG: log the response
        let resp_str =
            serde_json::to_string_pretty(&resp_json).unwrap_or_else(|_| resp_json.to_string());
        eprintln!("[{}] RESPONSE:\n{}", self.label, resp_str);

        self.parse_response(resp_json)
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
        delta_sender: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<LlmResponse, String> {
        let mut body = serde_json::json!({
            "model": &*self.model.lock().unwrap(),
            "messages": self.build_messages(messages),
            "max_tokens": self.max_tokens,
            "temperature": self.temperature,
            "stream": true,
        });

        if let Some(t) = thinking {
            if t.protocol == ThinkingProtocol::DeepSeek && self.label == "deepseek" {
                body["thinking"] = match t.effort {
                    ThinkingEffort::Low => serde_json::json!({ "type": "disabled" }),
                    ThinkingEffort::Medium | ThinkingEffort::High => {
                        serde_json::json!({ "type": "enabled" })
                    }
                };
                if t.effort != ThinkingEffort::Low {
                    body["reasoning_effort"] = serde_json::json!(match t.effort {
                        ThinkingEffort::Medium => "high",
                        ThinkingEffort::High => "max",
                        ThinkingEffort::Low => unreachable!(),
                    });
                    body.as_object_mut()
                        .expect("completion body is an object")
                        .remove("temperature");
                }
            }
        }

        if let Some(tools) = tools {
            body["tools"] = serde_json::json!(tools
                .iter()
                .map(ToolSchema::to_openai_format)
                .collect::<Vec<_>>());
            body["tool_choice"] = serde_json::json!("auto");
        }
        self.validate_request(&body)?;

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| format!("[{}] Client build failed: {}", self.label, e))?;
        let request = client
            .post(&url)
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body);
        let request = self.apply_request_headers(request);
        let mut response = request
            .send()
            .await
            .map_err(|e| format!("[{}] HTTP streaming request failed: {}", self.label, e))?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("[{}] API error {}: {}", self.label, status, text));
        }

        let mut buffer = String::new();
        let mut raw_content = String::new();
        let mut sent_visible = String::new();
        let mut tool_calls: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
        // Issue #078: track the last SSE delta's `finish_reason` so we can
        // tag the response at the end. Some providers emit it on every
        // delta; others emit it once at the tail.
        let mut last_finish_reason: Option<String> = None;

        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("[{}] SSE read failed: {}", self.label, e))?
        {
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(newline) = buffer.find('\n') {
                let line = buffer[..newline].trim_end_matches('\r').trim().to_string();
                buffer.drain(..=newline);
                let Some(payload) = line.strip_prefix("data:") else {
                    continue;
                };
                let payload = payload.trim();
                if payload == "[DONE]" {
                    continue;
                }
                let Ok(event) = serde_json::from_str::<Value>(payload) else {
                    continue;
                };
                let Some(delta) = event["choices"]
                    .as_array()
                    .and_then(|choices| choices.first())
                    .and_then(|choice| choice.get("delta"))
                else {
                    continue;
                };

                if let Some(content) = delta.get("content").and_then(Value::as_str) {
                    raw_content.push_str(content);
                    // Never expose provider reasoning blocks. Recompute the visible
                    // prefix so tags split across SSE chunks remain private.
                    let visible = strip_hidden_thinking(&raw_content);
                    if let Some(suffix) = visible.strip_prefix(&sent_visible) {
                        if !suffix.is_empty() {
                            if let Some(sender) = &delta_sender {
                                let _ = sender.send(suffix.to_string());
                            }
                            sent_visible.push_str(suffix);
                        }
                    }
                }

                // Issue #078: capture the last non-null finish_reason.
                if let Some(choice) = event["choices"]
                    .as_array()
                    .and_then(|choices| choices.first())
                {
                    if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                        if !reason.is_empty() {
                            last_finish_reason = Some(reason.to_string());
                        }
                    }
                }

                if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for (position, call) in calls.iter().enumerate() {
                        let index = call
                            .get("index")
                            .and_then(Value::as_u64)
                            .map(|value| value as usize)
                            .unwrap_or(position);
                        let entry = tool_calls.entry(index).or_default();
                        if let Some(id) = call.get("id").and_then(Value::as_str) {
                            entry.0 = id.to_string();
                        }
                        if let Some(name) = call
                            .get("function")
                            .and_then(|function| function.get("name"))
                            .and_then(Value::as_str)
                        {
                            entry.1.push_str(name);
                        }
                        if let Some(arguments) = call
                            .get("function")
                            .and_then(|function| function.get("arguments"))
                            .and_then(Value::as_str)
                        {
                            entry.2.push_str(arguments);
                        }
                    }
                }
            }
        }

        let calls: Vec<Value> = tool_calls
            .into_values()
            .filter(|(_, name, _)| !name.is_empty())
            .map(|(id, name, arguments)| {
                serde_json::json!({
                    "id": if id.is_empty() { uuid::Uuid::new_v4().to_string() } else { id },
                    "type": "function",
                    "function": { "name": name, "arguments": arguments },
                })
            })
            .collect();
        let message = if calls.is_empty() {
            serde_json::json!({ "content": raw_content })
        } else {
            serde_json::json!({ "content": raw_content, "tool_calls": calls })
        };
        // Issue #078: forward the last finish_reason we observed so
        // parse_response can tag the response with a StopReason.
        let finish_reason_field = last_finish_reason
            .as_deref()
            .map(|r| serde_json::json!({ "finish_reason": r }))
            .unwrap_or(serde_json::json!({}));
        let mut choice = serde_json::json!({ "message": message });
        if let (Some(obj), Some(field)) = (choice.as_object_mut(), finish_reason_field.as_object())
        {
            obj.extend(field.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        self.parse_response(serde_json::json!({ "choices": [choice] }))
    }
}

/// Normalize tool arguments so handlers always receive a serde_json::Value::Object.
/// Some providers (e.g. DeepSeek) send `arguments` as a JSON string, others as an object.
/// This function parses strings and validates the result, returning an empty object on failure.
fn normalize_tool_args(raw: &Value) -> Value {
    match raw {
        // Already an object — use as-is
        Value::Object(_) => raw.clone(),
        // String — attempt to parse; on failure return empty object so the handler
        // can produce a structured error rather than panicking
        Value::String(s) => {
            match serde_json::from_str::<Value>(s) {
                Ok(Value::Object(_)) => {
                    serde_json::from_str(s).unwrap_or(Value::Object(Default::default()))
                }
                Ok(v) => v, // parsed but not an object — let handler deal with it
                Err(e) => {
                    eprintln!(
                        "[{}] Failed to parse tool arguments JSON string: {} — raw: {}",
                        "openai_compat", e, s
                    );
                    Value::Object(Default::default())
                }
            }
        }
        _ => raw.clone(),
    }
}

/// Compatibility parser for gateways that return tool calls as complete
/// XML-like text instead of the native OpenAI `tool_calls` response field.
/// A narrow grammar keeps ordinary assistant prose from becoming executable.
fn parse_xml_tool_calls(content: &str) -> Vec<ToolCall> {
    fn attribute(tag: &str, name: &str) -> Option<String> {
        for quote in ['\"', '\''] {
            let prefix = format!("{}={}", name, quote);
            let start = tag.find(&prefix)? + prefix.len();
            let end = tag[start..].find(quote)? + start;
            return Some(tag[start..end].to_string());
        }
        None
    }

    fn decode(value: &str) -> String {
        value
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
    }

    let mut calls = Vec::new();
    let mut remaining = content;
    while let Some(block_start) = remaining.find("<tool_call>") {
        let after_start = &remaining[block_start + "<tool_call>".len()..];
        let Some(block_end) = after_start.find("</tool_call>") else {
            break;
        };
        let block = &after_start[..block_end];
        remaining = &after_start[block_end + "</tool_call>".len()..];

        let mut invocations = block;
        while let Some(invoke_start) = invocations.find("<invoke") {
            let invoke_tail = &invocations[invoke_start..];
            let Some(open_end) = invoke_tail.find('>') else {
                break;
            };
            let invoke_tag = &invoke_tail[..=open_end];
            let Some(name) = attribute(invoke_tag, "name") else {
                invocations = &invoke_tail[open_end + 1..];
                continue;
            };
            let Some(invoke_end) = invoke_tail[open_end + 1..].find("</invoke>") else {
                break;
            };
            let invoke_body = &invoke_tail[open_end + 1..open_end + 1 + invoke_end];

            let mut arguments = serde_json::Map::new();
            let mut parameters = invoke_body;
            while let Some(parameter_start) = parameters.find("<parameter") {
                let parameter_tail = &parameters[parameter_start..];
                let Some(parameter_open_end) = parameter_tail.find('>') else {
                    break;
                };
                let parameter_tag = &parameter_tail[..=parameter_open_end];
                let Some(parameter_name) = attribute(parameter_tag, "name") else {
                    parameters = &parameter_tail[parameter_open_end + 1..];
                    continue;
                };
                let value_start = parameter_open_end + 1;
                let Some(parameter_end) = parameter_tail[value_start..].find("</parameter>") else {
                    break;
                };
                let value = parameter_tail[value_start..value_start + parameter_end].trim();
                arguments.insert(parameter_name, Value::String(decode(value)));
                parameters = &parameter_tail[value_start + parameter_end + "</parameter>".len()..];
            }

            calls.push(ToolCall {
                id: uuid::Uuid::new_v4().to_string(),
                name: decode(&name),
                arguments: Value::Object(arguments),
            });
            invocations = &invoke_tail[open_end + 1 + invoke_end + "</invoke>".len()..];
        }
    }
    calls
}

/// A few OpenAI-compatible gateways wrap the same `<invoke name="…">`
/// grammar in `<function_calls>` rather than `<tool_call>`. Normalize only
/// that exact wrapper and reuse the constrained XML parser above.
fn parse_function_calls_xml_tool_calls(content: &str) -> Vec<ToolCall> {
    if !content.contains("<function_calls>") || !content.contains("</function_calls>") {
        return Vec::new();
    }

    let normalized = content
        .replace("<function_calls>", "<tool_call>")
        .replace("</function_calls>", "</tool_call>");
    parse_xml_tool_calls(&normalized)
}

/// Some proxy models place an otherwise standard OpenAI `tool_calls` payload
/// inside a fenced JSON block in `message.content`. Others emit the compact
/// legacy form used by older tool prompts: `[ { "name": "…", "arguments":
/// { … } } ]`. Accept one well-formed fenced block only; free-form JSON in
/// prose remains chat content rather than an executable instruction.
fn parse_fenced_json_tool_calls(content: &str) -> Vec<ToolCall> {
    let mut blocks = Vec::new();
    let mut remaining = content;
    while let Some(start) = remaining.find("```") {
        let after_start = &remaining[start + 3..];
        let Some(end) = after_start.find("```") else {
            break;
        };
        let block = &after_start[..end];
        let json = block
            .strip_prefix("json")
            .or_else(|| block.strip_prefix("JSON"))
            .or_else(|| block.strip_prefix('\n'));
        if let Some(json) = json {
            blocks.push(json.trim());
        }
        remaining = &after_start[end + 3..];
    }

    let [json] = blocks.as_slice() else {
        return Vec::new();
    };
    let Ok(payload) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let entries = payload
        .get("tool_calls")
        .and_then(Value::as_array)
        .or_else(|| payload.as_array());
    let Some(entries) = entries else {
        return Vec::new();
    };

    entries
        .iter()
        .filter_map(|entry| {
            let function = entry.get("function").unwrap_or(entry);
            let name = function.get("name")?.as_str()?.trim();
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return None;
            }
            let arguments = match function.get("arguments")? {
                Value::String(raw) => serde_json::from_str::<Value>(raw).ok()?,
                Value::Object(arguments) => Value::Object(arguments.clone()),
                _ => return None,
            };
            arguments.is_object().then(|| ToolCall {
                id: entry
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                name: name.to_string(),
                arguments,
            })
        })
        .collect()
}

/// Legacy gateway format: `<functional_commands><command type="tool_call">`
/// with a named `<tool>` and `<param>` values.
fn parse_functional_commands_xml_tool_calls(content: &str) -> Vec<ToolCall> {
    fn attr(tag: &str, key: &str) -> Option<String> {
        for quote in ['\"', '\''] {
            let prefix = format!("{}={}", key, quote);
            let start = tag.find(&prefix)? + prefix.len();
            let end = tag[start..].find(quote)? + start;
            return Some(tag[start..end].to_string());
        }
        None
    }
    let mut calls = Vec::new();
    let mut remaining = content;
    while let Some(start) = remaining.find("<command type=\"tool_call\">") {
        let block = &remaining[start + "<command type=\"tool_call\">".len()..];
        let Some(end) = block.find("</command>") else {
            break;
        };
        remaining = &block[end + "</command>".len()..];
        let command = &block[..end];
        let Some(tool_start) = command.find("<tool") else {
            continue;
        };
        let tool_tail = &command[tool_start..];
        let Some(tool_open_end) = tool_tail.find('>') else {
            continue;
        };
        let Some(name) = attr(&tool_tail[..=tool_open_end], "name") else {
            continue;
        };
        let Some(tool_end) = tool_tail[tool_open_end + 1..].find("</tool>") else {
            continue;
        };
        let mut fields = &tool_tail[tool_open_end + 1..tool_open_end + 1 + tool_end];
        let mut arguments = serde_json::Map::new();
        while let Some(param_start) = fields.find("<param") {
            let tail = &fields[param_start..];
            let Some(open_end) = tail.find('>') else {
                break;
            };
            let Some(key) = attr(&tail[..=open_end], "name") else {
                break;
            };
            let Some(value_end) = tail[open_end + 1..].find("</param>") else {
                break;
            };
            arguments.insert(
                key,
                Value::String(
                    tail[open_end + 1..open_end + 1 + value_end]
                        .trim()
                        .to_string(),
                ),
            );
            fields = &tail[open_end + 1 + value_end + "</param>".len()..];
        }
        calls.push(ToolCall {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            arguments: Value::Object(arguments),
        });
    }
    calls
}

/// Some Anthropic-compatible gateways serialize a tool call as
/// `<invoke_name>tool</invoke_name><parameters><path>…</path></parameters>`.
/// Keep this separate from the older `<invoke name="…">` grammar so both
/// formats remain explicit and auditable.
fn parse_named_xml_tool_calls(content: &str) -> Vec<ToolCall> {
    let mut calls = Vec::new();
    let mut remaining = content;
    while let Some(block_start) = remaining.find("<tool_call>") {
        let after_start = &remaining[block_start + "<tool_call>".len()..];
        let Some(block_end) = after_start.find("</tool_call>") else {
            break;
        };
        let block = &after_start[..block_end];
        remaining = &after_start[block_end + "</tool_call>".len()..];

        let Some(name_start) = block.find("<invoke_name>") else {
            continue;
        };
        let name_body = &block[name_start + "<invoke_name>".len()..];
        let Some(name_end) = name_body.find("</invoke_name>") else {
            continue;
        };
        let name = name_body[..name_end].trim();
        if name.is_empty() {
            continue;
        }

        let Some(parameters_start) = block.find("<parameters>") else {
            continue;
        };
        let parameters_body = &block[parameters_start + "<parameters>".len()..];
        let Some(parameters_end) = parameters_body.find("</parameters>") else {
            continue;
        };
        let mut fields = &parameters_body[..parameters_end];
        let mut arguments = serde_json::Map::new();
        while let Some(open_start) = fields.find('<') {
            let tail = &fields[open_start + 1..];
            let Some(open_end) = tail.find('>') else {
                break;
            };
            let key = tail[..open_end].trim();
            if key.is_empty() || key.contains(' ') || key.contains('/') {
                fields = &tail[open_end + 1..];
                continue;
            }
            let value_body = &tail[open_end + 1..];
            let close_tag = format!("</{}>", key);
            let Some(value_end) = value_body.find(&close_tag) else {
                break;
            };
            arguments.insert(
                key.to_string(),
                Value::String(value_body[..value_end].trim().to_string()),
            );
            fields = &value_body[value_end + close_tag.len()..];
        }

        calls.push(ToolCall {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            arguments: Value::Object(arguments),
        });
    }
    calls
}

/// Reasoning tags are provider control data, never chat content. Remove both
/// complete and truncated tags so a gateway cannot expose hidden reasoning.
fn strip_hidden_thinking(content: &str) -> String {
    let mut visible = String::new();
    let mut remaining = content;
    const OPEN: &str = "<anthropic:thinking>";
    const CLOSE: &str = "</anthropic:thinking>";
    while let Some(start) = remaining.find(OPEN) {
        visible.push_str(&remaining[..start]);
        let after_open = &remaining[start + OPEN.len()..];
        let Some(end) = after_open.find(CLOSE) else {
            return visible.trim().to_string();
        };
        remaining = &after_open[end + CLOSE.len()..];
    }
    visible.push_str(remaining);
    visible.trim().to_string()
}

/// Parse the compact fallback emitted by some tool-using models, for example
/// `read_file({"path":"brief.txt"})`. A line must consist only of a safe
/// tool identifier plus one JSON object; prose and code snippets are ignored.
fn parse_function_style_tool_calls(content: &str) -> Vec<ToolCall> {
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let open = line.find('(')?;
            if !line.ends_with(')') || open == 0 {
                return None;
            }
            let name = &line[..open];
            if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return None;
            }
            let arguments = serde_json::from_str::<Value>(&line[open + 1..line.len() - 1]).ok()?;
            if !arguments.is_object() {
                return None;
            }
            Some(ToolCall {
                id: uuid::Uuid::new_v4().to_string(),
                name: name.to_string(),
                arguments,
            })
        })
        .collect()
}

/// Detect text which explicitly presents itself as a tool call but cannot be
/// safely normalized by one of the structured compatibility parsers above.
///
/// This is a quarantine signal, not a parser: it never turns model text into
/// an executable call. Keeping this distinction in the provider adapter lets
/// the runner retry the protocol once and then surface an auditable failure.
fn textual_tool_protocol_violation(content: &str) -> Option<&'static str> {
    let normalized = content.trim().to_ascii_lowercase();
    if normalized.contains("<function_result") && normalized.contains("action:") {
        return Some("textual function-result tag");
    }

    let looks_like_named_argument_call = |candidate: &str| {
        let candidate = candidate.trim();
        let Some(open) = candidate.find('(') else {
            return false;
        };
        if open == 0 || !candidate.ends_with(')') || candidate.contains('\n') {
            return false;
        }
        let name = &candidate[..open];
        name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && candidate[open + 1..candidate.len() - 1].contains('=')
    };

    // Gate on an explicit tool-call fence when prose surrounds the invocation.
    // This covers gateways/models that render a pseudo-call in markdown rather
    // than using the API's `tool_calls` field.
    let mut fenced_tool_body = Vec::new();
    let mut in_tool_fence = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(label) = trimmed.strip_prefix("```") {
            if in_tool_fence {
                break;
            }
            let label = label.trim().to_ascii_lowercase();
            in_tool_fence = matches!(
                label.as_str(),
                "tool_call" | "tool_calls" | "function_call" | "function_calls"
            );
            continue;
        }
        if in_tool_fence {
            fenced_tool_body.push(trimmed);
        }
    }
    if looks_like_named_argument_call(&fenced_tool_body.join("\n")) {
        return Some("textual named-argument tool call in a tool-call fence");
    }

    // Do not inspect arbitrary prose. A bare single-line pseudo-call is an
    // explicit protocol-shaped response and may be quarantined safely.
    if looks_like_named_argument_call(content) {
        return Some("textual named-argument tool call");
    }

    None
}

impl OpenAiCompatibleProvider {
    /// Build messages for the API request, with validation to ensure
    /// DeepSeek (and other providers) receive properly formatted messages.
    fn build_messages(&self, messages: &[Message]) -> Vec<Value> {
        let mut result = Vec::with_capacity(messages.len());

        for (i, msg) in messages.iter().enumerate() {
            let validated = self.validate_message_for_api(msg, i);
            result.push(validated);
        }

        result
    }

    /// Validate a single message before serialization.
    /// DeepSeek is strict: content must be a string, not a map/object.
    /// Returns a serde_json::Value ready for the API request.
    fn validate_message_for_api(&self, msg: &Message, index: usize) -> Value {
        let has_tool_calls = msg.tool_calls.as_ref().map_or(false, |tc| !tc.is_empty());

        // Force content to be a string — this is critical for DeepSeek compatibility.
        // If content somehow contains JSON-like objects, they must be escaped/serialized.
        let projected_content = msg.text_for_unsupported_tool_images();
        let content_str = &projected_content;

        // Determine content value
        let content = if has_tool_calls {
            // When tool_calls are present, content must be null per OpenAI spec
            Value::Null
        } else {
            // Always use the string directly — no serde_json::json! macro which
            // might misinterpret content containing JSON-like patterns
            Value::String(content_str.clone())
        };

        // Build the message object
        let mut obj = serde_json::json!({
            "role": msg.role,
            "content": content,
        });

        // Add tool_calls if present
        if let Some(tool_calls) = &msg.tool_calls {
            if !tool_calls.is_empty() {
                let tc_array: Vec<Value> = tool_calls
                    .iter()
                    .map(|tc| {
                        // Pi pattern: arguments MUST be a JSON string, not a JSON object
                        // DeepSeek and other OpenAI-compatible APIs require this
                        let args_str = tc.arguments.to_string();
                        serde_json::json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": args_str,
                            }
                        })
                    })
                    .collect();
                obj["tool_calls"] = serde_json::json!(tc_array);
            }
        }

        // Add tool_call_id if present
        if let Some(tool_call_id) = &msg.tool_call_id {
            obj["tool_call_id"] = serde_json::json!(tool_call_id);
        }

        // Validate the built object
        if let Err(e) = self.validate_message_format(&obj, index) {
            eprintln!(
                "[{}] Message validation warning at index {}: {}",
                self.label, index, e
            );
        }

        obj
    }

    /// Validate a message object conforms to OpenAI API format.
    /// Returns an error string if invalid, which is logged but doesn't block processing.
    fn validate_message_format(&self, obj: &Value, index: usize) -> Result<(), String> {
        // Must be an object
        let obj = obj
            .as_object()
            .ok_or_else(|| format!("message[{}] is not an object", index))?;

        // Must have role
        if !obj.contains_key("role") {
            return Err(format!("message[{}] missing 'role' field", index));
        }

        // Role must be a string
        if let Some(role) = obj.get("role") {
            if !role.is_string() {
                return Err(format!("message[{}] 'role' is not a string", index));
            }
            let role_str = role.as_str().unwrap();
            let valid_roles = ["system", "user", "assistant", "tool", "function"];
            if !valid_roles.contains(&role_str) {
                return Err(format!(
                    "message[{}] has unknown role '{}'. Valid: {:?}",
                    index, role_str, valid_roles
                ));
            }
        }

        // Must have content field
        if !obj.contains_key("content") {
            return Err(format!("message[{}] missing 'content' field", index));
        }

        // Content must be string or null
        let content = obj.get("content");
        match content {
            Some(Value::String(_)) | Some(Value::Null) => {}
            Some(v) => {
                return Err(format!(
                    "message[{}] 'content' is {}, expected string or null. Full type: {:?}",
                    index,
                    if v.is_array() {
                        "array"
                    } else if v.is_object() {
                        "object/map"
                    } else {
                        "unknown"
                    },
                    v
                ));
            }
            None => {
                return Err(format!("message[{}] 'content' is missing", index));
            }
        }

        // If tool_calls present, validate format
        if let Some(tc) = obj.get("tool_calls") {
            if let Some(tc_arr) = tc.as_array() {
                for (ti, tcv) in tc_arr.iter().enumerate() {
                    let tc_obj = tcv.as_object().ok_or_else(|| {
                        format!("message[{}] tool_calls[{}] is not an object", index, ti)
                    })?;

                    // Must have id
                    if !tc_obj.contains_key("id")
                        || !tc_obj.get("id").map_or(false, |v| v.is_string())
                    {
                        return Err(format!(
                            "message[{}] tool_calls[{}] missing valid 'id'",
                            index, ti
                        ));
                    }

                    // Must have function.name
                    if let Some(func) = tc_obj.get("function") {
                        if !func.is_object() || !func.get("name").map_or(false, |v| v.is_string()) {
                            return Err(format!(
                                "message[{}] tool_calls[{}] function.name is invalid",
                                index, ti
                            ));
                        }
                    } else {
                        return Err(format!(
                            "message[{}] tool_calls[{}] missing 'function'",
                            index, ti
                        ));
                    }
                }
            }
        }

        // If tool_call_id present, must be string
        if let Some(tcid) = obj.get("tool_call_id") {
            if !tcid.is_string() {
                return Err(format!("message[{}] 'tool_call_id' is not a string", index));
            }
        }

        Ok(())
    }

    /// Validate the entire request body before sending to the API.
    /// This catches DeepSeek-specific issues like invalid message formats.
    fn validate_request(&self, body: &Value) -> Result<(), String> {
        let obj = body
            .as_object()
            .ok_or_else(|| "Request body is not an object".to_string())?;

        // Validate model
        if !obj.contains_key("model") {
            return Err("Request missing 'model' field".to_string());
        }
        if !obj.get("model").map_or(false, |v| v.is_string()) {
            return Err("'model' must be a string".to_string());
        }

        // Validate messages array
        let messages = obj
            .get("messages")
            .ok_or_else(|| "Request missing 'messages' field".to_string())?;
        let msg_arr = messages
            .as_array()
            .ok_or_else(|| "'messages' must be an array".to_string())?;

        for (i, msg) in msg_arr.iter().enumerate() {
            // Each message must be an object
            if !msg.is_object() {
                return Err(format!("messages[{}] is not an object", i));
            }

            let msg_obj = msg.as_object().unwrap();

            // Must have role
            if !msg_obj.contains_key("role") {
                return Err(format!("messages[{}] missing 'role'", i));
            }
            let role = &msg_obj["role"];
            if !role.is_string() {
                return Err(format!("messages[{}] 'role' is not a string", i));
            }

            // Must have content
            if !msg_obj.contains_key("content") {
                return Err(format!("messages[{}] missing 'content'", i));
            }
            let content = &msg_obj["content"];
            // content can be null or string
            if !content.is_string() && !content.is_null() {
                return Err(format!(
                    "messages[{}] 'content' must be string or null, got: {}",
                    i,
                    if content.is_array() {
                        "array"
                    } else if content.is_object() {
                        "object/map"
                    } else if content.is_number() {
                        "number"
                    } else if content.is_boolean() {
                        "boolean"
                    } else {
                        "unknown"
                    }
                ));
            }
        }

        // OpenAI-compatible providers (including DeepSeek) require every
        // assistant tool-call message to be followed by one tool result for
        // each call, before another conversational message is sent. Structural
        // validation above cannot catch a broken sequence, which otherwise
        // becomes a remote 400 with little actionable context.
        let mut index = 0;
        while index < msg_arr.len() {
            let message = &msg_arr[index];
            let role = message["role"].as_str().unwrap_or_default();

            if role == "tool" {
                return Err(format!(
                    "messages[{}] is a tool result without a preceding assistant tool call",
                    index
                ));
            }

            let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) else {
                index += 1;
                continue;
            };

            if role != "assistant" || tool_calls.is_empty() {
                index += 1;
                continue;
            }

            let mut expected_ids = std::collections::HashSet::new();
            for tool_call in tool_calls {
                let id = tool_call.get("id").and_then(Value::as_str).ok_or_else(|| {
                    format!("messages[{}] contains a tool call without an id", index)
                })?;
                if !expected_ids.insert(id) {
                    return Err(format!(
                        "messages[{}] contains duplicate tool call id '{}'",
                        index, id
                    ));
                }
            }

            let mut result_index = index + 1;
            while result_index < msg_arr.len()
                && msg_arr[result_index]["role"].as_str() == Some("tool")
            {
                let tool_call_id = msg_arr[result_index]
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        format!(
                            "messages[{}] tool result is missing 'tool_call_id'",
                            result_index
                        )
                    })?;
                if !expected_ids.remove(tool_call_id) {
                    return Err(format!(
                        "messages[{}] references unknown or duplicate tool call id '{}'",
                        result_index, tool_call_id
                    ));
                }
                result_index += 1;
            }

            if !expected_ids.is_empty() {
                return Err(format!(
                    "messages[{}] has tool_calls without matching tool results for: {}",
                    index,
                    expected_ids.into_iter().collect::<Vec<_>>().join(", ")
                ));
            }

            index = result_index;
        }

        Ok(())
    }

    fn parse_response(&self, resp: Value) -> Result<LlmResponse, String> {
        if let (Some(input), Some(output)) = (
            resp["usage"]["prompt_tokens"].as_u64(),
            resp["usage"]["completion_tokens"].as_u64(),
        ) {
            *self.last_usage.lock().unwrap() = Some(ProviderUsage {
                input_tokens: input as u32,
                output_tokens: output as u32,
            });
        }
        let choice = resp["choices"]
            .as_array()
            .and_then(|arr| arr.first())
            .ok_or_else(|| {
                format!(
                    "[{}] No choices in response: {}",
                    self.label,
                    serde_json::to_string(&resp).unwrap_or_default()
                )
            })?;

        // Issue #078: capture the finish_reason so every Text/ToolCalls
        // construction below can carry a StopReason. Default to EndTurn
        // when the provider omits it; that preserves previous behavior
        // for any provider that never sets finish_reason.
        let stop_reason = StopReason::from_openai_finish_reason(
            choice.get("finish_reason").and_then(Value::as_str),
        );

        let message = &choice["message"];

        // Extract both text content and tool calls
        let raw_content = message["content"]
            .as_str()
            .map(String::from)
            .unwrap_or_default();
        let content = strip_hidden_thinking(&raw_content);

        // Check for tool calls
        if let Some(tool_calls) = message["tool_calls"].as_array() {
            if !tool_calls.is_empty() {
                let calls: Vec<ToolCall> = tool_calls
                    .iter()
                    .filter_map(|tc| {
                        let raw_args = &tc["function"]["arguments"];
                        // DeepSeek sometimes sends arguments as a JSON string instead of an object.
                        // Normalize to a serde_json::Value::Object so handlers can deserialize it.
                        let arguments = normalize_tool_args(raw_args);
                        Some(ToolCall {
                            id: tc["id"].as_str()?.to_string(),
                            name: tc["function"]["name"].as_str()?.to_string(),
                            arguments,
                        })
                    })
                    .collect();

                if !calls.is_empty() {
                    let text = if content.is_empty() {
                        None
                    } else {
                        Some(content)
                    };
                    return Ok(LlmResponse::ToolCalls {
                        calls,
                        text,
                        stop_reason,
                    });
                }
            }
        }

        // Quarantine explicit pseudo-calls before permissive compatibility
        // parsers run. A named-argument expression is not JSON and must never
        // become executable merely because it was wrapped in a tool-call tag.
        if let Some(reason) = textual_tool_protocol_violation(&content) {
            return Ok(LlmResponse::ProtocolViolation {
                raw_text: content,
                reason: reason.to_string(),
            });
        }

        // No tool calls — try structured compatibility formats, then return text.
        let mut xml_calls = parse_xml_tool_calls(&raw_content);
        xml_calls.extend(parse_function_calls_xml_tool_calls(&raw_content));
        xml_calls.extend(parse_named_xml_tool_calls(&raw_content));
        if !xml_calls.is_empty() {
            return Ok(LlmResponse::ToolCalls {
                calls: xml_calls,
                text: None,
                stop_reason,
            });
        }

        let fenced_json_calls = parse_fenced_json_tool_calls(&content);
        if !fenced_json_calls.is_empty() {
            return Ok(LlmResponse::ToolCalls {
                calls: fenced_json_calls,
                text: None,
                stop_reason,
            });
        }

        let functional_calls = parse_functional_commands_xml_tool_calls(&content);
        if !functional_calls.is_empty() {
            return Ok(LlmResponse::ToolCalls {
                calls: functional_calls,
                text: None,
                stop_reason,
            });
        }

        let function_calls = parse_function_style_tool_calls(&content);
        if !function_calls.is_empty() {
            return Ok(LlmResponse::ToolCalls {
                calls: function_calls,
                text: None,
                stop_reason,
            });
        }

        if content.is_empty()
            && message["tool_calls"]
                .as_array()
                .map_or(false, |a| !a.is_empty())
        {
            return Ok(LlmResponse::Text {
                text: "(tool call)".to_string(),
                stop_reason,
            });
        }

        Ok(LlmResponse::Text {
            text: content,
            stop_reason,
        })
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
    use super::OpenAiCompatibleProvider;
    use serde_json::json;

    fn provider() -> OpenAiCompatibleProvider {
        OpenAiCompatibleProvider::new(
            "test-key".to_string(),
            "test-model".to_string(),
            "https://example.invalid".to_string(),
            128,
            0.0,
            "test".to_string(),
        )
    }

    fn headers_for(label: &str, api_key: &str) -> Vec<(&'static str, String)> {
        OpenAiCompatibleProvider::new(
            api_key.to_string(),
            "test-model".to_string(),
            "https://example.invalid".to_string(),
            128,
            0.0,
            label.to_string(),
        )
        .request_headers()
    }

    #[test]
    fn google_uses_openai_compatibility_auth_and_client_header() {
        let headers = headers_for("google", "gemini-key");
        assert!(headers
            .iter()
            .any(|(name, value)| *name == "Authorization" && value == "Bearer gemini-key"));
        assert!(headers.iter().any(|(name, value)| {
            *name == "x-goog-api-client" && value.starts_with("angelbot-oai/")
        }));
    }

    #[test]
    fn azure_uses_api_key_header_instead_of_bearer_auth() {
        let headers = headers_for("azure", "azure-key");
        assert!(headers
            .iter()
            .any(|(name, value)| *name == "api-key" && value == "azure-key"));
        assert!(!headers.iter().any(|(name, _)| *name == "Authorization"));
    }

    #[test]
    fn local_provider_without_key_sends_no_auth_header() {
        let headers = headers_for("ollama", "");
        assert!(!headers
            .iter()
            .any(|(name, _)| *name == "Authorization" || *name == "api-key"));
    }

    #[test]
    fn rejects_assistant_tool_calls_without_all_tool_results() {
        let body = json!({
            "model": "test-model",
            "messages": [
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "call-1", "type": "function", "function": {"name": "read_file", "arguments": "{}"}}
                ]},
                {"role": "user", "content": "continue"}
            ]
        });

        let error = provider().validate_request(&body).unwrap_err();
        assert!(error.contains("without matching tool results"));
    }

    #[test]
    fn accepts_complete_tool_call_sequence() {
        let body = json!({
            "model": "test-model",
            "messages": [
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "call-1", "type": "function", "function": {"name": "read_file", "arguments": "{}"}},
                    {"id": "call-2", "type": "function", "function": {"name": "list_dir", "arguments": "{}"}}
                ]},
                {"role": "tool", "tool_call_id": "call-1", "content": "contents"},
                {"role": "tool", "tool_call_id": "call-2", "content": "listing"},
                {"role": "user", "content": "continue"}
            ]
        });

        provider().validate_request(&body).unwrap();
    }

    #[test]
    fn stop_reason_is_propagated_from_finish_reason() {
        // finish_reason: "length" → MaxTokens
        let response = provider()
            .parse_response(json!({
                "choices": [{"message": {"content": "halfway"}, "finish_reason": "length"}]
            }))
            .unwrap();
        match response {
            super::LlmResponse::Text { text, stop_reason } => {
                assert_eq!(text, "halfway");
                assert_eq!(stop_reason, super::super::StopReason::MaxTokens);
            }
            _ => panic!("finish_reason=length should produce Text"),
        }

        // finish_reason: "tool_calls" + tool_calls → ToolUse
        let response = provider()
            .parse_response(json!({
                "choices": [{
                    "finish_reason": "tool_calls",
                    "message": {
                        "content": null,
                        "tool_calls": [{
                            "id": "call-1",
                            "type": "function",
                            "function": {"name": "read_file", "arguments": "{\"path\":\"x\"}"}
                        }]
                    }
                }]
            }))
            .unwrap();
        match response {
            super::LlmResponse::ToolCalls {
                calls, stop_reason, ..
            } => {
                assert_eq!(calls.len(), 1);
                assert_eq!(stop_reason, super::super::StopReason::ToolUse);
            }
            _ => panic!("finish_reason=tool_calls should produce ToolCalls"),
        }
    }

    #[test]
    fn missing_finish_reason_defaults_to_end_turn() {
        let response = provider()
            .parse_response(json!({
                "choices": [{"message": {"content": "ok"}}]
            }))
            .unwrap();
        match response {
            super::LlmResponse::Text { stop_reason, .. } => {
                assert_eq!(stop_reason, super::super::StopReason::EndTurn);
            }
            _ => panic!("missing finish_reason should produce Text"),
        }
    }

    #[test]
    fn parses_xml_fallback_tool_call() {
        let response = provider()
            .parse_response(json!({
                "choices": [{"message": {
                    "content": "<tool_call><invoke name=\"read_file\"><parameter name=\"path\">reference.txt</parameter></invoke></tool_call>"
                }}]
            }))
            .unwrap();

        match response {
            super::LlmResponse::ToolCalls { calls, text, .. } => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "read_file");
                assert_eq!(calls[0].arguments["path"], "reference.txt");
                assert!(text.is_none());
            }
            _ => panic!("expected XML fallback to produce a tool call"),
        }
    }

    #[test]
    fn quarantines_textual_named_argument_tool_call_in_fence() {
        let response = provider()
            .parse_response(json!({
                "choices": [{"message": {
                    "content": "Ready to write.\n```tool_call\nwrite_file(path=\"note.txt\", content=\"smoke\")\n```"
                }}]
            }))
            .unwrap();

        match response {
            super::LlmResponse::ProtocolViolation { raw_text, reason } => {
                assert!(raw_text.contains("write_file"));
                assert!(reason.contains("named-argument"));
            }
            other => panic!("textual tool calls must be quarantined, got {other:?}"),
        }
    }

    #[test]
    fn parses_gateway_xml_with_multiple_invocations() {
        let response = provider()
            .parse_response(json!({
                "choices": [{"message": {
                    "content": "<tool_call>\n<invoke name=\"read_file\">\n<parameter name=\"path\" string=\"true\">C:\\temp\\reference\\requirements.txt</parameter>\n</invoke>\n<invoke name=\"read_file\">\n<parameter name=\"path\" string=\"true\">C:\\temp\\reference\\style-guide.txt</parameter>\n</invoke>\n</tool_call>"
                }}]
            }))
            .unwrap();

        match response {
            super::LlmResponse::ToolCalls { calls, .. } => {
                assert_eq!(calls.len(), 2);
                assert_eq!(
                    calls[0].arguments["path"],
                    "C:\\temp\\reference\\requirements.txt"
                );
                assert_eq!(
                    calls[1].arguments["path"],
                    "C:\\temp\\reference\\style-guide.txt"
                );
            }
            _ => panic!("expected gateway XML to produce tool calls"),
        }
    }

    #[test]
    fn parses_function_calls_wrapper_for_the_agent_loop() {
        let response = provider()
            .parse_response(json!({
                "choices": [{"message": {"content": concat!(
                    "<function_calls><invoke name=\"read_file\">",
                    "<parameter name=\"path\">brief.txt</parameter></invoke>",
                    "<invoke name=\"read_file\"><parameter name=\"path\">tone.txt</parameter>",
                    "</invoke></function_calls>"
                )}}]
            }))
            .unwrap();

        match response {
            super::LlmResponse::ToolCalls {
                calls, text: None, ..
            } => {
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[0].name, "read_file");
                assert_eq!(calls[0].arguments["path"], "brief.txt");
                assert_eq!(calls[1].arguments["path"], "tone.txt");
            }
            _ => panic!("expected function_calls wrapper to produce tool calls"),
        }
    }

    #[test]
    fn parses_fenced_json_tool_calls_for_the_agent_loop() {
        let response = provider()
            .parse_response(json!({
                "choices": [{"message": {"content": r#"```json
{"tool_calls":[
  {"id":"call-read-brief","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"brief.txt\"}"}},
  {"id":"call-read-tone","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"tone.txt\"}"}}
]}
```"#}}]
            }))
            .unwrap();

        match response {
            super::LlmResponse::ToolCalls {
                calls, text: None, ..
            } => {
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[0].id, "call-read-brief");
                assert_eq!(calls[0].arguments["path"], "brief.txt");
                assert_eq!(calls[1].id, "call-read-tone");
                assert_eq!(calls[1].arguments["path"], "tone.txt");
            }
            _ => panic!("expected fenced JSON to produce tool calls"),
        }
    }

    #[test]
    fn parses_legacy_json_array_after_prose_for_the_agent_loop() {
        let response = provider()
            .parse_response(json!({
                "choices": [{"message": {"content": r#"I'll start by reading both files simultaneously.

```json
[
  {"name":"read_file","arguments":{"path":"brief.txt"}},
  {"name":"read_file","arguments":{"path":"tone.txt"}}
]
```"#}}]
            }))
            .unwrap();

        match response {
            super::LlmResponse::ToolCalls {
                calls, text: None, ..
            } => {
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[0].name, "read_file");
                assert_eq!(calls[0].arguments["path"], "brief.txt");
                assert_eq!(calls[1].arguments["path"], "tone.txt");
            }
            _ => panic!("expected legacy JSON array to produce tool calls"),
        }
    }

    #[test]
    fn parses_functional_commands_for_the_agent_loop() {
        let response = provider().parse_response(json!({"choices":[{"message":{"content":
            "<functional_commands><command type=\"tool_call\"><tool name=\"read_file\"><param name=\"path\">brief.txt</param></tool></command><command type=\"tool_call\"><tool name=\"read_file\"><param name=\"path\">tone.txt</param></tool></command></functional_commands>"
        }}]})).unwrap();
        match response {
            super::LlmResponse::ToolCalls {
                calls, text: None, ..
            } => {
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[0].arguments["path"], "brief.txt");
                assert_eq!(calls[1].arguments["path"], "tone.txt");
            }
            _ => panic!("expected functional_commands to produce tool calls"),
        }
    }

    #[test]
    fn leaves_incomplete_xml_as_text() {
        let response = provider()
            .parse_response(json!({
                "choices": [{"message": {"content": "<tool_call><invoke name=\"read_file\">"}}]
            }))
            .unwrap();

        assert!(matches!(response, super::LlmResponse::Text { text, .. }));
    }

    #[test]
    fn hides_anthropic_thinking_and_parses_named_tool_call() {
        let response = provider()
            .parse_response(json!({
                "choices": [{"message": {"content": concat!(
                    "<anthropic:thinking>private plan</anthropic:thinking>",
                    "<tool_call><invoke_name>read_file</invoke_name><parameters>",
                    "<path>brief.txt</path></parameters></tool_call>"
                )}}]
            }))
            .unwrap();

        match response {
            super::LlmResponse::ToolCalls { calls, text, .. } => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "read_file");
                assert_eq!(calls[0].arguments["path"], "brief.txt");
                assert!(text.is_none());
            }
            _ => panic!("expected named XML tool call"),
        }
    }

    #[test]
    fn strips_thinking_from_plain_text_response() {
        let response = provider()
            .parse_response(json!({
                "choices": [{"message": {"content": "<anthropic:thinking>private</anthropic:thinking>Visible reply"}}]
            }))
            .unwrap();

        assert!(
            matches!(response, super::LlmResponse::Text { text, .. } if text == "Visible reply")
        );
    }

    #[test]
    fn parses_function_style_tool_calls_for_the_agent_loop() {
        let response = provider()
            .parse_response(json!({
                "choices": [{"message": {"content": "先读取文件。\nread_file({\"path\":\"brief.txt\"})\nread_file({\"path\":\"tone.txt\"})"}}]
            }))
            .unwrap();

        match response {
            super::LlmResponse::ToolCalls { calls, text, .. } => {
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[0].name, "read_file");
                assert_eq!(calls[0].arguments["path"], "brief.txt");
                assert_eq!(calls[1].arguments["path"], "tone.txt");
                assert!(text.is_none());
            }
            _ => panic!("expected function-style tool calls"),
        }
    }

    #[test]
    fn leaves_ordinary_function_prose_as_text() {
        let response = provider()
            .parse_response(json!({
                "choices": [{"message": {"content": "Use read_file({\"path\":\"brief.txt\"}) when needed."}}]
            }))
            .unwrap();

        assert!(matches!(response, super::LlmResponse::Text { text, .. }));
    }
}

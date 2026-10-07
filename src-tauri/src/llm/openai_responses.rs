//! Stateless Responses adapter. Credentials and OAuth refresh live behind the
//! credential seam; no request is retried here, especially after external tools.
//!
//! Protocol references (checked 2026-10-04):
//! https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations
//! https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference
//! https://developers.openai.com/api/docs/guides/function-calling
//! https://developers.openai.com/api/docs/guides/reasoning
//! https://developers.openai.com/api/docs/guides/streaming-responses

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{HeaderValue, AUTHORIZATION};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::auth::ModelCredentialSource;
use super::protocol::{MAX_CONTINUATION_BYTES, MAX_CONTINUATION_ITEMS};
use super::{
    LlmProvider, LlmResponse, Message, ModelAuthMode, ModelProtocol, ProtocolContinuation,
    ProviderConfig, ProviderUsage, StopReason, ThinkingEffort, ThinkingSettings, ToolCall,
    ToolSchema,
};

const PLAN_ENDPOINT: &str = "https://api.openai.com/v1/responses";
const TOOL_NAMESPACE: &str = "angelbot";
const MAX_BUFFER_BYTES: usize = 2 * 1024 * 1024;
const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;
const MAX_ERROR_BYTES: usize = 256 * 1024;
use super::{MAX_TOOL_IMAGES, MAX_TOOL_IMAGE_TOTAL_BYTES as MAX_TOOL_IMAGE_BYTES};
const INVALID_RESPONSE: &str = "Responses 返回的协议数据无效；未提交工具调用。";
const INVALID_HISTORY: &str = "Responses 对话历史中的工具调用顺序或私有续接数据无效。";

fn invalid_response_at(stage: &'static str) -> String {
    // Stages are static allowlisted labels, never remote field values.
    format!("{INVALID_RESPONSE} 校验阶段：{stage}。")
}

fn request_transport_error(error: reqwest::Error) -> String {
    let kind = if error.is_timeout() {
        "请求超时"
    } else if error.is_connect() {
        "连接建立失败（网络、代理或 TLS）"
    } else {
        "请求发送中断"
    };
    format!("Responses {kind}；不会自动重试，请确认先前外部操作的状态。")
}

pub struct OpenAiResponsesProvider {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    provider_name: String,
    model: String,
    auth_mode: ModelAuthMode,
    credentials: Arc<dyn ModelCredentialSource>,
    max_tokens: u32,
    temperature: f64,
    last_usage: Mutex<Option<ProviderUsage>>,
}

impl OpenAiResponsesProvider {
    pub fn new(
        config: &ProviderConfig,
        credentials: Arc<dyn ModelCredentialSource>,
    ) -> Result<Self, String> {
        if config.model.trim().is_empty() || config.model.len() > 200 {
            return Err("Responses 需要有效的模型名称。".into());
        }
        let reference = credentials.credential_ref();
        if reference.trim().is_empty()
            || reference.len() > 256
            || config
                .credential_ref
                .as_deref()
                .is_some_and(|configured| configured != reference)
        {
            return Err("Responses 凭据引用与当前账号不匹配。".into());
        }
        let endpoint = responses_endpoint(&config.base_url, config.auth_mode)?;
        let client = reqwest::Client::builder()
            // Never forward bearer credentials to a redirect target.
            .redirect(reqwest::redirect::Policy::none())
            // Disable even reqwest's default protocol-NACK retries. A caller
            // must decide explicitly whether a whole model turn is safe to retry.
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(600))
            .build()
            .map_err(|_| "无法初始化 Responses 连接。".to_string())?;
        Ok(Self {
            client,
            endpoint,
            provider_name: config.provider.clone(),
            model: config.model.clone(),
            auth_mode: config.auth_mode,
            credentials,
            max_tokens: config.max_tokens,
            temperature: config.temperature,
            last_usage: Mutex::new(None),
        })
    }

    fn request_body(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
    ) -> Result<Value, String> {
        let (input, instructions) = encode_messages(
            messages,
            &self.model,
            self.credentials.credential_ref(),
            self.auth_mode,
        )?;
        let mut body = json!({
            "model": self.model,
            "input": input,
            "store": false,
            "stream": true,
        });
        if !instructions.is_empty() {
            body["instructions"] = Value::String(instructions.join("\n\n"));
        }
        if let Some(tools) = tools.filter(|tools| !tools.is_empty()) {
            let mut names = HashSet::new();
            let mut functions = Vec::with_capacity(tools.len());
            for tool in tools {
                if !valid_tool_name(&tool.name)
                    || !names.insert(&tool.name)
                    || !tool.parameters.is_object()
                {
                    return Err("Responses 工具定义包含无效或重复的名称/参数。".into());
                }
                functions.push(json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                    // Existing AngelBot schemas are not necessarily strict-schema
                    // compatible. Do not silently rewrite their semantics.
                    "strict": false,
                }));
            }
            body["tools"] = if self.auth_mode == ModelAuthMode::ChatgptPlan {
                json!([{
                    "type": "namespace",
                    "name": TOOL_NAMESPACE,
                    "description": "Local AngelBot tools, executed by the application.",
                    "tools": functions,
                }])
            } else {
                Value::Array(functions)
            };
        }
        // SIWC preview rejects these fields regardless of model capability.
        if self.auth_mode != ModelAuthMode::ChatgptPlan {
            if self.max_tokens > 0 {
                body["max_output_tokens"] = json!(self.max_tokens);
            }
            if supports_temperature(&self.model) {
                if !self.temperature.is_finite() || !(0.0..=2.0).contains(&self.temperature) {
                    return Err("Responses temperature 必须介于 0 到 2。".into());
                }
                body["temperature"] = json!(self.temperature);
            }
        }
        if supports_reasoning(&self.model) {
            if let Some(thinking) = thinking {
                let effort = match thinking.effort {
                    ThinkingEffort::Low => "low",
                    ThinkingEffort::Medium => "medium",
                    ThinkingEffort::High => "high",
                };
                body["reasoning"] = json!({"effort": effort});
            }
        }
        // With store:false the current API returns encrypted reasoning by
        // default. No previous_response_id or account-crossing storage is used.
        if serde_json::to_vec(&body)
            .map_err(|_| INVALID_HISTORY)?
            .len()
            > MAX_STREAM_BYTES
        {
            return Err("Responses 对话历史过大，请缩短上下文后再试。".into());
        }
        Ok(body)
    }

    async fn request(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
        delta_sender: Option<UnboundedSender<String>>,
    ) -> Result<LlmResponse, String> {
        *self.last_usage.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let body = self.request_body(messages, tools, thinking)?;
        let mut request = self
            .client
            .post(self.endpoint.clone())
            .header("Accept", "text/event-stream")
            .json(&body);
        if self.auth_mode != ModelAuthMode::None {
            // Resolve on EVERY request. Refresh failures stop before inference;
            // this adapter neither retries inference nor changes billing mode.
            let token = self.credentials.bearer_token().await.map_err(|_| {
                if self.auth_mode == ModelAuthMode::ChatgptPlan {
                    "ChatGPT 登录凭据读取或刷新失败，请检查连接状态或重新登录；本次请求未发送。"
                } else {
                    "Responses API 凭据读取失败；本次请求未发送。"
                }
            })?;
            if token.trim().is_empty() {
                return Err("Responses 缺少有效登录凭据；本次请求未发送。".into());
            }
            let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| "Responses 登录凭据格式无效；本次请求未发送。")?;
            authorization.set_sensitive(true);
            request = request.header(AUTHORIZATION, authorization);
        }
        let mut response = request.send().await.map_err(request_transport_error)?;
        if !response.status().is_success() {
            return Err(read_http_error(response).await);
        }
        let content_type_header = response.headers().get(reqwest::header::CONTENT_TYPE);
        let content_type = content_type_header
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        let missing_content_type = content_type_header.is_none()
            || content_type_header
                .is_some_and(|value| value.to_str().is_ok_and(|value| value.trim().is_empty()));
        if !content_type
            .split(';')
            .next()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
            && !missing_content_type
        {
            return Err(read_non_sse_error(response).await);
        }
        let status = response.status().as_u16();
        let transport_error = |error: String| {
            if missing_content_type {
                format!("Responses 响应类型：缺失（HTTP {status}）；{error}")
            } else {
                error
            }
        };
        let allowed_tools = tools
            .unwrap_or_default()
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        let mut parser = ResponsesStream::new(
            &self.model,
            self.credentials.credential_ref(),
            self.auth_mode,
            allowed_tools,
        );
        // Some actual plan responses omit Content-Type. Reuse the same SSE
        // decoder and completion validation, but reject non-SSE framing rather
        // than treating a successful HTTP status or arbitrary body as output.
        parser.strict_framing = missing_content_type;
        parser.prior_call_ids = messages
            .iter()
            .filter_map(|message| message.tool_calls.as_ref())
            .flatten()
            .map(|call| call.id.clone())
            .collect();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            let kind = if error.is_timeout() {
                "读取超时"
            } else {
                "读取中断"
            };
            transport_error(format!(
                "Responses 事件流{kind}；未提交工具调用，不会自动重试外部操作。"
            ))
        })? {
            parser
                .feed(&chunk, delta_sender.as_ref())
                .map_err(&transport_error)?;
            if let Some(completed) = parser.completed.take() {
                return self.accept_completed(completed);
            }
        }
        if missing_content_type && parser.total_bytes == 0 {
            return Err(transport_error(
                "Responses 返回空响应；未提交工具调用。".into(),
            ));
        }
        parser.finish().map_err(transport_error)?;
        Err("Responses 事件流结束但没有 response.completed；未提交工具调用。".into())
    }

    fn accept_completed(&self, completed: CompletedResponse) -> Result<LlmResponse, String> {
        self.credentials.validate_session().map_err(|_| {
            "模型会话已失效，请重新连接账号；未提交工具调用，也不会自动重试外部操作。".to_string()
        })?;
        *self.last_usage.lock().unwrap_or_else(|e| e.into_inner()) = completed.usage;
        Ok(completed.response)
    }
}

#[async_trait]
impl LlmProvider for OpenAiResponsesProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    fn supports_tool_images(&self) -> bool {
        // This describes the wire protocol, not every remotely selected model.
        true
    }

    fn latest_usage(&self) -> Option<ProviderUsage> {
        *self.last_usage.lock().unwrap_or_else(|e| e.into_inner())
    }

    async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
    ) -> Result<LlmResponse, String> {
        self.request(messages, tools, thinking, None).await
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
        delta_sender: Option<UnboundedSender<String>>,
    ) -> Result<LlmResponse, String> {
        self.request(messages, tools, thinking, delta_sender).await
    }
}

fn responses_endpoint(base: &str, mode: ModelAuthMode) -> Result<reqwest::Url, String> {
    let base = base.trim();
    let mut endpoint = reqwest::Url::parse(if base.is_empty() { PLAN_ENDPOINT } else { base })
        .map_err(|_| "Responses 服务地址无效。")?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err("Responses 服务地址不得包含登录信息、查询或片段。".into());
    }
    if mode == ModelAuthMode::ChatgptPlan {
        if endpoint.scheme() != "https"
            || endpoint.host_str() != Some("api.openai.com")
            || endpoint.port_or_known_default() != Some(443)
            || !matches!(
                endpoint.path(),
                "" | "/" | "/v1" | "/v1/" | "/v1/responses" | "/v1/responses/"
            )
        {
            return Err("ChatGPT 套餐仅支持官方 https://api.openai.com/v1/responses 端点。".into());
        }
        return reqwest::Url::parse(PLAN_ENDPOINT).map_err(|_| "Responses 服务地址无效。".into());
    }
    let path = endpoint.path().trim_end_matches('/').to_string();
    if path.is_empty() {
        endpoint.set_path("/v1/responses");
    } else if !path.ends_with("/responses") {
        endpoint.set_path(&format!("{path}/responses"));
    } else {
        endpoint.set_path(&path);
    }
    Ok(endpoint)
}

fn supports_temperature(model: &str) -> bool {
    // Positive known-model capability, not a guess that future reasoning
    // models accept sampling fields. GPT-5/6 defaults are not effort:none.
    let model = model.to_ascii_lowercase();
    model == "gpt-4.1"
        || model.starts_with("gpt-4.1-")
        || model == "gpt-4o"
        || model.starts_with("gpt-4o-")
        || model == "gpt-4.5-preview"
        || model.starts_with("gpt-4.5-preview-")
}

fn supports_reasoning(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.starts_with("gpt-5")
        || model.starts_with("gpt-6")
        || model == "o1"
        || model.starts_with("o1-")
        || model == "o3"
        || model.starts_with("o3-")
        || model == "o4-mini"
        || model.starts_with("o4-mini-")
}

fn valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_call_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'"' && byte != b'\\')
}

fn local_call_name(item: &Value, mode: ModelAuthMode) -> Result<String, String> {
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .ok_or(INVALID_RESPONSE)?;
    let namespace = match item.get("namespace") {
        None | Some(Value::Null) => None,
        Some(Value::String(namespace)) => Some(namespace.as_str()),
        _ => return Err(INVALID_RESPONSE.into()),
    };
    let (qualified_namespace, local_name) = match name.split_once('.') {
        Some((namespace, name)) => (Some(namespace), name),
        None => (None, name),
    };
    if namespace.is_some_and(|namespace| namespace != TOOL_NAMESPACE)
        || qualified_namespace.is_some_and(|namespace| namespace != TOOL_NAMESPACE)
        || (mode != ModelAuthMode::ChatgptPlan
            && (namespace.is_some() || qualified_namespace.is_some()))
        || !valid_tool_name(local_name)
    {
        return Err("Responses 返回了未知工具 namespace 或无效工具名称；未提交工具调用。".into());
    }
    Ok(local_name.to_string())
}

struct ParsedOutput {
    text: String,
    calls: Vec<ToolCall>,
    replay: Vec<Value>,
    refusal: bool,
}

fn parse_output_items(
    items: &[Value],
    mode: ModelAuthMode,
    allowed_tools: Option<&HashSet<String>>,
) -> Result<ParsedOutput, String> {
    let mut parsed = ParsedOutput {
        text: String::new(),
        calls: Vec::new(),
        replay: Vec::with_capacity(items.len()),
        refusal: false,
    };
    let mut item_ids = HashSet::new();
    let mut call_ids = HashSet::new();
    for item in items {
        let mut replay = item.as_object().ok_or(INVALID_RESPONSE)?.clone();
        if let Some(id) = item.get("id") {
            let id = id
                .as_str()
                .filter(|id| valid_call_id(id))
                .ok_or(INVALID_RESPONSE)?;
            if !item_ids.insert(id) {
                return Err("Responses 返回了重复 output item ID；未提交工具调用。".into());
            }
        }
        if item
            .get("status")
            .is_some_and(|status| !status.is_null() && status.as_str() != Some("completed"))
        {
            return Err("Responses output item 尚未完成；未提交工具调用。".into());
        }
        replay.remove("status");
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                if item.get("role").and_then(Value::as_str) != Some("assistant") {
                    return Err(INVALID_RESPONSE.into());
                }
                if item.get("phase").is_some_and(|phase| {
                    !phase.is_null()
                        && !matches!(phase.as_str(), Some("commentary" | "final_answer"))
                }) {
                    return Err(invalid_response_at("message phase"));
                }
                let contents = item
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or(INVALID_RESPONSE)?;
                let mut replay_contents = Vec::with_capacity(contents.len());
                for content in contents {
                    let mut content_replay = content.as_object().ok_or(INVALID_RESPONSE)?.clone();
                    match content.get("type").and_then(Value::as_str) {
                        Some("output_text") => parsed.text.push_str(
                            content
                                .get("text")
                                .and_then(Value::as_str)
                                .ok_or(INVALID_RESPONSE)?,
                        ),
                        Some("refusal") => {
                            parsed.refusal = true;
                            parsed.text.push_str(
                                content
                                    .get("refusal")
                                    .and_then(Value::as_str)
                                    .ok_or(INVALID_RESPONSE)?,
                            );
                        }
                        _ => return Err(INVALID_RESPONSE.into()),
                    }
                    content_replay.remove("annotations");
                    content_replay.remove("logprobs");
                    replay_contents.push(Value::Object(content_replay));
                }
                replay.insert("content".into(), Value::Array(replay_contents));
            }
            Some("reasoning") => {
                if !item.get("summary").is_some_and(Value::is_array)
                    || item
                        .get("encrypted_content")
                        .is_some_and(|content| !content.is_null() && !content.is_string())
                {
                    return Err(INVALID_RESPONSE.into());
                }
                if item["summary"].as_array().unwrap().iter().any(|summary| {
                    summary.get("type").and_then(Value::as_str) != Some("summary_text")
                        || !summary.get("text").is_some_and(Value::is_string)
                }) {
                    return Err(INVALID_RESPONSE.into());
                }
                // Keep encrypted_content, summary, IDs and any future opaque
                // reasoning fields. None are ever streamed to the UI.
            }
            Some("function_call") => {
                let id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .filter(|id| valid_call_id(id))
                    .ok_or(INVALID_RESPONSE)?;
                if !call_ids.insert(id) {
                    return Err("Responses 返回了重复 function call ID；未提交工具调用。".into());
                }
                let name = local_call_name(item, mode)?;
                if allowed_tools.is_some_and(|tools| !tools.contains(&name)) {
                    return Err("Responses 调用了本次未提供的工具；未提交工具调用。".into());
                }
                let arguments: Value = serde_json::from_str(
                    item.get("arguments")
                        .and_then(Value::as_str)
                        .ok_or(INVALID_RESPONSE)?,
                )
                .map_err(|_| "Responses 函数参数 JSON 无效或被截断；未提交工具调用。")?;
                if !arguments.is_object() {
                    return Err("Responses 函数参数必须是 JSON 对象；未提交工具调用。".into());
                }
                parsed.calls.push(ToolCall {
                    id: id.to_string(),
                    name,
                    arguments,
                });
            }
            // No hosted tool or custom-call authority is granted by this
            // adapter, which only advertises local function schemas.
            _ => return Err(INVALID_RESPONSE.into()),
        }
        parsed.replay.push(Value::Object(replay));
    }
    if parsed.refusal && !parsed.calls.is_empty() {
        return Err("Responses 同时返回拒绝与工具调用；未提交工具调用。".into());
    }
    Ok(parsed)
}

fn same_calls(left: &[ToolCall], right: &[ToolCall]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.id == right.id && left.name == right.name && left.arguments == right.arguments
        })
}

fn encode_messages(
    messages: &[Message],
    model: &str,
    credential_ref: &str,
    mode: ModelAuthMode,
) -> Result<(Vec<Value>, Vec<String>), String> {
    let mut input = Vec::new();
    let mut instructions = Vec::new();
    let mut call_ids = HashSet::new();
    let mut pending = HashMap::new();
    let mut image_count = 0usize;
    let mut image_bytes = 0usize;
    for message in messages {
        if message.role != "assistant"
            && (message.protocol_state.is_some() || message.tool_calls.is_some())
            || message.role != "tool" && message.tool_call_id.is_some()
            || message.role != "tool" && !pending.is_empty()
            || message.role != "tool" && !message.tool_images.is_empty()
        {
            return Err(INVALID_HISTORY.into());
        }
        image_count = image_count.saturating_add(message.tool_images.len());
        for image in &message.tool_images {
            image_bytes = image_bytes.saturating_add(image.byte_len());
        }
        if image_count > MAX_TOOL_IMAGES || image_bytes > MAX_TOOL_IMAGE_BYTES {
            return Err(
                "Responses 工具图像超过请求预算（最多 4 张、合计 4 MiB）；本次请求未发送。".into(),
            );
        }
        match message.role.as_str() {
            "system" => instructions.push(message.content.clone()),
            "developer" | "user" => input.push(json!({
                "role": message.role,
                "content": message.content,
            })),
            "assistant" => {
                let calls = message.tool_calls.as_deref().unwrap_or_default();
                if let Some(state) = message.protocol_state.as_ref().filter(|state| {
                    state.protocol == ModelProtocol::OpenaiResponses
                        && state.matches(model, credential_ref)
                }) {
                    state.validate().map_err(|_| INVALID_HISTORY)?;
                    let parsed = parse_output_items(&state.output_items, mode, None)
                        .map_err(|_| INVALID_HISTORY)?;
                    // Private continuation is replay state, never a second
                    // source of tool authority or duplicate visible messages.
                    if !same_calls(&parsed.calls, calls) {
                        return Err(INVALID_HISTORY.into());
                    }
                    input.extend(parsed.replay);
                } else {
                    // A model/account switch MUST NOT copy opaque reasoning,
                    // output IDs, phases or other private protocol state.
                    if !message.content.is_empty() {
                        input.push(json!({"role": "assistant", "content": message.content}));
                    }
                    for call in calls {
                        if !valid_tool_name(&call.name) || !call.arguments.is_object() {
                            return Err(INVALID_HISTORY.into());
                        }
                        let mut item = json!({
                            "type": "function_call",
                            "call_id": call.id,
                            "name": call.name,
                            "arguments": serde_json::to_string(&call.arguments).map_err(|_| INVALID_HISTORY)?,
                        });
                        if mode == ModelAuthMode::ChatgptPlan {
                            item["namespace"] = json!(TOOL_NAMESPACE);
                        }
                        input.push(item);
                    }
                }
                for call in calls {
                    if !valid_call_id(&call.id) || !call_ids.insert(call.id.clone()) {
                        return Err(INVALID_HISTORY.into());
                    }
                    pending.insert(call.id.clone(), ());
                }
            }
            "tool" => {
                let id = message.tool_call_id.as_ref().ok_or(INVALID_HISTORY)?;
                if pending.remove(id).is_none() {
                    return Err(INVALID_HISTORY.into());
                }
                let output = if message.tool_images.is_empty() {
                    // Preserve the existing text-only wire contract exactly.
                    Value::String(message.content.clone())
                } else {
                    let mut content = Vec::with_capacity(message.tool_images.len() + 1);
                    content.push(json!({"type": "input_text", "text": message.content}));
                    for image in &message.tool_images {
                        content.push(json!({
                            "type": "input_image",
                            "image_url": image.data_url(),
                            "detail": "auto",
                        }));
                    }
                    Value::Array(content)
                };
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": id,
                    "output": output,
                }));
            }
            _ => return Err(INVALID_HISTORY.into()),
        }
    }
    if !pending.is_empty() {
        return Err(INVALID_HISTORY.into());
    }
    Ok((input, instructions))
}

struct CompletedResponse {
    response: LlmResponse,
    usage: Option<ProviderUsage>,
}

fn parse_completed(
    response: &Value,
    model: &str,
    credential_ref: &str,
    mode: ModelAuthMode,
    allowed_tools: &HashSet<String>,
) -> Result<CompletedResponse, String> {
    if response.get("object").and_then(Value::as_str) != Some("response") {
        return Err(invalid_response_at("完成对象"));
    }
    if response.get("status").and_then(Value::as_str) != Some("completed") {
        return Err(invalid_response_at("完成状态"));
    }
    if !response
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(valid_call_id)
    {
        return Err(invalid_response_at("完成 ID"));
    }
    if response.get("error").is_some_and(|error| !error.is_null()) {
        return Err(invalid_response_at("完成 error"));
    }
    if response
        .get("incomplete_details")
        .is_some_and(|details| !details.is_null())
    {
        return Err(invalid_response_at("完成 incomplete_details"));
    }
    let items = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_response_at("output 容器"))?;
    let protocol = ProtocolContinuation {
        protocol: ModelProtocol::OpenaiResponses,
        model: model.to_string(),
        credential_ref: credential_ref.to_string(),
        output_items: items.clone(),
    };
    protocol
        .validate()
        .map_err(|error| format!("{} 校验阶段：私有续接数据（{error}）。", INVALID_RESPONSE))?;
    let parsed = parse_output_items(items, mode, Some(allowed_tools))
        .map_err(|error| format!("{error} 校验阶段：output item。"))?;
    let usage = match response.get("usage") {
        None | Some(Value::Null) => None,
        Some(usage) => {
            let input = usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .ok_or_else(|| invalid_response_at("usage 字段"))?;
            let output = usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .ok_or_else(|| invalid_response_at("usage 字段"))?;
            Some(ProviderUsage {
                input_tokens: u32::try_from(input)
                    .map_err(|_| invalid_response_at("usage 范围"))?,
                output_tokens: u32::try_from(output)
                    .map_err(|_| invalid_response_at("usage 范围"))?,
            })
        }
    };
    let result = if parsed.calls.is_empty() {
        LlmResponse::Text {
            text: parsed.text,
            stop_reason: if parsed.refusal {
                StopReason::ContentFilter
            } else {
                StopReason::EndTurn
            },
        }
    } else {
        LlmResponse::ToolCalls {
            calls: parsed.calls,
            text: if parsed.text.is_empty() {
                None
            } else {
                Some(parsed.text)
            },
            stop_reason: StopReason::ToolUse,
        }
    };
    Ok(CompletedResponse {
        response: LlmResponse::WithProtocol {
            response: Box::new(result),
            protocol,
        },
        usage,
    })
}

/// Byte-first SSE framing retains split UTF-8 until a complete line exists.
/// It supports LF, CRLF, bare CR and multi-line data without keeping the full
/// stream. Limits apply both to the in-flight event and cumulative bytes.
struct ResponsesStream {
    line: Vec<u8>,
    data: Vec<u8>,
    event_name: Option<String>,
    after_cr: bool,
    first_line: bool,
    strict_framing: bool,
    total_bytes: usize,
    terminal: bool,
    completed: Option<CompletedResponse>,
    model: String,
    credential_ref: String,
    mode: ModelAuthMode,
    allowed_tools: HashSet<String>,
    prior_call_ids: HashSet<String>,
    response_id: Option<String>,
    started_items: BTreeMap<usize, Option<String>>,
    done_items: BTreeMap<usize, Value>,
    done_bytes: usize,
}

impl ResponsesStream {
    fn new(
        model: &str,
        credential_ref: &str,
        mode: ModelAuthMode,
        allowed_tools: HashSet<String>,
    ) -> Self {
        Self {
            line: Vec::new(),
            data: Vec::new(),
            event_name: None,
            after_cr: false,
            first_line: true,
            strict_framing: false,
            total_bytes: 0,
            terminal: false,
            completed: None,
            model: model.to_string(),
            credential_ref: credential_ref.to_string(),
            mode,
            allowed_tools,
            prior_call_ids: HashSet::new(),
            response_id: None,
            started_items: BTreeMap::new(),
            done_items: BTreeMap::new(),
            done_bytes: 0,
        }
    }

    fn feed(
        &mut self,
        chunk: &[u8],
        sender: Option<&UnboundedSender<String>>,
    ) -> Result<(), String> {
        self.total_bytes = self
            .total_bytes
            .checked_add(chunk.len())
            .ok_or(INVALID_RESPONSE)?;
        if self.total_bytes > MAX_STREAM_BYTES {
            return Err("Responses 事件流超过大小限制；未提交工具调用。".into());
        }
        for byte in chunk {
            if self.after_cr {
                self.after_cr = false;
                if *byte == b'\n' {
                    continue;
                }
            }
            match *byte {
                b'\r' => {
                    self.consume_line(sender)?;
                    self.after_cr = true;
                }
                b'\n' => self.consume_line(sender)?,
                byte => {
                    if self.line.len()
                        + self.data.len()
                        + self.event_name.as_ref().map_or(0, String::len)
                        >= MAX_BUFFER_BYTES
                    {
                        return Err("Responses 单个 SSE 事件超过大小限制；未提交工具调用。".into());
                    }
                    self.line.push(byte);
                }
            }
            if self.line.len() + self.data.len() + self.event_name.as_ref().map_or(0, String::len)
                > MAX_BUFFER_BYTES
            {
                return Err("Responses 单个 SSE 事件超过大小限制；未提交工具调用。".into());
            }
        }
        Ok(())
    }

    fn consume_line(&mut self, sender: Option<&UnboundedSender<String>>) -> Result<(), String> {
        let line = std::mem::take(&mut self.line);
        let first_line = std::mem::replace(&mut self.first_line, false);
        if line.is_empty() {
            return self.consume_event(sender);
        }
        let line = std::str::from_utf8(&line)
            .map_err(|_| "Responses SSE 包含无效 UTF-8；未提交工具调用。")?;
        let line = if first_line {
            line.strip_prefix('\u{feff}').unwrap_or(line)
        } else {
            line
        };
        if line.is_empty() {
            return self.consume_event(sender);
        }
        if line.starts_with(':') {
            return Ok(());
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => {
                self.data.extend_from_slice(value.as_bytes());
                self.data.push(b'\n');
            }
            "event" => self.event_name = Some(value.to_string()),
            "id" | "retry" => {}
            _ if self.strict_framing => {
                return Err("Responses 正文不是合法 SSE 事件流；未提交工具调用。".into());
            }
            _ => {}
        }
        Ok(())
    }

    fn consume_event(&mut self, sender: Option<&UnboundedSender<String>>) -> Result<(), String> {
        let event_name = self.event_name.take();
        if self.data.is_empty() {
            return Ok(());
        }
        let mut data = std::mem::take(&mut self.data);
        data.pop();
        if data == b"[DONE]" {
            return if self.terminal {
                Ok(())
            } else {
                Err("Responses 在 response.completed 之前结束；未提交工具调用。".into())
            };
        }
        let event: Value = serde_json::from_slice(&data)
            .map_err(|_| "Responses SSE 事件 JSON 无效；未提交工具调用。")?;
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_response_at("事件 type"))?;
        if event_name
            .as_deref()
            .is_some_and(|name| !name.is_empty() && name != kind)
        {
            return Err(invalid_response_at("事件名称"));
        }
        if self.terminal {
            return Err(invalid_response_at("终态后事件"));
        }
        match kind {
            "response.created" => {
                if let Some(id) = event.pointer("/response/id") {
                    let id = id
                        .as_str()
                        .filter(|id| valid_call_id(id))
                        .ok_or_else(|| invalid_response_at("创建 ID"))?;
                    if self.response_id.as_deref().is_some_and(|prior| prior != id) {
                        return Err(invalid_response_at("响应 ID 冲突"));
                    }
                    self.response_id = Some(id.to_string());
                }
            }
            "response.output_item.added" | "response.output_item.done" => {
                let index = event
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .and_then(|index| usize::try_from(index).ok())
                    .filter(|index| *index < MAX_CONTINUATION_ITEMS)
                    .ok_or_else(|| invalid_response_at("output item 索引"))?;
                let item = event
                    .get("item")
                    .filter(|item| item.is_object())
                    .ok_or_else(|| invalid_response_at("output item 对象"))?;
                let id = match item.get("id") {
                    None => None,
                    Some(id) => Some(
                        id.as_str()
                            .filter(|id| valid_call_id(id))
                            .ok_or_else(|| invalid_response_at("output item ID"))?,
                    ),
                };
                if kind == "response.output_item.added" {
                    if self
                        .started_items
                        .insert(index, id.map(str::to_string))
                        .is_some()
                        || self.done_items.contains_key(&index)
                    {
                        return Err(invalid_response_at("output item 重复起始"));
                    }
                } else {
                    if self.done_items.contains_key(&index) {
                        return Err(invalid_response_at("output item 重复完成"));
                    }
                    if self
                        .started_items
                        .get(&index)
                        .and_then(Option::as_deref)
                        .is_some_and(|started| Some(started) != id)
                    {
                        return Err(invalid_response_at("output item ID 冲突"));
                    }
                    let size = serde_json::to_vec(item)
                        .map_err(|_| invalid_response_at("output item 编码"))?
                        .len();
                    self.done_bytes = self
                        .done_bytes
                        .checked_add(size)
                        .filter(|size| *size <= MAX_CONTINUATION_BYTES)
                        .ok_or_else(|| invalid_response_at("output item 总大小"))?;
                    self.done_items.insert(index, item.clone());
                }
            }
            "response.completed" => {
                let mut response = event
                    .get("response")
                    .ok_or_else(|| invalid_response_at("完成事件 response"))?
                    .clone();
                if self
                    .response_id
                    .as_deref()
                    .is_some_and(|id| response.get("id").and_then(Value::as_str) != Some(id))
                {
                    return Err(invalid_response_at("响应 ID 冲突"));
                }
                // Like Codex, distinguish full item completion from response
                // lifecycle completion. Some direct plan streams omit the
                // consolidated output array. Never promote argument deltas or
                // unfinished items, and still require the validated terminal.
                if response
                    .get("output")
                    .and_then(Value::as_array)
                    .is_some_and(Vec::is_empty)
                    && !self.done_items.is_empty()
                {
                    if self
                        .started_items
                        .keys()
                        .any(|index| !self.done_items.contains_key(index))
                        || self
                            .done_items
                            .keys()
                            .enumerate()
                            .any(|(expected, index)| expected != *index)
                    {
                        return Err(invalid_response_at("output item 未完整完成"));
                    }
                    response["output"] =
                        Value::Array(std::mem::take(&mut self.done_items).into_values().collect());
                }
                let completed = parse_completed(
                    &response,
                    &self.model,
                    &self.credential_ref,
                    self.mode,
                    &self.allowed_tools,
                )?;
                if let LlmResponse::WithProtocol { response, .. } = &completed.response {
                    if let LlmResponse::ToolCalls { calls, .. } = response.as_ref() {
                        if calls
                            .iter()
                            .any(|call| self.prior_call_ids.contains(&call.id))
                        {
                            return Err("Responses 重复了历史中的工具调用 ID；未提交工具调用，不会自动重放外部操作。".into());
                        }
                    }
                }
                self.completed = Some(completed);
                self.terminal = true;
            }
            "response.failed" => {
                return Err(structured_error(
                    event.pointer("/response/error").unwrap_or(&Value::Null),
                    None,
                ));
            }
            "response.incomplete" => {
                return Err(
                    match event
                        .pointer("/response/incomplete_details/reason")
                        .and_then(Value::as_str)
                    {
                        Some("max_output_tokens") => {
                            "Responses 达到输出上限，函数参数可能不完整；未提交工具调用。".into()
                        }
                        Some("content_filter") => {
                            "Responses 被安全过滤中止；未提交工具调用。".into()
                        }
                        _ => "Responses 未完整完成；未提交工具调用。".into(),
                    },
                );
            }
            "error" => return Err(structured_error(event.get("error").unwrap_or(&event), None)),
            "response.output_text.delta" => {
                let delta = event
                    .get("delta")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid_response_at("文本 delta"))?;
                if let Some(sender) = sender {
                    sender
                        .send(delta.to_string())
                        .map_err(|_| "Responses 对话已取消；未提交工具调用。")?;
                }
            }
            // Argument deltas are NOT parsed/executed. Only a validated
            // terminal response (including full done-item reconstruction)
            // can yield ToolCall values.
            kind if kind.starts_with("response.") => {}
            _ => return Err(invalid_response_at("事件类型")),
        }
        Ok(())
    }

    fn finish(&self) -> Result<(), String> {
        if !self.line.is_empty() || !self.data.is_empty() || self.event_name.is_some() {
            return Err("Responses SSE 事件被截断；未提交工具调用。".into());
        }
        if !self.terminal {
            return Err("Responses 事件流结束但没有 response.completed；未提交工具调用。".into());
        }
        Ok(())
    }
}

async fn read_http_error(mut response: reqwest::Response) -> String {
    let status = response.status().as_u16();
    let bytes = match read_error_body(&mut response).await {
        Ok(bytes) => bytes,
        Err(_) => return structured_error(&Value::Null, Some(status)),
    };
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    structured_error(value.get("error").unwrap_or(&value), Some(status))
}

async fn read_error_body(response: &mut reqwest::Response) -> Result<Vec<u8>, &'static str> {
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if bytes.len() + chunk.len() <= MAX_ERROR_BYTES => {
                bytes.extend_from_slice(&chunk)
            }
            Ok(Some(_)) => return Err("正文超过诊断上限"),
            Ok(None) => return Ok(bytes),
            Err(_) => return Err("正文读取中断"),
        }
    }
}

async fn read_non_sse_error(mut response: reqwest::Response) -> String {
    let status = response.status().as_u16();
    // Only fixed categories leave the adapter. Headers and bodies may echo
    // credentials, account information, or conversation content.
    let media_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let media_type = match media_type {
        Some(value) if value.eq_ignore_ascii_case("application/json") => "JSON",
        Some(value) if value.eq_ignore_ascii_case("text/html") => "HTML",
        Some(value) if value.eq_ignore_ascii_case("text/plain") => "文本",
        Some(_) => "其他",
        None => "缺失",
    };
    let diagnostic = |kind: &str| {
        format!("Responses 未返回 SSE 事件流（HTTP {status}，响应类型：{media_type}，正文类别：{kind}）；未提交工具调用。")
    };
    let bytes =
        match tokio::time::timeout(Duration::from_secs(5), read_error_body(&mut response)).await {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(kind)) => return diagnostic(kind),
            Err(_) => return diagnostic("正文读取超时"),
        };
    if bytes.is_empty() {
        return diagnostic("空响应");
    }
    if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
        if let Some(error) = value.get("error").filter(|error| !error.is_null()) {
            // A JSON error can use a successful HTTP status. Preserve the same
            // allowlisted recovery as non-2xx errors, never its raw message.
            return format!(
                "{} {}",
                diagnostic("JSON 错误"),
                structured_error(error, Some(status))
            );
        }
        return diagnostic(
            if value.get("object").and_then(Value::as_str) == Some("response") {
                "JSON Response（非流式）"
            } else {
                "JSON"
            },
        );
    }
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let prefix = &bytes[start..];
    diagnostic(
        if prefix.starts_with(b"event:") || prefix.starts_with(b"data:") {
            "SSE 格式（响应类型不匹配）"
        } else if prefix.starts_with(b"<") {
            "HTML 或 XML"
        } else {
            "无法识别"
        },
    )
}

fn structured_error(error: &Value, status: Option<u16>) -> String {
    // Never display a raw error message/body: a gateway can echo credentials or
    // user inputs. Only known machine codes influence user-facing recovery.
    match error.get("code").and_then(Value::as_str) {
        Some("subscription_sharing_usage_limit_exceeded") => "当前 ChatGPT 套餐共享用量已达限制，请在 ChatGPT 设置的 Usage 页面查看额度后再试；不会自动切换到付费 API。".into(),
        Some("subscription_sharing_usage_unavailable") => "暂时无法核验 ChatGPT 套餐用量，请稍后再试；登录凭据已保留，不会自动切换到付费 API。".into(),
        Some("subscription_sharing_user_unavailable") => "ChatGPT 用户或工作区信息暂时不可用，请稍后再试；登录凭据已保留。".into(),
        Some("subscription_sharing_user_not_eligible") => "当前 ChatGPT 用户、工作区或策略不允许共享套餐用量，请检查账号资格和工作区设置。".into(),
        Some("subscription_sharing_unsupported_capability") => "当前 ChatGPT 套餐接口不支持请求中的模型、输入或工具能力；请修改配置，不会重试相同请求。".into(),
        Some("subscription_sharing_route_not_supported") => "ChatGPT 套餐接口不支持当前请求路由；仅允许官方 Responses 端点。".into(),
        Some("subscription_sharing_invalid_user") => "ChatGPT 登录账号校验失败，请检查连接状态，确认凭据失效后重新登录。".into(),
        Some("chatpass_v2_scope_not_authorized" | "chatpass_v2_invalid_authorization_context") => "ChatGPT 授权范围或权限上下文不允许此次操作，请检查应用授权配置。".into(),
        Some("insufficient_quota") => "OpenAI API 额度不足，请检查 API 账户额度；不会自动更换计费方式。".into(),
        Some("rate_limit_exceeded") => "Responses 请求达到速率限制，请稍后再试；不会自动重试外部操作。".into(),
        _ => match status {
            Some(401) => "Responses 登录凭据无效或已过期，请检查凭据或重新登录。".into(),
            Some(403) => "Responses 请求被拒绝，请检查模型权限和账号授权。".into(),
            Some(429) => "Responses 达到用量或速率限制，请检查额度并稍后再试；不会自动切换计费方式。".into(),
            Some(300..=399) => "Responses 返回重定向；为保护凭据，已拒绝转发登录信息。".into(),
            Some(400) => "Responses 拒绝了请求参数，请检查模型与工具配置。".into(),
            _ => "Responses 请求失败；未提交工具调用，不会自动重试或切换到付费 API。".into(),
        },
    }
}

#[cfg(test)]
#[path = "openai_responses_tests.rs"]
mod tests;

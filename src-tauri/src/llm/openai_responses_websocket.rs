//! Foreground Plan Responses transport. A socket may survive between admitted
//! turns of the same session, but credentials and usage belong to each provider.
//! https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations
//! https://developers.openai.com/api/docs/guides/websocket-mode

use super::*;
use futures_util::{SinkExt, StreamExt};
use reqwest_websocket::{Message as Frame, RequestBuilderExt, WebSocket};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::sync::Weak;
use std::time::Instant;
use tungstenite::protocol::WebSocketConfig;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_IDLE: Duration = Duration::from_secs(4 * 60);
const MAX_AGE: Duration = Duration::from_secs(55 * 60);
const MAX_SESSIONS: usize = 4;

pub(crate) type SessionSlot = Arc<tokio::sync::Mutex<SessionState>>;

#[derive(Default)]
pub(crate) struct SessionPool {
    recent: Mutex<VecDeque<SessionEntry>>,
}

struct SessionEntry {
    session_id: String,
    credential_ref: String,
    epoch: u64,
    model: String,
    execution_permission: String,
    slot: SessionSlot,
}

impl SessionPool {
    pub(crate) fn bind(
        &self,
        session_id: &str,
        credential_ref: &str,
        epoch: u64,
        model: &str,
        execution_permission: &str,
    ) -> SessionSlot {
        let mut recent = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(index) = recent
            .iter()
            .position(|entry| entry.session_id == session_id)
        {
            let entry = recent.remove(index).expect("session index is present");
            if entry.credential_ref == credential_ref
                && entry.epoch == epoch
                && entry.model == model
                && entry.execution_permission == execution_permission
            {
                let slot = entry.slot.clone();
                recent.push_back(entry);
                return slot;
            }
        }
        let slot = SessionSlot::default();
        if recent.len() == MAX_SESSIONS {
            let Some(index) = recent
                .iter()
                .position(|entry| Arc::strong_count(&entry.slot) == 1)
            else {
                // Don't evict active runs to retain a new idle connection.
                return slot;
            };
            recent.remove(index);
        }
        recent.push_back(SessionEntry {
            session_id: session_id.to_owned(),
            credential_ref: credential_ref.to_owned(),
            epoch,
            model: model.to_owned(),
            execution_permission: execution_permission.to_owned(),
            slot: slot.clone(),
        });
        slot
    }

    pub(crate) fn invalidate(&self, session_id: &str) {
        self.recent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|entry| entry.session_id != session_id);
    }
}

#[derive(Default)]
pub(crate) struct SessionState {
    connection: Option<Connection>,
}

struct Connection {
    socket: WebSocket,
    auth_digest: [u8; 32],
    opened_at: Instant,
    used_at: Instant,
    previous: Option<PreviousResponse>,
    idle_close: tokio::task::AbortHandle,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.idle_close.abort();
    }
}

struct PreviousResponse {
    id: String,
    settings: Value,
    input_with_output: Vec<Value>,
}

pub(super) fn is_side_channel(event: &Value) -> bool {
    // The public Codex client separates transport metadata from model output.
    // https://github.com/openai/codex/blob/main/codex-rs/codex-api/src/endpoint/responses_websocket.rs
    matches!(
        event.get("type").and_then(Value::as_str),
        Some(
            "codex.response.metadata"
                | "response.metadata"
                | "responsesapi.websocket_timing"
                | "codex.rate_limits"
        )
    )
}

fn request_parts(body: &Value) -> Result<(Value, Vec<Value>), LlmFailure> {
    let mut settings = body.clone();
    let fields = settings.as_object_mut().ok_or(INVALID_HISTORY)?;
    fields.remove("stream");
    fields.remove("background");
    fields.remove("previous_response_id");
    let Some(Value::Array(input)) = fields.remove("input") else {
        return Err(INVALID_HISTORY.into());
    };
    Ok((settings, input))
}

fn create_event(
    settings: &Value,
    full_input: &[Value],
    previous: Option<&PreviousResponse>,
) -> Value {
    let mut event = settings.clone();
    let continuation = previous
        .filter(|previous| &previous.settings == settings)
        .and_then(|previous| {
            full_input
                .strip_prefix(previous.input_with_output.as_slice())
                .filter(|delta| !delta.is_empty())
                .map(|delta| (&previous.id, delta))
        });
    let input = if let Some((id, delta)) = continuation {
        event["previous_response_id"] = json!(id);
        delta
    } else {
        full_input
    };
    event["type"] = json!("response.create");
    event["input"] = json!(input);
    event
}

fn validate_session(provider: &OpenAiResponsesProvider) -> Result<(), LlmFailure> {
    provider
        .credentials
        .validate_session()
        .map_err(|_| "模型会话已失效，请重新连接账号；本次请求未发送。".into())
}

async fn connect(
    provider: &OpenAiResponsesProvider,
    authorization: &HeaderValue,
) -> Result<Option<WebSocket>, LlmFailure> {
    let request = provider
        .client
        .get(provider.endpoint.clone())
        .version(reqwest::Version::HTTP_11)
        .header(AUTHORIZATION, authorization.clone());
    let request = provider.apply_session_affinity(request);
    let request = request.upgrade().web_socket_config(
        WebSocketConfig::default()
            .max_frame_size(Some(MAX_BUFFER_BYTES))
            .max_message_size(Some(MAX_BUFFER_BYTES)),
    );
    let upgrade = async {
        let response = request.send().await.ok()?;
        match response.status().as_u16() {
            101 => response.into_websocket().await.ok().map(Ok),
            // Upgrade can be unavailable even when the normal HTTPS route works.
            404 | 405 | 426 | 500..=599 => None,
            _ => Some(Err(response.into_inner())),
        }
    };
    match tokio::time::timeout(CONNECT_TIMEOUT, upgrade).await {
        Ok(Some(Ok(socket))) => Ok(Some(socket)),
        Ok(Some(Err(response))) => {
            let status = response.status().as_u16();
            Err(
                tokio::time::timeout(Duration::from_secs(5), read_http_error(response))
                    .await
                    .unwrap_or_else(|_| structured_failure(&Value::Null, Some(status))),
            )
        }
        Ok(None) | Err(_) => Ok(None),
    }
}

/// The pool keeps only live sockets and their most recent request until the
/// idle limit, even if the user leaves this session open without sending again.
async fn close_idle(slot: Weak<tokio::sync::Mutex<SessionState>>, opened_at: Instant) {
    let mut delay = MAX_IDLE;
    loop {
        tokio::time::sleep(delay).await;
        let Some(slot) = slot.upgrade() else {
            return;
        };
        let mut state = slot.lock().await;
        let Some(connection) = state
            .connection
            .as_ref()
            .filter(|c| c.opened_at == opened_at)
        else {
            return;
        };
        delay = MAX_IDLE
            .saturating_sub(connection.used_at.elapsed())
            .min(MAX_AGE.saturating_sub(connection.opened_at.elapsed()));
        if delay.is_zero() {
            state.connection = None;
            return;
        }
    }
}

pub(super) async fn request(
    provider: &OpenAiResponsesProvider,
    slot: &SessionSlot,
    state: &mut SessionState,
    authorization: &HeaderValue,
    body: &Value,
    parser: ResponsesStream,
    sender: Option<&UnboundedSender<String>>,
) -> Result<Option<LlmResponse>, LlmFailure> {
    // Taking ownership closes the socket if the request fails or is cancelled.
    let mut connection = state.connection.take();
    validate_session(provider)?;
    let (settings, mut full_input) = request_parts(body)?;
    let auth_digest: [u8; 32] = Sha256::digest(authorization.as_bytes()).into();
    connection = connection.filter(|connection| {
        connection.auth_digest == auth_digest
            && connection.used_at.elapsed() < MAX_IDLE
            && connection.opened_at.elapsed() < MAX_AGE
    });
    let mut connection = match connection {
        Some(connection) => connection,
        None => {
            let Some(socket) = connect(provider, authorization).await? else {
                validate_session(provider)?;
                return Ok(None);
            };
            let now = Instant::now();
            let idle_close = tokio::spawn(close_idle(Arc::downgrade(slot), now)).abort_handle();
            Connection {
                socket,
                auth_digest,
                opened_at: now,
                used_at: now,
                previous: None,
                idle_close,
            }
        }
    };

    let event = create_event(&settings, &full_input, connection.previous.as_ref());
    #[cfg(test)]
    let transport = if event.get("previous_response_id").is_some() {
        "websocket_delta"
    } else {
        "websocket_full"
    };
    validate_session(provider)?;
    // No return after sending a create event selects HTTP, even if the send
    // fails after only part of the frame reaches the server.
    let completed = tokio::time::timeout(
        RESPONSE_TIMEOUT,
        receive_response(&mut connection.socket, event, parser, sender),
    )
    .await
    .map_err(|_| transport_failure(LlmFailureKind::TimedOut, "读取超时"))?
    .map_err(|failure| failure.at_protocol_stage(LlmProtocolStage::ResponseValidation, None))?;
    #[cfg(test)]
    let completed = {
        let mut completed = completed;
        completed.metadata.transport = Some(transport.into());
        completed
    };
    if connection
        .previous
        .as_ref()
        .is_some_and(|previous| previous.id == completed.response_id)
    {
        return Err(invalid_response_at("重复响应 ID").into());
    }
    let LlmResponse::WithProtocol { protocol, .. } = &completed.response else {
        return Err(INVALID_RESPONSE.into());
    };
    // The HTTP encoder omits response-only status/annotations when building
    // model input, so compare against its normalized representation.
    full_input.extend(parse_output_items(&protocol.output_items, provider.auth_mode, None)?.replay);
    let previous = PreviousResponse {
        id: completed.response_id.clone(),
        settings,
        input_with_output: full_input,
    };
    let reply = provider.accept_completed(completed)?;
    connection.previous = Some(previous);
    connection.used_at = Instant::now();
    state.connection = Some(connection);
    Ok(Some(reply))
}

fn transport_failure(kind: LlmFailureKind, reason: &'static str) -> LlmFailure {
    LlmFailure {
        kind,
        diagnostic: format!(
            "Responses WebSocket {reason}；未提交工具调用，不会自动重发已送达的请求。"
        ),
    }
}

async fn receive_response(
    socket: &mut WebSocket,
    event: Value,
    mut parser: ResponsesStream,
    sender: Option<&UnboundedSender<String>>,
) -> Result<CompletedResponse, LlmFailure> {
    socket
        .send(Frame::Text(event.to_string()))
        .await
        .map_err(|_| transport_failure(LlmFailureKind::TransportUnavailable, "发送中断"))?;
    let mut total_bytes = 0usize;
    while let Some(frame) = socket.next().await {
        match frame
            .map_err(|_| transport_failure(LlmFailureKind::ResponseInterrupted, "读取中断"))?
        {
            Frame::Text(text) => {
                total_bytes += text.len();
                if total_bytes > MAX_STREAM_BYTES {
                    return Err(transport_failure(
                        LlmFailureKind::Unclassified,
                        "事件流超过大小限制",
                    ));
                }
                let event: Value = serde_json::from_str(&text).map_err(|_| INVALID_RESPONSE)?;
                if is_side_channel(&event) {
                    continue;
                }
                if matches!(event["type"].as_str(), None | Some("error")) {
                    if let Some(error) = event.get("error").filter(|value| value.is_object()) {
                        let status = event["status"]
                            .as_u64()
                            .filter(|status| (400..600).contains(status))
                            .map(|status| status as u16);
                        return Err(structured_failure(error, status));
                    }
                }
                parser.consume_json_event(&event, None, sender)?;
                if let Some(completed) = parser.completed.take() {
                    return Ok(completed);
                }
            }
            Frame::Ping(_) => socket
                .flush()
                .await
                .map_err(|_| transport_failure(LlmFailureKind::ResponseInterrupted, "心跳失败"))?,
            Frame::Pong(_) => (),
            Frame::Close { .. } => break,
            Frame::Binary(_) => return Err(INVALID_RESPONSE.into()),
        }
    }
    Err(transport_failure(
        LlmFailureKind::ResponseInterrupted,
        "完成前连接关闭",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use tungstenite::Message as MockFrame;

    fn user(content: &str) -> Message {
        Message {
            role: "user".into(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            tool_images: Vec::new(),
            protocol_state: None,
        }
    }

    fn mock_provider(address: std::net::SocketAddr) -> OpenAiResponsesProvider {
        mock_provider_with_slot(address, SessionSlot::default())
    }

    fn mock_provider_with_slot(
        address: std::net::SocketAddr,
        slot: SessionSlot,
    ) -> OpenAiResponsesProvider {
        mock_provider_with_affinity(address, slot, None)
    }

    fn mock_config() -> ProviderConfig {
        ProviderConfig {
            provider: "openai".into(),
            model: "gpt-6-luna".into(),
            base_url: PLAN_ENDPOINT.into(),
            api_key: String::new(),
            max_tokens: 512,
            temperature: 0.4,
            protocol: Some(ModelProtocol::OpenaiResponses),
            auth_mode: ModelAuthMode::ChatgptPlan,
            credential_ref: Some("fixture".into()),
        }
    }

    fn mock_provider_with_affinity(
        address: std::net::SocketAddr,
        slot: SessionSlot,
        session_id: Option<&str>,
    ) -> OpenAiResponsesProvider {
        let mut provider = OpenAiResponsesProvider::new(
            &mock_config(),
            super::super::super::auth::StaticCredentialSource::new(
                "synthetic-fixture".into(),
                "fixture".into(),
            ),
        )
        .unwrap()
        .with_foreground_websocket(slot);
        if let Some(session_id) = session_id {
            provider = provider.with_session_affinity(session_id);
        }
        provider.endpoint = reqwest::Url::parse(&format!("http://{address}/v1/responses")).unwrap();
        provider.client = reqwest::Client::builder()
            .no_proxy()
            // tiny_http closes rejected upgrade connections without a
            // Connection: close response header. Keep the fixture from
            // racing that close when its fallback POST starts.
            .pool_max_idle_per_host(0)
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        provider
    }

    fn completion(turn: usize) -> Value {
        json!({"type":"response.completed", "response":{
            "object":"response", "status":"completed", "id":format!("resp_{turn}"),
            "model":"gpt-6-luna", "usage":{"input_tokens":123,"output_tokens":2,
                "input_tokens_details":{"cached_tokens":100}},
            "output":[{"type":"message", "role":"assistant", "content":[{"type":"output_text","text":"OK"}]}]
        }})
    }

    fn assistant(reply: LlmResponse) -> Message {
        let (_, continuation) = reply.into_parts();
        let mut message = user("OK");
        message.role = "assistant".into();
        message.protocol_state = continuation;
        message
    }

    #[test]
    fn foreground_websocket_pool_bounds_and_invalidates_session_bindings() {
        let pool = SessionPool::default();
        let first = pool.bind("first", "fixture", 1, "luna", "workspace-write");
        let second = pool.bind("second", "fixture", 1, "luna", "workspace-write");
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(Arc::ptr_eq(
            &first,
            &pool.bind("first", "fixture", 1, "luna", "workspace-write")
        ));

        let reauthorized = pool.bind("first", "fixture", 2, "luna", "workspace-write");
        assert!(!Arc::ptr_eq(&first, &reauthorized));
        let switched_model = pool.bind("first", "fixture", 2, "sol", "workspace-write");
        assert!(!Arc::ptr_eq(&reauthorized, &switched_model));
        let switched_account = pool.bind("first", "different", 2, "sol", "workspace-write");
        assert!(!Arc::ptr_eq(&switched_model, &switched_account));
        let switched_permission = pool.bind("first", "different", 2, "sol", "read-only");
        assert!(!Arc::ptr_eq(&switched_account, &switched_permission));
        pool.invalidate("first");
        let reset = pool.bind("first", "different", 2, "sol", "read-only");
        assert!(!Arc::ptr_eq(&switched_permission, &reset));
        assert!(Arc::ptr_eq(
            &second,
            &pool.bind("second", "fixture", 1, "luna", "workspace-write")
        ));

        let third = pool.bind("third", "fixture", 1, "luna", "workspace-write");
        let _fourth = pool.bind("fourth", "fixture", 1, "luna", "workspace-write");
        let uncached = pool.bind("fifth", "fixture", 1, "luna", "workspace-write");
        assert_eq!(pool.recent.lock().unwrap().len(), MAX_SESSIONS);
        assert!(!Arc::ptr_eq(
            &uncached,
            &pool.bind("fifth", "fixture", 1, "luna", "workspace-write")
        ));
        let evictable = Arc::downgrade(&third);
        drop(third);
        let fifth = pool.bind("fifth", "fixture", 1, "luna", "workspace-write");
        assert!(evictable.upgrade().is_none());
        assert!(Arc::ptr_eq(
            &fifth,
            &pool.bind("fifth", "fixture", 1, "luna", "workspace-write")
        ));
    }

    #[tokio::test]
    async fn foreground_websocket_affinity_is_stable_across_reconnect_and_http_fallback() {
        use tiny_http::{Header, Method, Response, Server};

        let server = Server::http("127.0.0.1:0").unwrap();
        let address = server.server_addr().to_ip().unwrap();
        let session_id = "owned-test-session";
        let first = mock_provider_with_affinity(address, SessionSlot::default(), Some(session_id));
        let second = mock_provider_with_affinity(address, SessionSlot::default(), Some(session_id));
        let unbound = mock_provider(address);
        let different_session =
            mock_provider_with_affinity(address, SessionSlot::default(), Some("another-session"));
        assert_ne!(first.session_affinity, different_session.session_affinity);
        let mut other_account_config = mock_config();
        other_account_config.credential_ref = Some("other-account".into());
        let other_account = OpenAiResponsesProvider::new(
            &other_account_config,
            super::super::super::auth::StaticCredentialSource::new(
                "synthetic-fixture".into(),
                "other-account".into(),
            ),
        )
        .unwrap()
        .with_foreground_websocket(SessionSlot::default())
        .with_session_affinity(session_id);
        assert_ne!(first.session_affinity, other_account.session_affinity);
        let affinity = first
            .session_affinity
            .as_ref()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert!(affinity.starts_with("angelbot-"));
        assert_eq!(affinity.len(), "angelbot-".len() + 64);
        let mut key_config = mock_config();
        key_config.auth_mode = ModelAuthMode::ApiKey;
        let key_provider = OpenAiResponsesProvider::new(
            &key_config,
            super::super::super::auth::StaticCredentialSource::new(
                "synthetic-fixture".into(),
                "fixture".into(),
            ),
        )
        .unwrap()
        .with_foreground_websocket(SessionSlot::default())
        .with_session_affinity(session_id);
        assert!(key_provider.session_affinity.is_none());

        let worker = std::thread::spawn(move || {
            for turn in 1..=3 {
                for method in [Method::Get, Method::Post] {
                    let request = server
                        .recv_timeout(Duration::from_secs(5))
                        .unwrap()
                        .unwrap();
                    assert_eq!(request.method(), &method);
                    let actual = request
                        .headers()
                        .iter()
                        .find(|header| header.field.equiv("session-id"))
                        .map(|header| header.value.as_str());
                    let expected = (turn < 3).then_some(affinity.as_str());
                    assert_eq!(actual, expected);
                    if method == Method::Get {
                        request.respond(Response::empty(404)).unwrap();
                    } else {
                        request
                            .respond(
                                Response::from_string(format!("data: {}\n\n", completion(turn)))
                                    .with_header(
                                        Header::from_bytes("content-type", "text/event-stream")
                                            .unwrap(),
                                    ),
                            )
                            .unwrap();
                    }
                }
            }
        });
        for provider in [first, second, unbound] {
            provider
                .chat_classified(&[user("synthetic")], None, None)
                .await
                .unwrap();
        }
        worker.join().unwrap();
    }

    #[tokio::test]
    async fn foreground_websocket_falls_back_before_send_and_preserves_auth_refusal() {
        use tiny_http::{Header, Method, Response, Server};
        for (status, expected_posts) in [(404, 2), (401, 0)] {
            let server = Server::http("127.0.0.1:0").unwrap();
            let provider = mock_provider(server.server_addr().to_ip().unwrap());
            let worker = std::thread::spawn(move || {
                let get = server
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .unwrap();
                assert_eq!(get.method(), &Method::Get);
                get.respond(Response::empty(status)).unwrap();
                for turn in 1..=expected_posts {
                    let post = server
                        .recv_timeout(Duration::from_secs(5))
                        .unwrap()
                        .unwrap();
                    assert_eq!(post.method(), &Method::Post);
                    post.respond(
                        Response::from_string(format!("data: {}\n\n", completion(turn)))
                            .with_header(
                                Header::from_bytes("content-type", "text/event-stream").unwrap(),
                            ),
                    )
                    .unwrap();
                }
                assert!(server
                    .recv_timeout(Duration::from_millis(200))
                    .unwrap()
                    .is_none());
            });
            let first = provider.chat_classified(&[user("first")], None, None).await;
            if status == 404 {
                first.expect("the synthetic fallback response must complete");
                assert_eq!(
                    provider
                        .latest_response_metadata()
                        .unwrap()
                        .transport
                        .as_deref(),
                    Some("http")
                );
                provider
                    .chat_classified(&[user("second")], None, None)
                    .await
                    .unwrap();
            } else {
                assert!(first.unwrap_err().diagnostic.contains("HTTP 401"));
                assert!(provider.latest_usage().is_none());
            }
            worker.join().unwrap();
        }
    }

    #[tokio::test]
    async fn foreground_websocket_reuses_exact_prefix_and_keeps_full_request_on_change() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let start = Instant::now();
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && start.elapsed() < Duration::from_secs(5) =>
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("mock WebSocket accept: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            for turn in 1..=4 {
                let text = socket.read().unwrap().into_text().unwrap();
                let event: Value = serde_json::from_str(&text).unwrap();
                assert_eq!(event["type"], "response.create");
                assert_eq!(event["store"], false);
                assert!(event.get("stream").is_none());
                let input = event["input"].as_array().unwrap();
                match turn {
                    1 => {
                        assert!(event.get("previous_response_id").is_none());
                        assert_eq!(input.len(), 1);
                    }
                    2 => {
                        assert_eq!(event["previous_response_id"], "resp_1");
                        assert_eq!(input.len(), 1);
                        assert_eq!(input[0]["content"], "next");
                    }
                    3 => {
                        assert!(event.get("previous_response_id").is_none());
                        assert_eq!(input.len(), 3);
                        assert_eq!(input[0]["content"], "changed");
                    }
                    4 => {
                        assert_eq!(event["previous_response_id"], "resp_3");
                        assert_eq!(input.len(), 1);
                        socket
                            .send(MockFrame::Text(
                                json!({"type":"response.output_text.delta","delta":"partial"})
                                    .to_string()
                                    .into(),
                            ))
                            .unwrap();
                        let disconnected = socket.read().unwrap_err();
                        assert!(
                            !matches!(disconnected, tungstenite::Error::Io(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
                        );
                        assert_eq!(
                            listener.accept().unwrap_err().kind(),
                            std::io::ErrorKind::WouldBlock
                        );
                        break;
                    }
                    _ => unreachable!(),
                }
                for event in [
                    json!({"type":"codex.response.metadata","headers":{}}),
                    json!({"type":"response.output_text.delta","delta":"OK"}),
                    completion(turn),
                    json!({"type":"responsesapi.websocket_timing"}),
                ] {
                    socket
                        .send(MockFrame::Text(event.to_string().into()))
                        .unwrap();
                }
            }
        });
        let pool = SessionPool::default();
        let provider = mock_provider_with_slot(
            address,
            pool.bind("session", "fixture", 1, "luna", "workspace-write"),
        );
        let mut messages = vec![user("first")];
        let (delta_tx, mut delta_rx) = tokio::sync::mpsc::unbounded_channel();
        let first = provider
            .chat_stream_classified(&messages, None, None, Some(delta_tx))
            .await
            .unwrap();
        assert_eq!(delta_rx.try_recv().unwrap(), "OK");
        assert_eq!(
            provider.latest_usage().unwrap().cached_input_tokens,
            Some(100)
        );
        assert_eq!(
            provider
                .latest_response_metadata()
                .unwrap()
                .transport
                .as_deref(),
            Some("websocket_full")
        );
        drop(provider);
        messages.extend([assistant(first), user("next")]);
        let provider = mock_provider_with_slot(
            address,
            pool.bind("session", "fixture", 1, "luna", "workspace-write"),
        );
        assert!(provider.latest_usage().is_none());
        assert!(provider.latest_response_metadata().is_none());
        provider
            .chat_classified(&messages, None, None)
            .await
            .unwrap();
        assert_eq!(
            provider
                .latest_response_metadata()
                .unwrap()
                .transport
                .as_deref(),
            Some("websocket_delta")
        );
        messages[0] = user("changed");
        let changed = provider
            .chat_classified(&messages, None, None)
            .await
            .unwrap();
        messages.extend([assistant(changed), user("cancel")]);
        {
            let (delta_tx, mut delta_rx) = tokio::sync::mpsc::unbounded_channel();
            let pending = provider.chat_stream_classified(&messages, None, None, Some(delta_tx));
            tokio::pin!(pending);
            tokio::select! {
                response = &mut pending => panic!("request completed before cancellation: {response:?}"),
                delta = delta_rx.recv() => assert_eq!(delta.as_deref(), Some("partial")),
                _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("missing mock delta"),
            }
        }
        assert!(provider.latest_usage().is_none());
        assert!(provider.latest_response_metadata().is_none());
        assert!(provider
            .foreground_websocket
            .as_ref()
            .unwrap()
            .lock()
            .await
            .connection
            .is_none());
        server.join().unwrap();
    }
}

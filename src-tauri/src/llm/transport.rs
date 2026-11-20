//! Model HTTP only: never replay a delivered request, response stream or tool.
//! This seam does not apply to OAuth, MCP or external actions.

use super::{LlmFailure, LlmFailureKind, LlmProtocolStage};
use reqwest::{RequestBuilder, Response};
use serde_json::Value;
use std::{error::Error, io, time::Duration};

const MAX_ERROR_BYTES: usize = 256 * 1024;

pub(super) fn request_failure(error: reqwest::Error, protocol: &str) -> LlmFailure {
    use LlmFailureKind::*;
    let (kind, stage) = if error.is_builder() {
        (Unclassified, "请求参数无效")
    } else if error.is_timeout() {
        (TimedOut, "请求超时")
    } else if error.is_connect() {
        (TransportUnavailable, "连接建立失败（网络、代理或 TLS）")
    } else {
        (TransportUnavailable, "请求发送中断")
    };
    LlmFailure {
        kind,
        diagnostic: format!("{protocol} {stage}；不会自动重发已送达的请求或重做外部操作。"),
    }
    .at_protocol_stage(LlmProtocolStage::PrepareRequest, None)
}

pub(super) fn response_read_failure(error: reqwest::Error, protocol: &str) -> LlmFailure {
    let (kind, stage) = if error.is_timeout() {
        (LlmFailureKind::TimedOut, "读取超时")
    } else {
        (LlmFailureKind::ResponseInterrupted, "读取中断")
    };
    LlmFailure {
        kind,
        diagnostic: format!("{protocol} 响应{stage}；未提交工具调用。"),
    }
}

pub(super) fn api_failure_kind(error: &Value, status: Option<u16>) -> LlmFailureKind {
    use LlmFailureKind::*;
    match status {
        Some(401) => return Authentication,
        Some(403) => return PermissionDenied,
        _ => {}
    }
    let code = error.get("code").and_then(Value::as_str);
    let error_type = error.get("type").and_then(Value::as_str);
    for identifier in [code, error_type].into_iter().flatten() {
        match identifier {
            "insufficient_quota" | "subscription_sharing_usage_limit_exceeded" => {
                return QuotaExceeded
            }
            "rate_limit_exceeded" | "rate_limit_error" => return RateLimited,
            "invalid_api_key" | "authentication_error" => return Authentication,
            "permission_error" => return PermissionDenied,
            "overloaded_error" => return ServiceUnavailable,
            _ => {}
        }
    }
    match status {
        Some(408 | 504) => TimedOut,
        Some(429) => RateLimited,
        Some(500..=599) => ServiceUnavailable,
        _ => Unclassified,
    }
}

pub(super) fn json_response_failure(payload: &Value, protocol: &str) -> Option<LlmFailure> {
    let error = payload.get("error").filter(|error| !error.is_null())?;
    Some(LlmFailure {
        kind: api_failure_kind(error, None),
        diagnostic: format!("{protocol} returned an error instead of a completed response"),
    })
}

pub(super) async fn read_error_body(response: &mut Response) -> Result<Vec<u8>, &'static str> {
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if chunk.len() <= MAX_ERROR_BYTES - bytes.len() => {
                bytes.extend_from_slice(&chunk)
            }
            Ok(Some(_)) => return Err("正文超过诊断上限"),
            Ok(None) => return Ok(bytes),
            Err(_) => return Err("正文读取中断"),
        }
    }
}

pub(super) async fn api_response_failure(mut response: Response, protocol: &str) -> LlmFailure {
    let status = response.status();
    let kind =
        match tokio::time::timeout(Duration::from_secs(5), read_error_body(&mut response)).await {
            Ok(Ok(bytes)) => {
                let payload = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
                api_failure_kind(
                    payload.get("error").unwrap_or(&payload),
                    Some(status.as_u16()),
                )
            }
            _ => api_failure_kind(&Value::Null, Some(status.as_u16())),
        };
    LlmFailure {
        kind,
        diagnostic: format!("{protocol} API error {status}"),
    }
    .at_protocol_stage(LlmProtocolStage::RequestRejected, Some(status.as_u16()))
}

pub(super) async fn send_model_request(
    request: RequestBuilder,
) -> Result<Response, reqwest::Error> {
    // JSON bodies clone their byte buffer, not the conversation. Keep the exact
    // credentials/body, and do not retry an uncloneable streaming request.
    let template = request.try_clone();
    let mut result = request.send().await;
    for delay in [Duration::from_millis(250), Duration::from_millis(750)] {
        if !matches!(&result, Err(error) if reconnectable_failure(error)) {
            break;
        }
        let Some(attempt) = template.as_ref().and_then(RequestBuilder::try_clone) else {
            break;
        };
        // No spawned work: stopping the owning turn also drops this wait.
        tokio::time::sleep(delay).await;
        result = attempt.send().await;
    }
    result
}

fn reconnectable_failure(error: &reqwest::Error) -> bool {
    // A request/stream timeout may follow delivery. Never infer safe replay from
    // an EOF/reset alone: reqwest must identify the connection-establishment phase.
    if !error.is_connect() || error.is_timeout() {
        return false;
    }
    let mut cause = error.source();
    let mut retryable = false;
    while let Some(current) = cause {
        // The connector may expose the inner TLS error, not its I/O wrapper.
        // Do not treat certificate/protocol rejection as a reconnectable outage.
        if current.is::<rustls::Error>() {
            return false;
        }
        if let Some(error) = current.downcast_ref::<io::Error>() {
            match error.kind() {
                io::ErrorKind::ConnectionRefused
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
                | io::ErrorKind::NotConnected
                | io::ErrorKind::UnexpectedEof => retryable = true,
                // Certificate/protocol failures are not transient connectivity.
                io::ErrorKind::InvalidData
                | io::ErrorKind::InvalidInput
                | io::ErrorKind::PermissionDenied => return false,
                _ => {}
            }
        }
        // io::Error::source() can skip the inner error itself. Preserve its type
        // rather than guessing from localized TLS/socket diagnostic strings.
        cause = current
            .downcast_ref::<io::Error>()
            .and_then(io::Error::get_ref)
            .map(|inner| inner as &(dyn Error + 'static))
            .or_else(|| current.source());
    }
    retryable
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::Notify;

    #[tokio::test]
    async fn model_adapters_report_the_same_classified_http_failures() {
        use crate::llm::{
            create_provider_with_credentials, Message, ModelAuthMode, ModelProtocol, ProviderConfig,
        };
        use LlmFailureKind::*;

        let input = [Message {
            role: "user".into(),
            content: "fixture".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_images: vec![],
            protocol_state: None,
        }];
        for (protocol, path) in [
            (ModelProtocol::OpenaiResponses, "/v1/responses"),
            (ModelProtocol::OpenaiChatCompletions, "/v1/chat/completions"),
            (ModelProtocol::AnthropicMessages, "/v1/messages"),
        ] {
            for (status, error_type, expected) in [
                (401, "", Authentication),
                (403, "", PermissionDenied),
                (429, "", RateLimited),
                (429, "insufficient_quota", QuotaExceeded),
                (503, "", ServiceUnavailable),
                (504, "", TimedOut),
                (200, "rate_limit_error", RateLimited),
            ] {
                let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
                let provider = create_provider_with_credentials(
                    &ProviderConfig {
                        provider: "custom".into(),
                        api_key: String::new(),
                        model: "fixture".into(),
                        base_url: format!("http://{}/v1", server.server_addr()),
                        max_tokens: 128,
                        temperature: 0.4,
                        protocol: Some(protocol),
                        auth_mode: ModelAuthMode::None,
                        credential_ref: None,
                    },
                    None,
                )
                .unwrap();
                let fixture = std::thread::spawn(move || {
                    let request = server
                        .recv_timeout(Duration::from_secs(3))
                        .unwrap()
                        .unwrap();
                    assert_eq!(request.url(), path);
                    let body = serde_json::json!({
                        "error": {"type": error_type, "message": "private-marker insufficient_quota"}
                    });
                    request
                        .respond(
                            tiny_http::Response::from_string(body.to_string())
                                .with_status_code(status)
                                .with_header(
                                    tiny_http::Header::from_bytes(
                                        "Content-Type",
                                        "application/json",
                                    )
                                    .unwrap(),
                                ),
                        )
                        .unwrap();
                });
                let failure = provider
                    .chat_stream_classified(&input, None, None, None)
                    .await
                    .unwrap_err();
                fixture.join().unwrap();
                assert_eq!(
                    failure.kind, expected,
                    "{protocol:?} HTTP {status} {error_type}"
                );
                assert!(!failure.diagnostic.contains("private-marker"));
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum Reply {
        CloseTls,
        InvalidTls,
        CloseAfterRequest,
        Unavailable,
        Available,
    }

    struct Fixture {
        url: String,
        count: Arc<AtomicUsize>,
        accepted: Arc<Notify>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    async fn fixture(reply: Reply) -> Fixture {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        fixture_on(listener, reply)
    }
    fn fixture_on(listener: tokio::net::TcpListener, reply: Reply) -> Fixture {
        let address = listener.local_addr().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let accepted = Arc::new(Notify::new());
        let observed = count.clone();
        let notify = accepted.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                observed.fetch_add(1, Ordering::SeqCst);
                notify.notify_one();
                match reply {
                    Reply::CloseTls => {}
                    Reply::InvalidTls => {
                        let mut hello = [0; 4096];
                        let _ = socket.read(&mut hello).await;
                        let _ = socket
                            .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
                            .await;
                        // Keep/drain the socket until the client rejects TLS.
                        // Closing with unread ClientHello bytes sends a reset,
                        // which would test transient I/O rather than bad TLS.
                        while socket.read(&mut hello).await.unwrap_or(0) != 0 {}
                    }
                    Reply::CloseAfterRequest | Reply::Unavailable | Reply::Available => {
                        let mut bytes = Vec::new();
                        let mut chunk = [0; 1024];
                        while bytes.len() < 8192 {
                            let read = socket.read(&mut chunk).await.unwrap_or(0);
                            if read == 0 {
                                break;
                            }
                            bytes.extend_from_slice(&chunk[..read]);
                            // The fixture body is exactly "payload" (7 bytes).
                            if bytes
                                .windows(4)
                                .position(|w| w == b"\r\n\r\n")
                                .is_some_and(|end| bytes.len() >= end + 4 + 7)
                            {
                                break;
                            }
                        }
                        assert!(bytes.ends_with(b"payload"));
                        if matches!(reply, Reply::Available) {
                            let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        } else if matches!(reply, Reply::Unavailable) {
                            let _ = socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        }
                    }
                }
            }
        });
        Fixture {
            url: format!(
                "{}://{address}/model",
                if matches!(reply, Reply::CloseTls | Reply::InvalidTls) {
                    "https"
                } else {
                    "http"
                }
            ),
            count,
            accepted,
            task,
        }
    }
    fn request(url: &str) -> RequestBuilder {
        reqwest::Client::builder()
            .no_proxy()
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap()
            .post(url)
            .body("payload")
    }

    #[tokio::test]
    async fn transient_connect_failures_reconnect_only_twice() {
        let target = fixture(Reply::CloseTls).await;
        let error = send_model_request(request(&target.url)).await.unwrap_err();
        assert!(
            error.is_connect(),
            "fixture must fail before the model request"
        );
        assert_eq!(target.count.load(Ordering::SeqCst), 3, "{error:?}");
    }

    #[tokio::test]
    async fn restored_connection_delivers_the_original_request_once() {
        let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = reservation.local_addr().unwrap();
        drop(reservation);
        let startup = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            fixture_on(
                tokio::net::TcpListener::bind(address).await.unwrap(),
                Reply::Available,
            )
        });
        let result = send_model_request(request(&format!("http://{address}/model"))).await;
        let target = startup.await.unwrap();
        assert_eq!(result.unwrap().status(), reqwest::StatusCode::OK);
        assert_eq!(target.count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn delivered_requests_http_errors_and_invalid_tls_are_not_replayed() {
        for reply in [
            Reply::CloseAfterRequest,
            Reply::Unavailable,
            Reply::InvalidTls,
        ] {
            let target = fixture(reply).await;
            let result = send_model_request(request(&target.url)).await;
            let diagnostic = format!("{result:?}");
            if matches!(reply, Reply::Unavailable) {
                assert_eq!(
                    result.unwrap().status(),
                    reqwest::StatusCode::SERVICE_UNAVAILABLE
                );
            } else {
                let error = result.unwrap_err();
                if matches!(reply, Reply::CloseAfterRequest) {
                    assert!(!error.is_connect());
                } else {
                    assert!(!error.is_timeout(), "fixture must reject TLS, not stall");
                }
            }
            assert_eq!(
                target.count.load(Ordering::SeqCst),
                1,
                "{reply:?}: {diagnostic}"
            );
        }
    }

    #[tokio::test]
    async fn cancelling_the_request_leaves_no_background_reconnect() {
        let target = fixture(Reply::CloseTls).await;
        let prepared = request(&target.url);
        let operation = tokio::spawn(send_model_request(prepared));
        tokio::time::timeout(Duration::from_secs(2), target.accepted.notified())
            .await
            .unwrap();
        operation.abort();
        let _ = operation.await;
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert_eq!(target.count.load(Ordering::SeqCst), 1);
    }
}

//! Isolated protocol experiment. AngelBot's production MCP client does not use this crate.

#[cfg(test)]
mod tests {
    use rmcp::{
        model::{DiscoverResult, InitializeResult, ProtocolVersion},
        ClientLifecycleMode, ClientServiceExt,
    };
    use serde_json::{json, Value};
    use tokio::{
        io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, Lines},
        time::{timeout, Duration, Instant},
    };

    fn auto() -> ClientLifecycleMode {
        ClientLifecycleMode::Auto {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            legacy_version: Some(ProtocolVersion::V_2024_11_05),
        }
    }

    async fn request<R: AsyncBufRead + Unpin>(lines: &mut Lines<R>) -> Value {
        let line = timeout(Duration::from_secs(15), lines.next_line())
            .await
            .expect("request timeout")
            .expect("read request")
            .expect("request EOF");
        serde_json::from_str(&line).expect("JSON-RPC request")
    }

    async fn reply<W: AsyncWrite + Unpin>(writer: &mut W, value: Value) {
        writer
            .write_all(value.to_string().as_bytes())
            .await
            .unwrap();
        writer.write_all(b"\n").await.unwrap();
        writer.flush().await.unwrap();
    }

    #[tokio::test]
    async fn modern_discover_skips_initialize_and_sets_request_metadata() {
        let (client_io, server_io) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server_io);
            let mut lines = BufReader::new(read).lines();
            let discover = request(&mut lines).await;
            assert_eq!(discover["method"], "server/discover");
            assert_eq!(
                discover["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
                "2026-07-28"
            );
            let result =
                DiscoverResult::new(vec![ProtocolVersion::V_2026_07_28], Default::default());
            reply(
                &mut write,
                json!({"jsonrpc":"2.0", "id":discover["id"], "result":result}),
            )
            .await;

            let tools = request(&mut lines).await;
            assert_eq!(
                tools["method"], "tools/list",
                "modern peer must not initialize"
            );
            assert_eq!(
                tools["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
                "2026-07-28"
            );
            reply(
                &mut write,
                json!({"jsonrpc":"2.0", "id":tools["id"], "result":{"tools":[]}}),
            )
            .await;
        });
        let client = ().serve_with_lifecycle(client_io, auto()).await.unwrap();
        assert_eq!(
            client.peer_info().unwrap().protocol_version,
            ProtocolVersion::V_2026_07_28
        );
        assert!(client.list_all_tools().await.unwrap().is_empty());
        server.await.unwrap();
        client.cancel().await.unwrap();
    }

    #[tokio::test]
    async fn correlated_method_not_found_falls_back_to_current_legacy_version() {
        let (client_io, server_io) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server_io);
            let mut lines = BufReader::new(read).lines();
            let discover = request(&mut lines).await;
            assert_eq!(discover["method"], "server/discover");
            reply(&mut write, json!({"jsonrpc":"2.0", "id":discover["id"], "error":{"code":-32601, "message":"Method not found"}})).await;

            let initialize = request(&mut lines).await;
            assert_eq!(initialize["method"], "initialize");
            assert_eq!(initialize["params"]["protocolVersion"], "2024-11-05");
            let result = InitializeResult::new(Default::default())
                .with_protocol_version(ProtocolVersion::V_2024_11_05);
            reply(
                &mut write,
                json!({"jsonrpc":"2.0", "id":initialize["id"], "result":result}),
            )
            .await;
            let initialized = request(&mut lines).await;
            assert_eq!(initialized["method"], "notifications/initialized");
        });
        let client = ().serve_with_lifecycle(client_io, auto()).await.unwrap();
        assert_eq!(
            client.peer_info().unwrap().protocol_version,
            ProtocolVersion::V_2024_11_05
        );
        server.await.unwrap();
        client.cancel().await.unwrap();
    }

    #[tokio::test]
    async fn silent_discover_waits_for_sdk_timeout_then_falls_back() {
        let (client_io, server_io) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server_io);
            let mut lines = BufReader::new(read).lines();
            assert_eq!(request(&mut lines).await["method"], "server/discover");
            let initialize = request(&mut lines).await;
            assert_eq!(initialize["method"], "initialize");
            assert_eq!(initialize["params"]["protocolVersion"], "2024-11-05");
            let result = InitializeResult::new(Default::default())
                .with_protocol_version(ProtocolVersion::V_2024_11_05);
            reply(
                &mut write,
                json!({"jsonrpc":"2.0", "id":initialize["id"], "result":result}),
            )
            .await;
            assert_eq!(
                request(&mut lines).await["method"],
                "notifications/initialized"
            );
        });
        let started = Instant::now();
        let client = timeout(
            Duration::from_secs(20),
            ().serve_with_lifecycle(client_io, auto()),
        )
        .await
        .expect("SDK silent-discover probe exceeded the PoC safety timeout")
        .unwrap();
        assert!(started.elapsed() >= Duration::from_secs(10));
        assert_eq!(
            client.peer_info().unwrap().protocol_version,
            ProtocolVersion::V_2024_11_05
        );
        server.await.unwrap();
        client.cancel().await.unwrap();
    }

    #[tokio::test]
    async fn modern_rejection_does_not_trigger_legacy_initialize() {
        let (client_io, server_io) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server_io);
            let mut lines = BufReader::new(read).lines();
            let discover = request(&mut lines).await;
            reply(&mut write, json!({"jsonrpc":"2.0", "id":discover["id"], "error":{"code":-32021, "message":"Missing required client capability"}})).await;
            assert!(!matches!(
                timeout(Duration::from_millis(300), lines.next_line()).await,
                Ok(Ok(Some(_)))
            ));
        });
        let error =
            ().serve_with_lifecycle(client_io, auto())
                .await
                .err()
                .expect("modern rejection must fail");
        assert!(format!("{error:?}").contains("JsonRpcError"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn uncorrelated_error_does_not_trigger_legacy_initialize() {
        let (client_io, server_io) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server_io);
            let mut lines = BufReader::new(read).lines();
            let discover = request(&mut lines).await;
            assert_eq!(discover["method"], "server/discover");
            reply(&mut write, json!({"jsonrpc":"2.0", "id":99999, "error":{"code":-32601, "message":"Method not found"}})).await;
            assert!(!matches!(
                timeout(Duration::from_millis(300), lines.next_line()).await,
                Ok(Ok(Some(_)))
            ));
        });
        let error =
            ().serve_with_lifecycle(client_io, auto())
                .await
                .err()
                .expect("uncorrelated error must fail");
        assert!(format!("{error:?}").contains("UncorrelatedErrorResponse"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn unsupported_modern_version_does_not_masquerade_as_legacy() {
        let (client_io, server_io) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server_io);
            let mut lines = BufReader::new(read).lines();
            let discover = request(&mut lines).await;
            reply(&mut write, json!({
                "jsonrpc":"2.0", "id":discover["id"],
                "error":{"code":-32022, "message":"Unsupported protocol version", "data":{"supported":["2025-11-25"]}}
            })).await;
            assert!(!matches!(
                timeout(Duration::from_millis(300), lines.next_line()).await,
                Ok(Ok(Some(_)))
            ));
        });
        let error =
            ().serve_with_lifecycle(client_io, auto())
                .await
                .err()
                .expect("incompatible modern version must fail");
        assert!(format!("{error:?}").contains("NoCompatibleProtocolVersion"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn closed_transport_is_not_retried_as_legacy() {
        let (client_io, server_io) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            let (read, _write) = tokio::io::split(server_io);
            let mut lines = BufReader::new(read).lines();
            assert_eq!(request(&mut lines).await["method"], "server/discover");
        });
        let error =
            ().serve_with_lifecycle(client_io, auto())
                .await
                .err()
                .expect("closed transport must fail");
        assert!(format!("{error:?}").contains("ConnectionClosed"));
        server.await.unwrap();
    }
}

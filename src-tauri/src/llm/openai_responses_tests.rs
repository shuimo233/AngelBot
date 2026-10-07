use super::super::auth::StaticCredentialSource;
use super::super::{ThinkingEffort, ThinkingProtocol, ToolImage};
use super::*;

fn config(mode: ModelAuthMode, model: &str) -> ProviderConfig {
    ProviderConfig {
        provider: "openai".into(),
        model: model.into(),
        base_url: "https://api.openai.com/v1".into(),
        api_key: String::new(),
        max_tokens: 1234,
        temperature: 0.4,
        protocol: Some(ModelProtocol::OpenaiResponses),
        auth_mode: mode,
        credential_ref: Some("account-test".into()),
    }
}

fn provider(mode: ModelAuthMode, model: &str) -> OpenAiResponsesProvider {
    OpenAiResponsesProvider::new(
        &config(mode, model),
        StaticCredentialSource::new("deterministic-not-a-key".into(), "account-test".into()),
    )
    .unwrap()
}

fn message(role: &str, content: &str) -> Message {
    Message {
        role: role.into(),
        content: content.into(),
        tool_calls: None,
        tool_call_id: None,
        protocol_state: None,
        tool_images: Vec::new(),
    }
}

fn synthetic_png(width: u32, height: u32) -> ToolImage {
    use base64::Engine;
    let mut seed = 37_u32;
    let pixels: Vec<u8> = (0..width * height * 3)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect();
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        // Seeded noise has stable, near-raw compressed sizes for budget tests.
        encoder.set_compression(png::Compression::Fast);
        encoder.set_filter(png::FilterType::NoFilter);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&pixels).unwrap();
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    ToolImage::from_base64("image/png", &encoded).unwrap()
}

fn image_tool_history(images: Vec<ToolImage>) -> Vec<Message> {
    let mut assistant = message("assistant", "");
    assistant.tool_calls = Some(vec![ToolCall {
        id: "image-call".into(),
        name: "screenshot".into(),
        arguments: json!({}),
    }]);
    let mut output = message("tool", "Screenshot returned; inspect the attached image.");
    output.tool_call_id = Some("image-call".into());
    output.tool_images = images;
    vec![assistant, output]
}

#[test]
fn tool_images_are_correlated_function_outputs_without_changing_text_or_plan_contracts() {
    let image = synthetic_png(2, 2);
    for mode in [ModelAuthMode::ApiKey, ModelAuthMode::ChatgptPlan] {
        let provider = provider(mode, "gpt-5.4");
        assert!(provider.supports_tool_images());
        let history = image_tool_history(vec![image.clone()]);
        let body = provider
            .request_body(&history, Some(&[tool("screenshot")]), None)
            .unwrap();
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert_eq!(body["input"][0]["call_id"], "image-call");
        assert_eq!(body["input"][1]["type"], "function_call_output");
        assert_eq!(body["input"][1]["call_id"], "image-call");
        assert_eq!(
            body["input"][1]["output"],
            json!([
                {"type":"input_text", "text":history[1].content},
                {"type":"input_image", "image_url":image.data_url(), "detail":"auto"},
            ])
        );
        let encoded = serde_json::to_string(&body).unwrap();
        assert!(!encoded.contains("deterministic-not-a-key"));
        assert!(!encoded.contains("account-test"));
        if mode == ModelAuthMode::ChatgptPlan {
            assert_eq!(body["input"][0]["namespace"], TOOL_NAMESPACE);
            assert_eq!(body["tools"][0]["type"], "namespace");
            assert!(body.get("max_output_tokens").is_none());
            assert!(body.get("temperature").is_none());
        }
        let plain = image_tool_history(Vec::new());
        let (input, _) = encode_messages(&plain, "gpt-5.4", "account-test", mode).unwrap();
        assert_eq!(
            input[1],
            json!({
                "type":"function_call_output", "call_id":"image-call", "output":plain[1].content,
            })
        );
    }
}

#[test]
fn tool_images_never_serialize_or_rehydrate_through_the_private_message_contract() {
    let image = synthetic_png(2, 2);
    let history = image_tool_history(vec![image.clone()]);
    let encoded = serde_json::to_string(&history).unwrap();
    assert!(!encoded.contains("tool_images"));
    assert!(!encoded.contains(&image.data_url()));
    let decoded: Vec<Message> = serde_json::from_str(&encoded).unwrap();
    assert!(decoded.iter().all(|message| message.tool_images.is_empty()));
    let mut injected = serde_json::to_value(&history[1]).unwrap();
    injected["tool_images"] = json!([{"image_url":image.data_url()}]);
    let decoded: Message = serde_json::from_value(injected).unwrap();
    assert!(decoded.tool_images.is_empty());
}

#[test]
fn images_cannot_bypass_tool_order_role_or_request_budgets() {
    let image = synthetic_png(2, 2);
    let encode = |history: &[Message]| {
        encode_messages(history, "gpt-5.4", "account-test", ModelAuthMode::ApiKey)
    };
    let history = image_tool_history(vec![image.clone()]);
    assert!(encode(&history[1..]).is_err());
    assert!(encode(&[history[0].clone(), history[1].clone(), history[1].clone()]).is_err());
    assert!(encode(&[
        history[0].clone(),
        message("user", "interrupt"),
        history[1].clone()
    ])
    .is_err());
    for role in ["system", "developer", "user", "assistant"] {
        let mut invalid = message(role, "fixture");
        invalid.tool_images = vec![image.clone()];
        assert!(encode(&[invalid]).is_err());
    }
    let accepted = image_tool_history(vec![image.clone(); 4]);
    assert!(encode(&accepted).is_ok());
    let too_many = image_tool_history(vec![image; 5]);
    assert!(encode(&too_many).unwrap_err().contains("图像"));
    let large = synthetic_png(1024, 512);
    assert!(large.byte_len() > 1_500_000 && large.byte_len() < 2 * 1024 * 1024);
    assert!(encode(&image_tool_history(vec![large.clone(); 2])).is_ok());
    let too_large = image_tool_history(vec![large; 3]);
    assert!(encode(&too_large).unwrap_err().contains("图像"));

    // Existing serialized request-size protection still includes text plus base64.
    let mut history = accepted;
    history[1].content = "x".repeat(MAX_STREAM_BYTES);
    assert!(provider(ModelAuthMode::ApiKey, "gpt-5.4")
        .request_body(&history, None, None)
        .unwrap_err()
        .contains("历史过大"));
}

#[tokio::test]
async fn invalid_image_history_stops_before_credentials_or_transport() {
    let credentials = Arc::new(FailingCredentials {
        reads: std::sync::atomic::AtomicUsize::new(0),
    });
    let provider = OpenAiResponsesProvider::new(
        &config(ModelAuthMode::ChatgptPlan, "gpt-5.4"),
        credentials.clone(),
    )
    .unwrap();
    let history = image_tool_history(vec![synthetic_png(2, 2); 5]);
    for error in [
        provider.chat(&history, None, None).await.unwrap_err(),
        provider
            .chat_stream(&history, None, None, None)
            .await
            .unwrap_err(),
    ] {
        assert!(error.contains("图像"));
        assert!(error.contains("未发送"));
        assert!(!error.contains("secret-token"));
        assert!(!error.contains("data:image"));
    }
    assert_eq!(
        credentials.reads.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert!(provider.latest_usage().is_none());
}

fn tool(name: &str) -> ToolSchema {
    ToolSchema {
        name: name.into(),
        description: "offline test tool".into(),
        parameters: json!({"type":"object","properties":{}}),
    }
}

fn function(id: &str, name: &str, mode: ModelAuthMode) -> Value {
    let mut item = json!({"type":"function_call","id":format!("fc_{id}"),"call_id":id,"name":name,"arguments":"{\"value\":1}","status":"completed"});
    if mode == ModelAuthMode::ChatgptPlan {
        item["namespace"] = json!(TOOL_NAMESPACE);
    }
    item
}

fn output_message(text: &str, phase: &str) -> Value {
    json!({"type":"message","id":format!("msg_{phase}"),"role":"assistant","status":"completed","phase":phase,"content":[{"type":"output_text","text":text,"annotations":[],"logprobs":[]}]})
}

fn completed(items: Vec<Value>) -> Value {
    json!({"type":"response.completed","response":{
        "id":"resp_test","object":"response","status":"completed","error":null,"incomplete_details":null,"model":"gpt-5.4","output":items,
        "usage":{"input_tokens":31,"output_tokens":17}
    }})
}

fn event(value: &Value) -> Vec<u8> {
    format!(
        "event: {}\r\ndata: {}\r\n\r\n",
        value["type"].as_str().unwrap(),
        value
    )
    .into_bytes()
}

#[test]
fn protocol_diagnostics_identify_fixed_stages_without_echoing_payloads() {
    let mut cases = vec![
        (
            b"data: {\"private\":\"fixture-private\"}\n\n".to_vec(),
            "事件 type",
        ),
        (
            b"event: fixture-private\ndata: {\"type\":\"response.created\"}\n\n".to_vec(),
            "事件名称",
        ),
        (event(&json!({"type":"fixture-private"})), "事件类型"),
        (
            event(&json!({"type":"response.completed"})),
            "完成事件 response",
        ),
    ];
    let base = completed(vec![output_message("fixture-private", "final_answer")]);
    for (field, value, stage) in [
        ("object", json!("fixture-private"), "完成对象"),
        ("status", json!("fixture-private"), "完成状态"),
        ("id", json!("fixture-private\\id"), "完成 ID"),
        ("error", json!({"message":"fixture-private"}), "完成 error"),
        (
            "incomplete_details",
            json!({"reason":"fixture-private"}),
            "完成 incomplete_details",
        ),
        ("output", json!("fixture-private"), "output 容器"),
        (
            "usage",
            json!({"input_tokens":"fixture-private","output_tokens":1}),
            "usage 字段",
        ),
        (
            "usage",
            json!({"input_tokens":u64::MAX,"output_tokens":1}),
            "usage 范围",
        ),
    ] {
        let mut value_event = base.clone();
        value_event["response"][field] = value;
        cases.push((event(&value_event), stage));
    }
    for (bytes, stage) in cases {
        let mut stream = parser(ModelAuthMode::ChatgptPlan);
        let error = stream.feed(&bytes, None).unwrap_err();
        assert!(
            error.contains(stage),
            "expected fixed stage {stage}: {error}"
        );
        assert!(!error.contains("fixture-private"));
        assert!(stream.completed.is_none());
    }
}

#[tokio::test]
async fn request_transport_diagnostics_keep_fixed_failure_kinds_without_urls() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let mut transport_provider = provider(ModelAuthMode::ChatgptPlan, "gpt-5.4");
    transport_provider.endpoint =
        reqwest::Url::parse(&format!("http://{address}/fixture-private-url")).unwrap();
    transport_provider.client = reqwest::Client::builder()
        .no_proxy()
        .retry(reqwest::retry::never())
        .timeout(Duration::from_millis(50))
        .build()
        .unwrap();
    // A bound listener that does not accept/respond deterministically times out.
    let timeout_message = transport_provider
        .chat(&[message("user", "OK")], None, None)
        .await
        .unwrap_err();
    assert!(timeout_message.contains("请求超时"));
    assert!(!timeout_message.contains("fixture-private"));
    assert!(!timeout_message.contains(&address.to_string()));
    drop(listener);
    // Windows can take longer than the deliberately tiny timeout above to
    // return connection refused. Give this distinct failure its own deadline.
    transport_provider.client = reqwest::Client::builder()
        .no_proxy()
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let refused_message = transport_provider
        .chat(&[message("user", "OK")], None, None)
        .await
        .unwrap_err();
    assert!(
        refused_message.contains("连接建立失败"),
        "{refused_message}"
    );
    assert!(!refused_message.contains("fixture-private"));
    assert!(!refused_message.contains(&address.to_string()));
}

fn parser(mode: ModelAuthMode) -> ResponsesStream {
    ResponsesStream::new(
        "gpt-5.4",
        "account-test",
        mode,
        ["read".into(), "write".into()].into_iter().collect(),
    )
}

#[test]
fn plan_body_is_stateless_namespaced_and_has_no_unsupported_fields() {
    let provider = provider(ModelAuthMode::ChatgptPlan, "gpt-5.4");
    let body = provider
        .request_body(
            &[
                message("system", "first"),
                message("system", "second"),
                message("developer", "local instruction"),
                message("user", "hello"),
            ],
            Some(&[tool("read"), tool("write")]),
            None,
        )
        .unwrap();
    assert_eq!(body["instructions"], "first\n\nsecond");
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    assert!(body["input"].is_array());
    assert_eq!(body["input"].as_array().unwrap().len(), 2);
    assert_eq!(body["input"][0]["role"], "developer");
    assert_eq!(body["tools"][0]["type"], "namespace");
    assert_eq!(body["tools"][0]["name"], TOOL_NAMESPACE);
    assert_eq!(body["tools"][0]["tools"].as_array().unwrap().len(), 2);
    for field in [
        "previous_response_id",
        "background",
        "conversation",
        "max_output_tokens",
        "max_tool_calls",
        "metadata",
        "moderation",
        "multi_agent",
        "prompt",
        "prompt_cache_retention",
        "safety_identifier",
        "temperature",
        "top_logprobs",
        "top_p",
        "truncation",
        "user",
    ] {
        assert!(body.get(field).is_none(), "unsupported {field}");
    }
}

#[test]
fn api_body_preserves_supported_limits_and_flat_functions() {
    let provider = provider(ModelAuthMode::ApiKey, "gpt-4.1");
    let body = provider
        .request_body(&[message("user", "hello")], Some(&[tool("read")]), None)
        .unwrap();
    assert_eq!(body["max_output_tokens"], 1234);
    assert_eq!(body["temperature"], 0.4);
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["name"], "read");
    assert!(body["tools"][0].get("function").is_none());
}

#[test]
fn sampling_is_omitted_for_reasoning_or_unknown_models() {
    for model in [
        "gpt-5",
        "gpt-5-mini",
        "gpt-5.4",
        "gpt-6-astra",
        "gpt-6.1-sol",
        "o3",
        "o4-mini",
        "unknown-model",
    ] {
        let body = provider(ModelAuthMode::ApiKey, model)
            .request_body(&[message("user", "hello")], None, None)
            .unwrap();
        assert!(body.get("temperature").is_none(), "sampling on {model}");
        assert_eq!(body["max_output_tokens"], 1234);
    }
    let reasoning = ThinkingSettings {
        protocol: ThinkingProtocol::DeepSeek,
        effort: ThinkingEffort::High,
        budget_tokens: None,
    };
    let body = provider(ModelAuthMode::ChatgptPlan, "gpt-5.4")
        .request_body(&[message("user", "hello")], None, Some(&reasoning))
        .unwrap();
    assert_eq!(body["reasoning"]["effort"], "high");
    assert!(body.get("max_output_tokens").is_none());
}

#[test]
fn endpoint_never_routes_plan_credentials_to_custom_hosts_or_backend_api() {
    for base in [
        "https://evil.example/v1",
        "http://api.openai.com/v1",
        "https://api.openai.com:8080/v1",
        "https://api.openai.com/backend-api",
        "https://api.openai.com/v1?token=secret",
        "https://other@api.openai.com/v1",
    ] {
        assert!(
            responses_endpoint(base, ModelAuthMode::ChatgptPlan).is_err(),
            "accepted {base}"
        );
    }
    for base in [
        "",
        "https://api.openai.com",
        "https://api.openai.com/v1/",
        PLAN_ENDPOINT,
    ] {
        assert_eq!(
            responses_endpoint(base, ModelAuthMode::ChatgptPlan)
                .unwrap()
                .as_str(),
            PLAN_ENDPOINT
        );
    }
    assert_eq!(
        responses_endpoint("https://gateway.example/openai/v1", ModelAuthMode::ApiKey)
            .unwrap()
            .as_str(),
        "https://gateway.example/openai/v1/responses"
    );
    assert_eq!(
        responses_endpoint(
            "https://gateway.example/openai/v1/responses",
            ModelAuthMode::ApiKey
        )
        .unwrap()
        .as_str(),
        "https://gateway.example/openai/v1/responses"
    );
    assert_eq!(
        responses_endpoint(
            "https://gateway.example/openai/v1/responses/",
            ModelAuthMode::ApiKey
        )
        .unwrap()
        .as_str(),
        "https://gateway.example/openai/v1/responses"
    );
}

#[test]
fn completed_stream_returns_all_calls_only_after_terminal_event() {
    let mut parser = parser(ModelAuthMode::ChatgptPlan);
    let incremental = json!({"type":"response.function_call_arguments.done","item_id":"fc_call1","arguments":"{\"value\":1}"});
    parser.feed(&event(&incremental), None).unwrap();
    assert!(parser.completed.is_none());
    let mut call2 = function("call2", "write", ModelAuthMode::ChatgptPlan);
    call2.as_object_mut().unwrap().remove("namespace");
    call2["name"] = json!("angelbot.write");
    parser
        .feed(
            &event(&completed(vec![
                output_message("working", "commentary"),
                function("call1", "read", ModelAuthMode::ChatgptPlan),
                call2,
            ])),
            None,
        )
        .unwrap();
    parser.finish().unwrap();
    let result = parser.completed.take().unwrap();
    assert_eq!(result.usage.unwrap().input_tokens, 31);
    let (response, continuation) = result.response.into_parts();
    assert_eq!(continuation.unwrap().output_items.len(), 3);
    match response {
        LlmResponse::ToolCalls {
            calls,
            text,
            stop_reason,
        } => {
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0].id, "call1");
            assert_eq!(calls[0].name, "read");
            assert_eq!(calls[1].name, "write");
            assert_eq!(calls[1].arguments, json!({"value":1}));
            assert_eq!(text.as_deref(), Some("working"));
            assert_eq!(stop_reason, StopReason::ToolUse);
        }
        _ => panic!("expected completed tool calls"),
    }
}

#[test]
fn registered_plan_call_can_omit_optional_namespace_but_cannot_change_it() {
    let mut item = function("call1", "read", ModelAuthMode::ChatgptPlan);
    item.as_object_mut().unwrap().remove("namespace");
    let mut stream = parser(ModelAuthMode::ChatgptPlan);
    stream
        .feed(&event(&completed(vec![item.clone()])), None)
        .unwrap();
    item["namespace"] = json!("other");
    assert!(parser(ModelAuthMode::ChatgptPlan)
        .feed(&event(&completed(vec![item])), None)
        .is_err());
}

#[test]
fn encrypted_reasoning_and_phase_replay_without_duplicate_visible_content_or_calls() {
    let reasoning = json!({"type":"reasoning","id":"rs_private","summary":[],"encrypted_content":"opaque-account-secret","status":"completed"});
    let items = vec![
        reasoning,
        output_message("working", "commentary"),
        function("call1", "read", ModelAuthMode::ChatgptPlan),
    ];
    let completed = parse_completed(
        &completed(items)["response"],
        "gpt-5.4",
        "account-test",
        ModelAuthMode::ChatgptPlan,
        &["read".into()].into_iter().collect(),
    )
    .unwrap();
    let (response, state) = completed.response.into_parts();
    let calls = match response {
        LlmResponse::ToolCalls { calls, .. } => calls,
        _ => panic!("missing calls"),
    };
    let mut assistant = message("assistant", "normalized working");
    assistant.tool_calls = Some(calls);
    assistant.protocol_state = state;
    let mut result = message("tool", "result");
    result.tool_call_id = Some("call1".into());
    let (input, _) = encode_messages(
        &[message("user", "do it"), assistant, result],
        "gpt-5.4",
        "account-test",
        ModelAuthMode::ChatgptPlan,
    )
    .unwrap();
    assert_eq!(input.len(), 5);
    assert_eq!(input[1]["encrypted_content"], "opaque-account-secret");
    assert_eq!(input[2]["phase"], "commentary");
    assert_eq!(input[2]["content"][0]["text"], "working");
    assert!(input[2]["content"][0].get("annotations").is_none());
    assert!(input[2]["content"][0].get("logprobs").is_none());
    assert!(input[3].get("status").is_none());
    assert_eq!(
        input
            .iter()
            .filter(|item| item["type"] == "function_call")
            .count(),
        1
    );
    assert_eq!(
        input
            .iter()
            .filter(|item| item["type"] == "function_call_output")
            .count(),
        1
    );
    assert!(!serde_json::to_string(&input)
        .unwrap()
        .contains("normalized working"));
}

#[test]
fn switching_model_or_account_never_leaks_opaque_continuation() {
    let mut assistant = message("assistant", "visible result");
    assistant.tool_calls = Some(vec![ToolCall {
        id: "call1".into(),
        name: "read".into(),
        arguments: json!({"value":1}),
    }]);
    assistant.protocol_state = Some(ProtocolContinuation {
        protocol: ModelProtocol::OpenaiResponses,
        model: "gpt-5.4".into(),
        credential_ref: "account-test".into(),
        output_items: vec![
            json!({"type":"reasoning","id":"private-id","summary":[],"encrypted_content":"opaque-secret"}),
            function("call1", "read", ModelAuthMode::ChatgptPlan),
        ],
    });
    let mut result = message("tool", "output");
    result.tool_call_id = Some("call1".into());
    for (model, reference) in [("gpt-5.4", "other-account"), ("gpt-4.1", "account-test")] {
        let (input, _) = encode_messages(
            &[assistant.clone(), result.clone()],
            model,
            reference,
            ModelAuthMode::ChatgptPlan,
        )
        .unwrap();
        let encoded = serde_json::to_string(&input).unwrap();
        assert!(!encoded.contains("opaque-secret"));
        assert!(!encoded.contains("private-id"));
        assert_eq!(input[0]["content"], "visible result");
        assert_eq!(input[1]["name"], "read");
        assert_eq!(input[1]["namespace"], TOOL_NAMESPACE);
        assert_eq!(input[2]["call_id"], "call1");
    }
}

#[test]
fn invalid_history_fails_closed_instead_of_sending_orphan_tool_outputs() {
    let mut assistant = message("assistant", "");
    assistant.tool_calls = Some(vec![ToolCall {
        id: "call1".into(),
        name: "read".into(),
        arguments: json!({}),
    }]);
    let mut output = message("tool", "output");
    output.tool_call_id = Some("call1".into());
    let encode = |messages: &[Message]| {
        encode_messages(messages, "gpt-5.4", "account-test", ModelAuthMode::ApiKey)
    };
    assert!(encode(&[output.clone()]).is_err());
    assert!(encode(&[assistant.clone()]).is_err());
    assert!(encode(&[
        assistant.clone(),
        message("user", "interrupt"),
        output.clone()
    ])
    .is_err());
    assert!(encode(&[assistant.clone(), output.clone(), output.clone()]).is_err());
    assert!(encode(&[assistant.clone(), output.clone(), assistant, output]).is_err());
}

#[test]
fn private_state_cannot_inject_additional_tool_authority() {
    let mut assistant = message("assistant", "hello");
    assistant.protocol_state = Some(ProtocolContinuation {
        protocol: ModelProtocol::OpenaiResponses,
        model: "gpt-5.4".into(),
        credential_ref: "account-test".into(),
        output_items: vec![function("call1", "write", ModelAuthMode::ApiKey)],
    });
    assert!(encode_messages(
        &[assistant],
        "gpt-5.4",
        "account-test",
        ModelAuthMode::ApiKey
    )
    .is_err());
}

#[test]
fn split_utf8_crlf_multiline_data_and_comments_are_supported() {
    let mut parser = parser(ModelAuthMode::ApiKey);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let delta = json!({"type":"response.output_text.delta","delta":"你好🙂"});
    let mut bytes = b": keepalive\r\n\r\n".to_vec();
    bytes.extend(event(&delta));
    let done = completed(vec![output_message("你好🙂", "final_answer")]);
    let mut data = serde_json::to_string(&done).unwrap();
    // A legal split at a JSON whitespace location makes two data lines.
    data = data.replacen("{\"response\":", "{\n\"response\":", 1);
    let framed = format!(
        "event: response.completed\r\ndata: {}\r\n\r\n",
        data.replace('\n', "\r\ndata: ")
    );
    bytes.extend(framed.as_bytes());
    for byte in bytes {
        parser.feed(&[byte], Some(&tx)).unwrap();
    }
    assert_eq!(rx.try_recv().unwrap(), "你好🙂");
    assert!(rx.try_recv().is_err());
    parser.finish().unwrap();
    assert!(parser.completed.is_some());
}

#[test]
fn bare_cr_sse_line_endings_work() {
    let data = completed(vec![output_message("hello", "final_answer")]);
    let frame = format!("event: response.completed\rdata: {data}\r\r");
    let mut parser = parser(ModelAuthMode::ApiKey);
    parser.feed(frame.as_bytes(), None).unwrap();
    parser.finish().unwrap();
    assert!(parser.completed.is_some());
}

#[test]
fn optional_utf8_bom_can_be_split_across_chunks() {
    let value = completed(vec![output_message("hello", "final_answer")]);
    let bytes = format!("\u{feff}data: {value}\n\n").into_bytes();
    let mut stream = parser(ModelAuthMode::ApiKey);
    for byte in bytes {
        stream.feed(&[byte], None).unwrap();
    }
    stream.finish().unwrap();
    assert!(stream.completed.is_some());
}

#[test]
fn eof_incomplete_failed_and_error_events_never_produce_tool_calls() {
    let mut parser = parser(ModelAuthMode::ApiKey);
    parser
        .feed(
            &event(&json!({"type":"response.function_call_arguments.done","arguments":"{}"})),
            None,
        )
        .unwrap();
    assert!(parser.finish().unwrap_err().contains("response.completed"));
    assert!(parser.completed.is_none());
    for failure in [
        json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}),
        json!({"type":"response.failed","response":{"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"never expose token-123"}}}),
        json!({"type":"error","code":"subscription_sharing_usage_unavailable","message":"never expose token-123"}),
    ] {
        let mut stream = super::tests::parser(ModelAuthMode::ApiKey);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        stream
            .feed(
                &event(&json!({"type":"response.output_text.delta","delta":"partial"})),
                Some(&tx),
            )
            .unwrap();
        let error = stream.feed(&event(&failure), Some(&tx)).unwrap_err();
        assert_eq!(rx.try_recv().unwrap(), "partial");
        assert!(!error.contains("token-123"));
        assert!(stream.completed.is_none());
    }
}

#[test]
fn malformed_sse_and_incomplete_terminal_payloads_fail_closed() {
    for bytes in [
        b"data: not-json\n\n".as_slice(),
        b"data: [DONE]\n\n",
        b"event: response.completed\ndata: {\"type\":\"response.failed\"}\n\n",
        b"data: \xff\n\n",
    ] {
        assert!(parser(ModelAuthMode::ApiKey).feed(bytes, None).is_err());
    }
    let mut stream = parser(ModelAuthMode::ApiKey);
    stream
        .feed(b"data: {\"type\":\"response.completed\"}", None)
        .unwrap();
    assert!(stream.finish().unwrap_err().contains("截断"));
    for status in ["incomplete", "failed", "in_progress"] {
        let mut data = completed(vec![function("call1", "read", ModelAuthMode::ApiKey)]);
        data["response"]["status"] = json!(status);
        assert!(parser(ModelAuthMode::ApiKey)
            .feed(&event(&data), None)
            .is_err());
    }
}

#[test]
fn duplicate_ids_bad_arguments_unknown_namespace_and_unoffered_tools_are_rejected() {
    let mode = ModelAuthMode::ChatgptPlan;
    let original = function("call1", "read", mode);
    let mut duplicate_call = function("call1", "write", mode);
    duplicate_call["id"] = json!("fc_different");
    let mut duplicate_item = function("call2", "write", mode);
    duplicate_item["id"] = original["id"].clone();
    for items in [
        vec![original.clone(), duplicate_call],
        vec![original.clone(), duplicate_item],
    ] {
        assert!(parser(mode).feed(&event(&completed(items)), None).is_err());
    }
    for (field, value) in [
        ("arguments", json!("{\"unfinished\":")),
        ("arguments", json!("[]")),
        ("namespace", json!("foreign")),
        ("name", json!("foreign.read")),
        ("name", json!("unoffered")),
        ("call_id", json!("")),
        ("status", json!("in_progress")),
    ] {
        let mut item = original.clone();
        item[field] = value;
        let mut stream = parser(mode);
        assert!(
            stream.feed(&event(&completed(vec![item])), None).is_err(),
            "accepted bad {field}"
        );
        assert!(stream.completed.is_none());
    }
}

#[test]
fn api_rejects_namespace_when_it_only_advertised_flat_functions() {
    assert!(parser(ModelAuthMode::ApiKey)
        .feed(
            &event(&completed(vec![function(
                "call1",
                "read",
                ModelAuthMode::ChatgptPlan
            )])),
            None
        )
        .is_err());
}

#[test]
fn bounded_framing_and_total_stream_limits_are_enforced() {
    let mut stream = parser(ModelAuthMode::ApiKey);
    assert!(stream
        .feed(&vec![b'x'; MAX_BUFFER_BYTES + 1], None)
        .unwrap_err()
        .contains("大小限制"));
    let mut stream = parser(ModelAuthMode::ApiKey);
    let comment = vec![b':', b' ', b'a', b'\n'];
    let chunk = comment.repeat(64 * 1024);
    for _ in 0..MAX_STREAM_BYTES / chunk.len() {
        stream.feed(&chunk, None).unwrap();
    }
    assert!(stream
        .feed(&comment, None)
        .unwrap_err()
        .contains("大小限制"));
}

#[test]
fn invalid_usage_and_oversized_private_output_fail_before_tools() {
    let mut data = completed(vec![function("call1", "read", ModelAuthMode::ApiKey)]);
    data["response"]["usage"]["input_tokens"] = json!(-1);
    assert!(parser(ModelAuthMode::ApiKey)
        .feed(&event(&data), None)
        .is_err());
    let data = completed(vec![
        json!({"type":"reasoning","summary":[],"encrypted_content":"x".repeat(512*1024)}),
        function("call1", "read", ModelAuthMode::ApiKey),
    ]);
    assert!(parser(ModelAuthMode::ApiKey)
        .feed(&event(&data), None)
        .is_err());
}

#[test]
fn refusal_is_visible_but_does_not_grant_tool_authority() {
    let refusal = json!({"type":"message","role":"assistant","content":[{"type":"refusal","refusal":"Cannot help"}]});
    let mut stream = parser(ModelAuthMode::ApiKey);
    stream
        .feed(&event(&completed(vec![refusal.clone()])), None)
        .unwrap();
    match stream.completed.unwrap().response.into_parts().0 {
        LlmResponse::Text { text, stop_reason } => {
            assert_eq!(text, "Cannot help");
            assert_eq!(stop_reason, StopReason::ContentFilter);
        }
        _ => panic!("expected refusal text"),
    }
    assert!(parser(ModelAuthMode::ApiKey)
        .feed(
            &event(&completed(vec![
                refusal,
                function("call1", "write", ModelAuthMode::ApiKey)
            ])),
            None
        )
        .is_err());
}

#[test]
fn known_structured_errors_are_clear_and_do_not_echo_remote_secrets() {
    for code in [
        "subscription_sharing_usage_limit_exceeded",
        "subscription_sharing_usage_unavailable",
        "subscription_sharing_user_not_eligible",
        "subscription_sharing_invalid_user",
        "subscription_sharing_unsupported_capability",
        "chatpass_v2_scope_not_authorized",
        "unknown",
    ] {
        let error = structured_error(
            &json!({"code":code,"message":"secret-token","param":"secret-token"}),
            Some(403),
        );
        assert!(!error.contains("secret-token"));
        assert!(!error.is_empty());
    }
    assert!(structured_error(
        &json!({"code":"subscription_sharing_usage_limit_exceeded"}),
        None
    )
    .contains("不会自动切换"));
    assert!(structured_error(&Value::Null, Some(302)).contains("重定向"));
}

#[test]
fn continuation_debug_does_not_contain_reasoning_or_credentials() {
    let state = ProtocolContinuation {
        protocol: ModelProtocol::OpenaiResponses,
        model: "gpt-5.4".into(),
        credential_ref: "private-account".into(),
        output_items: vec![
            json!({"type":"reasoning","summary":[],"encrypted_content":"private-reasoning"}),
        ],
    };
    let debug = format!("{state:?}");
    assert!(!debug.contains("private-account"));
    assert!(!debug.contains("private-reasoning"));
}

#[test]
fn completed_calls_cannot_reuse_ids_from_already_executed_history() {
    let mut stream = parser(ModelAuthMode::ApiKey);
    stream.prior_call_ids.insert("call1".into());
    let error = stream
        .feed(
            &event(&completed(vec![function(
                "call1",
                "write",
                ModelAuthMode::ApiKey,
            )])),
            None,
        )
        .unwrap_err();
    assert!(error.contains("历史"));
    assert!(stream.completed.is_none());
}

#[test]
fn multiple_tool_outputs_are_correlated_by_id_not_result_order() {
    let mut assistant = message("assistant", "");
    assistant.tool_calls = Some(vec![
        ToolCall {
            id: "call1".into(),
            name: "read".into(),
            arguments: json!({}),
        },
        ToolCall {
            id: "call2".into(),
            name: "write".into(),
            arguments: json!({}),
        },
    ]);
    let mut output2 = message("tool", "second");
    output2.tool_call_id = Some("call2".into());
    let mut output1 = message("tool", "first");
    output1.tool_call_id = Some("call1".into());
    let (input, _) = encode_messages(
        &[assistant, output2, output1, message("user", "continue")],
        "gpt-5.4",
        "account-test",
        ModelAuthMode::ApiKey,
    )
    .unwrap();
    assert_eq!(input[2]["call_id"], "call2");
    assert_eq!(input[2]["output"], "second");
    assert_eq!(input[3]["call_id"], "call1");
    assert_eq!(input[3]["output"], "first");
}

struct FailingCredentials {
    reads: std::sync::atomic::AtomicUsize,
}

struct RevocableCredentials {
    revoked: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl ModelCredentialSource for RevocableCredentials {
    async fn bearer_token(&self) -> Result<String, String> {
        Ok("deterministic-not-a-key".into())
    }

    fn credential_ref(&self) -> &str {
        "account-test"
    }

    fn validate_session(&self) -> Result<(), String> {
        if self.revoked.load(std::sync::atomic::Ordering::SeqCst) {
            Err("revoked protected account contains secret-token".into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn completion_after_session_revocation_cannot_return_tools_or_commit_usage() {
    let credentials = Arc::new(RevocableCredentials {
        revoked: std::sync::atomic::AtomicBool::new(false),
    });
    let provider = OpenAiResponsesProvider::new(
        &config(ModelAuthMode::ChatgptPlan, "gpt-5.4"),
        credentials.clone(),
    )
    .unwrap();
    let mut stream = parser(ModelAuthMode::ChatgptPlan);
    stream
        .feed(
            &event(&completed(vec![function(
                "call1",
                "write",
                ModelAuthMode::ChatgptPlan,
            )])),
            None,
        )
        .unwrap();
    credentials
        .revoked
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let error = provider
        .accept_completed(stream.completed.take().unwrap())
        .unwrap_err();
    assert!(error.contains("会话已失效"));
    assert!(error.contains("未提交工具调用"));
    assert!(!error.contains("secret-token"));
    assert!(!error.contains("protected account"));
    assert!(provider.latest_usage().is_none());
}

#[async_trait]
impl ModelCredentialSource for FailingCredentials {
    async fn bearer_token(&self) -> Result<String, String> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err("refresh failure contains secret-token".into())
    }
    fn credential_ref(&self) -> &str {
        "account-test"
    }
}

#[tokio::test]
async fn refresh_is_resolved_every_request_and_failures_stop_before_network_without_echoing_secrets(
) {
    let credentials = Arc::new(FailingCredentials {
        reads: std::sync::atomic::AtomicUsize::new(0),
    });
    let provider = OpenAiResponsesProvider::new(
        &config(ModelAuthMode::ChatgptPlan, "gpt-5.4"),
        credentials.clone(),
    )
    .unwrap();
    *provider.last_usage.lock().unwrap() = Some(ProviderUsage {
        input_tokens: 999,
        output_tokens: 999,
    });
    let messages = [message("user", "offline test")];
    for error in [
        provider.chat(&messages, None, None).await.unwrap_err(),
        provider
            .chat_stream(&messages, None, None, None)
            .await
            .unwrap_err(),
    ] {
        assert!(error.contains("刷新失败"));
        assert!(error.contains("未发送"));
        assert!(!error.contains("secret-token"));
    }
    assert_eq!(
        credentials.reads.load(std::sync::atomic::Ordering::SeqCst),
        2
    );
    assert!(provider.latest_usage().is_none());
}

fn responses_http_fixture(
    status: u16,
    media_type: Option<&str>,
    body: Vec<u8>,
) -> (reqwest::Url, std::thread::JoinHandle<()>) {
    responses_http_fixture_checked(status, media_type, body, |_, _| {})
}

fn responses_http_fixture_checked(
    status: u16,
    media_type: Option<&str>,
    body: Vec<u8>,
    check_request: impl FnOnce(&str, &Value) + Send + 'static,
) -> (reqwest::Url, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let header = format!(
        "HTTP/1.1 {status} Fixture\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n",
        media_type
            .map(|value| format!("Content-Type: {value}\r\n"))
            .unwrap_or_default(),
        body.len()
    );
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "fixture accept timed out"
                    );
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(_) => panic!("fixture accept failed"),
            }
        };
        // Windows accepted sockets can retain the listener's nonblocking mode.
        // Read the full fixture request with a bounded blocking timeout.
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            let read = stream.read(&mut chunk).unwrap();
            assert!(read > 0 && request.len() + read <= 64 * 1024);
            request.extend_from_slice(&chunk[..read]);
            if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&request[..end]).unwrap();
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                if request.len() >= end + 4 + length {
                    let value: Value = serde_json::from_slice(&request[end + 4..]).unwrap();
                    assert_eq!(value["stream"], true);
                    assert_eq!(value["store"], false);
                    assert!(value.get("tools").is_none());
                    check_request(headers, &value);
                    break;
                }
            }
        }
        stream.write_all(header.as_bytes()).unwrap();
        stream.write_all(&body).unwrap();
    });
    (
        reqwest::Url::parse(&format!("http://{address}/responses")).unwrap(),
        server,
    )
}

#[tokio::test]
async fn local_http_receives_image_function_output_without_live_credentials() {
    let image = synthetic_png(2, 2);
    let image_url = image.data_url();
    let (endpoint, server) = responses_http_fixture_checked(
        200,
        Some("text/event-stream"),
        event(&completed(vec![output_message(
            "fixture image understood",
            "final_answer",
        )])),
        move |headers, request| {
            assert!(!headers.to_ascii_lowercase().contains("authorization:"));
            assert_eq!(request["input"][0]["call_id"], "image-call");
            assert_eq!(request["input"][1]["call_id"], "image-call");
            assert_eq!(request["input"][1]["output"][1]["image_url"], image_url);
            assert_eq!(request["input"][1]["output"][1]["type"], "input_image");
            let encoded = serde_json::to_string(request).unwrap();
            assert!(!encoded.contains("deterministic-not-a-key"));
            assert!(!encoded.contains("account-test"));
        },
    );
    let mut provider = provider(ModelAuthMode::None, "gpt-5.4");
    provider.endpoint = endpoint;
    provider.client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let result = provider
        .chat(&image_tool_history(vec![image]), None, None)
        .await;
    server.join().unwrap();
    match result.unwrap().into_parts().0 {
        LlmResponse::Text { text, .. } => assert_eq!(text, "fixture image understood"),
        _ => panic!("fixture must return ordinary completed text"),
    }
}

async fn fixture_request(
    status: u16,
    media_type: Option<&str>,
    body: Vec<u8>,
) -> Result<LlmResponse, String> {
    let (endpoint, server) = responses_http_fixture(status, media_type, body);
    let mut provider = provider(ModelAuthMode::ChatgptPlan, "gpt-5.4");
    // Test-only transport override; production plan endpoint remains fixed.
    provider.endpoint = endpoint;
    provider.client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let result = provider
        .chat(&[message("user", "fixture")], None, None)
        .await;
    server.join().unwrap();
    result
}

#[tokio::test]
async fn http_response_seam_exposes_safe_non_sse_categories_without_accepting_output() {
    for (status, media_type, body, expected) in [
        (
            200,
            Some("text/html; private-account"),
            b"<html>secret-token</html>".to_vec(),
            "HTML",
        ),
        (
            200,
            Some("application/json"),
            br#"{"object":"response","status":"completed","output":[{"secret":"secret-token"}]}"#
                .to_vec(),
            "JSON",
        ),
        (204, None, Vec::new(), "空响应"),
        (
            200,
            None,
            b"event: response.completed\ndata: secret-token\n\n".to_vec(),
            "缺失",
        ),
    ] {
        let error = fixture_request(status, media_type, body).await.unwrap_err();
        assert!(error.contains(&format!("HTTP {status}")), "{error}");
        assert!(error.contains(expected), "{error}");
        assert!(error.contains("未提交工具调用"), "{error}");
        assert!(!error.contains("secret-token"));
        assert!(!error.contains("private-account"));
    }
}

#[tokio::test]
async fn http_json_error_preserves_recovery_even_when_status_is_success_without_echoing_body() {
    for status in [200, 429] {
        let error = fixture_request(
            status,
            Some("application/json"),
            br#"{"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"secret-token"}}"#.to_vec(),
        )
        .await
        .unwrap_err();
        assert!(error.contains("套餐共享用量已达限制"), "{error}");
        assert!(!error.contains("secret-token"));
    }
}

#[tokio::test]
async fn http_sse_content_type_parameters_and_completed_response_are_accepted() {
    for media_type in [Some("Text/Event-Stream; charset=utf-8"), None] {
        let response = fixture_request(
            200,
            media_type,
            event(&completed(vec![output_message("OK", "final_answer")])),
        )
        .await
        .unwrap();
        match response.into_parts().0 {
            LlmResponse::Text {
                text,
                stop_reason: StopReason::EndTurn,
            } => assert_eq!(text, "OK"),
            _ => panic!("fixture did not produce completed text"),
        }
    }
}

#[tokio::test]
async fn completed_empty_output_uses_complete_stream_items_after_terminal_only() {
    let done = json!({"type":"response.output_item.done","output_index":0,"item":output_message("OK", "final_answer")});
    for media_type in [Some("text/event-stream"), None] {
        let mut body = event(&done);
        body.extend(event(&completed(vec![])));
        let response = fixture_request(200, media_type, body).await.unwrap();
        match response.into_parts().0 {
            LlmResponse::Text {
                text,
                stop_reason: StopReason::EndTurn,
            } => assert_eq!(text, "OK"),
            _ => panic!("fixture did not return complete stream text"),
        }
    }
    let mut stream = parser(ModelAuthMode::ChatgptPlan);
    let reasoning = json!({"type":"reasoning","id":"rs_test","summary":[],"encrypted_content":"private-opaque"});
    for (index, item) in [
        function("call1", "read", ModelAuthMode::ChatgptPlan),
        reasoning,
    ]
    .into_iter()
    .enumerate()
    .rev()
    {
        stream
            .feed(
                &event(
                    &json!({"type":"response.output_item.done","output_index":index,"item":item}),
                ),
                None,
            )
            .unwrap();
        assert!(stream.completed.is_none());
    }
    stream.feed(&event(&completed(vec![])), None).unwrap();
    let result = stream.completed.take().unwrap();
    let (response, continuation) = result.response.into_parts();
    match response {
        LlmResponse::ToolCalls { calls, .. } => assert_eq!(calls.len(), 1),
        _ => panic!("missing validated tool call"),
    }
    let continuation = continuation.unwrap();
    assert_eq!(continuation.output_items[0]["call_id"], "call1");
    assert_eq!(
        continuation.output_items[1]["encrypted_content"],
        "private-opaque"
    );
}

#[test]
fn streamed_items_do_not_bypass_terminal_identity_completeness_or_tool_checks() {
    let good = json!({"type":"response.output_item.done","output_index":0,"item":function("call1","read",ModelAuthMode::ChatgptPlan)});
    let cases = [
        vec![event(&good)],
        vec![
            event(&json!({"type":"response.output_item.done","item":good["item"]})),
            event(&completed(vec![])),
        ],
        vec![
            event(
                &json!({"type":"response.output_item.done","output_index":MAX_CONTINUATION_ITEMS,"item":good["item"]}),
            ),
            event(&completed(vec![])),
        ],
        vec![
            event(
                &json!({"type":"response.output_item.added","output_index":0,"item":{"id":"fc_other"}}),
            ),
            event(&good),
            event(&completed(vec![])),
        ],
        vec![
            event(
                &json!({"type":"response.output_item.done","output_index":0,"item":output_message(&"x".repeat(MAX_CONTINUATION_BYTES),"final_answer")}),
            ),
            event(&completed(vec![])),
        ],
        vec![event(&good), event(&good), event(&completed(vec![]))],
        vec![
            event(
                &json!({"type":"response.output_item.done","output_index":1,"item":good["item"]}),
            ),
            event(&completed(vec![])),
        ],
        vec![
            event(
                &json!({"type":"response.output_item.added","output_index":1,"item":{"id":"msg_missing"}}),
            ),
            event(&good),
            event(&completed(vec![])),
        ],
        vec![
            event(&json!({"type":"response.created","response":{"id":"resp_other"}})),
            event(&good),
            event(&completed(vec![])),
        ],
        vec![
            event(
                &json!({"type":"response.output_item.done","output_index":0,"item":function("call1","not_allowed",ModelAuthMode::ChatgptPlan)}),
            ),
            event(&completed(vec![])),
        ],
        vec![
            event(&good),
            event(
                &json!({"type":"response.failed","response":{"error":{"code":"subscription_sharing_usage_unavailable"}}}),
            ),
        ],
        vec![
            event(&json!({"type":"response.output_text.delta","delta":"not-authoritative"})),
            event(&completed(vec![])),
        ],
    ];
    for events in cases {
        let mut stream = parser(ModelAuthMode::ChatgptPlan);
        let mut failure = None;
        for bytes in events {
            if let Err(error) = stream.feed(&bytes, None) {
                failure = Some(error);
                break;
            }
        }
        assert!(failure.is_some() || stream.finish().is_err());
        assert!(stream.completed.is_none());
    }
    // A nonempty terminal array remains authoritative; done items are not a
    // second source of tool authority or duplicate visible messages.
    let mut stream = parser(ModelAuthMode::ChatgptPlan);
    stream.feed(&event(&good), None).unwrap();
    stream
        .feed(
            &event(&completed(vec![output_message("final", "final_answer")])),
            None,
        )
        .unwrap();
    match stream.completed.take().unwrap().response.into_parts().0 {
        LlmResponse::Text { text, .. } => assert_eq!(text, "final"),
        _ => panic!("terminal output was not authoritative"),
    }
}

#[tokio::test]
async fn missing_content_type_does_not_accept_non_sse_bodies_or_incomplete_events() {
    let valid = completed(vec![output_message("secret-token", "final_answer")]);
    let mut html = b"<html>\n".to_vec();
    html.extend(event(&valid));
    html.extend(b"</html>\n");
    let mut truncated = event(&valid);
    // Bare CR is a valid SSE line ending. Remove the whole final separator,
    // not just its LF, to represent a genuinely uncommitted event.
    truncated.truncate(truncated.len() - 2);
    for body in [
        html,
        serde_json::to_vec(&valid["response"]).unwrap(),
        truncated,
        b"event: response.created\ndata: {\"type\":\"response.created\"}\n\n".to_vec(),
    ] {
        let error = fixture_request(200, None, body).await.unwrap_err();
        assert!(error.contains("HTTP 200"));
        assert!(error.contains("缺失"));
        assert!(error.contains("未提交工具调用"));
        assert!(!error.contains("secret-token"));
    }
}

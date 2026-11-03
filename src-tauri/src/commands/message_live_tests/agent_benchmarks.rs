//! Opt-in BFCL call selection and two synthetic state tasks. No real services,
//! files, memory, account actions, desktop or production architecture changes.
use super::*;
use crate::agent::tool::{Tool, ToolHandler, ToolResult};
use crate::agent::ToolRegistry;
use crate::agent::{AgentResponse, AgentSliceOutcome};
use crate::llm::{
    LlmFailure, LlmFailureKind, LlmProvider, LlmResponse, ThinkingEffort, ThinkingProtocol,
    ThinkingSettings, ToolSchema,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    question: String,
    tools: Vec<ToolSchema>,
}

#[derive(Default)]
pub(super) struct Metrics {
    calls: AtomicUsize,
    input: AtomicUsize,
    output: AtomicUsize,
    reported_usage_count: AtomicUsize,
    failure: Mutex<Option<serde_json::Value>>,
    inputs: Mutex<Vec<serde_json::Value>>,
    outputs: Mutex<Vec<serde_json::Value>>,
    requests: Mutex<Vec<serde_json::Value>>,
}

impl Metrics {
    pub(super) fn request_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

struct BoundedProvider {
    inner: Box<dyn LlmProvider>,
    metrics: Arc<Metrics>,
    max_calls: usize,
    root: PathBuf,
}

/// Owned evaluations retain only bounded public text, never private replay or
/// malformed protocol payloads. This is diagnostic evidence, not model input.
fn public_response(response: &LlmResponse, schemas: &[ToolSchema]) -> serde_json::Value {
    match response {
        LlmResponse::WithProtocol { response, .. } => public_response(response, schemas),
        LlmResponse::Text { text, stop_reason } => serde_json::json!({"kind":"text",
            "text":text.chars().take(16_000).collect::<String>(),
            "truncated":text.chars().count()>16_000,"stopReason":format!("{stop_reason:?}")}),
        LlmResponse::ToolCalls { calls, .. } => {
            serde_json::json!({"kind":"tool_calls","count":calls.len(),"names":calls.iter().take(32).map(|c|if schemas.iter().any(|s|s.name==c.name){c.name.as_str()}else{"[unrecognized]"}).collect::<Vec<_>>()})
        }
        LlmResponse::ProtocolViolation { .. } => serde_json::json!({"kind":"protocol_violation"}),
    }
}

fn private_failure(root: &Path, diagnostic: &str) -> Option<String> {
    let conn = crate::db::init(&root.join("diagnostics.db")).ok()?;
    crate::db::migrate(&conn).ok()?;
    // Attention evidence requires an owner session, even in this isolated DB.
    // Never depend on (or copy) the ordinary app's sessions for diagnostics.
    conn.execute("INSERT OR IGNORE INTO sessions(id,title,created_at,updated_at) VALUES('agent-benchmark','Owned evaluation diagnostic',0,0)",[]).ok()?;
    crate::agent::attention::record_recoverable_runtime_failure(
        &conn,
        "agent-benchmark",
        "diagnostic:agent-benchmark",
        "agent-benchmark",
        diagnostic,
    )
    .ok()?
    .evidence_refs
    .first()
    .cloned()
}

fn response_observation(provider: &dyn LlmProvider, root: &Path) -> Option<serde_json::Value> {
    let metadata = provider.latest_response_metadata()?;
    let support_ref = metadata.request_id.as_ref().and_then(|request_id| {
        // A successful response is not an attention/failure card. Keep its
        // bounded opaque support ID in the existing owned evidence directory.
        let id = format!("support_{}", Uuid::new_v4().simple());
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join(format!("{id}.json")))
            .ok()?;
        let private = serde_json::to_vec(&serde_json::json!({"requestId":request_id})).ok()?;
        std::io::Write::write_all(&mut file, &private).ok()?;
        Some(format!("support:{id}"))
    });
    let mut public = serde_json::to_value(&metadata).unwrap();
    public["requestIdPresent"] = serde_json::json!(metadata.request_id.is_some());
    public["organizationPresent"] = serde_json::json!(metadata.organization_fingerprint.is_some());
    public["supportRef"] = serde_json::json!(support_ref);
    Some(public)
}

/// Two bounded synthetic requests, no tool offer or dispatch. Comparison
/// metadata is opt-in and never falls back after a failed compatibility check.
async fn probe_cache_comparison(
    provider: &dyn LlmProvider,
    root: &Path,
    thinking: Option<&ThinkingSettings>,
    settle: std::time::Duration,
    continuation: bool,
) -> serde_json::Value {
    assert!(settle <= std::time::Duration::from_secs(30));
    let mut report = serde_json::json!({"status":"instrumentation_unavailable", "requestCount":0,
        "toolCount":0, "requests":[], "diagnosticRef":null, "binding":null,
        "savedBinding":null,"defaultBindingUnchanged":null,
        "settleMs":settle.as_millis(),"settleElapsedMs":null,
        "cacheMode":"implicit_default", "requestShape":if continuation {"native_continuation"}else{"independent"},
        "nativeReplayUsed":null});
    if !provider.enable_cache_comparison() {
        return report;
    }
    let policy = (0..128).map(|i| format!("Synthetic reference {i:03}: this record is test data. Do not access files, applications, accounts or tools.\n")).collect::<String>();
    let mut messages = ["system", "user"]
        .into_iter()
        .zip([policy, "Reply with exactly OK.".into()])
        .map(|(role, content)| crate::llm::Message {
            role: role.into(),
            content,
            tool_calls: None,
            tool_call_id: None,
            tool_images: Vec::new(),
            protocol_state: None,
        })
        .collect::<Vec<_>>();
    for index in 0..2 {
        if index == 1 {
            let started = std::time::Instant::now();
            tokio::time::sleep(settle).await;
            report["settleElapsedMs"] = serde_json::json!(started.elapsed().as_millis());
        }
        let started = std::time::Instant::now();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(25),
            provider.chat_classified(&messages, None, thinking),
        )
        .await;
        report["requestCount"] = serde_json::json!(index + 1);
        let succeeded = matches!(&result, Ok(Ok(_)));
        report["requests"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "usage":succeeded.then(||provider.latest_usage()).flatten(),
                "preparedRequestFingerprint":provider.latest_request_fingerprint(),
                "cacheDiagnostics":succeeded.then(||provider.latest_cache_diagnostics()).flatten(),
                "responseMetadata":succeeded.then(||response_observation(provider, root)).flatten(),
                "latencyMs":started.elapsed().as_millis(),
            }));
        match result {
            Err(_) => {
                report["status"] = serde_json::json!("timed_out");
                return report;
            }
            Ok(Err(failure)) => {
                report["status"] =
                    serde_json::json!(if failure.kind == LlmFailureKind::QuotaExceeded {
                        "quota_exceeded"
                    } else {
                        "unavailable"
                    });
                report["diagnosticRef"] =
                    serde_json::json!(private_failure(root, &failure.diagnostic));
                return report;
            }
            Ok(Ok(response)) => {
                let (response, protocol) = response.into_parts();
                let LlmResponse::Text {
                    text,
                    stop_reason: crate::llm::StopReason::EndTurn,
                } = response
                else {
                    report["status"] = serde_json::json!("unexpected_response");
                    return report;
                };
                if index == 0 && continuation {
                    report["nativeReplayUsed"] = serde_json::json!(protocol.is_some());
                    messages.push(crate::llm::Message {
                        role: "assistant".into(),
                        content: text,
                        tool_calls: None,
                        tool_call_id: None,
                        tool_images: Vec::new(),
                        protocol_state: protocol,
                    });
                    let mut user = messages[1].clone();
                    user.content = "Reply with exactly OK again.".into();
                    messages.push(user);
                }
            }
        }
    }
    report["status"] = serde_json::json!(match provider.latest_cache_diagnostics() {
        Some(d) if d.details_invalid => "invalid_diagnostics",
        Some(d) if matches!(d.kind.as_str(), "cache_hit" | "cache_miss") => "diagnostics_reported",
        Some(_) => "diagnostics_unavailable",
        None => "not_reported",
    });
    report
}

pub(super) fn configured_cache_comparison() -> serde_json::Value {
    let settle_ms = std::env::var("ANGELBOT_CACHE_COMPARISON_SETTLE_MS")
        .unwrap_or_else(|_| "0".into())
        .parse::<u64>()
        .unwrap();
    assert!(settle_ms <= 30_000);
    let settle = std::time::Duration::from_millis(settle_ms);
    let continuation =
        std::env::var("ANGELBOT_CACHE_COMPARISON_CONTINUATION").as_deref() == Ok("1");
    let unavailable = || {
        serde_json::json!({"status":"instrumentation_unavailable","requestCount":0,
        "toolCount":0,"requests":[],"diagnosticRef":null,"binding":null,
        "savedBinding":null,"defaultBindingUnchanged":null,
        "settleMs":settle_ms,"settleElapsedMs":null,
        "cacheMode":"implicit_default", "requestShape":if continuation {"native_continuation"}else{"independent"},
        "nativeReplayUsed":null})
    };
    let Ok(app) = headless_credential_host() else {
        return unavailable();
    };
    let Ok(mut config) =
        crate::commands::foreground_model_factory::resolve_model_config(Some(app.handle()))
    else {
        return unavailable();
    };
    if config.auth_mode != crate::llm::ModelAuthMode::ChatgptPlan || !config.model.contains("luna")
    {
        return unavailable();
    }
    let saved = config.clone();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    if std::env::var("ANGELBOT_CACHE_COMPARISON_ALTERNATE").as_deref() == Ok("1") {
        let selected = runtime.block_on(async {
            let status = crate::llm::chatgpt_auth::status(app.handle())
                .map_err(|_| "catalog_unavailable")?;
            if status.credential_ref != saved.credential_ref {
                return Err("binding_changed");
            }
            let models =
                crate::commands::model_connection::chatgpt_plan_models(app.handle().clone())
                    .await
                    .map_err(|_| "catalog_unavailable")?;
            let status =
                crate::llm::chatgpt_auth::status(app.handle()).map_err(|_| "binding_changed")?;
            if status.credential_ref != saved.credential_ref {
                return Err("binding_changed");
            }
            let models = serde_json::to_value(models).map_err(|_| "catalog_unavailable")?;
            select_cache_comparison_model(&models).ok_or("model_not_listed")
        });
        match selected {
            Ok(model) => config.model = model.into(),
            Err(status) => {
                let mut report = unavailable();
                report["status"] = serde_json::json!(status);
                return report;
            }
        }
    }
    config.max_tokens = config.max_tokens.min(256);
    let Ok(provider) = crate::llm::create_provider_for_app(&config, Some(app.handle())) else {
        return unavailable();
    };
    let Ok(thinking) = crate::llm::resolve_thinking_settings(&config, Some("low")) else {
        return unavailable();
    };
    let root = PathBuf::from(std::env::var_os("ANGELBOT_PRIVATE_CACHE_PROBE_ROOT").unwrap());
    let mut report = runtime.block_on(probe_cache_comparison(
        provider.as_ref(),
        &root,
        thinking.as_ref(),
        settle,
        continuation,
    ));
    report["binding"] = serde_json::json!({"provider":config.provider,"model":config.model});
    report["savedBinding"] = serde_json::json!({"provider":saved.provider,"model":saved.model});
    report["defaultBindingUnchanged"] = serde_json::json!(
        crate::commands::foreground_model_factory::resolve_model_config(Some(app.handle()))
            .is_ok_and(|after| after.provider == saved.provider
                && after.model == saved.model
                && after.base_url == saved.base_url
                && after.auth_mode == saved.auth_mode
                && after.credential_ref == saved.credential_ref)
    );
    report
}

fn select_cache_comparison_model(models: &serde_json::Value) -> Option<&'static str> {
    ["gpt-6.1-sol", "gpt-6-sol"].into_iter().find(|id| {
        models
            .as_array()
            .is_some_and(|models| models.iter().any(|model| model["id"].as_str() == Some(*id)))
    })
}

#[tokio::test]
async fn cache_comparison_probe_is_bounded_and_stops_without_retry_or_tools() {
    assert_eq!(
        select_cache_comparison_model(
            &serde_json::json!([{"id":"gpt-6-luna"},{"id":"unverified-sol"}])
        ),
        None
    );
    assert_eq!(
        select_cache_comparison_model(
            &serde_json::json!([{"id":"gpt-6-sol"},{"id":"gpt-6.1-sol"}])
        ),
        Some("gpt-6.1-sol")
    );
    struct Probe {
        enabled: bool,
        fail_at: Option<usize>,
        calls: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl LlmProvider for Probe {
        fn name(&self) -> &str {
            "owned-cache-probe"
        }
        fn enable_cache_comparison(&self) -> bool {
            self.enabled
        }
        fn latest_usage(&self) -> Option<crate::llm::ProviderUsage> {
            Some(crate::llm::ProviderUsage {
                input_tokens: 1500,
                output_tokens: 1,
                cached_input_tokens: Some(0),
                ..Default::default()
            })
        }
        fn latest_cache_diagnostics(&self) -> Option<crate::llm::ProviderCacheDiagnostics> {
            Some(crate::llm::ProviderCacheDiagnostics {
                kind: "cache_hit".into(),
                reason: None,
                comparison_reusable_tokens: Some(1500),
                cache_missed_tokens: Some(0),
                details_invalid: false,
            })
        }
        fn latest_response_metadata(&self) -> Option<crate::llm::ProviderResponseMetadata> {
            Some(crate::llm::ProviderResponseMetadata {
                model_matches_requested: Some(true),
                service_tier: Some("default".into()),
                request_id: Some("req_PRIVATE-CACHE-SUPPORT".into()),
                organization_fingerprint: Some("PRIVATE-ORGANIZATION-FINGERPRINT".into()),
                ..Default::default()
            })
        }
        async fn chat(
            &self,
            _: &[crate::llm::Message],
            _: Option<&[ToolSchema]>,
            _: Option<&ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            panic!("use the classified seam")
        }
        async fn chat_classified(
            &self,
            messages: &[crate::llm::Message],
            tools: Option<&[ToolSchema]>,
            _: Option<&ThinkingSettings>,
        ) -> Result<LlmResponse, LlmFailure> {
            let index = self.calls.fetch_add(1, Ordering::SeqCst);
            assert!(index < 2 && tools.is_none());
            assert!(messages.len() == 2 || index == 1 && messages.len() == 4);
            assert_eq!(messages[0].role, "system");
            assert!(messages[0].content.starts_with("Synthetic reference 000:"));
            assert_eq!(messages[1].content, "Reply with exactly OK.");
            if messages.len() == 4 {
                assert_eq!(messages[2].role, "assistant");
                assert_eq!(messages[2].content, "OK");
                assert_eq!(messages[3].role, "user");
                assert_eq!(messages[3].content, "Reply with exactly OK again.");
            }
            if self.fail_at == Some(index) {
                return Err(LlmFailure {
                    kind: LlmFailureKind::QuotaExceeded,
                    diagnostic: "PRIVATE-CACHE-PROBE-FAILURE".into(),
                });
            }
            Ok(LlmResponse::Text {
                text: "OK".into(),
                stop_reason: crate::llm::StopReason::EndTurn,
            })
        }
    }
    for continuation in [false, true] {
        for (enabled, fail_at, expected, status) in [
            (false, None, 0, "instrumentation_unavailable"),
            (true, Some(0), 1, "quota_exceeded"),
            (true, Some(1), 2, "quota_exceeded"),
            (true, None, 2, "diagnostics_reported"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let provider = Probe {
                enabled,
                fail_at,
                calls: AtomicUsize::new(0),
            };
            let report = probe_cache_comparison(
                &provider,
                root.path(),
                None,
                std::time::Duration::ZERO,
                continuation,
            )
            .await;
            assert_eq!(provider.calls.load(Ordering::SeqCst), expected);
            assert_eq!(report["requestCount"], expected);
            assert_eq!(report["status"], status);
            assert_eq!(report["toolCount"], 0);
            assert!(!report.to_string().contains("PRIVATE-CACHE"));
            assert!(!report.to_string().contains("PRIVATE-ORGANIZATION"));
            for request in report["requests"].as_array().unwrap() {
                if let Some(reference) = request["responseMetadata"]["supportRef"].as_str() {
                    let path = root.path().join(format!(
                        "{}.json",
                        reference.strip_prefix("support:").unwrap()
                    ));
                    assert!(std::fs::read_to_string(path)
                        .unwrap()
                        .contains("req_PRIVATE-CACHE-SUPPORT"));
                }
            }
            if let Some(failed_index) = fail_at {
                assert!(report["requests"][failed_index]["usage"].is_null());
                assert!(report["requests"][failed_index]["cacheDiagnostics"].is_null());
                assert!(report["requests"][failed_index]["responseMetadata"].is_null());
            }
        }
    }
}

#[test]
fn benchmark_failure_evidence_has_an_isolated_owner_and_no_public_payload() {
    let root = tempfile::tempdir().unwrap();
    let reference = private_failure(root.path(), "PRIVATE-OWNED-PROVIDER-ERROR").unwrap();
    assert!(reference.starts_with("diag:"));
    let conn = crate::db::init(&root.path().join("diagnostics.db")).unwrap();
    let payload: String = conn
        .query_row(
            "SELECT payload FROM attention_diagnostic_evidence WHERE id=?1",
            [reference.strip_prefix("diag:").unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(payload, "PRIVATE-OWNED-PROVIDER-ERROR");
    let visible = crate::agent::attention::load_visible_summary(&conn, "agent-benchmark")
        .unwrap()
        .unwrap();
    let encoded = serde_json::to_string(&visible).unwrap();
    assert!(!encoded.contains("PRIVATE-OWNED-PROVIDER-ERROR"));
    assert!(!encoded.contains(&reference));
    let preview = public_response(
        &LlmResponse::Text {
            text: "界".repeat(16_001),
            stop_reason: crate::llm::StopReason::EndTurn,
        },
        &[],
    );
    assert_eq!(preview["text"].as_str().unwrap().chars().count(), 16_000);
    assert_eq!(preview["truncated"], true);
    let malformed = public_response(
        &LlmResponse::ProtocolViolation {
            raw_text: "PRIVATE-PROTOCOL".into(),
            reason: "PRIVATE-REASON".into(),
        },
        &[],
    );
    assert!(!malformed.to_string().contains("PRIVATE"));
    let calls = LlmResponse::ToolCalls {
        calls: vec![crate::llm::ToolCall {
            id: "PRIVATE-ID".into(),
            name: "PRIVATE-UNKNOWN".into(),
            arguments: serde_json::json!({"value":"PRIVATE-VALUE"}),
        }],
        text: Some("PRIVATE-TEXT".into()),
        stop_reason: crate::llm::StopReason::ToolUse,
    };
    let hidden = public_response(&calls, &[]);
    assert_eq!(hidden["names"], serde_json::json!(["[unrecognized]"]));
    assert!(!hidden.to_string().contains("PRIVATE"));
}

#[async_trait::async_trait]
impl LlmProvider for BoundedProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn latest_request_fingerprint(&self) -> Option<crate::llm::ProviderRequestFingerprint> {
        self.inner.latest_request_fingerprint()
    }
    fn latest_response_metadata(&self) -> Option<crate::llm::ProviderResponseMetadata> {
        self.inner.latest_response_metadata()
    }
    fn latest_usage(&self) -> Option<crate::llm::ProviderUsage> {
        self.inner.latest_usage()
    }
    async fn chat(
        &self,
        messages: &[crate::llm::Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
    ) -> Result<LlmResponse, String> {
        self.chat_classified(messages, tools, thinking)
            .await
            .map_err(|e| e.diagnostic)
    }
    async fn chat_classified(
        &self,
        messages: &[crate::llm::Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
    ) -> Result<LlmResponse, LlmFailure> {
        if let Some(failure) = self.metrics.failure.lock().unwrap().as_ref() {
            return Err(LlmFailure {
                kind: if failure["status"] == "quota_exceeded" {
                    LlmFailureKind::QuotaExceeded
                } else {
                    LlmFailureKind::Unclassified
                },
                diagnostic: "Owned evaluation stopped after its first failure".into(),
            });
        }
        let request_index =
            self.metrics
                .calls
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                    (n < self.max_calls).then_some(n + 1)
                });
        let request_index = match request_index {
            Ok(index) => index,
            Err(_) => {
                *self.metrics.failure.lock().unwrap() =
                    Some(serde_json::json!({"status":"budget_exceeded","diagnosticRef":null}));
                return Err(LlmFailure {
                    kind: LlmFailureKind::Unclassified,
                    diagnostic: "Owned evaluation provider-call budget reached".into(),
                });
            }
        };
        // Hold schema order constant between A/B fixtures. Test-only control,
        // not a change to the production registry or a caching claim.
        let ordered_tools = tools.map(|schemas| {
            let mut schemas = schemas.to_vec();
            schemas.sort_by(|a, b| a.name.cmp(&b.name));
            schemas
        });
        let tools = ordered_tools.as_deref();
        self.metrics.inputs.lock().unwrap().push(serde_json::json!({
            "messages":messages.iter().map(|m| serde_json::json!({"role":m.role,"content":m.content,"toolCalls":m.tool_calls,"toolCallId":m.tool_call_id})).collect::<Vec<_>>(),
            "tools":tools,
            "resolvedThinking":thinking.map(|settings| serde_json::json!({
                "protocol":match settings.protocol {
                    ThinkingProtocol::OpenAiResponses => "openai_responses",
                    ThinkingProtocol::DeepSeek => "deepseek",
                    ThinkingProtocol::AnthropicManual => "anthropic_manual",
                    ThinkingProtocol::AnthropicAdaptive => "anthropic_adaptive",
                    ThinkingProtocol::AnthropicEffortOnly => "anthropic_effort_only",
                },
                "effort":match settings.effort {
                    ThinkingEffort::Low => "low",
                    ThinkingEffort::Medium => "medium",
                    ThinkingEffort::High => "high",
                },
                "budgetTokens":settings.budget_tokens,
            })),
        }));
        let request_started = std::time::Instant::now();
        let result = self.inner.chat_classified(messages, tools, thinking).await;
        let reported_usage = result.as_ref().ok().and_then(|_| self.inner.latest_usage());
        self.metrics.requests.lock().unwrap().push(serde_json::json!({
            "requestIndex":request_index, "latencyMs":request_started.elapsed().as_millis(),
            "preparedRequestFingerprint":self.inner.latest_request_fingerprint(),
            "responseMetadata":result.as_ref().ok().and_then(|_|response_observation(self.inner.as_ref(), &self.root)),
            "usage":reported_usage, "usageStatus":if result.is_err(){"failed"}else if reported_usage.is_some(){"reported"}else{"missing"},
            "firstTokenMs":null, "actualCostUsd":null,
        }));
        self.metrics.outputs.lock().unwrap().push(match &result {
            Ok(response) => public_response(response, tools.unwrap_or_default()),
            Err(_) => serde_json::json!({"kind":"provider_error"}),
        });
        if let Err(failure) = &result {
            let status = if failure.kind == LlmFailureKind::QuotaExceeded {
                "quota_exceeded"
            } else {
                "unavailable"
            };
            *self.metrics.failure.lock().unwrap() = Some(
                serde_json::json!({"status":status,"diagnosticRef":private_failure(&self.root,&failure.diagnostic)}),
            );
        } else if let Some(usage) = reported_usage {
            self.metrics
                .reported_usage_count
                .fetch_add(1, Ordering::SeqCst);
            self.metrics
                .input
                .fetch_add(usage.input_tokens as usize, Ordering::SeqCst);
            self.metrics
                .output
                .fetch_add(usage.output_tokens as usize, Ordering::SeqCst);
        }
        result
    }
}

fn agent_config(root: Option<&Path>) -> crate::agent::config::AgentConfig {
    let mut config = crate::agent::config::AgentConfig::default();
    config.preferences.use_long_term_memory = false;
    config.preferences.evolution_enabled = false;
    config.work_dir = root.map(Path::to_path_buf);
    config
}

pub(super) fn row_metrics(
    row: &mut serde_json::Value,
    metrics: &Metrics,
    started: std::time::Instant,
) {
    row["requestCount"] = serde_json::json!(metrics.calls.load(Ordering::SeqCst));
    row["latencyMs"] = serde_json::json!(started.elapsed().as_millis());
    row["inputTokens"] = serde_json::json!(metrics.input.load(Ordering::SeqCst));
    row["outputTokens"] = serde_json::json!(metrics.output.load(Ordering::SeqCst));
    row["reportedUsageCount"] =
        serde_json::json!(metrics.reported_usage_count.load(Ordering::SeqCst));
    row["providerInputs"] = serde_json::json!(&*metrics.inputs.lock().unwrap());
    row["providerOutputs"] = serde_json::json!(&*metrics.outputs.lock().unwrap());
    row["requestMetrics"] = serde_json::json!(&*metrics.requests.lock().unwrap());
    if let Some(failure) = metrics.failure.lock().unwrap().as_ref() {
        row["status"] = failure["status"].clone();
        row["diagnosticRef"] = failure["diagnosticRef"].clone();
    }
}

pub(super) fn bounded_provider(
    config: &crate::llm::ProviderConfig,
    app: &tauri::AppHandle,
    root: &Path,
    max_calls: usize,
) -> (Arc<dyn LlmProvider>, Arc<Metrics>) {
    let (provider, metrics) = boxed_bounded_provider(config, app, root, max_calls);
    (Arc::from(provider), metrics)
}

pub(super) fn boxed_bounded_provider(
    config: &crate::llm::ProviderConfig,
    app: &tauri::AppHandle,
    root: &Path,
    max_calls: usize,
) -> (Box<dyn LlmProvider>, Arc<Metrics>) {
    meter_provider(
        crate::llm::create_provider_for_app(config, Some(app)).unwrap(),
        root,
        max_calls,
    )
}

pub(super) fn meter_provider(
    inner: Box<dyn LlmProvider>,
    root: &Path,
    max_calls: usize,
) -> (Box<dyn LlmProvider>, Arc<Metrics>) {
    let metrics = Arc::new(Metrics::default());
    (
        meter_with_metrics(inner, root, max_calls, metrics.clone()),
        metrics,
    )
}

/// One journey budget includes foreground replies, background learning and
/// reader continuations, even though each resolves a fresh provider instance.
pub(super) fn meter_with_metrics(
    inner: Box<dyn LlmProvider>,
    root: &Path,
    max_calls: usize,
    metrics: Arc<Metrics>,
) -> Box<dyn LlmProvider> {
    let provider = BoundedProvider {
        inner,
        metrics,
        max_calls,
        root: root.to_path_buf(),
    };
    Box::new(provider)
}

#[tokio::test]
async fn benchmark_metrics_record_forwarded_thinking_and_missing_usage() {
    struct ReportOnce(AtomicUsize);
    #[async_trait::async_trait]
    impl LlmProvider for ReportOnce {
        fn name(&self) -> &str {
            "fixture"
        }
        fn latest_usage(&self) -> Option<crate::llm::ProviderUsage> {
            (self.0.load(Ordering::SeqCst) == 1).then_some(crate::llm::ProviderUsage {
                input_tokens: 11,
                output_tokens: 7,
                cached_input_tokens: Some(5),
                ..crate::llm::ProviderUsage::default()
            })
        }
        async fn chat(
            &self,
            _: &[crate::llm::Message],
            _: Option<&[ToolSchema]>,
            thinking: Option<&ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            let index = self.0.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                thinking.unwrap().effort,
                if index == 0 {
                    ThinkingEffort::Low
                } else {
                    ThinkingEffort::Medium
                }
            );
            Ok(LlmResponse::Text {
                text: "done".into(),
                stop_reason: crate::llm::StopReason::EndTurn,
            })
        }
    }
    let root = tempfile::tempdir().unwrap();
    let (provider, metrics) =
        meter_provider(Box::new(ReportOnce(AtomicUsize::new(0))), root.path(), 2);
    let started = std::time::Instant::now();
    for effort in [ThinkingEffort::Low, ThinkingEffort::Medium] {
        let settings = ThinkingSettings {
            protocol: ThinkingProtocol::OpenAiResponses,
            effort,
            budget_tokens: None,
        };
        provider
            .chat_classified(&[], None, Some(&settings))
            .await
            .unwrap();
    }
    let mut row = serde_json::json!({});
    row_metrics(&mut row, &metrics, started);
    assert_eq!(row["requestCount"], 2);
    assert_eq!(row["reportedUsageCount"], 1);
    assert_eq!(row["inputTokens"], 11);
    assert_eq!(row["outputTokens"], 7);
    assert_eq!(row["requestMetrics"].as_array().unwrap().len(), 2);
    assert_eq!(row["requestMetrics"][0]["usage"]["cachedInputTokens"], 5);
    assert!(row["requestMetrics"][0]["usage"]["cacheWriteTokens"].is_null());
    assert!(row["requestMetrics"][1]["usage"].is_null());
    assert_eq!(row["requestMetrics"][1]["usageStatus"], "missing");
    // Cross the same Python summary seam used by the live Main report.
    let mut child = std::process::Command::new("python")
        .args(["-c", r#"
import json, sys
from companion_eval import summarize_metering
row=json.load(sys.stdin)
s=summarize_metering([row])
assert s['cacheReadKnownRequests']==1 and s['cacheReadCoverage']==0.5
assert s['tokenCacheHitRateObserved']==5/11 and not s['cacheComplete']
assert s['cacheWriteTokensObserved'] is None and s['actualCostUsd'] is None
assert s['reportedUsageRequests']==1 and not s['usageComplete']
old=summarize_metering([{'requestCount':2}])
assert old['tokenCacheHitRateObserved'] is None and old['cacheReadCoverage']==0
row['requestMetrics']=[{'usageStatus':'reported','usage':{'inputTokens':100,'cachedInputTokens':0},'latencyMs':1},{'usageStatus':'reported','usage':{'inputTokens':900,'cachedInputTokens':900},'latencyMs':2}]
s=summarize_metering([row])
assert s['tokenCacheHitRateObserved']==0.9 and s['requestCacheHitRateObserved']==0.5
assert s['cacheComplete'] and s['cacheWriteTokensObserved'] is None
row['requestMetrics'][0]['usage']['detailsInvalid']=True
assert summarize_metering([row])['cacheReadKnownRequests']==1
from context_eval import natural_preference_checks
natural={'id':'natural-journey','status':'completed','forcedBoundedReview':False,'cooldownModified':False,'ordinaryThreshold':10,
 'turns':[{'completed':True}]*10,'sourceMessages':20,'originalQuotePreserved':True,'coldOpened':True,
 'view':{'originalMessages':0,'explicitPreferenceCount':1},'source':'a','target':'b','answer':'绿茶',
 'requestCount':12,'reportedUsageCount':12,'providerInputs':[{'messages':[{'role':'user','content':'我平常喝点什么更合口味？只说饮品，不确定就说明。'}]}]}
assert all(natural_preference_checks([natural]).values())
for key,value,check in [('forcedBoundedReview',True,'naturalLearning'),('requestCount',17,'sharedBudget'),('reportedUsageCount',11,'metered'),('sourceMessages',19,'persistedSource')]:
 changed=dict(natural);changed[key]=value
 assert not natural_preference_checks([changed])[check]
"#])
        .env("PYTHONPATH", std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts"))
        .stdin(std::process::Stdio::piped()).spawn().unwrap();
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(row.to_string().as_bytes())
            .unwrap();
    }
    assert!(child.wait().unwrap().success());
    for (index, effort) in ["low", "medium"].iter().enumerate() {
        assert_eq!(
            row["providerInputs"][index]["resolvedThinking"],
            serde_json::json!({
                "protocol":"openai_responses", "effort":effort, "budgetTokens":null,
            })
        );
    }
    // Fresh provider objects must not reset a journey-wide budget.
    let shared = Arc::new(Metrics::default());
    let settings = ThinkingSettings {
        protocol: ThinkingProtocol::OpenAiResponses,
        effort: ThinkingEffort::Low,
        budget_tokens: None,
    };
    for attempt in 0..2 {
        let provider = meter_with_metrics(
            Box::new(ReportOnce(AtomicUsize::new(0))),
            root.path(),
            1,
            shared.clone(),
        );
        assert_eq!(
            provider
                .chat_classified(&[], None, Some(&settings))
                .await
                .is_ok(),
            attempt == 0
        );
    }
    assert_eq!(shared.request_count(), 1);
    assert_eq!(
        shared.failure.lock().unwrap().as_ref().unwrap()["status"],
        "budget_exceeded"
    );
}

struct SelectionOnly;
impl ToolHandler for SelectionOnly {
    fn name(&self) -> &str {
        "selection_only"
    }
    fn execute(&self, _: &serde_json::Value, _: &Path) -> ToolResult {
        ToolResult::error(self.name(), "BFCL selection tools must not be executed")
    }
}

fn selection_registry(schemas: &[ToolSchema]) -> ToolRegistry {
    let mut registry = ToolRegistry::empty();
    for schema in schemas {
        registry.register(
            Tool::new(&schema.name, &schema.description, schema.parameters.clone()),
            Arc::new(SelectionOnly),
        );
    }
    registry
}

#[derive(Serialize)]
struct Delivery {
    day: String,
    revision: i64,
    writes: usize,
    reads_before: usize,
    reads_after: usize,
    #[serde(skip)]
    conflict_pending: bool,
}
impl Default for Delivery {
    fn default() -> Self {
        Self {
            day: "Wednesday".into(),
            revision: 17,
            writes: 0,
            reads_before: 0,
            reads_after: 0,
            conflict_pending: false,
        }
    }
}
struct DeliveryTool {
    name: &'static str,
    world: Arc<Mutex<Delivery>>,
}
impl ToolHandler for DeliveryTool {
    fn name(&self) -> &str {
        self.name
    }
    fn execute(&self, args: &serde_json::Value, _: &Path) -> ToolResult {
        let mut world = self.world.lock().unwrap();
        if self.name == "read_delivery" {
            if world.writes == 0 {
                world.reads_before += 1;
            } else {
                world.reads_after += 1;
            }
            let snapshot = serde_json::json!({"day":world.day,"revision":world.revision});
            if world.conflict_pending {
                // A concurrent metadata update after this snapshot makes its revision stale.
                world.revision += 1;
                world.conflict_pending = false;
            }
            return ToolResult::success(self.name, snapshot.to_string());
        } else {
            if args["revision"].as_i64() != Some(world.revision)
                || !matches!(args["day"].as_str(), Some("Wednesday" | "Thursday"))
            {
                return ToolResult::error(
                    self.name,
                    "No update: use the current revision and an available day.",
                );
            }
            world.day = args["day"].as_str().unwrap().to_owned();
            world.revision += 1;
            world.writes += 1;
        }
        ToolResult::success(
            self.name,
            serde_json::json!({"day":world.day,"revision":world.revision}).to_string(),
        )
    }
}

fn delivery_registry(world: &Arc<Mutex<Delivery>>) -> ToolRegistry {
    let mut registry = ToolRegistry::empty();
    for (name,description,schema) in [
        ("read_delivery","Read the current synthetic delivery day and revision. Does not modify delivery data.",serde_json::json!({"type":"object","properties":{},"required":[],"additionalProperties":false})),
        ("set_delivery_day","Update only the synthetic delivery day. Requires the current revision from read_delivery; stale revisions do not modify data.",serde_json::json!({"type":"object","properties":{"day":{"type":"string","enum":["Wednesday","Thursday"]},"revision":{"type":"integer"}},"required":["day","revision"],"additionalProperties":false})),
    ] {
        registry.register(Tool::new(name,description,schema),Arc::new(DeliveryTool {name,world:world.clone()}));
    }
    registry
}

#[derive(Default, Serialize)]
struct MemoryLookup {
    queries: Vec<String>,
    target_seen: bool,
    irrelevant_seen: bool,
    #[serde(skip)]
    updated: bool,
}
struct MemoryTool(Arc<Mutex<MemoryLookup>>);
impl ToolHandler for MemoryTool {
    fn name(&self) -> &str {
        "recall_memories"
    }
    fn execute(&self, args: &serde_json::Value, _: &Path) -> ToolResult {
        let query = args["query"].as_str().unwrap_or("").trim();
        let mut world = self.0.lock().unwrap();
        world.queries.push(query.to_owned());
        let mut found = vec![];
        let records = if world.updated {
            vec![
                (
                    "m-old",
                    "我平时最常喝的茶是白毫银针。",
                    "2026-10-01",
                    Some("m-tea"),
                ),
                (
                    "m-tea",
                    "我已经不喝白毫银针了，现在最常喝祁门红茶。",
                    "2026-10-22",
                    None,
                ),
                ("m-cat", "我的猫叫梨子。", "2026-10-21", None),
            ]
        } else {
            vec![
                ("m-tea", "我平时最常喝的茶是白毫银针。", "2026-10-01", None),
                ("m-cat", "我的猫叫梨子。", "2026-10-01", None),
            ]
        };
        for (id, content, recorded_at, superseded_by) in records {
            if !query.is_empty() && content.contains(query) {
                world.target_seen |= id == "m-tea";
                world.irrelevant_seen |= id == "m-cat";
                found.push(serde_json::json!({"id":id,"content":content,"recorded_at":recorded_at,"superseded_by":superseded_by}));
            }
        }
        ToolResult::success(self.name(), serde_json::to_string(&found).unwrap())
    }
}

fn memory_registry(world: &Arc<Mutex<MemoryLookup>>) -> ToolRegistry {
    let mut registry = ToolRegistry::empty();
    registry.register(
        Tool::new(
            "recall_memories",
            "Search synthetic personal memories by a literal keyword. Choose a short keyword; an empty query returns no records. No automatic memory context is supplied.",
            serde_json::json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}),
        ),
        Arc::new(MemoryTool(world.clone())),
    );
    registry
}

#[test]
fn agent_benchmark_world_rejects_stale_updates_and_hides_gold_input() {
    assert!(serde_json::from_value::<Case>(
        serde_json::json!({"id":"x","question":"x","tools":[],"expected":"gold"})
    )
    .is_err());
    let world = Arc::new(Mutex::new(Delivery::default()));
    let write = DeliveryTool {
        name: "set_delivery_day",
        world: world.clone(),
    };
    let read = DeliveryTool {
        name: "read_delivery",
        world: world.clone(),
    };
    let args = serde_json::json!({"day":"Thursday","revision":17});
    assert!(
        !write
            .execute(
                &serde_json::json!({"day":"Thursday","revision":16}),
                Path::new(".")
            )
            .success
    );
    assert!(read.execute(&serde_json::json!({}), Path::new(".")).success);
    assert!(write.execute(&args, Path::new(".")).success);
    assert!(!write.execute(&args, Path::new(".")).success);
    assert!(read.execute(&serde_json::json!({}), Path::new(".")).success);
    let world = world.lock().unwrap();
    assert_eq!(
        (
            world.writes,
            world.revision,
            world.reads_before,
            world.reads_after
        ),
        (1, 18, 1, 1)
    );
    drop(world);
    assert!(
        write
            .execute(
                &serde_json::json!({"day":"Wednesday","revision":18}),
                Path::new(".")
            )
            .success,
        "the fixture must permit a wrong-for-this-goal action; the independent grader rejects it"
    );
    let memory = Arc::new(Mutex::new(MemoryLookup::default()));
    let tool = MemoryTool(memory.clone());
    assert!(
        tool.execute(&serde_json::json!({"query":"茶"}), Path::new("."))
            .success
    );
    let memory = memory.lock().unwrap();
    assert!(memory.target_seen);
    assert!(!memory.irrelevant_seen);
    assert_eq!(memory.queries, ["茶"]);
}

#[test]
fn agent_benchmark_holdout_preserves_conflict_and_argument_contracts() {
    let world = Arc::new(Mutex::new(Delivery {
        conflict_pending: true,
        ..Delivery::default()
    }));
    let read = DeliveryTool {
        name: "read_delivery",
        world: world.clone(),
    };
    let write = DeliveryTool {
        name: "set_delivery_day",
        world: world.clone(),
    };
    assert!(read.execute(&serde_json::json!({}), Path::new(".")).success);
    assert_eq!(world.lock().unwrap().revision, 18);
    assert!(
        !write
            .execute(
                &serde_json::json!({"revision":17,"day":"Thursday"}),
                Path::new(".")
            )
            .success
    );
    assert!(read.execute(&serde_json::json!({}), Path::new(".")).success);
    assert!(
        write
            .execute(
                &serde_json::json!({"revision":18,"day":"Thursday"}),
                Path::new(".")
            )
            .success
    );
    assert_eq!(world.lock().unwrap().writes, 1);
    let memory = Arc::new(Mutex::new(MemoryLookup {
        updated: true,
        ..MemoryLookup::default()
    }));
    assert!(
        MemoryTool(memory.clone())
            .execute(&serde_json::json!({"query":"茶"}), Path::new("."))
            .success
    );
    assert!(memory.lock().unwrap().target_seen);
    assert!(!memory.lock().unwrap().irrelevant_seen);
    let registry = selection_registry(&[ToolSchema {
        name: "rect".into(),
        description: "synthetic argument validation".into(),
        parameters: serde_json::json!({"type":"object","properties":{"tolerance":{"type":"number"}},"required":[]}),
    }]);
    assert!(registry
        .validate_arguments("rect", &serde_json::json!({}))
        .is_ok());
    assert!(registry
        .validate_arguments("rect", &serde_json::json!({"tolerance":""}))
        .is_err());
}

#[test]
#[ignore = "explicit scripts/agent_eval.py --live; saved Luna and synthetic tools only"]
fn configured_agent_benchmarks() {
    assert_eq!(
        std::env::var("ANGELBOT_AGENT_EVAL_LIVE").as_deref(),
        Ok("1")
    );
    let root = PathBuf::from(std::env::var_os("ANGELBOT_PRIVATE_AGENT_EVAL_ROOT").unwrap());
    let input = std::fs::read(root.join("input.json")).unwrap();
    assert!(input.len() <= 64 * 1024);
    let cases: Vec<Case> = serde_json::from_slice(&input).unwrap();
    let suite = std::env::var("ANGELBOT_AGENT_EVAL_SUITE").unwrap_or_else(|_| "smoke".into());
    assert!(matches!(suite.as_str(), "smoke" | "holdout"));
    let holdout = suite == "holdout";
    let selected_arm = std::env::var("ANGELBOT_AGENT_EVAL_ARM").ok();
    assert!(selected_arm
        .as_deref()
        .is_none_or(|arm| matches!(arm, "minimal_fc" | "angelbot_prompt")));
    assert_eq!(cases.len(), if holdout { 8 } else { 6 });
    assert!(cases
        .iter()
        .all(|c| c.id.len() <= 80 && c.question.len() <= 8192 && c.tools.len() <= 5));
    let app = headless_credential_host().unwrap();
    let mut config =
        crate::commands::foreground_model_factory::resolve_model_config(Some(app.handle()))
            .unwrap();
    assert_eq!(config.auth_mode, crate::llm::ModelAuthMode::ChatgptPlan);
    assert!(config.model.contains("luna"));
    config.max_tokens = config.max_tokens.min(1024);
    config.temperature = 0.0;
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        for case in &cases {
            let registry=selection_registry(&case.tools);
            for arm in ["minimal_fc","angelbot_prompt"] {
                if selected_arm.as_deref().is_some_and(|selected| selected != arm) { continue; }
                let metrics=Arc::new(Metrics::default());
                let provider=BoundedProvider {inner:crate::llm::create_provider_for_app(&config,Some(app.handle())).unwrap(),metrics:metrics.clone(),max_calls:1,root:root.clone()};
                let system=if arm=="angelbot_prompt" {crate::agent::prompt::build_agent_system_prompt(&agent_config(None),&case.tools,&[])} else {String::new()};
                let mut messages=vec![];
                if !system.is_empty() {messages.push(super::companion::message("system",system));}
                messages.push(super::companion::message("user",case.question.clone()));
                let started=std::time::Instant::now();
                let mut row=serde_json::json!({"kind":"bfcl","id":case.id,"arm":arm,"status":"invalid_response","model":config.model,"calls":[],"text":null,"diagnosticRef":null,"requestedTemperature":0,"maxTokens":config.max_tokens,"input":messages.iter().map(|m|serde_json::json!({"role":m.role,"content":m.content})).collect::<Vec<_>>(),"inputChars":messages.iter().map(|m|m.content.chars().count()).sum::<usize>()});
                match tokio::time::timeout(Duration::from_secs(45),provider.chat_classified(&messages,Some(&case.tools),None)).await {
                    Err(_)=>row["status"]=serde_json::json!("timed_out"),
                    Ok(Ok(response))=> {
                        let (response,protocol)=response.into_parts();
                        if protocol.is_none_or(|p|p.validate().is_ok()) {
                            match response {
                                LlmResponse::ToolCalls {calls,stop_reason:crate::llm::StopReason::ToolUse,..} if !calls.is_empty()=>{row["argumentsValid"]=serde_json::json!(calls.iter().all(|c|registry.validate_arguments(&c.name,&c.arguments).is_ok()));row["calls"]=serde_json::json!(calls);row["status"]=serde_json::json!("completed");},
                                LlmResponse::Text {text,stop_reason:crate::llm::StopReason::EndTurn} if !text.trim().is_empty()=>{row["text"]=serde_json::json!(text);row["status"]=serde_json::json!("completed");},
                                _=>{}
                            }
                        }
                    }
                    Ok(Err(_))=>{}
                }
                row_metrics(&mut row,&metrics,started);
                println!("AGENT_EVAL_ROW {row}");
                if row["status"]!="completed" {return;}
            }
        }
        if std::env::var("ANGELBOT_AGENT_EVAL_STATIC_ONLY").as_deref()==Ok("1") { return; }
        let tasks=if holdout {[
            ("delivery_conflict","Read the current delivery details, change its day to Thursday using the current revision, then read it again to confirm. If a revision is stale, reread and use the new revision; do not repeat an update that succeeded. Work only with the delivery tools provided."),
            ("memory_updated","请用记忆查询工具，只查与饮茶有关的资料，告诉我现在最常喝哪种茶。按记录日期和替代关系区分以前与现在，不读取无关资料。最后只回答当前茶名。"),
        ]} else {[
            ("delivery_change","Read the current delivery details, change its day to Thursday using the current revision, then read it again to confirm. Work only with the delivery tools provided."),
            ("memory_keyword","请用记忆查询工具，只查与饮茶有关的资料，告诉我平时最常喝哪种茶。不要根据常识猜测，不读取无关资料。最后只回答茶名。"),
        ]};
        for (id,goal) in tasks {
            let world=Arc::new(Mutex::new(Delivery {conflict_pending:holdout,..Delivery::default()}));
            let memory=Arc::new(Mutex::new(MemoryLookup {updated:holdout,..MemoryLookup::default()}));
            let metrics=Arc::new(Metrics::default());
            let provider=BoundedProvider {inner:crate::llm::create_provider_for_app(&config,Some(app.handle())).unwrap(),metrics:metrics.clone(),max_calls:if holdout {6} else {4},root:root.clone()};
            let mut task_config=agent_config(Some(&root));
            let registry=if id.starts_with("memory_") {
                task_config.preferences.use_long_term_memory=true;
                memory_registry(&memory)
            } else {delivery_registry(&world)};
            let runner=AgentRunner::with_registry(Box::new(provider),task_config,registry);
            let started=std::time::Instant::now();
            let mut row=serde_json::json!({"kind":"synthetic_state","id":id,"arm":"agent_runner","model":config.model,"status":"timed_out","world":null,"steps":[],"text":null,"diagnosticRef":null});
            if let Ok(outcome)=tokio::time::timeout(Duration::from_secs(if holdout {180} else {120}),runner.run_slice(goal,&[])).await {
                let (status,response)=match outcome {
                    AgentSliceOutcome::Completed(r)=>("completed",r),
                    AgentSliceOutcome::AwaitingConfirmation(r)=>("awaiting_confirmation",r),
                    AgentSliceOutcome::NeedsAttention(r)=>("needs_attention",r),
                    AgentSliceOutcome::Stopped(r)=>("stopped",r),
                };
                row["status"]=serde_json::json!(status);
                match response {
                    AgentResponse::TextWithToolCalls {text,steps}=>{row["text"]=serde_json::json!(text);row["steps"]=serde_json::json!(steps);},
                    AgentResponse::Text(text)=>row["text"]=serde_json::json!(text),
                    AgentResponse::Error(_)=>row["status"]=serde_json::json!("runner_error"),
                }
            }
            row["world"]=if id.starts_with("memory_") {serde_json::json!(&*memory.lock().unwrap())} else {serde_json::json!(&*world.lock().unwrap())};
            row_metrics(&mut row,&metrics,started);
            println!("AGENT_EVAL_ROW {row}");
            if row["status"]=="timed_out" {return;}
            if metrics.failure.lock().unwrap().as_ref().is_some_and(|f| f["status"]!="budget_exceeded") {return;}
        }
    });
}

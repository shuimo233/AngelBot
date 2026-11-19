//! Opt-in BFCL call selection and state tasks with disposable delivery and memory data.
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
    pacer: Option<Arc<RequestPacer>>,
}

impl Metrics {
    pub(super) fn request_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

pub(super) struct RequestPacer {
    origin: tokio::time::Instant,
    next: tokio::sync::Mutex<Option<tokio::time::Instant>>,
}

impl RequestPacer {
    pub(super) fn five_second_interval() -> Self {
        Self {
            origin: tokio::time::Instant::now(),
            next: tokio::sync::Mutex::new(None),
        }
    }

    async fn before_request(&self) -> u128 {
        let mut next = self.next.lock().await;
        if let Some(at) = *next {
            tokio::time::sleep_until(at).await;
        }
        let now = tokio::time::Instant::now();
        *next = Some(now + std::time::Duration::from_secs(5));
        (now - self.origin).as_millis()
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum CacheProbeMode {
    Comparison,
    Continuation,
    ExactRepeat,
    ImplicitBreakpoint,
}

impl CacheProbeMode {
    fn request_limit(self) -> usize {
        if self == Self::ExactRepeat {
            4
        } else {
            2
        }
    }

    fn request_shape(self) -> &'static str {
        match self {
            Self::Comparison => "independent",
            Self::Continuation => "native_continuation",
            Self::ExactRepeat => "exact_repeat",
            Self::ImplicitBreakpoint => "implicit_breakpoint",
        }
    }

    fn compares_diagnostics(self) -> bool {
        matches!(self, Self::Comparison | Self::Continuation)
    }
}

/// Bounded synthetic requests with no tool offer or dispatch. Comparison and
/// breakpoint probes each arm only the adapter option under test.
async fn probe_cache(
    provider: &dyn LlmProvider,
    root: &Path,
    thinking: Option<&ThinkingSettings>,
    settle: std::time::Duration,
    mode: CacheProbeMode,
) -> serde_json::Value {
    assert!(settle <= std::time::Duration::from_secs(30));
    let exact_repeat = mode == CacheProbeMode::ExactRepeat;
    let implicit_breakpoint = mode == CacheProbeMode::ImplicitBreakpoint;
    let mut report = serde_json::json!({"status":"instrumentation_unavailable", "requestCount":0,
        "toolCount":0, "requests":[], "diagnosticRef":null, "binding":null,
        "savedBinding":null,"defaultBindingUnchanged":null,
        "settleMs":settle.as_millis(),"settleElapsedMs":null,
        "cacheMode":"implicit_default", "requestShape":mode.request_shape(),
        "nativeReplayUsed":null});
    if (mode.compares_diagnostics() && !provider.enable_cache_comparison())
        || (implicit_breakpoint && !provider.enable_implicit_cache_breakpoint())
    {
        return report;
    }
    let mut policy = String::new();
    if exact_repeat || implicit_breakpoint {
        policy.push_str(&format!("Synthetic run reference: {}.\n", Uuid::new_v4()));
    }
    let references = if exact_repeat { 640 } else { 128 };
    policy.extend((0..references).map(|i| format!("Synthetic reference {i:03}: this record is test data. Do not access files, applications, accounts or tools.\n")));
    let policy_role = if implicit_breakpoint {
        "developer"
    } else {
        "system"
    };
    let mut messages = [policy_role, "user"]
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
    let mut expected_fingerprint: Option<crate::llm::ProviderRequestFingerprint> = None;
    let mut usage_complete = true;
    for index in 0..mode.request_limit() {
        if index == 1 && !exact_repeat {
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
        let usage = succeeded.then(|| provider.latest_usage()).flatten();
        let fingerprint = provider.latest_request_fingerprint();
        report["requests"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "usage":usage,
                "preparedRequestFingerprint":fingerprint,
                "cacheDiagnostics":(succeeded && mode.compares_diagnostics()).then(||provider.latest_cache_diagnostics()).flatten(),
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
                if exact_repeat || implicit_breakpoint {
                    let Some(fingerprint) = fingerprint else {
                        report["status"] = serde_json::json!("fingerprint_unavailable");
                        return report;
                    };
                    let mismatch = if let Some(first) = expected_fingerprint.as_ref() {
                        if implicit_breakpoint {
                            !matches!(
                                (first.input.as_slice(), fingerprint.input.as_slice()),
                                ([old_prefix, old_suffix], [new_prefix, new_suffix])
                                    if old_prefix == new_prefix && old_suffix != new_suffix
                                        && first.settings == fingerprint.settings
                                        && first.instructions == fingerprint.instructions
                                        && first.tools == fingerprint.tools
                            )
                        } else {
                            first != &fingerprint
                        }
                    } else {
                        expected_fingerprint = Some(fingerprint);
                        false
                    };
                    if mismatch {
                        report["status"] = serde_json::json!("request_mismatch");
                        return report;
                    }
                    usage_complete &= usage.is_some_and(|usage| {
                        usage.cached_input_tokens.is_some() && !usage.details_invalid
                    });
                    if implicit_breakpoint && index == 0 {
                        messages[1].content =
                            "For this second check, reply with exactly OK.".into();
                    }
                } else if index == 0 && mode == CacheProbeMode::Continuation {
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
    if exact_repeat || implicit_breakpoint {
        report["status"] = serde_json::json!(if usage_complete {
            "measured"
        } else {
            "metering_unavailable"
        });
        return report;
    }
    report["status"] = serde_json::json!(match provider.latest_cache_diagnostics() {
        Some(d) if d.details_invalid => "invalid_diagnostics",
        Some(d) if matches!(d.kind.as_str(), "cache_hit" | "cache_miss") => "diagnostics_reported",
        Some(_) => "diagnostics_unavailable",
        None => "not_reported",
    });
    report
}

pub(super) fn configured_cache_comparison(exact_repeat: bool) -> serde_json::Value {
    let settle_ms = std::env::var("ANGELBOT_CACHE_COMPARISON_SETTLE_MS")
        .unwrap_or_else(|_| "0".into())
        .parse::<u64>()
        .unwrap();
    assert!(settle_ms <= 30_000);
    let mode = if exact_repeat {
        CacheProbeMode::ExactRepeat
    } else if std::env::var("ANGELBOT_CACHE_COMPARISON_CONTINUATION").as_deref() == Ok("1") {
        CacheProbeMode::Continuation
    } else {
        CacheProbeMode::Comparison
    };
    let root = PathBuf::from(std::env::var_os("ANGELBOT_PRIVATE_CACHE_PROBE_ROOT").unwrap());
    configured_cache_probe(mode, settle_ms, &root)
}

fn configured_cache_probe(mode: CacheProbeMode, settle_ms: u64, root: &Path) -> serde_json::Value {
    let settle = std::time::Duration::from_millis(settle_ms);
    let unavailable = || {
        serde_json::json!({"status":"instrumentation_unavailable","requestCount":0,
        "toolCount":0,"requests":[],"diagnosticRef":null,"binding":null,
        "savedBinding":null,"defaultBindingUnchanged":null,
        "settleMs":settle_ms,"settleElapsedMs":null,
        "cacheMode":"implicit_default", "requestShape":mode.request_shape(),
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
    if mode.compares_diagnostics()
        && std::env::var("ANGELBOT_CACHE_COMPARISON_ALTERNATE").as_deref() == Ok("1")
    {
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
    if mode != CacheProbeMode::ImplicitBreakpoint {
        config.max_tokens = config.max_tokens.min(256);
    }
    let Ok(provider) = crate::llm::create_provider_for_app(&config, Some(app.handle())) else {
        return unavailable();
    };
    let Ok(thinking) = crate::llm::resolve_thinking_settings(&config, Some("low")) else {
        return unavailable();
    };
    let mut report = runtime.block_on(probe_cache(
        provider.as_ref(),
        root,
        thinking.as_ref(),
        settle,
        mode,
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

#[test]
#[ignore = "explicit saved-Luna implicit breakpoint capability probe, at most two requests"]
fn configured_implicit_cache_breakpoint() {
    assert_eq!(
        std::env::var("ANGELBOT_CONTEXT_EVAL_LIVE").as_deref(),
        Ok("1")
    );
    let root = PathBuf::from(std::env::var_os("ANGELBOT_PRIVATE_CONTEXT_EVAL_ROOT").unwrap());
    let report = configured_cache_probe(CacheProbeMode::ImplicitBreakpoint, 0, &root);
    println!("CACHE_IMPLICIT_BREAKPOINT_ROW {report}");
}

#[test]
#[ignore = "explicit saved-Luna cache chain, 12 requests or 24 with ANGELBOT_CACHE_APPEND_CHAIN_LONG=1"]
fn configured_cache_append_chain() {
    use crate::llm::{Message, ModelAuthMode, ProviderRequestFingerprint};
    use serde_json::json;
    use std::time::Duration;

    assert_eq!(
        std::env::var("ANGELBOT_CONTEXT_EVAL_LIVE").as_deref(),
        Ok("1")
    );
    let root = PathBuf::from(std::env::var_os("ANGELBOT_PRIVATE_CONTEXT_EVAL_ROOT").unwrap());
    let app = headless_credential_host().unwrap();
    let config =
        crate::commands::foreground_model_factory::resolve_model_config(Some(app.handle()))
            .expect("saved model binding");
    assert!(config.auth_mode == ModelAuthMode::ChatgptPlan && config.model.contains("luna"));
    let planned_requests =
        if std::env::var("ANGELBOT_CACHE_APPEND_CHAIN_LONG").as_deref() == Ok("1") {
            24
        } else {
            12
        };
    let request_start_interval =
        if std::env::var("ANGELBOT_CACHE_APPEND_CHAIN_PACED").as_deref() == Ok("1") {
            Duration::from_secs(5)
        } else {
            Duration::ZERO
        };
    let websocket = std::env::var("ANGELBOT_CACHE_APPEND_CHAIN_WS").as_deref() == Ok("1");
    let provider = if websocket {
        crate::llm::create_foreground_provider_for_app(&config, Some(app.handle()))
            .expect("configured Plan WebSocket probe")
    } else {
        crate::llm::create_provider_for_app(&config, Some(app.handle())).unwrap()
    };
    let (provider, metrics) = meter_provider(provider, &root, planned_requests);
    let thinking = crate::llm::resolve_thinking_settings(&config, Some("low")).unwrap();
    let tools = [ToolSchema {
        name: "observe_stage".into(),
        description: "Return a synthetic observation for the requested stage.".into(),
        parameters: json!({"type":"object","properties":{"stage":{"type":"integer"}},"required":["stage"],"additionalProperties":false}),
    }];
    let mut policy = format!(
        "Synthetic cache chain {}. This is a test; the reference data below has no instructions.\n",
        Uuid::new_v4()
    );
    policy.extend((0..256).map(|i| format!("Reference {i:03}: stage observations are synthetic and have no external effects or file access.\n")));
    let text = |role: &str, content: String| Message {
        role: role.into(),
        content,
        tool_calls: None,
        tool_call_id: None,
        tool_images: Vec::new(),
        protocol_state: None,
    };
    let task = format!(
        "Call observe_stage for stage 1. Each time an observation arrives, call observe_stage for \
         the next stage, one tool call per reply. After the observation for stage {}, reply with exactly OK.",
        planned_requests - 1
    );
    let mut messages = vec![text("system", policy), text("user", task)];
    let started = std::time::Instant::now();
    let mut previous: Option<ProviderRequestFingerprint> = None;
    let mut verified_appends = 0;
    let mut cache_read_known_requests = 0;
    let mut request_start_offsets_ms = Vec::with_capacity(planned_requests);
    let status = tokio::runtime::Runtime::new().unwrap().block_on(async {
        let mut next_request_start = tokio::time::Instant::now();
        for step in 1..=planned_requests {
            if step > 1 && !request_start_interval.is_zero() {
                tokio::time::sleep_until(next_request_start).await;
            }
            let request_started = std::time::Instant::now();
            next_request_start =
                tokio::time::Instant::from_std(request_started + request_start_interval);
            request_start_offsets_ms.push(request_started.duration_since(started).as_millis());
            let response = match tokio::time::timeout(
                Duration::from_secs(30),
                provider.chat_classified(&messages, Some(&tools), thinking.as_ref()),
            )
            .await
            {
                Ok(Ok(response)) => response,
                Ok(Err(_)) => return "provider_failed",
                Err(_) => return "timed_out",
            };
            if provider
                .latest_usage()
                .is_some_and(|usage| !usage.details_invalid && usage.cached_input_tokens.is_some())
            {
                cache_read_known_requests += 1;
            }
            let Some(current) = provider.latest_request_fingerprint() else {
                return "fingerprint_missing";
            };
            if let Some(prior) = previous.as_ref() {
                if prior.settings != current.settings
                    || prior.tools != current.tools
                    || prior.instructions != current.instructions
                    || current.input.len() <= prior.input.len()
                    || !current.input.starts_with(&prior.input)
                {
                    return "prefix_changed";
                }
                verified_appends += 1;
            }
            previous = Some(current);
            let (reply, protocol) = response.into_parts();
            if step == planned_requests {
                return if matches!(reply, LlmResponse::Text { text, .. } if text.trim() == "OK") {
                    "passed"
                } else {
                    "unexpected_reply"
                };
            }
            let (
                LlmResponse::ToolCalls {
                    calls,
                    text: content,
                    ..
                },
                Some(protocol),
            ) = (reply, protocol)
            else {
                return "unexpected_reply";
            };
            let [call] = calls.as_slice() else {
                return "unexpected_tool_call";
            };
            if call.name != "observe_stage" || call.arguments != json!({"stage":step}) {
                return "unexpected_tool_call";
            }
            let mut result = text(
                "tool",
                format!("Result: {}", json!({"stage":step,"status":"ok"})),
            );
            result.tool_call_id = Some(call.id.clone());
            let mut assistant = text("assistant", content.unwrap_or_default());
            assistant.tool_calls = Some(calls);
            assistant.protocol_state = Some(protocol);
            messages.extend([assistant, result]);
        }
        unreachable!("the final request returns a status")
    });
    let mut row = json!({
        "status": status,
        "requestedTransport": if websocket { "foreground" } else { "http" },
        "model": config.model,
        "plannedRequests": planned_requests,
        "verifiedAppends": verified_appends,
        "minimumRequestStartIntervalMs": request_start_interval.as_millis(),
        "requestStartOffsetsMs": request_start_offsets_ms,
        "cacheReadKnownRequests": cache_read_known_requests,
        "toolDispatchCount": 0,
    });
    row_metrics(&mut row, &metrics, started);
    row.as_object_mut().unwrap().remove("providerInputs");
    println!("CACHE_APPEND_CHAIN_ROW {row}");
    assert_eq!(status, "passed");
    assert_eq!(verified_appends, planned_requests - 1);
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
        mode: CacheProbeMode,
        enabled: bool,
        fail_at: Option<usize>,
        calls: AtomicUsize,
        first_request: Mutex<Option<String>>,
    }
    #[async_trait::async_trait]
    impl LlmProvider for Probe {
        fn name(&self) -> &str {
            "owned-cache-probe"
        }
        fn enable_cache_comparison(&self) -> bool {
            assert!(self.mode.compares_diagnostics());
            self.enabled
        }
        fn enable_implicit_cache_breakpoint(&self) -> bool {
            assert!(self.mode == CacheProbeMode::ImplicitBreakpoint);
            self.enabled
        }
        fn latest_request_fingerprint(&self) -> Option<crate::llm::ProviderRequestFingerprint> {
            let input = if self.mode == CacheProbeMode::ImplicitBreakpoint {
                let item = |role: &str, sha256: String| crate::llm::ProviderInputFingerprint {
                    kind: "message".into(),
                    role: Some(role.into()),
                    sha256,
                    bytes: 1,
                };
                vec![
                    item("developer", "fixed-prefix".into()),
                    item("user", self.calls.load(Ordering::SeqCst).to_string()),
                ]
            } else {
                Vec::new()
            };
            Some(crate::llm::ProviderRequestFingerprint {
                settings: "fixed-settings".into(),
                instructions: "fixed-instructions".into(),
                tools: "no-tools".into(),
                input,
            })
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
            assert!(self.mode.compares_diagnostics());
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
            assert!(index < self.mode.request_limit() && tools.is_none());
            assert_eq!(
                messages.len(),
                if index == 1 && self.mode == CacheProbeMode::Continuation {
                    4
                } else {
                    2
                }
            );
            let implicit_breakpoint = self.mode == CacheProbeMode::ImplicitBreakpoint;
            assert_eq!(
                messages[0].role,
                if implicit_breakpoint {
                    "developer"
                } else {
                    "system"
                }
            );
            if implicit_breakpoint {
                let mut first_request = self.first_request.lock().unwrap();
                assert_eq!(
                    first_request
                        .get_or_insert_with(|| messages[0].content.clone())
                        .as_str(),
                    messages[0].content.as_str()
                );
            } else if self.mode == CacheProbeMode::ExactRepeat {
                assert!(messages[0].content.starts_with("Synthetic run reference:"));
                let actual = serde_json::to_string(messages).unwrap();
                let mut first_request = self.first_request.lock().unwrap();
                assert_eq!(
                    first_request.get_or_insert_with(|| actual.clone()).as_str(),
                    actual
                );
            } else {
                assert!(messages[0].content.starts_with("Synthetic reference 000:"));
            }
            assert_eq!(
                messages[1].content,
                if implicit_breakpoint && index == 1 {
                    "For this second check, reply with exactly OK."
                } else {
                    "Reply with exactly OK."
                }
            );
            if messages.len() == 4 {
                assert_eq!(messages[2].role, "assistant");
                assert_eq!(messages[2].content, "OK");
                assert_eq!(messages[3].role, "user");
                assert_eq!(messages[3].content, "Reply with exactly OK again.");
            }
            if self.fail_at == Some(index) {
                return Err(LlmFailure {
                    kind: if implicit_breakpoint {
                        LlmFailureKind::Unclassified
                    } else {
                        LlmFailureKind::QuotaExceeded
                    },
                    diagnostic: "PRIVATE-CACHE-PROBE-FAILURE".into(),
                });
            }
            Ok(LlmResponse::Text {
                text: "OK".into(),
                stop_reason: crate::llm::StopReason::EndTurn,
            })
        }
    }
    for mode in [
        CacheProbeMode::Comparison,
        CacheProbeMode::Continuation,
        CacheProbeMode::ExactRepeat,
        CacheProbeMode::ImplicitBreakpoint,
    ] {
        let cases = if mode == CacheProbeMode::ExactRepeat {
            &[
                (false, Some(0), 1, "quota_exceeded"),
                (false, Some(2), 3, "quota_exceeded"),
                (false, None, 4, "measured"),
            ][..]
        } else if mode == CacheProbeMode::ImplicitBreakpoint {
            &[
                (false, None, 0, "instrumentation_unavailable"),
                (true, Some(0), 1, "unavailable"),
                (true, Some(1), 2, "unavailable"),
                (true, None, 2, "measured"),
            ][..]
        } else {
            &[
                (false, None, 0, "instrumentation_unavailable"),
                (true, Some(0), 1, "quota_exceeded"),
                (true, Some(1), 2, "quota_exceeded"),
                (true, None, 2, "diagnostics_reported"),
            ][..]
        };
        for &(enabled, fail_at, expected, status) in cases {
            let root = tempfile::tempdir().unwrap();
            let provider = Probe {
                mode,
                enabled,
                fail_at,
                calls: AtomicUsize::new(0),
                first_request: Mutex::new(None),
            };
            let report = probe_cache(
                &provider,
                root.path(),
                None,
                std::time::Duration::ZERO,
                mode,
            )
            .await;
            assert_eq!(provider.calls.load(Ordering::SeqCst), expected);
            assert_eq!(report["requestCount"], expected);
            assert_eq!(report["status"], status);
            assert_eq!(report["toolCount"], 0);
            assert_eq!(report["requestShape"], mode.request_shape());
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
        let start_offset_ms = match &self.metrics.pacer {
            Some(pacer) => Some(pacer.before_request().await),
            None => None,
        };
        let request_started = std::time::Instant::now();
        let result = self.inner.chat_classified(messages, tools, thinking).await;
        let reported_usage = result.as_ref().ok().and_then(|_| self.inner.latest_usage());
        let mut request = serde_json::json!({
            "requestIndex":request_index, "latencyMs":request_started.elapsed().as_millis(),
            "preparedRequestFingerprint":self.inner.latest_request_fingerprint(),
            "responseMetadata":result.as_ref().ok().and_then(|_|response_observation(self.inner.as_ref(), &self.root)),
            "usage":reported_usage, "usageStatus":if result.is_err(){"failed"}else if reported_usage.is_some(){"reported"}else{"missing"},
            "firstTokenMs":null, "actualCostUsd":null,
        });
        if let Some(offset) = start_offset_ms {
            request["requestStartOffsetMs"] = serde_json::json!(offset);
        }
        self.metrics.requests.lock().unwrap().push(request);
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
    meter_provider_with_pacer(inner, root, max_calls, None)
}

pub(super) fn meter_provider_with_pacer(
    inner: Box<dyn LlmProvider>,
    root: &Path,
    max_calls: usize,
    pacer: Option<Arc<RequestPacer>>,
) -> (Box<dyn LlmProvider>, Arc<Metrics>) {
    let metrics = Arc::new(Metrics {
        pacer,
        ..Metrics::default()
    });
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
            let Some(day @ ("Wednesday" | "Thursday")) = args["day"].as_str() else {
                return ToolResult::error(self.name, "No update: use an available day.");
            };
            if args["revision"].as_i64() != Some(world.revision) {
                return ToolResult::precondition_rejected(
                    self.name,
                    "revision",
                    "No update: reread the current revision before retrying the same day.",
                );
            }
            world.day = day.to_owned();
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

const WORKSPACE_PAIR_INPUTS: [(&str, &str); 2] = [
    ("east.json", "{\"total\":247}\n"),
    ("west.json", "{\"total\":389}\n"),
];
const DAILY_ARTIFACT: &str = "invoice-summary.json";

fn workspace_pair_registry(root: &Path) -> ToolRegistry {
    std::fs::create_dir(root).unwrap();
    for (name, contents) in WORKSPACE_PAIR_INPUTS {
        std::fs::write(root.join(name), contents).unwrap();
    }
    let mut project_tools = ToolRegistry::empty();
    crate::agent::handlers::register_workspace_file_handlers(&mut project_tools);
    let mut registry = ToolRegistry::empty();
    registry.register(
        project_tools.get("read_file").unwrap(),
        Arc::new(crate::agent::handlers::ReadFileHandler::new()),
    );
    registry
}

fn workspace_pair_observations(root: &Path, row: &serde_json::Value) -> serde_json::Value {
    let mut world = workspace_input_observations(
        root,
        row["steps"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default(),
    );
    world["firstToolBatchSize"] = row["providerOutputs"][0]["count"].clone();
    world
}

fn workspace_input_observations(root: &Path, steps: &[serde_json::Value]) -> serde_json::Value {
    let read_files: std::collections::BTreeSet<_> = steps
        .iter()
        .filter(|step| step["success"] == true && step["tool_name"] == "read_file")
        .filter_map(|step| {
            let receipt: serde_json::Value = serde_json::from_str(step["output"].as_str()?).ok()?;
            let name = Path::new(receipt["resolvedPath"].as_str()?)
                .file_name()?
                .to_str()?;
            WORKSPACE_PAIR_INPUTS
                .iter()
                .find(|(input, contents)| {
                    *input == name && receipt["content"].as_str() == Some(contents)
                })
                .map(|(input, _)| *input)
        })
        .collect();
    let inputs_preserved = WORKSPACE_PAIR_INPUTS.iter().all(|(name, contents)| {
        std::fs::read(root.join(name)).is_ok_and(|actual| actual == contents.as_bytes())
    });
    serde_json::json!({
        "readFiles": read_files,
        "inputsPreserved": inputs_preserved,
    })
}

#[derive(Default)]
struct DailyWriteOnce(AtomicUsize);

impl ToolHandler for DailyWriteOnce {
    fn name(&self) -> &str {
        "write_file"
    }

    fn execute(&self, args: &serde_json::Value, root: &Path) -> ToolResult {
        if args["path"] != DAILY_ARTIFACT
            || !args["content"]
                .as_str()
                .is_some_and(|text| text.len() <= 4096)
        {
            return ToolResult::error(
                self.name(),
                "Only a small invoice-summary.json may be written",
            );
        }
        if self.0.swap(1, Ordering::SeqCst) != 0 {
            return ToolResult::error(
                self.name(),
                "The one write was already dispatched; do not retry",
            );
        }
        crate::agent::handlers::WriteFileHandler.execute(args, root)
    }
}

fn daily_registry(root: &Path) -> (ToolRegistry, Arc<Mutex<MemoryLookup>>, Arc<DailyWriteOnce>) {
    let mut registry = workspace_pair_registry(root);
    let mut file_tools = ToolRegistry::empty();
    crate::agent::handlers::register_workspace_file_handlers(&mut file_tools);
    let write = Arc::new(DailyWriteOnce::default());
    registry.register(file_tools.get("write_file").unwrap(), write.clone());

    let conn = memory_fixture();
    conn.execute_batch(
        "INSERT INTO memories (id,scope,category,content,importance,source,forget_stage,created_at,updated_at)
         VALUES ('m-heading','global','knowledge','当前月报标题是青灯月报。',5,'user','active',2,2),
                ('m-cat','global','knowledge','我的猫叫梨子。',5,'user','active',2,2);
         INSERT INTO memories (id,scope,category,content,importance,source,forget_stage,created_at,updated_at,superseded_by)
         VALUES ('m-old','global','knowledge','之前的月报标题是暮云月报。',5,'user','active',1,1,'m-heading');",
    ).unwrap();
    let memory = register_observed_recall(&mut registry, conn, "m-heading");
    (registry, memory, write)
}

fn daily_observations(
    root: &Path,
    row: &serde_json::Value,
    memory: &MemoryLookup,
    write: &DailyWriteOnce,
) -> serde_json::Value {
    let steps = row["steps"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let first_write = steps
        .iter()
        .position(|step| step["tool_name"] == "write_file");
    let before_write = first_write.map_or(steps, |index| &steps[..index]);
    let mut world = workspace_input_observations(root, before_write);
    let artifact_path = root.join(DAILY_ARTIFACT);
    let artifact = std::fs::read_to_string(&artifact_path).ok();
    let read_back = first_write.is_some_and(|index| {
        steps[index]["success"] == true
            && steps[index]["arguments"]["path"] == DAILY_ARTIFACT
            && steps[index + 1..].iter().any(|step| {
                if step["tool_name"] != "read_file" || step["success"] != true {
                    return false;
                }
                let Some(receipt) = step["output"]
                    .as_str()
                    .and_then(|output| serde_json::from_str::<serde_json::Value>(output).ok())
                else {
                    return false;
                };
                let path_matches = receipt["resolvedPath"].as_str().is_some_and(|path| {
                    std::fs::canonicalize(path)
                        .ok()
                        .zip(std::fs::canonicalize(&artifact_path).ok())
                        .is_some_and(|(actual, expected)| actual == expected)
                });
                path_matches
                    && artifact
                        .as_deref()
                        .is_some_and(|text| receipt["content"] == text)
            })
    });
    world["target_seen"] = serde_json::json!(memory.target_seen);
    world["superseded_seen"] = serde_json::json!(memory.superseded_seen);
    world["irrelevant_seen"] = serde_json::json!(memory.irrelevant_seen);
    world["recallBeforeWrite"] = serde_json::json!(before_write.iter().any(|step| {
        step["tool_name"] == "recall_memories"
            && step["success"] == true
            && step["output"].as_str().is_some_and(|output| {
                output.contains("[memory:m-heading]") && output.contains("青灯月报")
            })
    }));
    world["writes"] = serde_json::json!(steps
        .iter()
        .filter(|step| step["tool_name"] == "write_file")
        .count());
    world["writeDispatches"] = serde_json::json!(write.0.load(Ordering::SeqCst));
    world["artifactReadBack"] = serde_json::json!(read_back);
    world["artifact"] = artifact
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    world
}

struct DeliveryConflictReplay(AtomicUsize);

struct DailyReplay(AtomicUsize);

#[async_trait::async_trait]
impl LlmProvider for DailyReplay {
    fn name(&self) -> &str {
        "daily-replay"
    }

    async fn chat(
        &self,
        _: &[crate::llm::Message],
        _: Option<&[ToolSchema]>,
        _: Option<&ThinkingSettings>,
    ) -> Result<LlmResponse, String> {
        use serde_json::json;
        let payload = json!({"east":247,"west":389,"total":636,"heading":"青灯月报"}).to_string();
        let index = self.0.fetch_add(1, Ordering::SeqCst);
        let batch = match index {
            0 => vec![
                ("read_file", json!({"path":"east.json"})),
                ("read_file", json!({"path":"west.json"})),
            ],
            1 => vec![("recall_memories", json!({"query":"当前月报标题"}))],
            2 => vec![(
                "write_file",
                json!({"path":DAILY_ARTIFACT,"content":payload}),
            )],
            3 => vec![("read_file", json!({"path":DAILY_ARTIFACT}))],
            4 => {
                return Ok(LlmResponse::Text {
                    text: payload,
                    stop_reason: crate::llm::StopReason::EndTurn,
                })
            }
            _ => panic!("daily fixture must complete without another model reply"),
        };
        Ok(LlmResponse::ToolCalls {
            calls: batch
                .into_iter()
                .enumerate()
                .map(|(slot, (name, arguments))| crate::llm::ToolCall {
                    id: format!("daily-{index}-{slot}"),
                    name: name.into(),
                    arguments,
                })
                .collect(),
            text: None,
            stop_reason: crate::llm::StopReason::ToolUse,
        })
    }
}

#[tokio::test]
async fn agent_benchmark_daily_uses_real_tools_and_records_only_the_owned_write() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("workspace");
    let (registry, memory, write) = daily_registry(&root);
    assert_eq!(registry.get_schemas().len(), 3);
    assert!(
        !write
            .execute(
                &serde_json::json!({"path":"east.json","content":"blocked"}),
                &root
            )
            .success
    );
    let mut config = agent_config(Some(&root));
    config.preferences.use_long_term_memory = true;
    let runner =
        AgentRunner::with_registry(Box::new(DailyReplay(AtomicUsize::new(0))), config, registry)
            .with_execution_permission("workspace_auto".into());
    let outcome = runner
        .run_slice(
            "Verify and deliver the monthly totals as a JSON artifact.",
            &[],
        )
        .await;
    let AgentSliceOutcome::Completed(AgentResponse::TextWithToolCalls { steps, .. }) = outcome
    else {
        panic!("expected a completed daily delivery, got {outcome:?}");
    };
    let world = daily_observations(
        &root,
        &serde_json::json!({"steps":steps}),
        &memory.lock().unwrap(),
        &write,
    );
    assert_eq!(
        world["readFiles"],
        serde_json::json!(["east.json", "west.json"])
    );
    for key in [
        "inputsPreserved",
        "target_seen",
        "recallBeforeWrite",
        "artifactReadBack",
    ] {
        assert_eq!(world[key], true, "{world}");
    }
    for key in ["irrelevant_seen", "superseded_seen"] {
        assert_eq!(world[key], false, "{world}");
    }
    assert_eq!(world["writes"], 1);
    assert_eq!(world["writeDispatches"], 1);
    assert!(
        !write
            .execute(
                &serde_json::json!({"path":DAILY_ARTIFACT,"content":"blocked"}),
                &root
            )
            .success
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(root.join(DAILY_ARTIFACT)).unwrap()
        )
        .unwrap(),
        world["artifact"]
    );
}

#[async_trait::async_trait]
impl LlmProvider for DeliveryConflictReplay {
    fn name(&self) -> &str {
        "delivery-conflict-replay"
    }

    async fn chat(
        &self,
        _: &[crate::llm::Message],
        _: Option<&[ToolSchema]>,
        _: Option<&ThinkingSettings>,
    ) -> Result<LlmResponse, String> {
        let index = self.0.fetch_add(1, Ordering::SeqCst);
        let (name, arguments) = match index {
            0 | 2 | 4 => ("read_delivery", serde_json::json!({})),
            1 => (
                "set_delivery_day",
                serde_json::json!({"day":"Thursday","revision":17}),
            ),
            3 => (
                "set_delivery_day",
                serde_json::json!({"day":"Thursday","revision":18}),
            ),
            5 => {
                return Ok(LlmResponse::Text {
                    text: "Thursday".into(),
                    stop_reason: crate::llm::StopReason::EndTurn,
                })
            }
            _ => panic!("recovered conflict must not require another model reply"),
        };
        Ok(LlmResponse::ToolCalls {
            calls: vec![crate::llm::ToolCall {
                id: format!("delivery-{index}"),
                name: name.into(),
                arguments,
            }],
            text: None,
            stop_reason: crate::llm::StopReason::ToolUse,
        })
    }
}

#[tokio::test]
async fn agent_benchmark_conflict_recovery_keeps_the_final_reply_and_failed_receipt() {
    let world = Arc::new(Mutex::new(Delivery {
        conflict_pending: true,
        ..Delivery::default()
    }));
    let runner = AgentRunner::with_registry(
        Box::new(DeliveryConflictReplay(AtomicUsize::new(0))),
        agent_config(None),
        delivery_registry(&world),
    );
    let outcome = runner
        .run_slice("Move the delivery to Thursday and confirm it.", &[])
        .await;
    let AgentSliceOutcome::Completed(AgentResponse::TextWithToolCalls { text, steps }) = outcome
    else {
        panic!("expected completed delivery after conflict recovery, got {outcome:?}");
    };
    assert_eq!(text, "Thursday");
    assert_eq!(steps.len(), 5);
    assert!(!steps[1].success);
    assert!(steps[3].success && steps[4].success);
    let world = world.lock().unwrap();
    assert_eq!(world.day, "Thursday");
    assert_eq!(
        (world.revision, world.writes, world.reads_after),
        (19, 1, 1)
    );
}

#[derive(Default, Serialize)]
struct MemoryLookup {
    queries: Vec<serde_json::Value>,
    target_seen: bool,
    irrelevant_seen: bool,
    superseded_seen: bool,
}

struct ObservedRecall {
    handler: crate::agent::extensions::memory::RecallMemoriesHandler,
    world: Arc<Mutex<MemoryLookup>>,
    target: &'static str,
}

impl ToolHandler for ObservedRecall {
    fn name(&self) -> &str {
        self.handler.name()
    }

    fn execute(&self, args: &serde_json::Value, dir: &Path) -> ToolResult {
        let result = self.handler.execute(args, dir);
        let mut world = self.world.lock().unwrap();
        world.queries.push(args.clone());
        if result.success {
            world.target_seen |= result
                .content
                .contains(&format!("[memory:{}]", self.target));
            world.irrelevant_seen |= result.content.contains("[memory:m-cat]");
            world.superseded_seen |= result.content.contains("[memory:m-old]");
        }
        result
    }
}

fn memory_fixture() -> Connection {
    // Use the same schema and retrieval as Main, with disposable synthetic data.
    unsafe { crate::vector_store::load_extension_globally() };
    let conn = Connection::open_in_memory().unwrap();
    crate::db::migrate(&conn).unwrap();
    conn
}

fn memory_registry(updated: bool) -> (ToolRegistry, Arc<Mutex<MemoryLookup>>) {
    let conn = memory_fixture();
    let insert = |id: &str, content: &str, date: &str, superseded: Option<&str>| {
        conn.execute(
            "INSERT INTO memories (id,scope,category,content,importance,source,forget_stage,created_at,updated_at,superseded_by)
             VALUES (?1,'global','preference',?2,5,'user','active',unixepoch(?3),unixepoch(?3),?4)",
            params![id, content, date, superseded],
        )
        .unwrap();
    };
    let old = "我平时最常喝的茶是白毫银针。";
    if updated {
        insert(
            "m-tea",
            "我已经不喝白毫银针了，现在最常喝祁门红茶。",
            "2026-10-22",
            None,
        );
        insert("m-old", old, "2026-10-01", Some("m-tea"));
    } else {
        insert("m-tea", old, "2026-10-01", None);
    }
    insert("m-cat", "我的猫叫梨子。", "2026-10-21", None);

    let mut registry = ToolRegistry::empty();
    let world = register_observed_recall(&mut registry, conn, "m-tea");
    (registry, world)
}

fn register_observed_recall(
    registry: &mut ToolRegistry,
    conn: Connection,
    target: &'static str,
) -> Arc<Mutex<MemoryLookup>> {
    let db = Arc::new(Mutex::new(conn));
    crate::agent::extensions::memory::register(registry, db.clone());
    for name in ["remember_fact", "update_memory", "forget_memory"] {
        registry.unregister(name);
    }
    let recall = registry.get("recall_memories").unwrap();
    let world = Arc::new(Mutex::new(MemoryLookup::default()));
    registry.register(
        recall,
        Arc::new(ObservedRecall {
            handler: crate::agent::extensions::memory::RecallMemoriesHandler { db: Some(db) },
            world: world.clone(),
            target,
        }),
    );
    world
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
}

#[test]
fn agent_benchmark_uses_production_recall_and_keeps_superseded_records_hidden() {
    for (updated, answer) in [(false, "白毫银针"), (true, "祁门红茶")] {
        let (registry, world) = memory_registry(updated);
        assert_eq!(registry.get_schemas().len(), 1);
        let recall = |args| {
            registry
                .execute(
                    &crate::agent::tool::ToolCall::new("recall_memories".into(), args),
                    "recall",
                    Path::new("."),
                )
                .result
        };
        let miss = recall(serde_json::json!({"query":"饮茶"}));
        assert!(
            miss.success && !miss.content.contains("[memory:"),
            "{}",
            miss.content
        );
        let hit = recall(serde_json::json!({"query":"饮茶","alternatives":["常喝"]}));
        assert!(
            hit.success && hit.content.contains(answer),
            "{}",
            hit.content
        );
        let world = world.lock().unwrap();
        assert!(world.target_seen);
        assert!(!world.irrelevant_seen);
        assert!(!world.superseded_seen);
    }
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
#[ignore = "explicit scripts/agent_eval.py --live; saved Luna and synthetic data only"]
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
    assert!(matches!(
        suite.as_str(),
        "smoke" | "holdout" | "workspace" | "daily"
    ));
    let holdout = suite == "holdout";
    let workspace = suite == "workspace";
    let daily = suite == "daily";
    let selected_arm = std::env::var("ANGELBOT_AGENT_EVAL_ARM").ok();
    assert!(selected_arm
        .as_deref()
        .is_none_or(|arm| matches!(arm, "minimal_fc" | "angelbot_prompt")));
    assert_eq!(
        cases.len(),
        if workspace || daily {
            0
        } else if holdout {
            8
        } else {
            6
        }
    );
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
        let tasks: &[(&str, &str)]=if daily {&[
            ("daily_memory_delivery","请先用 read_file 读取 east.json、west.json，用 recall_memories 按当前月报标题查询，仅使用相关的当前记录。按两份原件各自的 total 计算合计，保留原件。然后用 write_file 只写一次 invoice-summary.json，内容是含 east、west、total、heading 四个键的 JSON 对象；三个金额为整数，heading 为当前月报标题。写入成功后用 read_file 读回该文件核对。最后仅回复同一 JSON 对象，不加 Markdown 或解释。"),
        ]} else if workspace {&[
            ("workspace_pair","Read east.json and west.json in the project. Report each file's total and their sum. Reply with only a JSON object using the keys east, west and total."),
        ]} else if holdout {&[
            ("delivery_conflict","Read the current delivery details, change its day to Thursday using the current revision, then read it again to confirm. If a revision is stale, reread and use the new revision; do not repeat an update that succeeded. Work only with the delivery tools provided. In your final reply, give only the confirmed weekday in English."),
            ("memory_updated","请用记忆查询工具，只查与饮茶有关的资料，告诉我现在最常喝哪种茶。按记录日期和替代关系区分以前与现在，不读取无关资料。最后只回答当前茶名。"),
        ]} else {&[
            ("delivery_change","Read the current delivery details, change its day to Thursday using the current revision, then read it again to confirm. Work only with the delivery tools provided. In your final reply, give only the confirmed weekday in English."),
            ("memory_keyword","请用记忆查询工具，只查与饮茶有关的资料，告诉我平时最常喝哪种茶。不要根据常识猜测，不读取无关资料。最后只回答茶名。"),
        ]};
        for &(id,goal) in tasks {
            let world=Arc::new(Mutex::new(Delivery {conflict_pending:holdout,..Delivery::default()}));
            let metrics=Arc::new(Metrics::default());
            let provider=BoundedProvider {inner:crate::llm::create_provider_for_app(&config,Some(app.handle())).unwrap(),metrics:metrics.clone(),max_calls:if holdout || daily {6} else {4},root:root.clone()};
            let mut task_config=agent_config(Some(&root));
            let workspace_root=root.join("workspace-pair");
            let (registry,memory,write)=if daily {
                task_config.work_dir=Some(workspace_root.clone());
                task_config.preferences.use_long_term_memory=true;
                let (registry,memory,write)=daily_registry(&workspace_root);
                (registry,Some(memory),Some(write))
            } else if id=="workspace_pair" {
                task_config.work_dir=Some(workspace_root.clone());
                (workspace_pair_registry(&workspace_root),None,None)
            } else if id.starts_with("memory_") {
                task_config.preferences.use_long_term_memory=true;
                let (registry,memory)=memory_registry(holdout);
                (registry,Some(memory),None)
            } else {(delivery_registry(&world),None,None)};
            let mut runner=AgentRunner::with_registry(Box::new(provider),task_config,registry);
            if daily { runner=runner.with_execution_permission("workspace_auto".into()); }
            let started=std::time::Instant::now();
            let mut row=serde_json::json!({"kind":"synthetic_state","id":id,"arm":"agent_runner","model":config.model,"status":"timed_out","world":null,"steps":[],"text":null,"diagnosticRef":null});
            if let Ok(outcome)=tokio::time::timeout(Duration::from_secs(if holdout || daily {180} else {120}),runner.run_slice(goal,&[])).await {
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
            row_metrics(&mut row,&metrics,started);
            row["world"]=if let Some(write)=write {
                daily_observations(&workspace_root,&row,&memory.unwrap().lock().unwrap(),&write)
            } else if id=="workspace_pair" {workspace_pair_observations(&workspace_root,&row)} else if let Some(memory)=memory {serde_json::json!(&*memory.lock().unwrap())} else {serde_json::json!(&*world.lock().unwrap())};
            println!("AGENT_EVAL_ROW {row}");
            if row["status"]=="timed_out" {return;}
            if metrics.failure.lock().unwrap().as_ref().is_some_and(|f| f["status"]!="budget_exceeded") {return;}
        }
    });
}

//! OpenTelemetry integration for agent observability.
//!
//! Issue #063: Exports agent events as OTEL spans.
//!
//! Architecture:
//! - `create_tracer(config)` — creates a tracer with Console or OTLP HTTP exporter
//! - `AgentOtelBridge` — wraps an AgentEventEmitter, emits spans for each event
//! - Dev mode: spans output to stdout in readable format
//! - Prod mode: spans exported via OTLP HTTP to configured endpoint
//!
//! Dependencies (to be added to Cargo.toml):
//!   opentelemetry = "0.24"
//!   opentelemetry_sdk = { version = "0.24", features = ["rt-tokio"] }
//!   opentelemetry-otlp = { version = "0.17", features = ["http-tonic"] }
//!   tracing = "0.1"
//!   tracing-opentelemetry = "0.25"

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// OTEL exporter type.
#[derive(Debug, Clone)]
pub enum OtelExporter {
    /// Dev mode: print spans to stdout.
    Stdout,
    /// Production mode: export via OTLP HTTP.
    OtlpHttp { endpoint: String },
    /// Disabled.
    Disabled,
}

impl Default for OtelExporter {
    fn default() -> Self {
        Self::Stdout
    }
}

/// OTEL configuration.
#[derive(Debug, Clone)]
pub struct OtelConfig {
    pub exporter: OtelExporter,
    pub service_name: String,
}

impl Default for OtelConfig {
    fn default() -> Self {
        Self {
            exporter: OtelExporter::Stdout,
            service_name: "angelbot-agent".to_string(),
        }
    }
}

/// OTEL span data captured during agent execution.
#[derive(Debug, Clone)]
pub struct SpanData {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub name: String,
    pub start_time: i64,
    pub end_time: Option<i64>,
    pub attributes: HashMap<String, String>,
    pub events: Vec<SpanEvent>,
}

impl SpanData {
    pub fn new(name: String, trace_id: String, span_id: String) -> Self {
        Self {
            trace_id,
            span_id,
            parent_span_id: None,
            name,
            start_time: chrono::Utc::now().timestamp_millis(),
            end_time: None,
            attributes: HashMap::new(),
            events: Vec::new(),
        }
    }

    pub fn with_parent(mut self, parent_span_id: String) -> Self {
        self.parent_span_id = Some(parent_span_id);
        self
    }

    pub fn set_attribute(&mut self, key: String, value: String) {
        self.attributes.insert(key, value);
    }

    pub fn add_event(&mut self, event: SpanEvent) {
        self.events.push(event);
    }

    pub fn end(&mut self) {
        self.end_time = Some(chrono::Utc::now().timestamp_millis());
    }

    pub fn duration_ms(&self) -> i64 {
        self.end_time.unwrap_or(self.start_time) - self.start_time
    }
}

/// An event within a span.
#[derive(Debug, Clone)]
pub struct SpanEvent {
    pub name: String,
    pub timestamp: i64,
    pub attributes: HashMap<String, String>,
}

impl SpanEvent {
    pub fn new(name: String) -> Self {
        Self {
            name,
            timestamp: chrono::Utc::now().timestamp_millis(),
            attributes: HashMap::new(),
        }
    }
}

/// In-memory span collector (for dev/console mode).
#[derive(Default)]
pub struct SpanCollector {
    spans: Mutex<Vec<SpanData>>,
}

impl SpanCollector {
    pub fn new() -> Self {
        Self {
            spans: Mutex::new(Vec::new()),
        }
    }

    pub fn add_span(&self, span: SpanData) {
        if let Ok(mut spans) = self.spans.lock() {
            spans.push(span);
        }
    }

    pub fn get_spans(&self) -> Vec<SpanData> {
        self.spans.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn clear(&self) {
        if let Ok(mut spans) = self.spans.lock() {
            spans.clear();
        }
    }

    pub fn print_trace(&self) {
        let spans = self.get_spans();
        for span in &spans {
            let duration = span.duration_ms();
            let attrs_str = span
                .attributes
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!(
                "[OTEL] trace={} span={} name={} duration={}ms {}",
                &span.trace_id[..8],
                &span.span_id[..8],
                span.name,
                duration,
                if attrs_str.is_empty() {
                    String::new()
                } else {
                    format!("({})", attrs_str)
                }
            );
        }
    }
}

/// Thread-safe wrapper.
#[derive(Clone, Default)]
pub struct SharedSpanCollector(Arc<SpanCollector>);

impl SharedSpanCollector {
    pub fn new() -> Self {
        Self(Arc::new(SpanCollector::new()))
    }

    pub fn add_span(&self, span: SpanData) {
        self.0.add_span(span)
    }

    pub fn get_spans(&self) -> Vec<SpanData> {
        self.0.get_spans()
    }

    pub fn print_trace(&self) {
        self.0.print_trace()
    }
}

/// OTEL tracer stub — a no-op when opentelemetry deps are not available.
///
/// When the full opentelemetry stack is wired up, this is replaced by the real tracer.
pub struct OtelTracer {
    config: OtelConfig,
    collector: SharedSpanCollector,
}

impl Default for OtelTracer {
    fn default() -> Self {
        Self::new(OtelConfig::default())
    }
}

impl OtelTracer {
    pub fn new(config: OtelConfig) -> Self {
        Self {
            config,
            collector: SharedSpanCollector::new(),
        }
    }

    /// Start a new span.
    pub fn start_span(&self, name: String, parent: Option<&str>) -> ActiveSpan {
        let trace_id = format!("{:032x}", rand_u64());
        let span_id = format!("{:016x}", rand_u64());

        let mut span = SpanData::new(name, trace_id.clone(), span_id.clone());
        if let Some(parent_id) = parent {
            span.set_attribute("parent_span_id".to_string(), parent_id.to_string());
        }

        ActiveSpan {
            span,
            collector: self.collector.clone(),
        }
    }

    /// Emit a root span for an agent turn.
    pub fn emit_agent_turn_span(&self, session_id: &str, turn_id: &str, tool_calls: usize) {
        let span = self.start_span(format!("agent.turn.{}", session_id), None);
        let mut active = span;
        active
            .span
            .set_attribute("session_id".to_string(), session_id.to_string());
        active
            .span
            .set_attribute("turn_id".to_string(), turn_id.to_string());
        active
            .span
            .set_attribute("tool_calls".to_string(), tool_calls.to_string());
        active.end();
    }

    /// Emit a tool call span.
    pub fn emit_tool_span(
        &self,
        tool_name: &str,
        call_id: &str,
        parent_trace: Option<(&str, &str)>,
    ) -> ActiveSpan {
        let trace_id = parent_trace
            .map(|_| format!("{:032x}", rand_u64()))
            .unwrap_or_else(|| format!("{:032x}", rand_u64()));
        let span_id = format!("{:016x}", rand_u64());

        let mut span = SpanData::new(format!("tool.{}", tool_name), trace_id, span_id);
        span.set_attribute("tool.name".to_string(), tool_name.to_string());
        span.set_attribute("tool.call_id".to_string(), call_id.to_string());
        if let Some((trace, parent)) = parent_trace {
            span.set_attribute("trace_id".to_string(), trace.to_string());
            span.set_attribute("parent_span_id".to_string(), parent.to_string());
        }

        ActiveSpan {
            span,
            collector: self.collector.clone(),
        }
    }

    /// Print collected trace to stdout.
    pub fn print_trace(&self) {
        if matches!(self.config.exporter, OtelExporter::Stdout) {
            self.collector.print_trace();
        }
    }

    /// Get collector for testing.
    pub fn collector(&self) -> &SharedSpanCollector {
        &self.collector
    }
}

/// An active span — drop to end.
pub struct ActiveSpan {
    span: SpanData,
    collector: SharedSpanCollector,
}

impl ActiveSpan {
    pub fn set_attribute(&mut self, key: String, value: String) {
        self.span.set_attribute(key, value);
    }

    pub fn add_event(&mut self, name: String) {
        self.span.add_event(SpanEvent::new(name));
    }

    pub fn end(mut self) {
        self.span.end();
        self.collector.add_span(self.span);
    }
}

/// Generate a random u64 as hex string.
fn rand_u64() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Mix with a counter for some uniqueness
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let c = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    ((nanos as u64) ^ c).wrapping_mul(0x517cc1b727220a95)
}

/// Bridge between AgentEvent and OTEL spans.
///
/// Wraps an existing AgentEventEmitter and adds span emission.
pub struct AgentOtelBridge {
    tracer: OtelTracer,
    inner: std::sync::Arc<dyn crate::agent::event::AgentEventEmitter>,
}

impl AgentOtelBridge {
    pub fn new(
        tracer: OtelTracer,
        inner: std::sync::Arc<dyn crate::agent::event::AgentEventEmitter>,
    ) -> Self {
        Self { tracer, inner }
    }
}

impl crate::agent::event::AgentEventEmitter for AgentOtelBridge {
    fn emit(&self, session_id: &str, event: crate::agent::event::AgentEvent) {
        // Emit the original event first
        self.inner.emit(session_id, event.clone());

        // Then emit OTEL spans
        match &event {
            crate::agent::event::AgentEvent::AgentStart { session_id, .. } => {
                let mut span = self.tracer.start_span("agent.start".to_string(), None);
                span.set_attribute("session_id".to_string(), session_id.clone());
                span.end();
            }
            crate::agent::event::AgentEvent::TurnStart {
                turn_id,
                session_id,
                message,
            } => {
                let mut span = self.tracer.start_span("agent.turn".to_string(), None);
                span.set_attribute("session_id".to_string(), session_id.clone());
                span.set_attribute("turn_id".to_string(), turn_id.clone());
                span.set_attribute(
                    "message_preview".to_string(),
                    message.chars().take(50).collect(),
                );
                span.end();
            }
            crate::agent::event::AgentEvent::ToolExecutionStart {
                call_id,
                tool_name,
                arguments,
            } => {
                let mut span = self.tracer.emit_tool_span(tool_name, call_id, None);
                span.set_attribute(
                    "arguments_preview".to_string(),
                    serde_json::to_string(arguments)
                        .map(|s| s.chars().take(100).collect())
                        .unwrap_or_default(),
                );
                span.end();
            }
            crate::agent::event::AgentEvent::ToolExecutionEnd {
                call_id,
                tool_name,
                success,
                output,
                error,
            } => {
                let mut span = self
                    .tracer
                    .start_span(format!("tool.end.{}", tool_name), None);
                span.set_attribute("call_id".to_string(), call_id.clone());
                span.set_attribute("tool.name".to_string(), tool_name.clone());
                span.set_attribute("success".to_string(), success.to_string());
                span.set_attribute("output_len".to_string(), output.len().to_string());
                if let Some(err) = error {
                    span.add_event(format!("error: {}", err));
                }
                span.end();
            }
            crate::agent::event::AgentEvent::TurnEnd {
                turn_id,
                success,
                tool_calls_count,
            } => {
                let mut span = self.tracer.start_span("agent.turn.end".to_string(), None);
                span.set_attribute("turn_id".to_string(), turn_id.clone());
                span.set_attribute("success".to_string(), success.to_string());
                span.set_attribute("tool_calls_count".to_string(), tool_calls_count.to_string());
                span.end();
            }
            crate::agent::event::AgentEvent::AgentEnd {
                session_id,
                summary,
                total_turns,
                total_tool_calls,
            } => {
                let mut span = self.tracer.start_span("agent.end".to_string(), None);
                span.set_attribute("session_id".to_string(), session_id.clone());
                if let Some(s) = summary {
                    span.set_attribute("summary".to_string(), s.clone());
                }
                span.set_attribute("total_turns".to_string(), total_turns.to_string());
                span.set_attribute("total_tool_calls".to_string(), total_tool_calls.to_string());
                span.end();
                self.tracer.print_trace();
            }
            crate::agent::event::AgentEvent::Error { code, message, .. } => {
                let mut span = self.tracer.start_span("agent.error".to_string(), None);
                span.set_attribute("error.code".to_string(), format!("{:?}", code));
                span.set_attribute("error.message".to_string(), message.clone());
                span.end();
            }
            _ => {
                // Other events (MessageDelta, MessageEnd, etc.) don't get dedicated spans
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_span_lifecycle() {
        let tracer = OtelTracer::default();
        let span = tracer.start_span("test.op".to_string(), None);
        let mut active = span;
        active.set_attribute("key".to_string(), "value".to_string());
        active.end();
        let spans = tracer.collector().get_spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "test.op");
        assert_eq!(
            spans[0].attributes.get("key").map(|s| s.as_str()),
            Some("value")
        );
        assert!(spans[0].end_time.is_some());
    }

    #[test]
    fn test_span_duration() {
        let tracer = OtelTracer::default();
        let span = tracer.start_span("duration.test".to_string(), None);
        let mut active = span;
        std::thread::sleep(std::time::Duration::from_millis(5));
        active.end();
        let spans = tracer.collector().get_spans();
        assert!(spans[0].duration_ms() >= 5);
    }

    #[test]
    fn test_emit_agent_turn_span() {
        let tracer = OtelTracer::default();
        tracer.emit_agent_turn_span("sess-123", "turn-456", 3);
        let spans = tracer.collector().get_spans();
        assert!(!spans.is_empty());
    }
}

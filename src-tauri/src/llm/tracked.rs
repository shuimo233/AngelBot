//! Per-request metering for one foreground provider binding. The same wrapper
//! follows Main and compaction; no extra requests, retry policy or credentials.
use super::{
    LlmFailure, LlmProvider, LlmResponse, Message, ProviderUsage, ThinkingSettings, ToolSchema,
    UsageTracker,
};
use std::sync::Mutex;
use std::time::Instant;

pub(crate) struct TrackedProvider {
    inner: Box<dyn LlmProvider>,
    tracker: UsageTracker,
    session: String,
    model: Mutex<String>,
}

impl TrackedProvider {
    pub(crate) fn new(
        inner: Box<dyn LlmProvider>,
        tracker: UsageTracker,
        session: String,
        model: String,
    ) -> Self {
        Self {
            inner,
            tracker,
            session,
            model: Mutex::new(model),
        }
    }

    fn record(&self, succeeded: bool, started: Instant) {
        // A failure may leave a provider's previous usage intact. Never count
        // stale usage, or invent zero tokens for failed/cancelled/missing usage.
        if !succeeded {
            return;
        }
        if let Some(usage) = self.inner.latest_usage() {
            if let Err(error) = self.tracker.record_provider_usage(
                Some(&self.session),
                self.inner.name(),
                &self.model.lock().unwrap(),
                usage,
                u32::try_from(started.elapsed().as_millis()).ok(),
            ) {
                eprintln!("Could not persist reported model usage: {error}");
            }
        } else {
            eprintln!("Model response omitted usage; recorded totals remain partial");
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for TrackedProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }
    #[cfg(test)]
    fn latest_request_fingerprint(&self) -> Option<super::ProviderRequestFingerprint> {
        self.inner.latest_request_fingerprint()
    }
    #[cfg(test)]
    fn latest_response_metadata(&self) -> Option<super::ProviderResponseMetadata> {
        self.inner.latest_response_metadata()
    }
    fn supports_tool_images(&self) -> bool {
        self.inner.supports_tool_images()
    }
    fn latest_usage(&self) -> Option<ProviderUsage> {
        self.inner.latest_usage()
    }
    fn refresh_api_key(&self, key: &str) -> bool {
        self.inner.refresh_api_key(key)
    }
    fn set_model(&self, model: &str) -> bool {
        if !self.inner.set_model(model) {
            return false;
        }
        *self.model.lock().unwrap() = model.into();
        true
    }

    async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
    ) -> Result<LlmResponse, String> {
        let started = Instant::now();
        let result = self.inner.chat(messages, tools, thinking).await;
        self.record(result.is_ok(), started);
        result
    }
    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
        deltas: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<LlmResponse, String> {
        let started = Instant::now();
        let result = self
            .inner
            .chat_stream(messages, tools, thinking, deltas)
            .await;
        self.record(result.is_ok(), started);
        result
    }
    async fn chat_classified(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
    ) -> Result<LlmResponse, LlmFailure> {
        let started = Instant::now();
        let result = self.inner.chat_classified(messages, tools, thinking).await;
        self.record(result.is_ok(), started);
        result
    }
    async fn chat_stream_classified(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
        deltas: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<LlmResponse, LlmFailure> {
        let started = Instant::now();
        let result = self
            .inner
            .chat_stream_classified(messages, tools, thinking, deltas)
            .await;
        self.record(result.is_ok(), started);
        result
    }
}

#[cfg(test)]
mod tests;

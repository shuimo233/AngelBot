use super::*;
use crate::llm::{LlmFailureKind, StopReason};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct Provider {
    calls: AtomicUsize,
    usage: Mutex<Option<ProviderUsage>>,
}
#[async_trait::async_trait]
impl LlmProvider for Provider {
    fn name(&self) -> &str {
        "offline"
    }
    fn supports_tool_images(&self) -> bool {
        true
    }
    fn latest_usage(&self) -> Option<ProviderUsage> {
        *self.usage.lock().unwrap()
    }
    fn set_model(&self, _: &str) -> bool {
        true
    }
    fn refresh_api_key(&self, _: &str) -> bool {
        true
    }
    async fn chat(
        &self,
        _: &[Message],
        _: Option<&[ToolSchema]>,
        _: Option<&ThinkingSettings>,
    ) -> Result<LlmResponse, String> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 4 {
            *self.usage.lock().unwrap() = None;
        }
        if call == 5 {
            // Deliberately stale usage on error: it must never be counted.
            *self.usage.lock().unwrap() = Some(ProviderUsage {
                input_tokens: 999,
                output_tokens: 999,
                ..ProviderUsage::default()
            });
            return Err("owned failure".into());
        }
        if call >= 6 {
            return std::future::pending().await;
        }
        if call < 4 {
            *self.usage.lock().unwrap() = Some(ProviderUsage {
                input_tokens: 10,
                output_tokens: 2,
                cached_input_tokens: Some(0),
                ..ProviderUsage::default()
            });
        }
        Ok(LlmResponse::Text {
            text: "ok".into(),
            stop_reason: StopReason::EndTurn,
        })
    }
    async fn chat_stream_classified(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
        deltas: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<LlmResponse, LlmFailure> {
        if let Some(sender) = deltas {
            sender.send("delta".into()).unwrap();
        }
        self.chat(messages, tools, thinking)
            .await
            .map_err(|diagnostic| LlmFailure {
                kind: LlmFailureKind::QuotaExceeded,
                diagnostic,
            })
    }
}

#[tokio::test]
async fn meters_each_request_once_without_replacing_failures_or_inventing_usage() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::db::migrate(&conn).unwrap();
    conn.execute(
        "INSERT INTO sessions(id,title,created_at,updated_at) VALUES('metered','fixture',1,1)",
        [],
    )
    .unwrap();
    let db = Arc::new(Mutex::new(conn));
    let provider = TrackedProvider::new(
        Box::new(Provider {
            calls: AtomicUsize::new(0),
            usage: Mutex::new(None),
        }),
        UsageTracker::new(db.clone()),
        "metered".into(),
        "model-before".into(),
    );
    assert!(provider.supports_tool_images() && provider.refresh_api_key("synthetic"));
    provider.chat(&[], None, None).await.unwrap();
    provider.chat_classified(&[], None, None).await.unwrap();
    assert!(provider.set_model("model-after"));
    provider.chat_stream(&[], None, None, None).await.unwrap();
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    provider
        .chat_stream_classified(&[], None, None, Some(sender))
        .await
        .unwrap();
    assert_eq!(receiver.recv().await.as_deref(), Some("delta"));
    provider.chat(&[], None, None).await.unwrap(); // no usage, not a zero-cost row
    let failure = provider
        .chat_stream_classified(&[], None, None, None)
        .await
        .unwrap_err();
    assert_eq!(failure.kind, LlmFailureKind::QuotaExceeded);
    assert_eq!(failure.diagnostic, "owned failure");
    assert!(tokio::time::timeout(
        std::time::Duration::from_millis(10),
        provider.chat(&[], None, None)
    )
    .await
    .is_err());
    let tracker = UsageTracker::new(db);
    let rows = tracker.get_session_usage("metered").unwrap();
    assert_eq!(
        rows.len(),
        4,
        "all four request paths, no duplicate/failed/missing/cancelled row"
    );
    assert!(rows.iter().all(|row| row.prompt_tokens == 10
        && row.completion_tokens == 2
        && row.latency_ms.is_some()));
    assert!(rows.iter().all(
        |row| row.provider_usage.unwrap().cached_input_tokens == Some(0)
            && row.provider_usage.unwrap().cache_write_tokens.is_none()
            && row.cost_basis == "legacy_static_estimate_not_bill"
    ));
    assert_eq!(
        rows.iter()
            .filter(|row| row.model == "model-before")
            .count(),
        2
    );
    assert_eq!(
        rows.iter().filter(|row| row.model == "model-after").count(),
        2
    );
    let totals = tracker.get_cost_summary().unwrap();
    assert_eq!(
        (totals.total_prompt_tokens, totals.total_completion_tokens),
        (40, 8)
    );
}

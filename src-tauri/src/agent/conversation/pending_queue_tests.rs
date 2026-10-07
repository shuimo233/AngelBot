//! Tests for `PendingMessageQueue` (Issue #097).
//!
//! The queue collects user messages that arrive while an agent turn is
//! in progress. The runner drains the queue before each LLM call so the
//! model can see mid-turn corrections instead of carrying them over to
//! the next turn.
//!
//! Reference: issues/097-pending-message-queue.md

#[cfg(test)]
mod tests {
    use crate::agent::pending_queue::PendingMessageQueue;
    use crate::llm::Message as LlmMessage;
    use std::sync::{Arc, Mutex};

    fn user_text(text: &str) -> LlmMessage {
        LlmMessage {
            tool_images: Vec::new(),
            protocol_state: None,
            role: "user".to_string(),
            content: text.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn fresh_queue_is_empty() {
        let q = PendingMessageQueue::new();
        assert_eq!(q.len(), 0);
        assert!(q.is_empty());
        assert!(q.drain().is_empty());
    }

    #[test]
    fn push_increments_length() {
        let q = PendingMessageQueue::new();
        q.push(user_text("first"));
        q.push(user_text("second"));
        assert_eq!(q.len(), 2);
        assert!(!q.is_empty());
    }

    #[test]
    fn drain_returns_messages_in_fifo_order() {
        let q = PendingMessageQueue::new();
        q.push(user_text("first"));
        q.push(user_text("second"));
        q.push(user_text("third"));

        let drained = q.drain();
        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0].content, "first");
        assert_eq!(drained[1].content, "second");
        assert_eq!(drained[2].content, "third");
        assert!(q.is_empty(), "drain must clear the queue");
    }

    #[test]
    fn drain_on_empty_queue_is_noop() {
        let q = PendingMessageQueue::new();
        let drained = q.drain();
        assert!(drained.is_empty());
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn queue_caps_at_max_size() {
        // Issue #097: a misbehaving caller must not be able to push
        // unlimited messages while the agent is running. The cap protects
        // the next LLM call from being forced into a flood.
        let q = PendingMessageQueue::new();
        let cap = q.cap();
        for i in 0..(cap + 50) {
            q.push(user_text(&format!("msg-{}", i)));
        }
        assert_eq!(q.len(), cap, "Queue must enforce its cap");
        let drained = q.drain();
        assert_eq!(drained.len(), cap, "Drain must not exceed cap");
    }

    #[test]
    fn overflow_is_documented() {
        // When the cap is hit, the queue MUST record that it dropped
        // messages so auditors can tell the user "we missed 50 of your
        // messages" rather than silently swallowing them.
        let q = PendingMessageQueue::new();
        let cap = q.cap();
        for i in 0..(cap + 5) {
            q.push(user_text(&format!("msg-{}", i)));
        }
        assert!(
            q.dropped_count() >= 5,
            "Dropped count must reflect overflow"
        );
    }

    #[test]
    fn queue_is_send_and_sync() {
        // The queue must be sharable across threads — the Tauri command
        // layer pushes from one thread, the runner drains from another.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PendingMessageQueue>();
        assert_send_sync::<Arc<Mutex<PendingMessageQueue>>>();
    }

    #[test]
    fn concurrent_pushes_do_not_panic() {
        // Stress test: many threads push while one drains. The queue must
        // not deadlock or panic even under heavy contention.
        let q = Arc::new(Mutex::new(PendingMessageQueue::new()));
        let mut handles = Vec::new();
        for _ in 0..4 {
            let qc = q.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..20 {
                    qc.lock().unwrap().push(user_text(&format!("t-{}", i)));
                }
            }));
        }
        for h in handles {
            h.join().expect("worker thread must not panic");
        }
        let drained = q.lock().unwrap().drain();
        assert!(drained.len() <= q.lock().unwrap().cap());
    }
}

//! PendingMessageQueue — collects user messages that arrive while an
//! agent turn is in progress (Issue #097).
//!
//! Pi's `pendingMessageQueue` serves two purposes:
//! 1. Preserve mid-turn corrections: the user notices the agent going
//!    the wrong way and types a correction *before* the agent has
//!    finished its current turn. Dropping these messages would force
//!    the user to wait for the next turn, which violates design
//!    philosophy §2.3 (user sovereignty).
//! 2. Avoid losing context when the turn is long: a long-running tool
//!    call may take tens of seconds; without a queue, the user has
//!    no way to inject "actually, never mind" without cancelling.
//!
//! The runner drains the queue before each LLM call so the model sees
//! the user's interjection as part of the conversation, not as a
//! separate run.
//!
//! Cap: 16 messages. Beyond that, additional pushes are dropped and
//! counted. The cap is enforced here rather than at the call site so
//! every consumer agrees on the same limit and the audit step can
//! honestly report `dropped_count`.

pub const PENDING_QUEUE_CAP: usize = 16;

use crate::llm::Message as LlmMessage;
use std::sync::Mutex;
use tokio::sync::oneshot;

/// FIFO queue of user messages waiting to be consumed by the next
/// runner iteration. Designed to be wrapped in `Arc<Mutex<...>>` and
/// shared between the Tauri command layer (producer) and the agent
/// runner (consumer).
pub struct PendingMessageQueue {
    inner: Mutex<Inner>,
}

struct Inner {
    queue: VecDeque<(LlmMessage, Option<oneshot::Sender<()>>)>,
    dropped: usize,
    closed: bool,
}

impl PendingMessageQueue {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                queue: VecDeque::with_capacity(PENDING_QUEUE_CAP),
                dropped: 0,
                closed: false,
            }),
        }
    }

    pub fn cap(&self) -> usize {
        PENDING_QUEUE_CAP
    }

    pub fn push(&self, message: LlmMessage) {
        let mut inner = self.inner.lock().expect("pending queue poisoned");
        if inner.queue.len() >= PENDING_QUEUE_CAP {
            inner.dropped += 1;
            return;
        }
        if !inner.closed {
            inner.queue.push_back((message, None));
        }
    }

    pub fn submit(&self, message: LlmMessage) -> Result<oneshot::Receiver<()>, String> {
        let mut inner = self.inner.lock().expect("pending queue poisoned");
        if inner.closed {
            return Err("当前轮已结束。".into());
        }
        if inner.queue.len() >= PENDING_QUEUE_CAP {
            return Err("当前轮的引导队列已满。".into());
        }
        let (ack, received) = oneshot::channel();
        inner.queue.push_back((message, Some(ack)));
        Ok(received)
    }

    /// Seal a successful turn, or let the runner take another step if a
    /// message arrived while the model was producing its final response.
    pub fn close_if_empty(&self) -> bool {
        let mut inner = self.inner.lock().expect("pending queue poisoned");
        if !inner.queue.is_empty() {
            return false;
        }
        inner.closed = true;
        true
    }

    pub fn close(&self) {
        let mut inner = self.inner.lock().expect("pending queue poisoned");
        inner.closed = true;
        inner.queue.clear();
    }

    /// Remove and return all queued messages, in FIFO order.
    /// The queue is left empty. `dropped_count` is *not* reset so
    /// callers can see the cumulative damage over the run's lifetime.
    pub fn drain(&self) -> Vec<LlmMessage> {
        let mut inner = self.inner.lock().expect("pending queue poisoned");
        inner
            .queue
            .drain(..)
            .map(|(message, ack)| {
                if let Some(ack) = ack {
                    let _ = ack.send(());
                }
                message
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("pending queue poisoned")
            .queue
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn dropped_count(&self) -> usize {
        self.inner.lock().expect("pending queue poisoned").dropped
    }
}

impl Default for PendingMessageQueue {
    fn default() -> Self {
        Self::new()
    }
}

use std::collections::VecDeque;

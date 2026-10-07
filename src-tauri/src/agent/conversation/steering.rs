//! User instruction queue for agent steering
//!
//! Issue #44: User intervention queue
//!
//! Supports:
//! - Normal queueing of instructions
//! - High-priority interrupts
//! - Agent pause/resume
//! - Yield points for user input

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Instruction priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstructionPriority {
    Low,
    Normal,
    High,
    Interrupt,
}

impl Default for InstructionPriority {
    fn default() -> Self {
        InstructionPriority::Normal
    }
}

/// A user instruction to be processed by the agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instruction {
    pub id: String,
    pub session_id: String,
    pub content: String,
    pub priority: InstructionPriority,
    pub created_at: i64,
    pub status: InstructionStatus,
}

impl Instruction {
    pub fn new(session_id: String, content: String, priority: InstructionPriority) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            session_id,
            content,
            priority,
            created_at: chrono::Utc::now().timestamp(),
            status: InstructionStatus::Pending,
        }
    }
}

/// Instruction processing status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstructionStatus {
    Pending,
    Processing,
    Completed,
    Cancelled,
}

/// The instruction queue for a session
#[derive(Debug, Clone)]
pub struct InstructionQueue {
    queue: VecDeque<Instruction>,
    current: Option<Instruction>,
    session_id: String,
}

impl InstructionQueue {
    pub fn new(session_id: String) -> Self {
        Self {
            queue: VecDeque::new(),
            current: None,
            session_id,
        }
    }

    /// Enqueue a new instruction
    pub fn enqueue(&mut self, content: String, priority: InstructionPriority) -> Instruction {
        let instruction = Instruction::new(self.session_id.clone(), content, priority);

        match priority {
            InstructionPriority::Interrupt => {
                // Interrupt goes to the front, cancelling current
                if let Some(mut curr) = self.current.take() {
                    curr.status = InstructionStatus::Cancelled;
                    self.queue.push_front(curr);
                }
                self.queue.push_front(instruction.clone());
            }
            InstructionPriority::High | InstructionPriority::Normal | InstructionPriority::Low => {
                let new_rank = priority_rank(priority);
                let insert_pos = self
                    .queue
                    .iter()
                    .position(|i| priority_rank(i.priority) < new_rank)
                    .unwrap_or(self.queue.len());
                self.queue.insert(insert_pos, instruction.clone());
            }
        }

        instruction
    }

    /// Get the next instruction if available
    pub fn dequeue(&mut self) -> Option<Instruction> {
        if self.current.is_some() {
            return None; // Currently processing
        }

        self.queue.pop_front().map(|mut instruction| {
            instruction.status = InstructionStatus::Processing;
            self.current = Some(instruction.clone());
            instruction
        })
    }

    /// Mark current instruction as completed
    pub fn complete(&mut self) -> Option<Instruction> {
        self.current.take().map(|mut instruction| {
            instruction.status = InstructionStatus::Completed;
            instruction
        })
    }

    /// Cancel current instruction
    pub fn cancel_current(&mut self) -> Option<Instruction> {
        self.current.take().map(|mut instruction| {
            instruction.status = InstructionStatus::Cancelled;
            instruction
        })
    }

    /// Get all pending instructions
    pub fn get_pending(&self) -> Vec<Instruction> {
        self.queue.iter().cloned().collect()
    }

    /// Check if there's an interrupt waiting
    pub fn has_interrupt(&self) -> bool {
        self.queue
            .iter()
            .any(|i| i.priority == InstructionPriority::Interrupt)
    }

    /// Get queue length
    pub fn len(&self) -> usize {
        self.queue.len() + if self.current.is_some() { 1 } else { 0 }
    }

    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty() && self.current.is_none()
    }

    /// Clear all pending instructions
    pub fn clear(&mut self) {
        self.queue.clear();
    }

    /// Get an instruction by ID (without removing)
    pub fn find(&self, id: &str) -> Option<Instruction> {
        self.queue.iter().find(|i| i.id == id).cloned()
    }

    /// Remove an instruction by ID
    pub fn remove(&mut self, id: &str) -> bool {
        let mut found = false;
        let mut new_queue = std::collections::VecDeque::new();
        while let Some(inst) = self.queue.pop_front() {
            if inst.id == id {
                found = true;
            } else {
                new_queue.push_back(inst);
            }
        }
        self.queue = new_queue;
        found
    }
}

fn priority_rank(priority: InstructionPriority) -> u8 {
    match priority {
        InstructionPriority::Interrupt => 4,
        InstructionPriority::High => 3,
        InstructionPriority::Normal => 2,
        InstructionPriority::Low => 1,
    }
}

/// Global instruction queue manager
pub struct QueueManager {
    queues: HashMap<String, Arc<Mutex<InstructionQueue>>>,
}

impl QueueManager {
    pub fn new() -> Self {
        Self {
            queues: HashMap::new(),
        }
    }

    /// Get or create a queue for a session
    pub fn get_queue(&mut self, session_id: &str) -> Arc<Mutex<InstructionQueue>> {
        let key = session_id.to_string();
        self.queues
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(InstructionQueue::new(key))))
            .clone()
    }

    /// Enqueue an instruction for a session
    pub fn enqueue(
        &mut self,
        session_id: &str,
        content: String,
        priority: InstructionPriority,
    ) -> Instruction {
        let queue = self.get_queue(session_id);
        let mut queue = queue.lock().unwrap();
        queue.enqueue(content, priority)
    }

    /// Check if agent should yield for user input
    pub fn should_yield(&self, session_id: &str) -> bool {
        if let Some(queue) = self.queues.get(session_id) {
            let queue = queue.lock().unwrap();
            queue.has_interrupt() || !queue.is_empty()
        } else {
            false
        }
    }

    /// Interrupt the agent for a session
    pub fn interrupt(&mut self, session_id: &str) -> Option<Instruction> {
        let queue = self.get_queue(session_id);
        let mut queue = queue.lock().unwrap();

        // Get interrupt instruction
        let interrupt_idx = queue
            .queue
            .iter()
            .position(|i| i.priority == InstructionPriority::Interrupt);

        interrupt_idx.and_then(|idx| queue.queue.remove(idx))
    }

    /// Remove a session's queue
    pub fn remove_session(&mut self, session_id: &str) {
        self.queues.remove(session_id);
    }
}

impl Default for QueueManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Agent control state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentControlState {
    Running,
    Paused,
    WaitingForInput,
    Interrupted,
    Stopped,
}

/// Yield point for agent execution
#[derive(Debug, Clone)]
pub enum AgentYield {
    /// Agent should wait for user input
    AwaitInput,
    /// Agent should process pending instructions
    ProcessInstructions,
    /// Agent should stop execution
    Stop,
    /// Agent should continue normally
    Continue,
}

impl AgentYield {
    pub fn should_await(&self) -> bool {
        matches!(
            self,
            AgentYield::AwaitInput | AgentYield::ProcessInstructions
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enqueue_normal() {
        let mut queue = InstructionQueue::new("test-session".to_string());
        let inst = queue.enqueue("Test instruction".to_string(), InstructionPriority::Normal);
        assert_eq!(queue.len(), 1);
        assert_eq!(inst.priority, InstructionPriority::Normal);
    }

    #[test]
    fn test_enqueue_interrupt() {
        let mut queue = InstructionQueue::new("test-session".to_string());

        // Add some normal instructions
        queue.enqueue("Normal 1".to_string(), InstructionPriority::Normal);
        queue.enqueue("Normal 2".to_string(), InstructionPriority::Normal);
        assert_eq!(queue.len(), 2);

        // Add interrupt
        queue.enqueue("INTERRUPT!".to_string(), InstructionPriority::Interrupt);
        assert_eq!(queue.len(), 3);
        assert!(queue.has_interrupt());

        // Interrupt should be at the front
        let dequeued = queue.dequeue().unwrap();
        assert_eq!(dequeued.content, "INTERRUPT!");
    }

    #[test]
    fn test_priority_ordering() {
        let mut queue = InstructionQueue::new("test".to_string());

        queue.enqueue("Low".to_string(), InstructionPriority::Low);
        queue.enqueue("Normal".to_string(), InstructionPriority::Normal);
        queue.enqueue("High".to_string(), InstructionPriority::High);

        let first = queue.dequeue().unwrap();
        assert_eq!(first.content, "High");
        queue.complete();

        let second = queue.dequeue().unwrap();
        assert_eq!(second.content, "Normal");
    }
}

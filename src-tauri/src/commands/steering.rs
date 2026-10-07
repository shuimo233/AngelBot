//! Agent steering commands for user intervention
//!
//! Issue #44: User intervention queue
//!
//! Provides commands for managing instruction queues and agent control.

use crate::agent::steering::{AgentControlState, Instruction, InstructionPriority};
use crate::agent::ToolCancellation;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

/// Global queue manager stored in AppState
pub struct SteeringState {
    queues: Mutex<HashMap<String, Arc<Mutex<InstructionQueue>>>>,
    agent_state: Arc<Mutex<HashMap<String, AgentControlState>>>,
    steer_messages: Arc<Mutex<HashMap<String, VecDeque<String>>>>,
    follow_up_messages: Arc<Mutex<HashMap<String, VecDeque<String>>>>,
    cancellations: Arc<Mutex<HashMap<String, ToolCancellation>>>,
    active_runs: Arc<Mutex<HashSet<String>>>,
}

/// Holds a session execution slot until the current agent slice ends.
/// Dropping the permit always releases the slot, including error and cancel paths.
pub struct AgentRunPermit {
    session_id: String,
    active_runs: Arc<Mutex<HashSet<String>>>,
}

impl Drop for AgentRunPermit {
    fn drop(&mut self) {
        if let Ok(mut active_runs) = self.active_runs.lock() {
            active_runs.remove(&self.session_id);
        }
    }
}

impl SteeringState {
    pub fn new() -> Self {
        Self {
            queues: Mutex::new(HashMap::new()),
            agent_state: Arc::new(Mutex::new(HashMap::new())),
            steer_messages: Arc::new(Mutex::new(HashMap::new())),
            follow_up_messages: Arc::new(Mutex::new(HashMap::new())),
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            active_runs: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Shared control state used by a running AgentRunner.  Commands mutate this
    /// map while the runner observes it at safe boundaries between model calls.
    pub fn control_states(&self) -> Arc<Mutex<HashMap<String, AgentControlState>>> {
        Arc::clone(&self.agent_state)
    }

    pub fn steering_queues(
        &self,
    ) -> (
        Arc<Mutex<HashMap<String, VecDeque<String>>>>,
        Arc<Mutex<HashMap<String, VecDeque<String>>>>,
    ) {
        (
            Arc::clone(&self.steer_messages),
            Arc::clone(&self.follow_up_messages),
        )
    }

    /// Return the signal for the current run, creating one only for callers
    /// that start before `mark_running` has been invoked.
    pub fn cancellation(&self, session_id: &str) -> ToolCancellation {
        self.cancellations
            .lock()
            .expect("steering cancellation lock poisoned")
            .entry(session_id.to_string())
            .or_insert_with(ToolCancellation::new)
            .clone()
    }

    fn cancel_running_tools(&self, session_id: &str) {
        if let Ok(cancellations) = self.cancellations.lock() {
            if let Some(cancellation) = cancellations.get(session_id) {
                cancellation.cancel();
            }
        }
    }

    fn enqueue_mode(&self, session_id: String, content: String, mode: SteeringMode) {
        let target = match mode {
            SteeringMode::Steer => &self.steer_messages,
            SteeringMode::FollowUp => &self.follow_up_messages,
            SteeringMode::Abort => return,
        };
        if let Ok(mut queues) = target.lock() {
            queues.entry(session_id).or_default().push_back(content);
        }
    }

    /// Start a new turn in a known runnable state after a previous pause/stop.
    pub fn mark_running(&self, session_id: &str) {
        if let Ok(mut states) = self.agent_state.lock() {
            states.insert(session_id.to_string(), AgentControlState::Running);
        }
        if let Ok(mut cancellations) = self.cancellations.lock() {
            cancellations.insert(session_id.to_string(), ToolCancellation::new());
        }
    }

    /// Allow independent sessions to run concurrently while preventing two
    /// overlapping slices from corrupting one session's conversation branch.
    pub fn try_start_run(&self, session_id: &str) -> Result<AgentRunPermit, String> {
        let mut active_runs = self
            .active_runs
            .lock()
            .map_err(|e| format!("agent run lock poisoned: {e}"))?;
        if !active_runs.insert(session_id.to_string()) {
            return Err("An agent task is already running in this session. Send an in-progress instruction or wait for it to finish.".to_string());
        }
        Ok(AgentRunPermit {
            session_id: session_id.to_string(),
            active_runs: Arc::clone(&self.active_runs),
        })
    }

    fn get_or_create_queue(&self, session_id: &str) -> Arc<Mutex<InstructionQueue>> {
        let mut queues = self.queues.lock().unwrap();
        let key = session_id.to_string();
        queues
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(InstructionQueue::new(session_id.to_string()))))
            .clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SteeringMode {
    Steer,
    FollowUp,
    Abort,
}

/// A single instruction in the queue
#[derive(Debug, Clone)]
pub struct QueuedInstruction {
    pub id: String,
    pub content: String,
    pub priority: InstructionPriority,
}

/// A simple instruction queue
#[derive(Debug, Clone)]
pub struct InstructionQueue {
    queue: VecDeque<QueuedInstruction>,
    session_id: String,
}

impl InstructionQueue {
    pub fn new(session_id: String) -> Self {
        Self {
            queue: VecDeque::new(),
            session_id,
        }
    }

    pub fn enqueue(&mut self, content: String, priority: InstructionPriority) -> Instruction {
        let id = uuid::Uuid::new_v4().to_string();
        self.queue.push_back(QueuedInstruction {
            id: id.clone(),
            content,
            priority,
        });
        Instruction {
            id,
            session_id: self.session_id.clone(),
            content: String::new(), // Will be set correctly
            priority,
            created_at: chrono::Utc::now().timestamp(),
            status: crate::agent::steering::InstructionStatus::Pending,
        }
    }

    pub fn get_pending(&self) -> Vec<Instruction> {
        self.queue
            .iter()
            .map(|q| Instruction {
                id: q.id.clone(),
                session_id: self.session_id.clone(),
                content: q.content.clone(),
                priority: q.priority,
                created_at: chrono::Utc::now().timestamp(),
                status: crate::agent::steering::InstructionStatus::Pending,
            })
            .collect()
    }

    pub fn has_interrupt(&self) -> bool {
        self.queue
            .iter()
            .any(|q| q.priority == InstructionPriority::Interrupt)
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn remove(&mut self, id: &str) {
        self.queue.retain(|q| q.id != id);
    }

    pub fn clear(&mut self) {
        self.queue.clear();
    }
}

impl Default for SteeringState {
    fn default() -> Self {
        Self::new()
    }
}

/// Enqueue an instruction for a session
#[tauri::command]
pub fn enqueue_instruction(
    state: tauri::State<'_, SteeringState>,
    session_id: String,
    content: String,
    priority: String,
) -> Result<Instruction, String> {
    let priority = match priority.to_lowercase().as_str() {
        "low" => InstructionPriority::Low,
        "high" => InstructionPriority::High,
        "interrupt" => InstructionPriority::Interrupt,
        _ => InstructionPriority::Normal,
    };

    let queue = state.get_or_create_queue(&session_id);
    let mut queue = queue.lock().map_err(|e| e.to_string())?;
    Ok(queue.enqueue(content, priority))
}

/// Submit a user intervention for a task that is currently running.
#[tauri::command]
pub fn submit_in_progress_command(
    state: tauri::State<'_, SteeringState>,
    session_id: String,
    content: String,
    mode: SteeringMode,
) -> Result<(), String> {
    if mode == SteeringMode::Abort {
        if let Ok(mut states) = state.agent_state.lock() {
            states.insert(session_id.clone(), AgentControlState::Interrupted);
        }
        state.cancel_running_tools(&session_id);
        return Ok(());
    }
    if content.trim().is_empty() {
        return Err("Steer and follow-up messages cannot be empty".to_string());
    }
    state.enqueue_mode(session_id, content, mode);
    Ok(())
}

/// Interrupt the agent for a session
#[tauri::command]
pub fn interrupt_agent(
    state: tauri::State<'_, SteeringState>,
    session_id: String,
) -> Result<Option<Instruction>, String> {
    // Update agent state
    if let Ok(mut states) = state.agent_state.lock() {
        states.insert(session_id.clone(), AgentControlState::Interrupted);
    }
    state.cancel_running_tools(&session_id);

    let queue = state.get_or_create_queue(&session_id);
    let mut queue = queue.lock().map_err(|e| e.to_string())?;

    // Find and remove interrupt instruction
    let idx = queue
        .queue
        .iter()
        .position(|q| q.priority == InstructionPriority::Interrupt);
    if let Some(idx) = idx {
        let q = queue.queue.remove(idx).unwrap();
        Ok(Some(Instruction {
            id: q.id,
            session_id,
            content: q.content,
            priority: q.priority,
            created_at: chrono::Utc::now().timestamp(),
            status: crate::agent::steering::InstructionStatus::Pending,
        }))
    } else {
        Ok(None)
    }
}

/// Pause the agent for a session
#[tauri::command]
pub fn pause_agent(
    state: tauri::State<'_, SteeringState>,
    session_id: String,
) -> Result<(), String> {
    if let Ok(mut states) = state.agent_state.lock() {
        states.insert(session_id, AgentControlState::Paused);
    }
    Ok(())
}

/// Resume the agent for a session
#[tauri::command]
pub fn resume_agent(
    state: tauri::State<'_, SteeringState>,
    session_id: String,
) -> Result<(), String> {
    if let Ok(mut states) = state.agent_state.lock() {
        states.insert(session_id, AgentControlState::Running);
    }
    Ok(())
}

/// Get agent control state for a session
#[tauri::command]
pub fn get_agent_state(
    state: tauri::State<'_, SteeringState>,
    session_id: String,
) -> Result<AgentControlState, String> {
    let states = state.agent_state.lock().map_err(|e| e.to_string())?;
    Ok(states
        .get(&session_id)
        .copied()
        .unwrap_or(AgentControlState::Running))
}

/// Get pending instructions for a session
#[tauri::command]
pub fn get_pending_instructions(
    state: tauri::State<'_, SteeringState>,
    session_id: String,
) -> Result<Vec<Instruction>, String> {
    let queues = state.queues.lock().map_err(|e| e.to_string())?;
    if let Some(queue) = queues.get(&session_id) {
        let queue = queue.lock().map_err(|e| e.to_string())?;
        Ok(queue.get_pending())
    } else {
        Ok(vec![])
    }
}

/// Check if agent should yield for user input
#[tauri::command]
pub fn should_agent_yield(
    state: tauri::State<'_, SteeringState>,
    session_id: String,
) -> Result<bool, String> {
    let queues = state.queues.lock().map_err(|e| e.to_string())?;
    if let Some(queue) = queues.get(&session_id) {
        let queue = queue.lock().map_err(|e| e.to_string())?;
        Ok(queue.has_interrupt() || !queue.is_empty())
    } else {
        Ok(false)
    }
}

/// Cancel a specific instruction
#[tauri::command]
pub fn cancel_instruction(
    state: tauri::State<'_, SteeringState>,
    session_id: String,
    instruction_id: String,
) -> Result<(), String> {
    let queues = state.queues.lock().map_err(|e| e.to_string())?;
    if let Some(queue) = queues.get(&session_id) {
        let mut queue = queue.lock().map_err(|e| e.to_string())?;
        queue.remove(&instruction_id);
    }
    Ok(())
}

/// Clear all pending instructions for a session
#[tauri::command]
pub fn clear_instructions(
    state: tauri::State<'_, SteeringState>,
    session_id: String,
) -> Result<(), String> {
    let queues = state.queues.lock().map_err(|e| e.to_string())?;
    if let Some(queue) = queues.get(&session_id) {
        let mut queue = queue.lock().map_err(|e| e.to_string())?;
        queue.clear();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_sessions_can_run_while_one_session_is_exclusive() {
        let state = SteeringState::new();
        let first = state.try_start_run("session-a").unwrap();
        assert!(state.try_start_run("session-a").is_err());
        let second_session = state.try_start_run("session-b").unwrap();

        drop(first);
        assert!(state.try_start_run("session-a").is_ok());
        drop(second_session);
    }

    #[test]
    fn steering_modes_keep_immediate_and_follow_up_messages_separate() {
        let state = SteeringState::new();
        state.enqueue_mode(
            "s1".to_string(),
            "change direction".to_string(),
            SteeringMode::Steer,
        );
        state.enqueue_mode(
            "s1".to_string(),
            "then summarize".to_string(),
            SteeringMode::FollowUp,
        );

        let (steer, follow_up) = state.steering_queues();
        assert_eq!(
            steer.lock().unwrap().get("s1").unwrap().front(),
            Some(&"change direction".to_string())
        );
        assert_eq!(
            follow_up.lock().unwrap().get("s1").unwrap().front(),
            Some(&"then summarize".to_string())
        );
    }

    #[test]
    fn abort_mode_never_enqueues_a_prompt() {
        let state = SteeringState::new();
        state.enqueue_mode("s1".to_string(), "stop".to_string(), SteeringMode::Abort);
        let (steer, follow_up) = state.steering_queues();
        assert!(steer.lock().unwrap().is_empty());
        assert!(follow_up.lock().unwrap().is_empty());
    }

    #[test]
    fn a_new_turn_gets_a_fresh_cancellation_signal() {
        let state = SteeringState::new();
        state.mark_running("s1");
        let first = state.cancellation("s1");
        state.cancel_running_tools("s1");
        assert!(first.is_cancelled());

        state.mark_running("s1");
        let second = state.cancellation("s1");
        assert!(!second.is_cancelled());
        assert!(first.is_cancelled());
    }
}

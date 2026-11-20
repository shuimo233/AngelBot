//! Agent steering commands for user intervention
//!
//! Issue #44: User intervention queue
//!
//! Provides commands for managing instruction queues and agent control.

use crate::agent::pending_queue::PendingMessageQueue;
use crate::agent::steering::{AgentControlState, Instruction, InstructionPriority};
use crate::agent::ToolCancellation;
use crate::llm::Message;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

/// Global queue manager stored in AppState
pub struct SteeringState {
    queues: Mutex<HashMap<String, Arc<Mutex<InstructionQueue>>>>,
    agent_state: Arc<Mutex<HashMap<String, AgentControlState>>>,
    active_steering: Mutex<HashMap<String, ActiveSteering>>,
    cancellations: Arc<Mutex<HashMap<String, ToolCancellation>>>,
    active_runs: Arc<Mutex<HashSet<String>>>,
}

struct ActiveSteering {
    reply_id: String,
    queue: Arc<Mutex<PendingMessageQueue>>,
}

pub(crate) struct SteeringRegistration<'a> {
    state: &'a SteeringState,
    session_id: String,
}

impl Drop for SteeringRegistration<'_> {
    fn drop(&mut self) {
        if let Some(active) = self
            .state
            .active_steering
            .lock()
            .unwrap()
            .remove(&self.session_id)
        {
            active.queue.lock().unwrap().close();
        }
    }
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
            active_steering: Mutex::new(HashMap::new()),
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            active_runs: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Hold the existing admission mutex while an idle-only data operation
    /// runs. Checking the set and then releasing it would race a new Main turn.
    pub(crate) fn with_idle<T>(
        &self,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let active = self
            .active_runs
            .lock()
            .map_err(|_| "无法核对当前任务状态。".to_owned())?;
        if !active.is_empty() {
            return Err("请等待当前 Agent 任务结束或停止后再进行数据操作。".to_owned());
        }
        operation()
    }

    /// Shared control state used by a running AgentRunner.  Commands mutate this
    /// map while the runner observes it at safe boundaries between model calls.
    pub fn control_states(&self) -> Arc<Mutex<HashMap<String, AgentControlState>>> {
        Arc::clone(&self.agent_state)
    }

    pub(crate) fn set_control_state(
        &self,
        session_id: String,
        state: AgentControlState,
    ) -> Result<(), String> {
        self.agent_state
            .lock()
            .map_err(|error| error.to_string())?
            .insert(session_id, state);
        Ok(())
    }

    /// Register the runner while its foreground session permit is held.
    pub(crate) fn register_steering(
        &self,
        session_id: &str,
        reply_id: &str,
        queue: Arc<Mutex<PendingMessageQueue>>,
    ) -> SteeringRegistration<'_> {
        self.active_steering.lock().unwrap().insert(
            session_id.to_owned(),
            ActiveSteering {
                reply_id: reply_id.to_owned(),
                queue,
            },
        );
        SteeringRegistration {
            state: self,
            session_id: session_id.to_owned(),
        }
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

    fn interrupt_run(&self, session_id: &str) -> Result<(), String> {
        // Start and Stop share this order so neither can replace only half of
        // the run's control state and cancellation signal.
        let _active_runs = self.active_runs.lock().map_err(|e| e.to_string())?;
        let mut states = self.agent_state.lock().map_err(|e| e.to_string())?;
        let cancellations = self.cancellations.lock().map_err(|e| e.to_string())?;
        states.insert(session_id.to_string(), AgentControlState::Interrupted);
        if let Some(cancellation) = cancellations.get(session_id) {
            cancellation.cancel();
        }
        Ok(())
    }

    // Caller holds active_runs before taking the control-state locks.
    fn reset_running_state(&self, session_id: &str) -> Result<ToolCancellation, String> {
        let mut states = self.agent_state.lock().map_err(|e| e.to_string())?;
        let mut cancellations = self.cancellations.lock().map_err(|e| e.to_string())?;
        let cancellation = ToolCancellation::new();
        states.insert(session_id.to_string(), AgentControlState::Running);
        cancellations.insert(session_id.to_string(), cancellation.clone());
        Ok(cancellation)
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

    /// Reserve a slice and its fresh Running state together. A Stop after
    /// this call cancels this exact signal even while run setup is pending.
    pub fn try_start_running_run(
        &self,
        session_id: &str,
    ) -> Result<(AgentRunPermit, ToolCancellation), String> {
        let mut active_runs = self
            .active_runs
            .lock()
            .map_err(|e| format!("agent run lock poisoned: {e}"))?;
        if active_runs.contains(session_id) {
            return Err("An agent task is already running in this session. Send an in-progress instruction or wait for it to finish.".to_string());
        }
        let cancellation = self.reset_running_state(session_id)?;
        active_runs.insert(session_id.to_string());
        Ok((
            AgentRunPermit {
                session_id: session_id.to_string(),
                active_runs: Arc::clone(&self.active_runs),
            },
            cancellation,
        ))
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
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
    content: String,
    priority: String,
) -> Result<Instruction, String> {
    let state = &state.steering;
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
pub async fn submit_in_progress_command(
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
    content: String,
    mode: SteeringMode,
    reply_id: Option<String>,
) -> Result<(), String> {
    submit_in_progress_command_impl(
        &state.steering,
        session_id,
        content,
        mode,
        reply_id.as_deref(),
    )
    .await
}

pub(crate) async fn submit_in_progress_command_impl(
    state: &SteeringState,
    session_id: String,
    content: String,
    mode: SteeringMode,
    reply_id: Option<&str>,
) -> Result<(), String> {
    if mode == SteeringMode::Abort {
        return state.interrupt_run(&session_id);
    }
    if mode == SteeringMode::FollowUp {
        return Err("请通过会话输入框排队。".into());
    }
    if content.trim().is_empty() {
        return Err("请输入引导内容。".into());
    }
    let received = {
        let active = state.active_steering.lock().unwrap();
        let run = active
            .get(&session_id)
            .filter(|run| Some(run.reply_id.as_str()) == reply_id)
            .ok_or("当前轮暂时无法接收引导。")?;
        let received = run.queue.lock().unwrap().submit(Message {
            role: "user".into(),
            content,
            tool_calls: None,
            tool_call_id: None,
            tool_images: Vec::new(),
            protocol_state: None,
        })?;
        received
    };
    received.await.map_err(|_| "当前轮已结束。".into())
}

/// Interrupt the agent for a session
#[tauri::command]
pub fn interrupt_agent(
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
) -> Result<Option<Instruction>, String> {
    interrupt_agent_impl(&state.steering, session_id)
}

pub(crate) fn interrupt_agent_impl(
    state: &SteeringState,
    session_id: String,
) -> Result<Option<Instruction>, String> {
    state.interrupt_run(&session_id)?;

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
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
) -> Result<(), String> {
    state
        .steering
        .set_control_state(session_id, AgentControlState::Paused)
}

/// Resume the agent for a session
#[tauri::command]
pub fn resume_agent(
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
) -> Result<(), String> {
    state
        .steering
        .set_control_state(session_id, AgentControlState::Running)
}

/// Get agent control state for a session
#[tauri::command]
pub fn get_agent_state(
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
) -> Result<AgentControlState, String> {
    let state = &state.steering;
    let states = state.agent_state.lock().map_err(|e| e.to_string())?;
    Ok(states
        .get(&session_id)
        .copied()
        .unwrap_or(AgentControlState::Running))
}

/// Get pending instructions for a session
#[tauri::command]
pub fn get_pending_instructions(
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
) -> Result<Vec<Instruction>, String> {
    let state = &state.steering;
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
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
) -> Result<bool, String> {
    let state = &state.steering;
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
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
    instruction_id: String,
) -> Result<(), String> {
    let state = &state.steering;
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
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
) -> Result<(), String> {
    let state = &state.steering;
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
        let (first, first_cancellation) = state.try_start_running_run("session-a").unwrap();
        assert!(state.try_start_run("session-a").is_err());
        assert!(state.try_start_running_run("session-a").is_err());
        let (second_session, second_cancellation) =
            state.try_start_running_run("session-b").unwrap();

        state.interrupt_run("session-a").unwrap();
        assert!(first_cancellation.is_cancelled());
        assert!(!second_cancellation.is_cancelled());

        drop(first);
        assert!(state.try_start_run("session-a").is_ok());
        drop(second_session);
    }

    #[test]
    fn steering_requires_the_current_reply_and_acknowledges_only_drained_input() {
        let state = SteeringState::new();
        let queue = Arc::new(Mutex::new(PendingMessageQueue::new()));
        let registration = state.register_steering("s1", "reply", queue.clone());
        let submit = |reply| {
            submit_in_progress_command_impl(
                &state,
                "s1".into(),
                "change direction".into(),
                SteeringMode::Steer,
                Some(reply),
            )
        };
        let mut stale = std::pin::pin!(submit("old"));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        use std::future::Future;
        assert!(matches!(
            stale.as_mut().poll(&mut context),
            std::task::Poll::Ready(Err(_))
        ));
        let mut active = std::pin::pin!(submit("reply"));
        assert!(active.as_mut().poll(&mut context).is_pending());
        assert_eq!(queue.lock().unwrap().drain()[0].content, "change direction");
        assert_eq!(
            active.as_mut().poll(&mut context),
            std::task::Poll::Ready(Ok(()))
        );
        let mut undrained = std::pin::pin!(submit("reply"));
        assert!(undrained.as_mut().poll(&mut context).is_pending());
        drop(registration);
        assert!(matches!(
            undrained.as_mut().poll(&mut context),
            std::task::Poll::Ready(Err(_))
        ));
    }

    #[test]
    fn abort_mode_never_enqueues_a_prompt() {
        let state = SteeringState::new();
        let (_permit, cancellation) = state.try_start_running_run("s1").unwrap();
        state.interrupt_run("s1").unwrap();
        assert!(cancellation.is_cancelled());
        assert_eq!(
            state.agent_state.lock().unwrap().get("s1"),
            Some(&AgentControlState::Interrupted),
        );
        assert!(state.active_steering.lock().unwrap().is_empty());
    }

    #[test]
    fn a_new_turn_gets_a_fresh_cancellation_signal() {
        let state = SteeringState::new();
        let (first_permit, first) = state.try_start_running_run("s1").unwrap();
        assert!(!first.is_cancelled());
        assert_eq!(
            state.agent_state.lock().unwrap().get("s1"),
            Some(&AgentControlState::Running),
        );
        state.interrupt_run("s1").unwrap();
        assert!(first.is_cancelled());
        // Setup still owns its permit. An overlapping attempt must not
        // replace the stopped run's state or its cancellation token.
        assert!(state.try_start_running_run("s1").is_err());
        assert!(state.cancellation("s1").is_cancelled());
        assert_eq!(
            state.agent_state.lock().unwrap().get("s1"),
            Some(&AgentControlState::Interrupted),
        );

        drop(first_permit);
        let (second_permit, second) = state.try_start_running_run("s1").unwrap();
        assert!(!second.is_cancelled());
        assert!(first.is_cancelled());
        state.interrupt_run("s1").unwrap();
        assert!(second.is_cancelled());
        drop(second_permit);

        let poisoned = SteeringState::new();
        assert!(std::panic::catch_unwind(|| {
            let _cancellations = poisoned.cancellations.lock().unwrap();
            panic!("simulate unavailable cancellation state");
        })
        .is_err());
        assert!(poisoned.try_start_running_run("s1").is_err());
        assert!(poisoned.agent_state.lock().unwrap().is_empty());
        assert!(poisoned.active_runs.lock().unwrap().is_empty());
        assert!(poisoned.try_start_run("s1").is_ok());
    }
}

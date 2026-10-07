use tauri::State;

use crate::{runtime_health::RuntimeHealthSnapshot, AppState};

#[tauri::command]
pub fn get_runtime_health(state: State<'_, AppState>) -> RuntimeHealthSnapshot {
    runtime_health(&state)
}

pub fn runtime_health(state: &AppState) -> RuntimeHealthSnapshot {
    state.runtime_health.snapshot(state.delegation_available)
}

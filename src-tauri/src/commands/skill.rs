//! Skill management commands
//!
//! Issue #45: Skill system standardization
//!
//! Provides commands for managing and invoking skills.

use crate::skill::{SkillManifest, SkillRegistry};
use crate::AppState;
use std::sync::{Arc, Mutex};
use tauri::State;

/// Global skill registry
pub struct SkillState {
    pub registry: Arc<Mutex<SkillRegistry>>,
}

impl SkillState {
    pub fn new() -> Self {
        let mut registry = SkillRegistry::new();
        // A registry is only an in-memory index. Rebuild it from AngelBot's
        // managed local store so installed skills survive application restart.
        if let Err(error) = registry.load_managed_skills() {
            eprintln!("[Skills] Failed to restore managed skills: {error}");
        }

        // Load built-in skills from embedded directory
        // In production, these would be loaded from the app bundle
        Self {
            registry: Arc::new(Mutex::new(registry)),
        }
    }
}

impl Default for SkillState {
    fn default() -> Self {
        Self::new()
    }
}

/// Get all registered skills
#[tauri::command]
pub fn get_skills(state: State<'_, AppState>) -> Result<Vec<SkillManifest>, String> {
    let registry = state.skills.registry.lock().map_err(|e| e.to_string())?;
    Ok(registry.all().iter().map(|s| (*s).clone()).collect())
}

/// Get a skill by ID
#[tauri::command]
pub fn get_skill(
    state: State<'_, AppState>,
    skill_id: String,
) -> Result<Option<SkillManifest>, String> {
    let registry = state.skills.registry.lock().map_err(|e| e.to_string())?;
    Ok(registry.get(&skill_id).cloned())
}

/// Load skills from a directory
#[tauri::command]
pub fn load_skills(state: State<'_, AppState>, directory: String) -> Result<usize, String> {
    let mut registry = state.skills.registry.lock().map_err(|e| e.to_string())?;
    let path = std::path::Path::new(&directory);
    registry.import_directory(path).map_err(|e| e.to_string())
}

/// Import templates from a public GitHub repository into the local skill store.
/// Network access and filesystem writes are intentionally exposed as a separate
/// command so UI and agent callers can require confirmation before invoking it.
#[tauri::command]
pub fn import_github_skills(
    state: State<'_, AppState>,
    repository_url: String,
) -> Result<usize, String> {
    let mut registry = state.skills.registry.lock().map_err(|e| e.to_string())?;
    registry.import_github_repository(&repository_url)
}

/// Register a skill from manifest
#[tauri::command]
pub fn register_skill(state: State<'_, AppState>, manifest: SkillManifest) -> Result<(), String> {
    let mut registry = state.skills.registry.lock().map_err(|e| e.to_string())?;
    registry.register(manifest);
    Ok(())
}

/// Check if a skill has an action
#[tauri::command]
pub fn skill_has_action(
    state: State<'_, AppState>,
    skill_id: String,
    action_name: String,
) -> Result<bool, String> {
    let registry = state.skills.registry.lock().map_err(|e| e.to_string())?;
    Ok(registry.has_action(&skill_id, &action_name))
}

/// Get skill IDs
#[tauri::command]
pub fn get_skill_ids(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let registry = state.skills.registry.lock().map_err(|e| e.to_string())?;
    Ok(registry.ids())
}

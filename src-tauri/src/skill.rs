//! Skill system for extensible agent capabilities
//!
//! Issue #45: Skill system standardization
//!
//! Follows Pi's SKILL.md format:
//! ```markdown
//! # Skill Name
//! Description: ...
//!
//! Actions:
//! - name: action_name
//!   description: ...
//!   parameters:
//!     - name: param1
//!       type: string
//!       required: true
//! ```

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Skill manifest metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: Option<String>,
    pub actions: Vec<SkillAction>,
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    pub prompts: Vec<PromptTemplate>,
}

impl SkillManifest {
    /// Parse a SKILL.md file into a SkillManifest
    pub fn from_markdown(content: &str) -> Result<Self, String> {
        let mut manifest = SkillManifest {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            version: "1.0.0".to_string(),
            author: None,
            actions: Vec::new(),
            dependencies: Vec::new(),
            permissions: Vec::new(),
            prompts: Vec::new(),
        };

        let mut current_section = String::new();
        let mut current_action: Option<SkillAction> = None;
        let mut current_param: Option<Parameter> = None;

        for line in content.lines() {
            let line = line.trim();

            // Section headers
            if line.starts_with("# ") {
                manifest.name = line.trim_start_matches("# ").to_string();
                manifest.id = manifest.name.to_lowercase().replace(' ', "-");
                current_section = "title".to_string();
                continue;
            }

            if line.starts_with("Description:") {
                manifest.description = line.trim_start_matches("Description:").trim().to_string();
                current_section = "description".to_string();
                continue;
            }

            if line.starts_with("Version:") {
                manifest.version = line.trim_start_matches("Version:").trim().to_string();
                continue;
            }

            if line.starts_with("Author:") {
                manifest.author = Some(line.trim_start_matches("Author:").trim().to_string());
                continue;
            }

            if line.starts_with("## Actions")
                || line.starts_with("## Prompts")
                || line.starts_with("## Permissions")
            {
                current_section = line.trim_start_matches("## ").to_lowercase();
                continue;
            }

            if current_section == "permissions" && line.starts_with("-") {
                manifest
                    .permissions
                    .push(line.trim_start_matches('-').trim().to_string());
                continue;
            }

            // Actions
            if current_section == "actions" && line.starts_with("- name:") {
                // Save previous action if exists
                if let Some(mut action) = current_action.take() {
                    if let Some(param) = current_param.take() {
                        action.parameters.push(param);
                    }
                    manifest.actions.push(action);
                }

                let name = line.trim_start_matches("- name:").trim().to_string();
                current_action = Some(SkillAction {
                    name,
                    description: String::new(),
                    parameters: Vec::new(),
                    handler: String::new(),
                });
                current_section = "action".to_string();
                continue;
            }

            if let Some(action) = current_action.as_mut() {
                if line.starts_with("  description:") {
                    action.description =
                        line.trim_start_matches("  description:").trim().to_string();
                    continue;
                }

                if line.starts_with("  handler:") {
                    action.handler = line.trim_start_matches("  handler:").trim().to_string();
                    continue;
                }

                // Parameters
                if line.starts_with("  parameters:") || line.starts_with("    - name:") {
                    // Save previous param
                    if let Some(param) = current_param.take() {
                        action.parameters.push(param);
                    }

                    if line.contains("- name:") {
                        let name = line.trim_start_matches("- name:").trim().to_string();
                        current_param = Some(Parameter {
                            name,
                            param_type: "string".to_string(),
                            required: false,
                            description: String::new(),
                        });
                    }
                    continue;
                }

                if let Some(param) = current_param.as_mut() {
                    if line.contains("type:") {
                        param.param_type = line
                            .split(':')
                            .nth(1)
                            .unwrap_or("string")
                            .trim()
                            .to_string();
                    }
                    if line.contains("required:") {
                        param.required = line.contains("true");
                    }
                    if line.contains("description:") {
                        param.description = line.split(':').nth(1).unwrap_or("").trim().to_string();
                    }
                }
            }

            // Prompts
            if current_section == "prompts" && line.starts_with("- name:") {
                let name = line.trim_start_matches("- name:").trim().to_string();
                manifest.prompts.push(PromptTemplate {
                    name,
                    content: String::new(),
                });
            }
        }

        // Save last action
        if let Some(mut action) = current_action.take() {
            if let Some(param) = current_param.take() {
                action.parameters.push(param);
            }
            manifest.actions.push(action);
        }

        Ok(manifest)
    }
}

/// A skill action that can be invoked
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillAction {
    pub name: String,
    pub description: String,
    pub parameters: Vec<Parameter>,
    pub handler: String,
}

/// Parameter definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub param_type: String,
    pub required: bool,
    pub description: String,
}

/// Prompt template
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTemplate {
    pub name: String,
    pub content: String,
}

/// Skill registry for managing available skills
pub struct SkillRegistry {
    skills: HashMap<String, SkillManifest>,
    source_files: HashMap<String, PathBuf>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
            source_files: HashMap::new(),
        }
    }

    /// Register a skill from a manifest
    pub fn register(&mut self, manifest: SkillManifest) {
        self.skills.insert(manifest.id.clone(), manifest);
    }

    /// Register a skill from a SKILL.md file
    pub fn register_from_file(&mut self, path: &Path) -> Result<(), String> {
        let content =
            fs::read_to_string(path).map_err(|e| format!("Failed to read skill file: {}", e))?;

        let mut manifest = SkillManifest::from_markdown(&content)?;
        if manifest.name.is_empty() {
            manifest.name = path
                .parent()
                .and_then(|value| value.file_name())
                .and_then(|value| value.to_str())
                .unwrap_or("Imported template")
                .to_string();
            manifest.id = manifest.name.to_lowercase().replace(' ', "-");
        }
        if manifest.description.is_empty() {
            manifest.description = format!("Imported from {}", path.display());
        }
        let id = manifest.id.clone();
        self.register(manifest);
        self.source_files.insert(id, path.to_path_buf());
        Ok(())
    }

    /// Get a skill by ID
    pub fn get(&self, id: &str) -> Option<&SkillManifest> {
        self.skills.get(id)
    }

    /// Get all registered skills
    pub fn all(&self) -> Vec<&SkillManifest> {
        self.skills.values().collect()
    }

    /// Get skill IDs
    pub fn ids(&self) -> Vec<String> {
        self.skills.keys().cloned().collect()
    }

    /// Check if a skill has an action
    pub fn has_action(&self, skill_id: &str, action_name: &str) -> bool {
        self.skills
            .get(skill_id)
            .map(|s| s.actions.iter().any(|a| a.name == action_name))
            .unwrap_or(false)
    }

    /// Load skills from a directory
    pub fn load_from_directory(&mut self, dir: &Path) -> Result<usize, String> {
        let mut count = 0;

        if !dir.exists() {
            return Ok(0);
        }

        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();

            if path.is_file() && path.file_name().unwrap_or_default() == "SKILL.md" {
                if self.register_from_file(&path).is_ok() {
                    count += 1;
                }
            }
        }

        Ok(count)
    }

    /// Copy local SKILL.md files into AngelBot's managed store before loading
    /// them. Persisting the source avoids a registry that vanishes after a
    /// restart or depends on a removable/external directory remaining present.
    pub fn import_directory(&mut self, dir: &Path) -> Result<usize, String> {
        let files = find_skill_files(dir)?;
        let root = managed_skill_store();
        let local_root = root.join("local");
        if let Err(error) = fs::create_dir_all(&local_root) {
            // Sandboxed/dev environments may intentionally deny writes to the
            // platform data directory. Keep the command useful by loading the
            // bounded text manifests in memory; a normal desktop install still
            // takes the durable branch above.
            return self.load_from_directory(dir).map_err(|fallback| {
                format!("skill store unavailable ({error}); fallback failed: {fallback}")
            });
        }
        let mut count = 0;
        for file in files {
            let key = stable_path_key(&file);
            let target_dir = local_root.join(key);
            if let Err(error) = fs::create_dir_all(&target_dir) {
                return self.load_from_directory(dir).map_err(|fallback| {
                    format!("skill store unavailable ({error}); fallback failed: {fallback}")
                });
            }
            let target = target_dir.join("SKILL.md");
            // Imports are idempotent across restarts. A managed file may be
            // opened by a concurrent read-only registry restore; avoid an
            // unnecessary overwrite (and the Windows sharing violation it
            // can cause) when the bytes are already identical.
            let already_stored = target.exists()
                && fs::read(&target).ok().as_deref() == fs::read(&file).ok().as_deref();
            if !already_stored {
                fs::copy(&file, &target)
                    .map_err(|e| format!("Failed to store skill {}: {e}", file.display()))?;
            }
            self.register_from_file(&target)?;
            count += 1;
        }
        Ok(count)
    }

    /// Restore all managed SKILL.md files at process startup. This is kept
    /// separate from imports so only data copied/downloaded into AngelBot's
    /// local store becomes durable runtime capability.
    pub fn load_managed_skills(&mut self) -> Result<usize, String> {
        let root = managed_skill_store();
        let files = find_skill_files(&root)?;
        let mut count = 0;
        for file in files {
            if self.register_from_file(&file).is_ok() {
                count += 1;
            }
        }
        Ok(count)
    }

    /// Return a bounded, text-only description of installed skills for the
    /// system prompt. Source instructions are explicitly treated as untrusted
    /// data by the prompt layer and never executed by the registry.
    pub fn prompt_instructions(&self) -> Vec<crate::agent::config::SkillPromptInstruction> {
        const MAX_SKILLS: usize = 12;
        const MAX_PER_SKILL_CHARS: usize = 4_000;
        const MAX_TOTAL_CHARS: usize = 12_000;
        let mut entries = self.skills.values().collect::<Vec<_>>();
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        let mut remaining = MAX_TOTAL_CHARS;
        entries
            .into_iter()
            .take(MAX_SKILLS)
            .filter_map(|manifest| {
                let source = self.source_files.get(&manifest.id)?;
                let raw = fs::read_to_string(source).ok()?;
                let instructions = raw
                    .chars()
                    .take(MAX_PER_SKILL_CHARS.min(remaining))
                    .collect::<String>();
                if instructions.trim().is_empty() {
                    return None;
                }
                remaining = remaining.saturating_sub(instructions.chars().count());
                Some(crate::agent::config::SkillPromptInstruction {
                    id: manifest.id.clone(),
                    name: manifest.name.clone(),
                    description: manifest.description.clone(),
                    instructions,
                })
            })
            .collect()
    }

    /// Clone or update a public GitHub repository in AngelBot's local store,
    /// then import common Codex, Claude, Cursor, and SKILL.md templates.
    pub fn import_github_repository(&mut self, repository_url: &str) -> Result<usize, String> {
        let (owner, repository) = parse_github_repository(repository_url)?;
        let store = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("angelbot")
            .join("skills")
            .join(&owner)
            .join(&repository);
        let parent = store.parent().ok_or("Invalid skill storage path")?;
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create skill storage: {e}"))?;

        let output = if store.exists() {
            Command::new("git")
                .args(["-C", store.to_string_lossy().as_ref(), "pull", "--ff-only"])
                .output()
        } else {
            Command::new("git")
                .args([
                    "clone",
                    "--depth",
                    "1",
                    repository_url,
                    store.to_string_lossy().as_ref(),
                ])
                .output()
        }
        .map_err(|e| format!("Unable to run git: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "GitHub import failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }

        self.load_templates_recursively(&store)
    }

    fn load_templates_recursively(&mut self, dir: &Path) -> Result<usize, String> {
        let mut count = 0;
        let mut pending = vec![dir.to_path_buf()];
        while let Some(current) = pending.pop() {
            for entry in fs::read_dir(&current).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                let name = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default();
                let is_template = matches!(
                    name,
                    "SKILL.md" | "CLAUDE.md" | "AGENTS.md" | ".cursorrules"
                ) || name.ends_with(".mdc");
                if is_template && self.register_from_file(&path).is_ok() {
                    count += 1;
                }
            }
        }
        Ok(count)
    }
}

fn managed_skill_store() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("angelbot")
        .join("skills")
}

fn stable_path_key(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    hex::encode(hasher.finalize())[..16].to_string()
}

fn find_skill_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(&current).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
                files.push(path);
            }
        }
    }
    Ok(files)
}

fn parse_github_repository(repository_url: &str) -> Result<(String, String), String> {
    let trimmed = repository_url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let path = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("git@github.com:"))
        .ok_or("Only public GitHub repository URLs are supported")?;
    let mut segments = path.split('/');
    let owner = segments
        .next()
        .filter(|value| !value.is_empty())
        .ok_or("GitHub URL is missing an owner")?;
    let repository = segments
        .next()
        .filter(|value| !value.is_empty())
        .ok_or("GitHub URL is missing a repository")?;
    if segments.next().is_some()
        || !owner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        || !repository
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err("GitHub URL must point to a repository root".to_string());
    }
    Ok((owner.to_string(), repository.to_string()))
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Skill invocation context
#[derive(Debug, Clone)]
pub struct SkillContext {
    pub skill_id: String,
    pub action: String,
    pub parameters: HashMap<String, serde_json::Value>,
}

impl SkillContext {
    pub fn new(skill_id: String, action: String) -> Self {
        Self {
            skill_id,
            action,
            parameters: HashMap::new(),
        }
    }

    pub fn with_param(mut self, name: String, value: serde_json::Value) -> Self {
        self.parameters.insert(name, value);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_skill_manifest() {
        let content = r#"# Test Skill
Description: A test skill for unit testing

Version: 1.0.0
Author: Test Author

## Actions
- name: test_action
  description: Test action description
  handler: test_handler
  parameters:
    - name: input
      type: string
      required: true
"#;

        let manifest = SkillManifest::from_markdown(content).unwrap();
        assert_eq!(manifest.name, "Test Skill");
        assert_eq!(manifest.description, "A test skill for unit testing");
        assert_eq!(manifest.version, "1.0.0");
        assert_eq!(manifest.author, Some("Test Author".to_string()));
        assert_eq!(manifest.actions.len(), 1);
        assert_eq!(manifest.actions[0].name, "test_action");
    }

    #[test]
    fn test_skill_registry() {
        let mut registry = SkillRegistry::new();

        let manifest = SkillManifest {
            id: "test-skill".to_string(),
            name: "Test Skill".to_string(),
            description: "Test".to_string(),
            version: "1.0.0".to_string(),
            author: None,
            actions: vec![],
            dependencies: vec![],
            permissions: vec![],
            prompts: vec![],
        };

        registry.register(manifest);

        assert!(registry.get("test-skill").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn registered_skill_exposes_bounded_text_guidance_not_an_executable_action() {
        let directory = tempfile::tempdir().unwrap();
        let skill_file = directory.path().join("SKILL.md");
        fs::write(
            &skill_file,
            "# Review Skill\nDescription: Review a change carefully\n\nUse the read_file tool before reporting findings.\n",
        )
        .unwrap();

        let mut registry = SkillRegistry::new();
        registry.register_from_file(&skill_file).unwrap();
        let instructions = registry.prompt_instructions();

        assert_eq!(instructions.len(), 1);
        assert_eq!(instructions[0].id, "review-skill");
        assert!(instructions[0].instructions.contains("read_file tool"));
        assert!(!registry.has_action("review-skill", "read_file"));
    }

    #[test]
    fn find_skill_files_ignores_non_skill_markdown() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("README.md"), "# Not a skill").unwrap();
        fs::create_dir_all(directory.path().join("nested")).unwrap();
        fs::write(
            directory.path().join("nested").join("SKILL.md"),
            "# Real Skill",
        )
        .unwrap();

        let files = find_skill_files(directory.path()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name().unwrap(), "SKILL.md");
    }

    #[test]
    fn imports_common_template_filenames_recursively() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join(".cursor").join("rules");
        fs::create_dir_all(&nested).unwrap();
        fs::write(
            directory.path().join("CLAUDE.md"),
            "# Claude Template\nDescription: Claude instructions",
        )
        .unwrap();
        fs::write(
            nested.join("frontend.mdc"),
            "# Cursor Template\nDescription: Cursor instructions",
        )
        .unwrap();

        let mut registry = SkillRegistry::new();
        assert_eq!(
            registry
                .load_templates_recursively(directory.path())
                .unwrap(),
            2
        );
        assert!(registry.get("claude-template").is_some());
        assert!(registry.get("cursor-template").is_some());
    }

    #[test]
    fn accepts_only_github_repository_roots() {
        assert_eq!(
            parse_github_repository("https://github.com/openai/skills.git").unwrap(),
            ("openai".to_string(), "skills".to_string())
        );
        assert!(parse_github_repository("https://example.com/openai/skills").is_err());
        assert!(parse_github_repository("https://github.com/openai/skills/tree/main").is_err());
    }
}

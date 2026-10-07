//! Agent tool extensions - modular tool sets loaded on demand
//!
//! Issue #095: Extensions provide isolated tool groups that can be loaded
//! independently of core tools.
//!
//! Extension categories:
//! - memory: Long-term memory management (remember, recall, update, forget)
//! - goal: Goal tracking (create, update_progress, complete)
//! - knowledge: Knowledge graph traversal and entity extraction
//! - constraint: Constraint extraction and checking
//! - skill: GitHub skill import

pub mod constraint;
pub mod goal;
pub mod knowledge;
pub mod memory;
pub mod skill;
pub mod web_search;

use crate::agent::tool::ToolCategory;

/// Extension identifier for registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extension {
    Memory,
    Goal,
    Knowledge,
    Constraint,
    Skill,
}

impl Extension {
    /// Returns the tool category for this extension's tools.
    pub fn category(&self) -> ToolCategory {
        ToolCategory::Extension
    }

    /// Returns all available extensions.
    pub fn all() -> &'static [Extension] {
        &[
            Extension::Memory,
            Extension::Goal,
            Extension::Knowledge,
            Extension::Constraint,
            Extension::Skill,
        ]
    }
}

impl std::fmt::Display for Extension {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Extension::Memory => write!(f, "memory"),
            Extension::Goal => write!(f, "goal"),
            Extension::Knowledge => write!(f, "knowledge"),
            Extension::Constraint => write!(f, "constraint"),
            Extension::Skill => write!(f, "skill"),
        }
    }
}

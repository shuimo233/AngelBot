//! Tauri command modules
//!
//! Architecture: each domain gets its own file.
//! Shared types are re-exported here.

#![allow(unused_imports)]

pub mod activity;
pub mod agent_event;
pub mod api_config;
pub mod automation;
pub mod branch;
pub mod channel;
pub(crate) mod delegated_delivery_follow_up;
pub mod desktop;
pub mod file;
pub(crate) mod foreground_automation_control;
pub(crate) mod foreground_automation_pump;
pub(crate) mod foreground_context_assembler;
pub(crate) mod foreground_delegation_control;
pub(crate) mod foreground_history;
pub(crate) mod foreground_message_contracts;
pub(crate) mod foreground_model_factory;
pub(crate) mod foreground_response_projector;
pub(crate) mod foreground_run_store;
pub(crate) mod foreground_task_understanding_control;
pub(crate) mod foreground_text_attachments;
pub(crate) mod foreground_tool_surface;
pub(crate) mod foreground_turn_coordinator;
pub mod mcp;
pub mod memory;
pub mod message;
pub mod model_connection;
pub mod proactive_tests;
pub mod project_network_approval;
pub mod runtime_health;
pub mod search;
pub mod session;
pub mod session_tree;
pub mod settings;
pub mod skill;
pub mod steering;
pub(crate) mod task_understanding_view;
pub mod usage;
pub mod web_search;
pub mod workspace;
pub mod workspace_activity;

pub use agent_event::{get_agent_events, get_agent_run_events};
pub use branch::{
    create_session_branch, delete_branch, get_all_branches, get_session_branch_tree,
    get_session_branches, switch_to_branch, BranchInfo, SessionWithBranch,
};
pub use desktop::{
    confirm_desktop_observation_scope, delete_desktop_trusted_app,
    get_desktop_trusted_app_statuses, get_desktop_trusted_apps, save_desktop_trusted_app,
};
pub use file::{
    get_work_dir, read_file, resolve_and_validate, set_work_dir, set_work_dir_impl, FileReadError,
};
pub use foreground_message_contracts::{
    AgentTaskRun, EditAndResendMessageRequest, Message, PersonalityPayload, PreferencesPayload,
    SendMessageRequest, ToolCallInfo, ToolResultInfo, TraitsPayload, WebSearchSetupPayload,
};
pub use mcp::*;
pub use memory::*;
pub use message::{call_deepseek, get_messages, resolve_agent_confirmation, send_message};
pub use session::{create_session, delete_session, get_sessions, update_session_title, Session};
pub use session_tree::{get_branch_tree, switch_to_message_branch};
pub use settings::*;
pub use skill::{
    get_skill, get_skill_ids, get_skills, load_skills, register_skill, skill_has_action, SkillState,
};
pub use steering::{
    cancel_instruction, clear_instructions, enqueue_instruction, get_agent_state,
    get_pending_instructions, interrupt_agent, pause_agent, resume_agent, should_agent_yield,
    submit_in_progress_command, SteeringState,
};
pub use usage::{
    get_cost_summary, get_session_usage, get_usage_history, get_usage_last_n_days, record_usage,
};
pub use workspace::{
    create_project_workspace, get_workspaces, open_workspace, OpenWorkspace, Workspace,
};

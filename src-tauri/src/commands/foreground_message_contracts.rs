//! Neutral foreground message/request contracts.
//!
//! These types are shared by the Tauri command adapter, transcript storage,
//! history projection, and turn coordinator.  Keeping them here prevents the
//! command module from becoming a dependency hub while preserving the JSON
//! shape exposed to the existing frontend.

use crate::agent::task_facts::TaskFacts;
pub use crate::commands::foreground_text_attachments::TextAttachment;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallInfo {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultInfo {
    #[serde(rename = "callId")]
    pub call_id: String,
    #[serde(rename = "toolName")]
    pub tool_name: String,
    pub success: bool,
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(rename = "confirmationRequired", default)]
    pub confirmation_required: bool,
    #[serde(rename = "confirmationStatus", skip_serializing_if = "Option::is_none")]
    pub confirmation_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTaskRun {
    pub id: String,
    pub goal: String,
    pub status: String,
    pub plan: Vec<String>,
    #[serde(rename = "confirmationState")]
    pub confirmation_state: String,
    pub resumable: bool,
    #[serde(rename = "stepCount")]
    pub step_count: usize,
    #[serde(rename = "completedStepCount")]
    pub completed_step_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub role: String,
    pub content: String,
    #[serde(
        default,
        rename = "textAttachments",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub text_attachments: Vec<TextAttachment>,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none", rename = "parentId")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "isDeleted")]
    pub is_deleted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "toolCalls")]
    pub tool_calls: Option<Vec<ToolCallInfo>>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "toolResults")]
    pub tool_results: Option<Vec<ToolResultInfo>>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "taskRun")]
    pub task_run: Option<AgentTaskRun>,
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "taskFacts")]
    pub task_facts: Option<TaskFacts>,
}

#[derive(Debug, Deserialize)]
pub struct WebSearchSetupPayload {
    pub provider: String,
    #[serde(rename = "apiKey")]
    pub api_key: String,
    #[serde(default)]
    pub endpoint: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PersonalityPayload {
    pub id: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub greeting: Option<String>,
    #[serde(default)]
    pub traits: Option<TraitsPayload>,
}

#[derive(Debug, Deserialize)]
pub struct TraitsPayload {
    #[serde(default)]
    pub tone: i32,
    #[serde(default)]
    pub verbosity: i32,
    #[serde(default)]
    pub formality: i32,
    #[serde(default)]
    pub humor: i32,
    #[serde(default)]
    pub dependence: i32,
    #[serde(default)]
    pub intimacy: i32,
    #[serde(default)]
    pub patience: i32,
}

#[derive(Debug, Deserialize)]
pub struct PreferencesPayload {
    #[serde(default, rename = "responseLength")]
    pub response_length: Option<String>,
    #[serde(default, rename = "responseLanguage")]
    pub response_language: Option<String>,
    #[serde(default, rename = "useLongTermMemory")]
    pub use_long_term_memory: Option<bool>,
    #[serde(default)]
    pub interests: Option<Vec<String>>,
    #[serde(default, rename = "avoidTopics")]
    pub avoid_topics: Option<Vec<String>>,
    #[serde(default, rename = "dislikedWords")]
    pub disliked_words: Option<Vec<String>>,
    #[serde(default, rename = "petPeeves")]
    pub pet_peeves: Option<Vec<String>>,
    #[serde(default, rename = "evolutionEnabled")]
    pub evolution_enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(default, rename = "clientMessageId")]
    pub client_message_id: Option<String>,
    pub role: String,
    pub content: String,
    #[serde(default, rename = "textAttachments")]
    pub text_attachments: Vec<TextAttachment>,
    #[serde(default, rename = "personality")]
    pub personality: Option<PersonalityPayload>,
    #[serde(default, rename = "preferences")]
    pub preferences: Option<PreferencesPayload>,
    #[serde(default, rename = "thinkingEffort")]
    pub thinking_effort: Option<String>,
    #[serde(default, rename = "webSearchSetup")]
    pub web_search_setup: Option<WebSearchSetupPayload>,
}

#[derive(Debug, Deserialize)]
pub struct EditAndResendMessageRequest {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "messageId")]
    pub message_id: String,
    pub content: String,
    #[serde(default)]
    pub personality: Option<PersonalityPayload>,
    #[serde(default)]
    pub preferences: Option<PreferencesPayload>,
    #[serde(default, rename = "thinkingEffort")]
    pub thinking_effort: Option<String>,
}

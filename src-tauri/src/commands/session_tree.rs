//! Session Tree Branch API
//!
//! Issue #41: Session branches with tree structure
//!
//! Provides commands for non-destructive message editing using a tree structure.
//! A "branch" is a leaf node (message with no children). The active branch
//! is determined by the session's leaf_message_id.

use crate::AppState;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tauri::State;

/// Message node in the session tree
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageNode {
    pub id: String,
    pub role: String,
    pub content: String,
    pub parent_id: Option<String>,
    pub is_deleted: bool,
    pub created_at: i64,
}

/// Branch information - represents a leaf node (message with no children)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub id: String,
    pub message_id: String,
    pub name: Option<String>,
    pub is_active: bool,
}

/// Complete branch tree for a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchTree {
    pub branches: Vec<Branch>,
    pub messages: Vec<MessageNode>,
}

/// Format branch name from message content
fn format_branch_name(content: &str) -> String {
    let mut characters = content.chars();
    let mut preview: String = characters.by_ref().take(30).collect();
    if characters.next().is_some() {
        preview.push_str("...");
    }
    preview.replace('\n', " ").trim().to_string()
}

/// Get the full branch tree (all branches and messages) for a session
///
/// A "branch" is a leaf node (message with no children). The active branch
/// is the one whose message_id matches the session's leaf_message_id.
#[tauri::command]
pub fn get_branch_tree(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<BranchTree, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;

    // Get current leaf_message_id from the session
    let leaf_message_id: Option<String> = conn
        .query_row(
            "SELECT leaf_message_id FROM sessions WHERE id = ?1",
            params![session_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    // Get all messages for this session (hard-deleted messages are already gone).
    // Migration 036: is_deleted column removed; deleted branches no longer exist in DB.
    let mut stmt = conn
        .prepare(
            "SELECT id, role, content, parent_id, created_at
             FROM messages
             WHERE session_id = ?1
             ORDER BY created_at ASC, id ASC",
        )
        .map_err(|e| e.to_string())?;

    let messages: Vec<MessageNode> = stmt
        .query_map(params![session_id], |r| {
            Ok(MessageNode {
                id: r.get(0)?,
                role: r.get(1)?,
                content: r.get(2)?,
                parent_id: r.get(3)?,
                is_deleted: false,
                created_at: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    // Build a set of all message IDs that have children
    let mut child_parent_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for msg in &messages {
        if let Some(ref parent_id) = msg.parent_id {
            child_parent_ids.insert(parent_id.clone());
        }
    }

    // Branches are messages that are NOT referenced as parent_id by any other message
    let branches: Vec<Branch> = messages
        .iter()
        .filter(|msg| !child_parent_ids.contains(&msg.id))
        .map(|msg| {
            let is_active = leaf_message_id
                .as_ref()
                .map(|leaf| leaf == &msg.id)
                .unwrap_or(false);
            Branch {
                id: msg.id.clone(),
                message_id: msg.id.clone(),
                name: Some(format_branch_name(&msg.content)),
                is_active,
            }
        })
        .collect();

    Ok(BranchTree { branches, messages })
}

/// Switch to a different branch by changing the session's leaf_message_id
///
/// Validates that the target message exists, belongs to the session, and is not deleted.
#[tauri::command]
pub fn switch_to_message_branch(
    state: State<'_, AppState>,
    session_id: String,
    branch_message_id: String,
) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;

    // Validate the message exists and belongs to the session. Migration 036 removed
    // is_deleted: deleted messages are physically gone, so existence check is enough.
    let stored_session_id: String = conn
        .query_row(
            "SELECT session_id FROM messages WHERE id = ?1",
            params![branch_message_id],
            |r| r.get(0),
        )
        .map_err(|_| "Message not found".to_string())?;

    if stored_session_id != session_id {
        return Err("Message does not belong to this session".to_string());
    }

    // Update the session's leaf_message_id
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE sessions SET leaf_message_id = ?1, updated_at = ?2 WHERE id = ?3",
        params![branch_message_id, now, session_id],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_branch_name_short() {
        let result = format_branch_name("Hello world");
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn test_format_branch_name_long() {
        let long_content = "This is a very long message that exceeds thirty characters";
        let result = format_branch_name(long_content);
        assert_eq!(result.len(), 33); // 30 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_format_branch_name_truncates_unicode_at_character_boundaries() {
        let content = "1234567890123456789012345678重构后的标题";
        let expected = format!("{}...", content.chars().take(30).collect::<String>());

        assert_eq!(format_branch_name(content), expected);
    }

    #[test]
    fn test_format_branch_name_newlines() {
        let content = "Hello\nWorld\nTest";
        let result = format_branch_name(content);
        assert!(!result.contains('\n'));
        assert_eq!(result, "Hello World Test");
    }

    #[test]
    fn test_message_node_serialization() {
        let node = MessageNode {
            id: "msg-123".to_string(),
            role: "user".to_string(),
            content: "Hello".to_string(),
            parent_id: Some("msg-122".to_string()),
            is_deleted: false,
            created_at: 1234567890,
        };

        let json = serde_json::to_string(&node).unwrap();
        assert!(json.contains("msg-123"));
        assert!(json.contains("user"));
        assert!(json.contains("Hello"));
    }

    #[test]
    fn test_branch_serialization() {
        let branch = Branch {
            id: "branch-1".to_string(),
            message_id: "msg-123".to_string(),
            name: Some("Hello...".to_string()),
            is_active: true,
        };

        let json = serde_json::to_string(&branch).unwrap();
        assert!(json.contains("branch-1"));
        assert!(json.contains("msg-123"));
        assert!(json.contains("Hello"));
        assert!(json.contains("is_active"));
    }

    #[test]
    fn test_branch_tree_serialization() {
        let tree = BranchTree {
            branches: vec![Branch {
                id: "b1".to_string(),
                message_id: "m1".to_string(),
                name: Some("Branch 1".to_string()),
                is_active: true,
            }],
            messages: vec![MessageNode {
                id: "m1".to_string(),
                role: "user".to_string(),
                content: "Test".to_string(),
                parent_id: None,
                is_deleted: false,
                created_at: 123,
            }],
        };

        let json = serde_json::to_string(&tree).unwrap();
        assert!(json.contains("branches"));
        assert!(json.contains("messages"));
        assert!(json.contains("m1"));
    }
}

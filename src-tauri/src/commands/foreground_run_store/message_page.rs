//! Bounded public projection of the active conversation branch.

use super::{read_projected_message, session_leaf_id, Message};
use crate::commands::foreground_history::{
    hydrate_paged_agent_steps, hydrate_task_facts_from_db, hydrate_task_runs_from_db,
    summarize_task_run,
};
use crate::AppState;
use rusqlite::{params, Connection, OptionalExtension, Rows};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use tauri::State;

const PAGE_SIZE: usize = 80;
pub(super) const ANCESTRY: &str = "WITH RECURSIVE ancestry(id, parent_id, role) AS (
    SELECT id, parent_id, role FROM messages
    WHERE id = ?1 AND session_id = ?2 AND role != 'tool' AND is_provisional = 0
    UNION
    SELECT m.id, m.parent_id, m.role
    FROM ancestry a JOIN messages m ON m.id = a.parent_id
    WHERE m.session_id = ?2 AND m.role != 'tool' AND m.is_provisional = 0
      AND (NOT ?3 OR a.role != 'user')
)";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MessagePageRequest {
    session_id: String,
    before_id: Option<String>,
    after_id: Option<String>,
    newer_id: Option<String>,
    around_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MessagePage {
    messages: Vec<Message>,
    has_older: bool,
    has_newer: bool,
    reset: bool,
}

#[derive(Clone, Copy)]
enum Cursor<'a> {
    Latest,
    Before(&'a str),
    After(&'a str),
    Newer(&'a str),
    Around(&'a str),
}

impl<'a> Cursor<'a> {
    fn id(self) -> Option<&'a str> {
        match self {
            Self::Latest => None,
            Self::Before(id) | Self::After(id) | Self::Newer(id) | Self::Around(id) => Some(id),
        }
    }
}

#[tauri::command]
pub(crate) fn get_message_page(
    state: State<'_, AppState>,
    session_id: String,
    before_id: Option<String>,
    after_id: Option<String>,
    newer_id: Option<String>,
    around_id: Option<String>,
) -> Result<MessagePage, String> {
    get_message_page_impl(
        &state,
        MessagePageRequest {
            session_id,
            before_id,
            after_id,
            newer_id,
            around_id,
        },
    )
}

pub(crate) fn get_message_page_impl(
    state: &AppState,
    request: MessagePageRequest,
) -> Result<MessagePage, String> {
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    let tx = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let page = project_page(&tx, &request)?;
    tx.commit().map_err(|error| error.to_string())?;
    Ok(page)
}

fn project_page(conn: &Connection, request: &MessagePageRequest) -> Result<MessagePage, String> {
    let cursor = match (
        request.before_id.as_deref(),
        request.after_id.as_deref(),
        request.newer_id.as_deref(),
        request.around_id.as_deref(),
    ) {
        (None, None, None, None) => Cursor::Latest,
        (Some(id), None, None, None) => Cursor::Before(id),
        (None, Some(id), None, None) => Cursor::After(id),
        (None, None, Some(id), None) => Cursor::Newer(id),
        (None, None, None, Some(id)) => Cursor::Around(id),
        _ => return Err("消息分页每次只接受一个游标。".to_owned()),
    };
    let session_id = &request.session_id;
    let leaf = session_leaf_id(conn, session_id)?;
    let (ids, mut page) =
        select_ids(conn, session_id, leaf.as_deref(), cursor).map_err(|error| error.to_string())?;
    if ids.is_empty() {
        return Ok(page);
    }

    let ids_json = serde_json::to_string(&ids).map_err(|error| error.to_string())?;
    let mut stmt = conn
        .prepare_cached(
            "SELECT m.id, m.session_id, m.role, m.content, m.created_at, m.parent_id,
                    m.tool_calls, m.metadata
             FROM json_each(?1) ids CROSS JOIN messages m ON m.id = ids.value
             WHERE m.session_id = ?2 ORDER BY ids.key DESC",
        )
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map(params![ids_json, session_id], read_projected_message)
        .map_err(|error| error.to_string())?;
    page.messages = rows
        .collect::<Result<_, _>>()
        .map_err(|error| error.to_string())?;
    hydrate_paged_agent_steps(conn, &mut page.messages)?;
    hydrate_task_runs_from_db(conn, &mut page.messages)?;
    hydrate_older_goal(conn, session_id, &mut page.messages)?;
    hydrate_task_facts_from_db(conn, &mut page.messages)?;
    Ok(page)
}

fn select_ids(
    conn: &Connection,
    session_id: &str,
    leaf: Option<&str>,
    cursor: Cursor<'_>,
) -> rusqlite::Result<(Vec<String>, MessagePage)> {
    // Each recursive step has at most one parent, so the stream runs from the
    // leaf toward the root regardless of timestamps or message IDs. UNION also
    // stops a malformed cycle without retaining any message bodies.
    let mut stmt = conn.prepare_cached(&format!("{ANCESTRY} SELECT id FROM ancestry"))?;
    let mut rows = stmt.query(params![leaf, session_id, false])?;
    let mut latest = Vec::with_capacity(PAGE_SIZE + 1);
    let mut newer = VecDeque::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        if cursor.id() == Some(id.as_str()) {
            return match cursor {
                Cursor::Before(_) => {
                    let (older, has_older) = take_ids(&mut rows, PAGE_SIZE)?;
                    Ok((older, message_page(has_older, true, false)))
                }
                Cursor::After(_) => Ok((latest, message_page(true, false, false))),
                Cursor::Newer(_) => {
                    let has_newer = newer.len() > PAGE_SIZE;
                    if has_newer {
                        newer.pop_front();
                    }
                    Ok((newer.into(), message_page(true, has_newer, false)))
                }
                Cursor::Around(_) => around_ids(&mut rows, newer, id),
                Cursor::Latest => unreachable!("the latest page has no cursor"),
            };
        }
        if latest.len() <= PAGE_SIZE {
            latest.push(id.clone());
        }
        if latest.len() > PAGE_SIZE && matches!(cursor, Cursor::Latest | Cursor::After(_)) {
            break;
        }
        if matches!(cursor, Cursor::Newer(_) | Cursor::Around(_)) {
            // Preserve the closest newer neighbors and one extra ID to
            // distinguish a full page from a traversal that reached the leaf.
            if newer.len() > PAGE_SIZE {
                newer.pop_front();
            }
            newer.push_back(id);
        }
    }

    let has_older = latest.len() > PAGE_SIZE;
    latest.truncate(PAGE_SIZE);
    Ok((
        latest,
        message_page(has_older, false, cursor.id().is_some()),
    ))
}

fn take_ids(rows: &mut Rows<'_>, limit: usize) -> rusqlite::Result<(Vec<String>, bool)> {
    let mut ids = Vec::with_capacity(limit);
    for _ in 0..limit {
        let Some(row) = rows.next()? else {
            return Ok((ids, false));
        };
        ids.push(row.get(0)?);
    }
    Ok((ids, rows.next()?.is_some()))
}

fn around_ids(
    rows: &mut Rows<'_>,
    mut newer: VecDeque<String>,
    id: String,
) -> rusqlite::Result<(Vec<String>, MessagePage)> {
    let preferred_newer = newer.len().min((PAGE_SIZE - 1) / 2);
    let (older, has_older) = take_ids(rows, PAGE_SIZE - 1 - preferred_newer)?;
    // Near the root, fill the remaining space with more recent neighbors.
    let skip = newer.len().saturating_sub(PAGE_SIZE - 1 - older.len());
    newer.drain(..skip);
    newer.push_back(id);
    newer.extend(older);
    Ok((newer.into(), message_page(has_older, skip > 0, false)))
}

fn message_page(has_older: bool, has_newer: bool, reset: bool) -> MessagePage {
    MessagePage {
        messages: Vec::new(),
        has_older,
        has_newer,
        reset,
    }
}

fn hydrate_older_goal(
    conn: &Connection,
    session_id: &str,
    messages: &mut [Message],
) -> Result<(), String> {
    // Older assistant rows can have calls and receipts without a durable task
    // run. Only the leading assistants can need a goal outside this page.
    let needs_goal = |message: &Message| {
        message.role == "assistant" && message.task_run.is_none() && message.tool_calls.is_some()
    };
    let leading: &mut [Message] = {
        let end = messages
            .iter()
            .position(|message| message.role == "user")
            .unwrap_or(messages.len());
        &mut messages[..end]
    };
    if !leading.iter().any(needs_goal) {
        return Ok(());
    }
    let goal: Option<String> = conn
        .query_row(
            &format!(
                "{ANCESTRY}
                SELECT m.content FROM ancestry a JOIN messages m ON m.id = a.id
                WHERE a.role = 'user' LIMIT 1"
            ),
            params![leading[0].parent_id.as_deref(), session_id, true],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if let Some(goal) = goal {
        for message in leading.iter_mut().filter(|message| needs_goal(message)) {
            message.task_run = summarize_task_run(message, goal.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

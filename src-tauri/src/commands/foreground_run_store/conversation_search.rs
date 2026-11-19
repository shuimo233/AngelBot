//! Literal search over the public, active conversation branch.

use super::{message_page::ANCESTRY, session_leaf_id};
use crate::AppState;
use regex::{Regex, RegexBuilder};
use rusqlite::{params, CachedStatement, Connection, Row};
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
use tauri::State;

const PAGE_SIZE: usize = 20;
const PREVIEW_SIZE: usize = 156;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConversationSearchRequest {
    session_id: String,
    query: String,
    before_id: Option<String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConversationSearchPage {
    results: Vec<ConversationSearchHit>,
    next_before_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationSearchHit {
    id: String,
    text: String,
    role: SearchRole,
    created_at: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum SearchRole {
    User,
    Assistant,
}

#[tauri::command]
pub(crate) async fn search_conversation_page(
    state: State<'_, AppState>,
    session_id: String,
    query: String,
    before_id: Option<String>,
) -> Result<ConversationSearchPage, String> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || {
        search_conversation_page_impl(
            &db,
            ConversationSearchRequest {
                session_id,
                query,
                before_id,
            },
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(crate) fn search_conversation_page_impl(
    db: &Mutex<Connection>,
    request: ConversationSearchRequest,
) -> Result<ConversationSearchPage, String> {
    let query = request.query.trim();
    if query.is_empty() {
        return Ok(ConversationSearchPage::default());
    }
    // Token and prefix indexes omit infix hits, including continuous Han text.
    let literal = RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .build()
        .map_err(|_| "搜索文字过长。".to_owned())?;
    let conn = db.lock().map_err(|error| error.to_string())?;
    let tx = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let leaf = session_leaf_id(&tx, &request.session_id)?;
    let page = search_branch(&tx, &request, leaf.as_deref(), &literal)?;
    tx.commit().map_err(|error| error.to_string())?;
    Ok(page)
}

fn search_branch(
    conn: &Connection,
    request: &ConversationSearchRequest,
    leaf: Option<&str>,
    literal: &Regex,
) -> Result<ConversationSearchPage, String> {
    let session = &request.session_id;
    let mut ancestry = conn
        .prepare_cached(&format!("{ANCESTRY} SELECT id, role FROM ancestry"))
        .map_err(|error| error.to_string())?;
    let mut rows = ancestry
        .query(params![leaf, session, false])
        .map_err(|error| error.to_string())?;
    let mut sources = SearchSources::new(conn).map_err(|error| error.to_string())?;
    let mut groups = Vec::with_capacity(PAGE_SIZE + 1);
    let mut replies = Vec::new();
    let mut before = request.before_id.as_deref();
    while let Some(row) = rows.next().map_err(|error| error.to_string())? {
        let id: String = row.get(0).map_err(|error| error.to_string())?;
        if let Some(cursor) = before {
            if id == cursor {
                before = None;
            }
            continue;
        }
        let role: String = row.get(1).map_err(|error| error.to_string())?;
        let is_reply = role == "assistant";
        // A normal turn needs only its first matching reply in this order.
        // Keep up to one page of replies in case this is an assistant-only
        // prefix, where each reply stands on its own.
        if is_reply && groups.len() + replies.len() > PAGE_SIZE {
            continue;
        }
        let hit = sources
            .find_hit(session, &id, &role, literal)
            .map_err(|error| error.to_string())?;
        if is_reply {
            replies.extend(hit);
        } else {
            if let Some(hit) = hit.or_else(|| replies.drain(..).next()) {
                groups.push((id, hit));
            }
            replies.clear();
            if groups.len() > PAGE_SIZE {
                break;
            }
        }
    }
    if before.is_some() {
        return Err("会话已变化，请重新搜索。".to_owned());
    }
    groups.extend(replies.into_iter().map(|hit| (hit.id.clone(), hit)));
    let has_more = groups.len() > PAGE_SIZE;
    groups.truncate(PAGE_SIZE);
    // The cursor is the older edge of a turn. The hit can be a newer reply,
    // which remains its own jump target.
    let next_before_id = has_more.then(|| groups.last().unwrap().0.clone());
    let results = groups.into_iter().map(|(_, hit)| hit).collect();
    Ok(ConversationSearchPage {
        results,
        next_before_id,
    })
}

struct SearchSources<'a> {
    message: CachedStatement<'a>,
    attachments: CachedStatement<'a>,
    files: CachedStatement<'a>,
    timeline: CachedStatement<'a>,
}

impl<'a> SearchSources<'a> {
    fn new(conn: &'a Connection) -> rusqlite::Result<Self> {
        Ok(Self {
            message: conn.prepare_cached(
                "SELECT content, created_at, metadata FROM messages WHERE id = ?1 AND session_id = ?2",
            )?,
            attachments: conn.prepare_cached(
                "SELECT CASE WHEN type = 'object' THEN value ->> '$.name' END
                 FROM json_each(CASE WHEN json_valid(?1) THEN ?1 END, '$.textAttachments')",
            )?,
            files: conn.prepare_cached(
                "SELECT path.value FROM task_run_facts f
                 CROSS JOIN json_each(f.facts_json, '$.modified_files') path
                 WHERE f.session_id = ?1 AND f.message_id = ?2 AND path.type = 'text'",
            )?,
            timeline: conn.prepare_cached(
                "SELECT payload ->> '$.data.block.data.content' FROM agent_run_events
                 WHERE run_id = ?1 AND session_id = ?2 AND event_type = 'ContentBlock'
                   AND CASE WHEN json_valid(payload) THEN payload ->> '$.data.block.kind' END = 'Text'
                 ORDER BY sequence",
            )?,
        })
    }

    fn find_hit(
        &mut self,
        session: &str,
        id: &str,
        role: &str,
        literal: &Regex,
    ) -> rusqlite::Result<Option<ConversationSearchHit>> {
        let Self {
            message,
            attachments,
            files,
            timeline,
        } = self;
        let mut rows = message.query(params![id, session])?;
        let row = rows.next()?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        let make_hit = |text| {
            Ok(Some(ConversationSearchHit {
                id: id.to_owned(),
                text,
                role: if role == "user" {
                    SearchRole::User
                } else {
                    SearchRole::Assistant
                },
                created_at: row.get(1)?,
            }))
        };
        if let Some(metadata) = text_column(row, 2)? {
            if let Some(name) = search_names(attachments.query([metadata])?, literal)? {
                return make_hit(name);
            }
        }
        if role == "assistant" {
            if let Some(path) = search_names(files.query(params![session, id])?, literal)? {
                return make_hit(path);
            }
        }
        let content = text_column(row, 0)?.unwrap_or_default();
        if let Some(path) = search_citations(content, literal) {
            return make_hit(path);
        }
        let mut body = literal.find(content).map(|at| preview(content, at.start()));
        if role == "assistant" {
            let mut blocks = timeline.query(params![id, session])?;
            // Query only indexed run rows and extract the visible text field.
            // Tool output, plans and attachment bodies never enter the results.
            while let Some(block) = blocks.next()? {
                let Some(text) = text_column(block, 0)? else {
                    continue;
                };
                if let Some(path) = search_citations(text, literal) {
                    return make_hit(path);
                }
                if body.is_none() {
                    body = literal.find(text).map(|at| preview(text, at.start()));
                }
            }
        }
        match body {
            Some(text) => make_hit(text),
            None => Ok(None),
        }
    }
}

fn text_column<'a>(row: &'a Row<'_>, index: usize) -> rusqlite::Result<Option<&'a str>> {
    let value = row.get_ref(index)?;
    value.as_str_or_null().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, value.data_type(), Box::new(error))
    })
}

fn search_names(mut rows: rusqlite::Rows<'_>, literal: &Regex) -> rusqlite::Result<Option<String>> {
    while let Some(row) = rows.next()? {
        if let Some(value) = text_column(row, 0)? {
            if let Some(at) = literal.find(value) {
                return Ok(Some(filename_preview(value, at.start())));
            }
        }
    }
    Ok(None)
}

fn search_citations(text: &str, literal: &Regex) -> Option<String> {
    // Ordinary prose need not go through the Markdown matcher's captures.
    const SCHEME: &[u8] = b"angelbot-file";
    if !text.match_indices(':').any(|(offset, _)| {
        offset
            .checked_sub(SCHEME.len())
            .is_some_and(|start| text.as_bytes()[start..offset].eq_ignore_ascii_case(SCHEME))
    }) {
        return None;
    }
    static CITATIONS: OnceLock<Regex> = OnceLock::new();
    let citations = CITATIONS
        .get_or_init(|| Regex::new(r"(?i)(?:\]\(\s*<?|<)(angelbot-file:[^\s<>()]+)").unwrap());
    for matched in citations.captures_iter(text) {
        if let Some(path) = citation_path(&matched[1]) {
            if let Some(at) = literal.find(&path) {
                return Some(filename_preview(&path, at.start()));
            }
        }
    }
    None
}

fn citation_path(href: &str) -> Option<String> {
    let url = reqwest::Url::parse(href).ok()?;
    if url.host().is_some() || url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    let mut encoded = url.path().as_bytes();
    let mut decoded = Vec::with_capacity(encoded.len());
    while let Some((&byte, rest)) = encoded.split_first() {
        encoded = rest;
        if byte == b'%' {
            let mut decoded_byte = [0];
            hex::decode_to_slice(encoded.get(..2)?, &mut decoded_byte).ok()?;
            decoded.extend(decoded_byte);
            encoded = &encoded[2..];
        } else {
            decoded.push(byte);
        }
    }
    let path = String::from_utf8(decoded).ok()?;
    let valid = !path
        .bytes()
        .any(|byte| matches!(byte, b'\\' | b':' | 0..=31 | 127))
        && !path.split('/').any(|part| matches!(part, "" | "." | ".."));
    valid.then_some(path)
}

fn filename_preview(value: &str, offset: usize) -> String {
    if value.chars().nth(PREVIEW_SIZE).is_none() {
        value.to_owned()
    } else {
        preview(value, offset)
    }
}

fn preview(text: &str, offset: usize) -> String {
    static HEADINGS: OnceLock<Regex> = OnceLock::new();
    let headings = HEADINGS.get_or_init(|| Regex::new(r"(?m)^[ \t]{0,3}#{1,6}[ \t]+").unwrap());
    let start = text[..offset]
        .char_indices()
        .rev()
        .nth(27)
        .map_or(0, |(index, _)| index);
    let text: String = text[start..]
        .trim_start()
        .chars()
        .take(PREVIEW_SIZE * 4)
        .collect();
    let text = headings.replace_all(&text, "");
    let mut snippet = String::new();
    if start > 0 {
        snippet.push('…');
    }
    snippet.extend(
        text.split_whitespace()
            .enumerate()
            .flat_map(|(index, word)| (index > 0).then_some(' ').into_iter().chain(word.chars()))
            .take(PREVIEW_SIZE - usize::from(start > 0)),
    );
    snippet.trim_end().to_owned()
}

#[cfg(test)]
mod tests;

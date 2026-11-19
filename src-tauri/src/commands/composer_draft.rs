use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::State;

use crate::AppState;

#[derive(Debug, Serialize)]
pub struct ComposerDraft {
    text: String,
    revision: i64,
}

fn has_durable_storage(conn: &Connection) -> bool {
    conn.path().is_some_and(|path| !path.is_empty())
}

pub(crate) fn read(conn: &Connection, session_id: &str) -> Result<Option<ComposerDraft>, String> {
    if !has_durable_storage(conn) {
        return Ok(None);
    }
    let draft = conn
        .query_row(
            "SELECT text, revision FROM composer_drafts WHERE session_id = ?1",
            [session_id],
            |row| {
                Ok(ComposerDraft {
                    text: row.get(0)?,
                    revision: row.get(1)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    Ok(Some(draft.unwrap_or(ComposerDraft {
        text: String::new(),
        revision: 0,
    })))
}

#[tauri::command]
pub fn save_composer_draft(
    state: State<'_, AppState>,
    session_id: String,
    revision: i64,
    text: String,
) -> Result<bool, String> {
    save_composer_draft_impl(&state, &session_id, revision, &text)
}

pub(crate) fn save_composer_draft_impl(
    state: &AppState,
    session_id: &str,
    revision: i64,
    text: &str,
) -> Result<bool, String> {
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    save(&conn, session_id, revision, text)
}

fn save(conn: &Connection, session_id: &str, revision: i64, text: &str) -> Result<bool, String> {
    if revision < 1 || session_id.is_empty() {
        return Err("草稿缺少有效的会话或版本。".into());
    }
    if !has_durable_storage(conn) {
        return Ok(false);
    }
    // Keep the revision even when clearing the text so a late write cannot
    // bring back a submitted or deleted draft.
    conn.execute(
        "INSERT INTO composer_drafts (session_id, text, revision)
         SELECT id, ?2, ?3 FROM sessions WHERE id = ?1
         ON CONFLICT(session_id) DO UPDATE SET text = excluded.text, revision = excluded.revision
         WHERE composer_drafts.revision < excluded.revision",
        params![session_id, text, revision],
    )
    .map(|changed| changed == 1)
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composer_drafts_reopen_in_their_owned_database_and_keep_the_latest_revision() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("data.db");
        let conn = crate::db::init(&path).unwrap();
        crate::db::migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO sessions (id,title,created_at,updated_at) VALUES ('another','',1,1)",
            [],
        )
        .unwrap();

        assert!(save(&conn, "personal-main", 2, "latest daily draft").unwrap());
        assert!(!save(&conn, "personal-main", 1, "older request").unwrap());
        assert!(save(&conn, "another", 1, "a different session").unwrap());
        drop(conn);

        let conn = crate::db::init(&path).unwrap();
        assert_eq!(
            read(&conn, "personal-main").unwrap().unwrap().text,
            "latest daily draft"
        );
        assert_eq!(
            read(&conn, "another").unwrap().unwrap().text,
            "a different session"
        );
        assert!(save(&conn, "personal-main", 3, "").unwrap());
        assert!(!save(&conn, "personal-main", 2, "must not return").unwrap());
        assert_eq!(read(&conn, "personal-main").unwrap().unwrap().text, "");

        conn.execute("DELETE FROM sessions WHERE id = 'another'", [])
            .unwrap();
        assert_eq!(read(&conn, "another").unwrap().unwrap().revision, 0);
        assert!(!save(&conn, "another", 2, "late write after deletion").unwrap());

        let isolated = crate::db::init(&directory.path().join("isolated.db")).unwrap();
        crate::db::migrate(&isolated).unwrap();
        assert_eq!(
            read(&isolated, "personal-main").unwrap().unwrap().revision,
            0
        );
        let temporary = Connection::open_in_memory().unwrap();
        crate::db::migrate(&temporary).unwrap();
        assert!(read(&temporary, "personal-main").unwrap().is_none());
        assert!(!save(&temporary, "personal-main", 1, "temporary text").unwrap());
    }
}

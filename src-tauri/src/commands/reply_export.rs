//! User-selected, no-clobber exports of durable replies and session files.
//! Neither native picker grants the Agent access to the chosen directory.

use std::{io::Write, path::Path};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri_plugin_dialog::DialogExt;

const MAX_REPLY_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Serialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ReplyExportOutcome {
    Cancelled,
    Saved { name: String, bytes: usize },
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReplyExportError {
    ReplyUnavailable,
    ReplyTooLarge,
    InvalidDestination,
    FileExists,
    SaveFailed,
}

fn reply_snapshot(
    conn: &Connection,
    session_id: &str,
    message_id: &str,
) -> Result<String, ReplyExportError> {
    // Do not allocate oversized text, hydrate private transcripts, or export
    // provisional replies, tool rows, deleted messages or another session.
    let content: Option<Option<String>> = conn
        .query_row(
            "SELECT CASE WHEN length(CAST(content AS BLOB)) <= ?3 THEN content END
             FROM messages WHERE id=?1 AND session_id=?2 AND role='assistant'
             AND is_provisional=0",
            params![message_id, session_id, MAX_REPLY_BYTES as i64],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| ReplyExportError::ReplyUnavailable)?;
    match content {
        None => Err(ReplyExportError::ReplyUnavailable),
        Some(None) => Err(ReplyExportError::ReplyTooLarge),
        Some(Some(text)) if text.trim().is_empty() => Err(ReplyExportError::ReplyUnavailable),
        Some(Some(text)) => Ok(text),
    }
}

fn persist_reply(text: &str, destination: &Path) -> Result<ReplyExportOutcome, ReplyExportError> {
    if !destination
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("txt")
        })
    {
        return Err(ReplyExportError::InvalidDestination);
    }
    persist_file(text.as_bytes(), destination)
}

fn persist_file(bytes: &[u8], destination: &Path) -> Result<ReplyExportOutcome, ReplyExportError> {
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| {
            !name.chars().any(|c| {
                c.is_control()
                    || matches!(c, ':' | '/' | '\\' | '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            })
                && !name.ends_with([' ', '.'])
        })
        .ok_or(ReplyExportError::InvalidDestination)?;
    if !destination.is_absolute() {
        return Err(ReplyExportError::InvalidDestination);
    }
    let parent = destination
        .parent()
        .ok_or(ReplyExportError::InvalidDestination)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".angelbot-reply-")
        .tempfile_in(parent)
        .map_err(|_| ReplyExportError::SaveFailed)?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|_| ReplyExportError::SaveFailed)?;
    // Atomic no-clobber publication, including when another writer creates the
    // destination after the picker closes. Errors drop the temporary file.
    temporary.persist_noclobber(destination).map_err(|error| {
        if error.error.kind() == std::io::ErrorKind::AlreadyExists {
            ReplyExportError::FileExists
        } else {
            ReplyExportError::SaveFailed
        }
    })?;
    Ok(ReplyExportOutcome::Saved {
        name: name.to_owned(),
        bytes: bytes.len(),
    })
}

/// A bounded binary snapshot permits exporting downloaded files too, without
/// rendering them as text or giving model-supplied paths authority outside the
/// active session. The native picker is the only destination authority.
pub(crate) fn session_file_snapshot(
    state: &crate::AppState,
    session_id: &str,
    path: &str,
) -> Result<(String, Vec<u8>), String> {
    use std::io::Read;
    let root = super::file::resolve_work_dir(state, Some(session_id))?;
    let source = super::file::resolve_and_validate(&root, Path::new(path), true)
        .map_err(|error| error.to_string())?;
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "无法另存该文件。".to_owned())?
        .to_owned();
    let file = std::fs::File::open(&source).map_err(|error| error.to_string())?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("只能另存文件，不能另存目录。".to_owned());
    }
    let mut bytes = Vec::new();
    file.take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("文件超过 16 MiB，请通过文件管理器复制。".to_owned());
    }
    Ok((name, bytes))
}

#[tauri::command]
pub async fn save_session_file(
    state: tauri::State<'_, crate::AppState>,
    window: tauri::Window,
    session_id: String,
    path: String,
) -> Result<ReplyExportOutcome, String> {
    let (name, bytes) = session_file_snapshot(&state, &session_id, &path)?;
    let (send, receive) = tokio::sync::oneshot::channel();
    window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("文件另存为（仅新建文件，不覆盖已有文件）")
        .set_file_name(name)
        .save_file(move |destination| {
            let _ = send.send(destination);
        });
    let Some(destination) = receive.await.map_err(|_| "另存窗口未返回结果。")? else {
        return Ok(ReplyExportOutcome::Cancelled);
    };
    let destination = destination.into_path().map_err(|_| "另存位置无效。")?;
    tauri::async_runtime::spawn_blocking(move || persist_file(&bytes, &destination))
        .await
        .map_err(|_| "无法确认保存结果，请检查所选位置，不要自动重试。")?
        .map_err(|error| match error {
            ReplyExportError::FileExists => "目标文件已存在，未覆盖；请选择新名称。".to_owned(),
            _ => "无法确认保存结果，请检查所选位置；原文件未改变。".to_owned(),
        })
}

#[tauri::command]
pub async fn save_message_reply(
    state: tauri::State<'_, crate::AppState>,
    window: tauri::Window,
    session_id: String,
    message_id: String,
) -> Result<ReplyExportOutcome, ReplyExportError> {
    let text = {
        let conn = state
            .db
            .lock()
            .map_err(|_| ReplyExportError::ReplyUnavailable)?;
        reply_snapshot(&conn, &session_id, &message_id)?
    };
    // Paths and text are not supplied by the frontend or a model. The native
    // picker is the user's authority for this one new output file only.
    let (send, receive) = tokio::sync::oneshot::channel();
    window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("保存回复（仅新建文件，不覆盖已有文件）")
        .set_file_name(format!(
            "AngelBot-{}.md",
            chrono::Local::now().format("%Y%m%d-%H%M%S-%3f")
        ))
        .add_filter("Markdown / 文本", &["md", "txt"])
        .save_file(move |destination| {
            let _ = send.send(destination);
        });
    let Some(destination) = receive.await.map_err(|_| ReplyExportError::SaveFailed)? else {
        return Ok(ReplyExportOutcome::Cancelled);
    };
    let destination = destination
        .into_path()
        .map_err(|_| ReplyExportError::InvalidDestination)?;
    tauri::async_runtime::spawn_blocking(move || persist_reply(&text, &destination))
        .await
        .map_err(|_| ReplyExportError::SaveFailed)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_visible_bounded_assistant_text_can_be_selected() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO sessions(id,title,created_at,updated_at) VALUES('session','test',1,1)",
            [],
        )
        .unwrap();
        for (id, role, text, provisional) in [
            ("visible", "assistant", "# 清单\r\n事项\n", 0),
            ("user", "user", "private user", 0),
            ("tool", "tool", "private tool", 0),
            ("hidden", "assistant", "private provisional", 1),
            ("deleted", "assistant", "deleted", 0),
            ("empty", "assistant", " \n\t", 0),
        ] {
            conn.execute(
                "INSERT INTO messages(id,session_id,role,content,created_at,is_provisional) VALUES (?1,'session',?2,?3,1,?4)",
                params![id, role, text, provisional],
            )
            .unwrap();
        }
        conn.execute("DELETE FROM messages WHERE id='deleted'", [])
            .unwrap();
        assert_eq!(
            reply_snapshot(&conn, "session", "visible").unwrap(),
            "# 清单\r\n事项\n"
        );
        for (session, id) in [
            ("other", "visible"),
            ("session", "missing"),
            ("session", "user"),
            ("session", "tool"),
            ("session", "hidden"),
            ("session", "deleted"),
            ("session", "empty"),
        ] {
            assert_eq!(
                reply_snapshot(&conn, session, id),
                Err(ReplyExportError::ReplyUnavailable)
            );
        }
        conn.execute(
            "UPDATE messages SET content=?1 WHERE id='visible'",
            ["中".repeat(MAX_REPLY_BYTES / 3 + 1)],
        )
        .unwrap();
        assert_eq!(
            reply_snapshot(&conn, "session", "visible"),
            Err(ReplyExportError::ReplyTooLarge)
        );
        conn.execute(
            "UPDATE messages SET content=?1 WHERE id='visible'",
            ["a".repeat(MAX_REPLY_BYTES)],
        )
        .unwrap();
        assert_eq!(
            reply_snapshot(&conn, "session", "visible").unwrap().len(),
            MAX_REPLY_BYTES
        );
        assert_eq!(
            serde_json::to_string(&ReplyExportError::ReplyUnavailable).unwrap(),
            "\"REPLY_UNAVAILABLE\""
        );
        assert_eq!(
            serde_json::to_string(&ReplyExportOutcome::Cancelled).unwrap(),
            "{\"status\":\"cancelled\"}"
        );
    }

    #[test]
    fn export_is_exact_atomic_and_never_replaces_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let text = "# 清单\r\n- 事项 <not executable>\n";
        for name in ["reply.md", "清单.TXT"] {
            let path = dir.path().join(name);
            let saved = persist_reply(text, &path).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), text.as_bytes());
            assert_eq!(
                saved,
                ReplyExportOutcome::Saved {
                    name: name.into(),
                    bytes: text.len()
                }
            );
            assert!(!serde_json::to_string(&saved)
                .unwrap()
                .contains(&dir.path().to_string_lossy().to_string()));
            assert_eq!(
                persist_reply("replacement", &path),
                Err(ReplyExportError::FileExists)
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        }
        for path in [
            Path::new("relative.md").to_path_buf(),
            dir.path().join("run.ps1"),
            dir.path().join("reply.txt:stream.md"),
            dir.path().join("bad\n.md"),
            dir.path().join("bad\u{202e}.md"),
            dir.path().join("no-extension"),
        ] {
            assert_eq!(
                persist_reply(text, &path),
                Err(ReplyExportError::InvalidDestination)
            );
            if path.is_absolute() {
                assert!(!path.exists());
            }
        }
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let target = dir.path().join("race.md");
        let workers: Vec<_> = ["first", "second"]
            .into_iter()
            .map(|text| {
                let barrier = barrier.clone();
                let target = target.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    persist_reply(text, &target)
                })
            })
            .collect();
        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| **result == Err(ReplyExportError::FileExists))
                .count(),
            1
        );
        assert!(["first", "second"].contains(&std::fs::read_to_string(target).unwrap().as_str()));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3);
        let binary = [0, 255, 1, 128];
        let binary_path = dir.path().join("download.bin");
        assert!(persist_file(&binary, &binary_path).is_ok());
        assert_eq!(std::fs::read(&binary_path).unwrap(), binary);
        assert_eq!(
            persist_file(b"replacement", &binary_path),
            Err(ReplyExportError::FileExists)
        );
    }
}

//! Ownership of browser scratch files inside Daily's existing file scope.
//! No process launch, file-type guessing or external roots.
use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub(crate) const KEY: &str = "browser_runtime_namespace";
const MARKER: &str = ".angelbot-browser-owner";
pub(crate) mod cleanup;

pub(crate) fn relative_namespace(conn: &Connection) -> Result<Option<String>, String> {
    let token: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key=?1", [KEY], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|error| error.to_string())?;
    token
        .map(|token| {
            let id = uuid::Uuid::parse_str(&token).map_err(|_| "浏览器临时区登记无效。")?;
            if id.to_string() != token {
                return Err("浏览器临时区登记无效。".to_owned());
            }
            Ok(format!("browser-runtime-{token}"))
        })
        .transpose()
}

fn validate_directory(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || path.canonicalize().map_err(|error| error.to_string())? != path
    {
        return Err("浏览器临时区不能是链接或重定向目录。".into());
    }
    Ok(())
}

pub(crate) fn owned_namespace(conn: &Connection) -> Result<Option<PathBuf>, String> {
    let Some(name) = relative_namespace(conn)? else {
        return Ok(None);
    };
    let path = crate::commands::file::personal_files_root(conn)?.join(&name);
    if !path.try_exists().map_err(|error| error.to_string())? {
        return Ok(None);
    }
    validate_directory(&path)?;
    let marker = path.join(MARKER);
    let metadata = std::fs::symlink_metadata(&marker)
        .map_err(|_| "浏览器临时区归属标记缺失；未排除其备份。")?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || marker.canonicalize().map_err(|error| error.to_string())? != marker
    {
        return Err("浏览器临时区归属标记无效。".into());
    }
    let mut value = String::new();
    std::fs::File::open(marker)
        .map_err(|error| error.to_string())?
        .take(128)
        .read_to_string(&mut value)
        .map_err(|error| error.to_string())?;
    if value != name {
        return Err("浏览器临时区归属已改变；未排除其备份。".into());
    }
    Ok(Some(path))
}

pub(crate) fn relative_output(namespace: &str, server_id: &str) -> String {
    format!("{namespace}/{:x}", Sha256::digest(server_id.as_bytes()))
}

pub(crate) fn prepare_output(conn: &Connection, server_id: &str) -> Result<PathBuf, String> {
    let root = crate::commands::file::personal_files_root(conn)?;
    let name = relative_namespace(conn)?
        .unwrap_or_else(|| format!("browser-runtime-{}", uuid::Uuid::new_v4()));
    let path = root.join(&name);
    if !path.try_exists().map_err(|error| error.to_string())? {
        // create_dir never adopts an existing folder as disposable storage.
        std::fs::create_dir(&path).map_err(|error| error.to_string())?;
        let mut marker = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path.join(MARKER))
            .map_err(|error| error.to_string())?;
        marker
            .write_all(name.as_bytes())
            .and_then(|()| marker.sync_all())
            .map_err(|error| error.to_string())?;
        conn.execute("INSERT INTO settings(key,value,updated_at) VALUES(?1,?2,?3) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",
            rusqlite::params![KEY, name.strip_prefix("browser-runtime-").unwrap(), chrono::Utc::now().timestamp()])
            .map_err(|error| error.to_string())?;
    }
    let owned = owned_namespace(conn)?.ok_or("浏览器临时区未登记。")?;
    let output = root.join(relative_output(&name, server_id));
    match std::fs::create_dir(&output) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.to_string()),
    }
    validate_directory(&output)?;
    if output.parent() != Some(owned.as_path()) {
        return Err("浏览器临时区位置无效。".into());
    }
    Ok(output)
}

pub(crate) fn matches_output(
    conn: &Connection,
    server_id: &str,
    output: &Path,
) -> Result<bool, String> {
    let Some(namespace) = relative_namespace(conn)? else {
        return Ok(false);
    };
    let _ = owned_namespace(conn)?; // Validate any existing ownership marker.
    let expected = crate::commands::file::personal_files_root(conn)?
        .join(relative_output(&namespace, server_id));
    if let Ok(path) = output.canonicalize() {
        return Ok(path == expected);
    }
    let display = crate::commands::file::display_work_dir(output);
    let expected = crate::commands::file::display_work_dir(&expected);
    #[cfg(windows)]
    return Ok(display.eq_ignore_ascii_case(&expected));
    #[cfg(not(windows))]
    Ok(display == expected)
}

pub(crate) fn configured_output(
    conn: &Connection,
    server_id: &str,
    raw: &str,
) -> Result<Option<PathBuf>, String> {
    let Some(namespace) = relative_namespace(conn)? else {
        return Ok(None);
    };
    if !raw.contains(&namespace) {
        return Ok(None);
    }
    let Ok(args) = crate::mcp_client::parse_command_args(raw) else {
        return Ok(None);
    };
    let positions = args
        .iter()
        .enumerate()
        .filter(|(_, arg)| *arg == "--output-dir")
        .map(|(index, _)| index + 1)
        .collect::<Vec<_>>();
    if positions.len() == 1 {
        if let Some(output) = args.get(positions[0]) {
            if matches_output(conn, server_id, Path::new(output))? {
                return Ok(Some(
                    crate::commands::file::personal_files_root(conn)?
                        .join(relative_output(&namespace, server_id)),
                ));
            }
        }
    }
    Ok(None)
}

/// Imported ownership is only a source-path hint, never local authority.
pub(crate) fn rebase_imported_server(
    conn: &Connection,
    server: &mut serde_json::Value,
    source_namespace: Option<&str>,
) -> Result<(), String> {
    let (Some(namespace), Some(id), Some(raw)) = (
        source_namespace,
        server.get("id").and_then(|v| v.as_str()),
        server.get("args").and_then(|v| v.as_str()),
    ) else {
        return Ok(());
    };
    if !raw.contains(namespace) {
        return Ok(());
    }
    let Ok(mut args) = crate::mcp_client::parse_command_args(raw) else {
        server["enabled"] = false.into();
        return Ok(());
    };
    let suffix = format!("/{}", relative_output(namespace, id));
    let positions = args
        .iter()
        .enumerate()
        .filter(|(_, arg)| *arg == "--output-dir")
        .map(|(index, _)| index + 1)
        .collect::<Vec<_>>();
    if positions.len() == 1
        && args
            .get(positions[0])
            .is_some_and(|value| value.replace('\\', "/").ends_with(&suffix))
    {
        args[positions[0]] = crate::commands::file::display_work_dir(&prepare_output(conn, id)?);
        server["args"] = serde_json::to_string(&args)
            .map_err(|error| error.to_string())?
            .into();
    } else {
        // An ambiguous old local path must not silently remain executable.
        server["enabled"] = false.into();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{file, mcp, settings};

    #[test]
    fn browser_outputs_publish_and_restore_through_existing_file_and_backup_contracts() {
        let source = tempfile::tempdir().unwrap();
        let conn = crate::db::init(&source.path().join("data.db")).unwrap();
        crate::db::migrate(&conn).unwrap();
        let state = crate::tests::test_app_state();
        *state.db.lock().unwrap() = conn;
        assert!(mcp::prepare_browser_mcp_impl(&state, "missing").is_err());
        let draft =
            serde_json::to_value(mcp::prepare_browser_mcp_impl(&state, "personal-main").unwrap())
                .unwrap();
        let id = draft["id"].as_str().unwrap();
        let args = draft["args"].as_str().unwrap();
        let (root, output, namespace) = {
            let conn = state.db.lock().unwrap();
            let root = file::personal_files_root(&conn).unwrap();
            let output = configured_output(&conn, id, args)
                .unwrap()
                .unwrap()
                .canonicalize()
                .unwrap();
            let namespace = relative_namespace(&conn).unwrap().unwrap();
            assert!(configured_output(&conn, "different-server", args)
                .unwrap()
                .is_none());
            conn.execute("INSERT INTO mcp_servers(id,name,command,args,enabled,created_at) VALUES(?1,'Browser','npx.cmd',?2,1,1)", rusqlite::params![id, args]).unwrap();
            conn.execute("INSERT INTO mcp_workspace_enablements(workspace_id,server_id,enabled_at) VALUES('personal',?1,1)", [id]).unwrap();
            let tools: Vec<crate::mcp_client::McpTool> = serde_json::from_value(serde_json::json!([{"name":"browser_snapshot","description":"fixture","inputSchema":{"type":"object","properties":{}}}])).unwrap();
            crate::commands::mcp_catalog::store(&conn, id, "npx.cmd", args, &tools).unwrap();
            (root, output, namespace)
        };
        let mut registry = crate::agent::ToolRegistry::empty();
        crate::agent::handlers::register_workspace_file_handlers(&mut registry);
        let revisions =
            mcp::register_session_tools(&state, None, &mut registry, "personal-main").unwrap();
        assert_eq!(revisions.len(), 1);
        let hint = registry
            .get("organize_workspace_item")
            .unwrap()
            .prompt_snippet
            .unwrap();
        assert!(hint.contains(&namespace) && hint.contains("不随备份") && hint.contains("copy"));

        std::fs::write(root.join("notes.yml"), "owner yaml").unwrap();
        std::fs::write(root.join("work.log"), "owner log").unwrap();
        std::fs::write(output.join("page.yml"), "browser snapshot").unwrap();
        std::fs::write(output.join("console.log"), "browser diagnostics").unwrap();
        std::fs::write(output.join("download.csv"), "item,count\nfixture,2\n").unwrap();
        std::fs::write(output.join("binary.bin"), [0, 255, 3, 128]).unwrap();
        file::organize_workspace_item_core(
            &root,
            "copy",
            output.join("download.csv").to_str().unwrap(),
            "answer.csv",
        )
        .unwrap();
        file::organize_workspace_item_core(
            &root,
            "copy",
            output.join("binary.bin").to_str().unwrap(),
            "answer.bin",
        )
        .unwrap();
        assert_eq!(
            file::read_file_core(&root, "answer.csv").unwrap(),
            "item,count\nfixture,2\n"
        );
        assert!(file::organize_workspace_item_core(
            &root,
            "copy",
            output.join("binary.bin").to_str().unwrap(),
            "answer.csv"
        )
        .is_err());
        assert_eq!(
            file::read_file_core(&root, "answer.csv").unwrap(),
            "item,count\nfixture,2\n"
        );
        let entries = file::list_work_dir_impl(&state, "personal-main", "").unwrap();
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.temporary == Some(true))
                .count(),
            1
        );
        assert!(entries
            .iter()
            .find(|entry| entry.name == "notes.yml")
            .is_some_and(|entry| entry.temporary == Some(false)));
        assert!(file::list_work_dir_impl(
            &state,
            "personal-main",
            &relative_output(&namespace, id)
        )
        .unwrap()
        .iter()
        .all(|entry| entry.temporary == Some(true)));

        let encrypted =
            settings::export_encrypted_data_impl(&state, "browser storage password".into(), None)
                .unwrap();
        let plaintext =
            crate::backup::decrypt_export(&encrypted, "browser storage password").unwrap();
        let payload: serde_json::Value = serde_json::from_str(&plaintext).unwrap();
        let paths = payload["daily_files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["path"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(paths, ["answer.bin", "answer.csv", "notes.yml", "work.log"]);

        let target = tempfile::tempdir().unwrap();
        let conn = crate::db::init(&target.path().join("data.db")).unwrap();
        crate::db::migrate(&conn).unwrap();
        prepare_output(&conn, "already-local").unwrap();
        let local_namespace = relative_namespace(&conn).unwrap().unwrap();
        assert_ne!(local_namespace, namespace);
        *state.db.lock().unwrap() = conn;
        settings::import_data_impl(&state, plaintext.clone(), None).unwrap();
        settings::import_data_impl(
            &state,
            plaintext,
            Some(settings::ImportOptions {
                mode: Some("replace".into()),
            }),
        )
        .unwrap();
        let conn = state.db.lock().unwrap();
        assert_eq!(relative_namespace(&conn).unwrap().unwrap(), local_namespace);
        let restored_root = file::personal_files_root(&conn).unwrap();
        assert_eq!(
            std::fs::read(restored_root.join("answer.bin")).unwrap(),
            [0, 255, 3, 128]
        );
        assert_eq!(
            file::read_file_core(&restored_root, "answer.csv").unwrap(),
            "item,count\nfixture,2\n"
        );
        assert!(!restored_root.join(namespace).exists());
        let restored_args: String = conn
            .query_row("SELECT args FROM mcp_servers WHERE id=?1", [id], |row| {
                row.get(0)
            })
            .unwrap();
        let restored_output = configured_output(&conn, id, &restored_args)
            .unwrap()
            .unwrap();
        assert!(restored_output.starts_with(&restored_root));
        assert!(!restored_output.join("download.csv").exists());
        assert!(!restored_output.join("page.yml").exists());
        assert!(state.mcp_manager.running_servers().is_empty());
    }

    #[test]
    fn similarly_named_files_are_not_adopted_and_changed_ownership_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let conn = crate::db::init(&directory.path().join("data.db")).unwrap();
        crate::db::migrate(&conn).unwrap();
        let root = file::personal_files_root(&conn).unwrap();
        let fake = root.join(format!("browser-runtime-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&fake).unwrap();
        std::fs::write(fake.join("keep.log"), "user data").unwrap();
        assert!(owned_namespace(&conn).unwrap().is_none());
        assert_eq!(crate::backup::files::snapshot(&root).unwrap().len(), 1);
        let first = prepare_output(&conn, "server").unwrap();
        assert_eq!(first, prepare_output(&conn, "server").unwrap());
        assert_ne!(first, prepare_output(&conn, "other").unwrap());
        let args = serde_json::json!([
            "--output-dir",
            crate::commands::file::display_work_dir(&first)
        ])
        .to_string();
        std::fs::remove_dir(&first).unwrap(); // Exact, empty fixture-owned child.
        assert_eq!(
            configured_output(&conn, "server", &args).unwrap(),
            Some(first.clone())
        );
        assert_eq!(prepare_output(&conn, "server").unwrap(), first);
        let owned = owned_namespace(&conn).unwrap().unwrap();
        assert_ne!(owned, fake);
        std::fs::write(owned.join(MARKER), "changed").unwrap();
        assert!(owned_namespace(&conn).is_err());
        assert!(prepare_output(&conn, "server").is_err());
        assert_eq!(
            std::fs::read_to_string(fake.join("keep.log")).unwrap(),
            "user data"
        );
        conn.execute("UPDATE settings SET value='../outside' WHERE key=?1", [KEY])
            .unwrap();
        assert!(prepare_output(&conn, "server").is_err());
        assert!(!directory.path().join("outside").exists());
    }
}

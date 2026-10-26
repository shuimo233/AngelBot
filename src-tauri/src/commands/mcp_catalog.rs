//! Derived MCP discovery across restarts. Loading never starts a process or
//! grants authority; dispatch still checks live workspace/config/schema state.
use crate::mcp_client::McpTool;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

const MAX_CATALOG_BYTES: usize = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct CachedTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
    #[serde(default)]
    read_only_hint: bool,
}

pub(crate) fn configuration_fingerprint(command: &str, args: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"angelbot:mcp-configuration:v1\0");
    hash.update(command.as_bytes());
    hash.update(b"\0");
    hash.update(args.as_bytes());
    hex::encode(hash.finalize())
}

pub(crate) fn store(
    conn: &Connection,
    server_id: &str,
    command: &str,
    args: &str,
    tools: &[McpTool],
) -> Result<(), String> {
    let entries: Vec<_> = tools
        .iter()
        .map(|tool| CachedTool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
            read_only_hint: tool.read_only_budget_hint(),
        })
        .collect();
    validate(&entries)?;
    let encoded = serde_json::to_string(&entries).map_err(|_| "MCP 工具目录无法编码")?;
    if encoded.len() > MAX_CATALOG_BYTES {
        return Err("MCP 工具目录过大，未缓存".into());
    }
    let changed = conn
        .execute(
            "INSERT INTO mcp_tool_catalog(server_id,config_fingerprint,tools_json,updated_at)
         SELECT id,?2,?3,?4 FROM mcp_servers
         WHERE id=?1 AND command=?5 AND COALESCE(args,'')=?6 AND enabled=1
         ON CONFLICT(server_id) DO UPDATE SET config_fingerprint=excluded.config_fingerprint,
             tools_json=excluded.tools_json,updated_at=excluded.updated_at",
            params![
                server_id,
                configuration_fingerprint(command, args),
                encoded,
                chrono::Utc::now().timestamp(),
                command,
                args
            ],
        )
        .map_err(|_| "MCP 工具目录无法保存")?;
    if changed != 1 {
        return Err("MCP 配置已变化或停用，工具目录未保存".into());
    }
    Ok(())
}

pub(crate) fn load(conn: &Connection, server_id: &str) -> Result<Option<Vec<McpTool>>, String> {
    let row = conn
        .query_row(
            "SELECT s.command,COALESCE(s.args,''),c.config_fingerprint,c.tools_json
         FROM mcp_servers s JOIN mcp_tool_catalog c ON c.server_id=s.id
         WHERE s.id=?1 AND s.enabled=1",
            [server_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|_| "MCP 工具目录无法读取")?;
    let Some((command, args, fingerprint, encoded)) = row else {
        return Ok(None);
    };
    if fingerprint != configuration_fingerprint(&command, &args) {
        return Ok(None);
    }
    if encoded.len() > MAX_CATALOG_BYTES {
        return Err("MCP 工具目录过大，请重新连接服务".into());
    }
    let entries: Vec<CachedTool> =
        serde_json::from_str(&encoded).map_err(|_| "MCP 工具目录已损坏，请在设置中重新连接服务")?;
    validate(&entries)?;
    Ok(Some(
        entries
            .into_iter()
            .map(|tool| McpTool {
                name: tool.name,
                description: tool.description,
                input_schema: tool.input_schema,
                annotations: Some(serde_json::json!({"readOnlyHint":tool.read_only_hint})),
            })
            .collect(),
    ))
}

fn validate(tools: &[CachedTool]) -> Result<(), String> {
    let mut names = HashSet::new();
    if tools.iter().any(|tool| {
        tool.name.is_empty() || !names.insert(&tool.name) || !tool.input_schema.is_object()
    }) {
        return Err("MCP 工具目录无效，请重新连接服务".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_survives_reopen_but_never_configuration_revocation_or_deletion() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.db");
        {
            let conn = crate::db::init(&path).unwrap();
            crate::db::migrate(&conn).unwrap();
            conn.execute(
                "INSERT INTO mcp_servers(id,name,command,args,env,enabled,created_at)
                VALUES('one','One','node','owned.js','',1,1)",
                [],
            )
            .unwrap();
            assert!(load(&conn, "one").unwrap().is_none());
            let tool: McpTool = serde_json::from_value(serde_json::json!({
                "name":"observe", "description":"Synthetic", "inputSchema":{"type":"object"},
                "annotations":{"readOnlyHint":true,"private":"PRIVATE_CATALOG_CANARY"}
            }))
            .unwrap();
            store(&conn, "one", "node", "owned.js", &[tool]).unwrap();
        }
        let conn = crate::db::init(&path).unwrap();
        crate::db::migrate(&conn).unwrap();
        let tools = load(&conn, "one").unwrap().unwrap();
        assert_eq!(tools.len(), 1);
        assert!(tools[0].read_only_budget_hint());
        let encoded: String = conn
            .query_row("SELECT tools_json FROM mcp_tool_catalog", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(!encoded.contains("PRIVATE_CATALOG_CANARY"));
        assert!(!encoded.contains("command"));
        assert!(!encoded.contains("env"));
        conn.execute(
            "UPDATE mcp_tool_catalog SET tools_json='[{}]' WHERE server_id='one'",
            [],
        )
        .unwrap();
        assert!(load(&conn, "one").unwrap_err().contains("损坏"));
        store(&conn, "one", "node", "owned.js", &tools).unwrap();
        conn.execute(
            "UPDATE mcp_servers SET args='replacement.js' WHERE id='one'",
            [],
        )
        .unwrap();
        assert!(load(&conn, "one").unwrap().is_none());
        assert!(store(&conn, "one", "node", "owned.js", &tools).is_err());
        store(&conn, "one", "node", "replacement.js", &tools).unwrap();
        conn.execute("UPDATE mcp_servers SET enabled=0 WHERE id='one'", [])
            .unwrap();
        assert!(load(&conn, "one").unwrap().is_none());
        assert!(store(&conn, "one", "node", "replacement.js", &tools).is_err());
        conn.execute("UPDATE mcp_servers SET enabled=1 WHERE id='one'", [])
            .unwrap();
        store(&conn, "one", "node", "replacement.js", &tools).unwrap();
        conn.execute("DELETE FROM mcp_servers WHERE id='one'", [])
            .unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM mcp_tool_catalog", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
                .optional()
                .unwrap(),
            None
        );
    }
}

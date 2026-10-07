//! MCP-specific environment credentials.
//!
//! SQLite stores only an opaque reference and variable names. The value blob
//! lives in the OS credential vault and is never returned by an MCP listing or
//! written to an ordinary backup. The store seam has a keyring adapter in the
//! desktop and an in-memory adapter in tests.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use tauri_plugin_keyring_store::KeyringExt;
use uuid::Uuid;

const REFERENCE_PREFIX: &str = "mcp_env_v1_";
const MAX_VARIABLES: usize = 64;
const MAX_BLOB_BYTES: usize = 16 * 1024;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct McpEnvVar {
    pub key: String,
    pub value: String,
}

// A failed assertion or accidental diagnostic must not print credentials.
impl std::fmt::Debug for McpEnvVar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpEnvVar")
            .field("key", &self.key)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

#[derive(Serialize, Deserialize)]
struct CredentialBlob {
    version: u8,
    vars: Vec<McpEnvVar>,
}

/// Internal seam: production uses the OS keyring; tests use a deterministic
/// in-memory adapter. References, never server IDs or variable names, are
/// passed to the store.
pub(crate) trait McpCredentialStore {
    fn read(&self, reference: &str) -> Result<Option<String>, String>;
    fn write(&self, reference: &str, blob: &str) -> Result<(), String>;
    fn delete(&self, reference: &str) -> Result<(), String>;
}

pub(crate) struct KeyringMcpCredentialStore<'a> {
    app: &'a tauri::AppHandle,
}

impl<'a> KeyringMcpCredentialStore<'a> {
    pub(crate) fn new(app: &'a tauri::AppHandle) -> Self {
        Self { app }
    }
}

#[cfg(feature = "desktop-e2e")]
fn e2e_store() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
        std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

impl McpCredentialStore for KeyringMcpCredentialStore<'_> {
    fn read(&self, reference: &str) -> Result<Option<String>, String> {
        validate_reference(reference)?;
        #[cfg(feature = "desktop-e2e")]
        {
            let _ = self.app;
            return e2e_store()
                .lock()
                .map(|store| store.get(reference).cloned())
                .map_err(|_| "无法访问 MCP 测试凭据库".to_string());
        }
        #[cfg(not(feature = "desktop-e2e"))]
        self.app
            .keyring()
            .store
            .get_password(reference)
            .map_err(|_| "无法读取系统 MCP 凭据库".to_string())
    }

    fn write(&self, reference: &str, blob: &str) -> Result<(), String> {
        validate_reference(reference)?;
        #[cfg(feature = "desktop-e2e")]
        {
            let _ = self.app;
            return e2e_store()
                .lock()
                .map(|mut store| {
                    store.insert(reference.to_string(), blob.to_string());
                })
                .map_err(|_| "无法访问 MCP 测试凭据库".to_string());
        }
        #[cfg(not(feature = "desktop-e2e"))]
        self.app
            .keyring()
            .store
            .set_password(reference, blob)
            .map_err(|_| "无法写入系统 MCP 凭据库".to_string())
    }

    fn delete(&self, reference: &str) -> Result<(), String> {
        validate_reference(reference)?;
        #[cfg(feature = "desktop-e2e")]
        {
            let _ = self.app;
            return e2e_store()
                .lock()
                .map(|mut store| {
                    store.remove(reference);
                })
                .map_err(|_| "无法访问 MCP 测试凭据库".to_string());
        }
        #[cfg(not(feature = "desktop-e2e"))]
        {
            let vault = &self.app.keyring().store;
            if vault
                .get_password(reference)
                .map_err(|_| "无法读取系统 MCP 凭据库".to_string())?
                .is_some()
            {
                vault
                    .delete(reference)
                    .map_err(|_| "无法删除系统 MCP 凭据".to_string())?;
            }
            Ok(())
        }
    }
}

fn validate_reference(reference: &str) -> Result<(), String> {
    let uuid = reference
        .strip_prefix(REFERENCE_PREFIX)
        .ok_or_else(|| "无效的 MCP 凭据引用".to_string())?;
    let parsed = Uuid::parse_str(uuid).map_err(|_| "无效的 MCP 凭据引用".to_string())?;
    if parsed.to_string() != uuid {
        return Err("无效的 MCP 凭据引用".to_string());
    }
    Ok(())
}

fn new_reference() -> String {
    format!("{REFERENCE_PREFIX}{}", Uuid::new_v4())
}

fn valid_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn validate_vars(vars: &[McpEnvVar]) -> Result<Vec<McpEnvVar>, String> {
    if vars.len() > MAX_VARIABLES {
        return Err("MCP 环境变量过多".to_string());
    }
    let mut unique = HashSet::new();
    let mut normalized = Vec::with_capacity(vars.len());
    for var in vars {
        if !valid_key(&var.key) {
            return Err("MCP 环境变量名无效".to_string());
        }
        if var.value.chars().any(|ch| matches!(ch, '\0' | '\n' | '\r'))
            || var.value.trim() != var.value
        {
            return Err("MCP 环境变量值包含不支持的字符".to_string());
        }
        // Windows environment keys are case-insensitive. Reject aliases on
        // every platform so backups preserve the same meaning across devices.
        if !unique.insert(var.key.to_ascii_uppercase()) {
            return Err("MCP 环境变量名重复".to_string());
        }
        normalized.push(var.clone());
    }
    normalized.sort_by(|a, b| a.key.cmp(&b.key));
    let bytes = serde_json::to_vec(&CredentialBlob {
        version: 1,
        vars: normalized.clone(),
    })
    .map_err(|_| "无法编码 MCP 凭据".to_string())?;
    if bytes.len() > MAX_BLOB_BYTES {
        return Err("MCP 凭据超过大小限制".to_string());
    }
    Ok(normalized)
}

/// Preflight an imported encrypted payload before any database or keyring
/// mutation. This intentionally never includes values in an error.
pub(crate) fn validate_env_vars(vars: &[McpEnvVar]) -> Result<(), String> {
    validate_vars(vars).map(|_| ())
}

fn encode_blob(vars: &[McpEnvVar]) -> Result<String, String> {
    serde_json::to_string(&CredentialBlob {
        version: 1,
        vars: vars.to_vec(),
    })
    .map_err(|_| "无法编码 MCP 凭据".to_string())
}

fn decode_blob(blob: &str) -> Result<Vec<McpEnvVar>, String> {
    if blob.len() > MAX_BLOB_BYTES {
        return Err("MCP 凭据超过大小限制".to_string());
    }
    let decoded: CredentialBlob =
        serde_json::from_str(blob).map_err(|_| "MCP 凭据格式无效".to_string())?;
    if decoded.version != 1 {
        return Err("不支持的 MCP 凭据版本".to_string());
    }
    validate_vars(&decoded.vars)
}

fn key_json(vars: &[McpEnvVar]) -> Result<String, String> {
    serde_json::to_string(&vars.iter().map(|var| &var.key).collect::<Vec<_>>())
        .map_err(|_| "无法编码 MCP 变量名".to_string())
}

/// The legacy launcher interpreted `KEY=VALUE` lines and let the final
/// duplicate key win. Reject malformed lines instead of silently dropping a
/// credential during migration or encrypted backup import.
pub(crate) fn parse_legacy_env(env: &str) -> Result<Vec<McpEnvVar>, String> {
    let mut vars = BTreeMap::new();
    for line in env.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| "旧 MCP 环境变量格式无效，请重新配置".to_string())?;
        let key = key.trim();
        if !valid_key(key) {
            return Err("旧 MCP 环境变量名无效，请重新配置".to_string());
        }
        vars.insert(
            key.to_ascii_uppercase(),
            McpEnvVar {
                key: key.to_string(),
                value: value.trim().to_string(),
            },
        );
    }
    validate_vars(&vars.into_values().collect::<Vec<_>>())
}

pub(crate) fn env_keys(conn: &Connection, server_id: &str) -> Result<Vec<String>, String> {
    let row: (String, String) = conn
        .query_row(
            "SELECT COALESCE(env, ''), env_keys FROM mcp_servers WHERE id = ?1",
            [server_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| "MCP 服务不存在".to_string())?;
    let mut keys: Vec<String> =
        serde_json::from_str(&row.1).map_err(|_| "MCP 变量名元数据无效".to_string())?;
    if !row.0.is_empty() {
        let legacy = parse_legacy_env(&row.0)?;
        keys.extend(legacy.into_iter().map(|var| var.key));
    }
    keys.sort();
    keys.dedup();
    Ok(keys)
}

fn read_by_reference(
    store: &impl McpCredentialStore,
    reference: &str,
) -> Result<Vec<McpEnvVar>, String> {
    validate_reference(reference)?;
    let blob = store
        .read(reference)?
        .ok_or_else(|| "MCP 凭据缺失，请重新配置".to_string())?;
    decode_blob(&blob)
}

fn verify_write(
    store: &impl McpCredentialStore,
    reference: &str,
    blob: &str,
) -> Result<(), String> {
    match store.read(reference)? {
        Some(actual) if actual == blob => Ok(()),
        _ => Err("无法确认 MCP 凭据已安全保存".to_string()),
    }
}

/// Resolve a server's launch environment, lazily migrating legacy plaintext.
/// Missing or unreadable referenced credentials fail closed; never fall back
/// to a possibly stale SQLite value.
pub(crate) fn read_effective_env(
    conn: &mut Connection,
    store: &impl McpCredentialStore,
    server_id: &str,
) -> Result<Vec<McpEnvVar>, String> {
    let (legacy, reference, expected_keys): (String, Option<String>, String) = conn
        .query_row(
            "SELECT COALESCE(env, ''), env_ref, env_keys FROM mcp_servers WHERE id = ?1",
            [server_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| "MCP 服务不存在".to_string())?;

    if let Some(reference) = reference {
        let vars = read_by_reference(store, &reference)?;
        if !legacy.is_empty() {
            conn.execute(
                "UPDATE mcp_servers SET env = '', env_keys = ?2 WHERE id = ?1 AND env_ref = ?3",
                params![server_id, key_json(&vars)?, reference],
            )
            .map_err(|_| "无法清除旧 MCP 明文凭据".to_string())?;
        }
        return Ok(vars);
    }
    if legacy.is_empty() {
        let expected: Vec<String> =
            serde_json::from_str(&expected_keys).map_err(|_| "MCP 变量名元数据无效".to_string())?;
        if !expected.is_empty() {
            // Plain backups carry only names. Do not launch the server as if
            // those required values had been restored.
            return Err("MCP 凭据缺失，请重新配置".to_string());
        }
        return Ok(Vec::new());
    }

    let vars = parse_legacy_env(&legacy)?;
    let reference = new_reference();
    let blob = encode_blob(&vars)?;
    store.write(&reference, &blob)?;
    if let Err(error) = verify_write(store, &reference, &blob) {
        let _ = store.delete(&reference);
        return Err(error);
    }
    let updated = conn.execute(
        "UPDATE mcp_servers SET env = '', env_ref = ?2, env_keys = ?3 \
         WHERE id = ?1 AND env_ref IS NULL AND COALESCE(env, '') = ?4",
        params![server_id, reference, key_json(&vars)?, legacy],
    );
    match updated {
        Ok(1) => Ok(vars),
        _ => {
            let _ = store.delete(&reference);
            Err("无法完成旧 MCP 凭据迁移".to_string())
        }
    }
}

/// Replace all variables after the caller has stopped the server and acquired
/// its MCP policy gate. A new random reference is written and verified before
/// SQLite switches to it; failed writes leave the old reference effective.
/// Successful replacement revokes workspace grants, including when clearing.
pub(crate) fn replace_env(
    conn: &mut Connection,
    store: &impl McpCredentialStore,
    server_id: &str,
    vars: &[McpEnvVar],
) -> Result<(), String> {
    let vars = validate_vars(vars)?;
    if vars.is_empty() {
        return delete_env(conn, store, server_id);
    }
    let old_reference: Option<String> = conn
        .query_row(
            "SELECT env_ref FROM mcp_servers WHERE id = ?1",
            [server_id],
            |row| row.get(0),
        )
        .map_err(|_| "MCP 服务不存在".to_string())?;
    let reference = new_reference();
    let blob = encode_blob(&vars)?;
    store.write(&reference, &blob)?;
    if let Err(error) = verify_write(store, &reference, &blob) {
        let _ = store.delete(&reference);
        return Err(error);
    }
    let new_reference = Some(reference);

    let transaction = conn
        .transaction()
        .map_err(|_| "无法更新 MCP 凭据".to_string())?;
    let changed = transaction.execute(
        "UPDATE mcp_servers SET env = '', env_ref = ?2, env_keys = ?3 WHERE id = ?1",
        params![server_id, new_reference, key_json(&vars)?],
    );
    let result = changed
        .map_err(|_| "无法更新 MCP 凭据".to_string())
        .and_then(|changed| {
            if changed != 1 {
                return Err("MCP 服务不存在".to_string());
            }
            transaction
                .execute(
                    "DELETE FROM mcp_workspace_enablements WHERE server_id = ?1",
                    [server_id],
                )
                .map_err(|_| "无法撤销 MCP 工作区授权".to_string())?;
            if let Some(old) = &old_reference {
                if Some(old) != new_reference.as_ref() {
                    transaction
                        .execute(
                            "INSERT OR IGNORE INTO mcp_credential_gc (env_ref) VALUES (?1)",
                            [old],
                        )
                        .map_err(|_| "无法安排旧 MCP 凭据清理".to_string())?;
                }
            }
            transaction
                .commit()
                .map_err(|_| "无法提交 MCP 凭据更新".to_string())
        });
    if result.is_err() {
        if let Some(reference) = &new_reference {
            let _ = store.delete(reference);
        }
        return result;
    }
    // A failed deletion is recoverable: the old reference is no longer active
    // and remains queued in SQLite for the next credential operation.
    let _ = reap_old_references(conn, store);
    Ok(())
}

pub(crate) fn delete_env(
    conn: &mut Connection,
    store: &impl McpCredentialStore,
    server_id: &str,
) -> Result<(), String> {
    let old_reference: Option<String> = conn
        .query_row(
            "SELECT env_ref FROM mcp_servers WHERE id = ?1",
            [server_id],
            |row| row.get(0),
        )
        .map_err(|_| "MCP 服务不存在".to_string())?;
    // For explicit revocation, fail before the DB change if the OS vault
    // refuses deletion. If SQLite then fails, the stale reference fails
    // closed and this operation can be retried.
    if let Some(reference) = &old_reference {
        store.delete(reference)?;
    }
    let transaction = conn
        .transaction()
        .map_err(|_| "无法更新 MCP 凭据".to_string())?;
    transaction
        .execute(
            "UPDATE mcp_servers SET env = '', env_ref = NULL, env_keys = '[]' WHERE id = ?1",
            [server_id],
        )
        .map_err(|_| "无法清除 MCP 凭据".to_string())?;
    transaction
        .execute(
            "DELETE FROM mcp_workspace_enablements WHERE server_id = ?1",
            [server_id],
        )
        .map_err(|_| "无法撤销 MCP 工作区授权".to_string())?;
    transaction
        .commit()
        .map_err(|_| "无法提交 MCP 凭据清除".to_string())
}

/// Best-effort cleanup of retired references, never of the active one.
pub(crate) fn reap_old_references(
    conn: &Connection,
    store: &impl McpCredentialStore,
) -> Result<(), String> {
    let mut statement = conn
        .prepare("SELECT env_ref FROM mcp_credential_gc ORDER BY env_ref")
        .map_err(|_| "无法读取 MCP 凭据清理队列".to_string())?;
    let refs = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| "无法读取 MCP 凭据清理队列".to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "无法读取 MCP 凭据清理队列".to_string())?;
    drop(statement);
    for reference in refs {
        validate_reference(&reference)?;
        let active: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM mcp_servers WHERE env_ref = ?1)",
                [&reference],
                |row| row.get(0),
            )
            .map_err(|_| "无法检查 MCP 凭据引用".to_string())?;
        if active {
            continue;
        }
        store.delete(&reference)?;
        conn.execute(
            "DELETE FROM mcp_credential_gc WHERE env_ref = ?1",
            [&reference],
        )
        .map_err(|_| "无法完成 MCP 凭据清理".to_string())?;
    }
    Ok(())
}

pub(crate) fn launch_env(vars: &[McpEnvVar]) -> String {
    vars.iter()
        .map(|var| format!("{}={}", var.key, var.value))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryStore {
        values: Mutex<HashMap<String, String>>,
        fail_write: AtomicBool,
        fail_delete: AtomicBool,
    }

    impl McpCredentialStore for MemoryStore {
        fn read(&self, reference: &str) -> Result<Option<String>, String> {
            Ok(self.values.lock().unwrap().get(reference).cloned())
        }

        fn write(&self, reference: &str, blob: &str) -> Result<(), String> {
            if self.fail_write.load(Ordering::SeqCst) {
                return Err("test vault write failure".to_string());
            }
            self.values
                .lock()
                .unwrap()
                .insert(reference.to_string(), blob.to_string());
            Ok(())
        }

        fn delete(&self, reference: &str) -> Result<(), String> {
            if self.fail_delete.load(Ordering::SeqCst) {
                return Err("test vault delete failure".to_string());
            }
            self.values.lock().unwrap().remove(reference);
            Ok(())
        }
    }

    fn database(legacy: &str) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE mcp_servers (
                id TEXT PRIMARY KEY,
                env TEXT,
                env_ref TEXT,
                env_keys TEXT NOT NULL DEFAULT '[]'
             );
             CREATE TABLE mcp_workspace_enablements (
                server_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL
             );
             CREATE TABLE mcp_credential_gc (env_ref TEXT PRIMARY KEY);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO mcp_servers (id, env) VALUES ('server-a', ?1)",
            [legacy],
        )
        .unwrap();
        conn
    }

    fn var(key: &str, value: &str) -> McpEnvVar {
        McpEnvVar {
            key: key.to_string(),
            value: value.to_string(),
        }
    }

    #[test]
    fn migration_writes_and_verifies_vault_before_scrubbing_legacy_plaintext() {
        let mut conn = database("TOKEN=secret-value=part\nSECOND=other");
        let store = MemoryStore::default();
        let vars = read_effective_env(&mut conn, &store, "server-a").unwrap();
        assert_eq!(
            vars,
            vec![var("SECOND", "other"), var("TOKEN", "secret-value=part")]
        );

        let (legacy, reference, keys): (String, String, String) = conn
            .query_row(
                "SELECT env, env_ref, env_keys FROM mcp_servers WHERE id = 'server-a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!(legacy.is_empty());
        assert!(reference.starts_with(REFERENCE_PREFIX));
        assert_eq!(keys, r#"["SECOND","TOKEN"]"#);
        assert_eq!(
            read_effective_env(&mut conn, &store, "server-a").unwrap(),
            vars
        );
    }

    #[test]
    fn vault_write_failure_preserves_legacy_and_never_creates_reference() {
        let mut conn = database("TOKEN=secret-value");
        let store = MemoryStore::default();
        store.fail_write.store(true, Ordering::SeqCst);
        assert!(read_effective_env(&mut conn, &store, "server-a").is_err());
        let row: (String, Option<String>) = conn
            .query_row(
                "SELECT env, env_ref FROM mcp_servers WHERE id = 'server-a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row.0, "TOKEN=secret-value");
        assert!(row.1.is_none());
        assert!(store.values.lock().unwrap().is_empty());
    }

    #[test]
    fn missing_reference_fails_closed_even_if_stale_legacy_value_exists() {
        let mut conn = database("TOKEN=stale");
        let store = MemoryStore::default();
        let reference = new_reference();
        conn.execute(
            "UPDATE mcp_servers SET env_ref = ?1 WHERE id = 'server-a'",
            [&reference],
        )
        .unwrap();
        assert!(read_effective_env(&mut conn, &store, "server-a").is_err());
        let legacy: String = conn
            .query_row(
                "SELECT env FROM mcp_servers WHERE id = 'server-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy, "TOKEN=stale");
    }

    #[test]
    fn plain_backup_variable_names_without_vault_values_fail_closed() {
        let mut conn = database("");
        conn.execute(
            "UPDATE mcp_servers SET env_keys = '[\"TOKEN\"]' WHERE id = 'server-a'",
            [],
        )
        .unwrap();
        assert!(read_effective_env(&mut conn, &MemoryStore::default(), "server-a").is_err());
    }

    #[test]
    fn replacement_swaps_reference_revokes_grants_and_reaps_old_blob() {
        let mut conn = database("");
        let store = MemoryStore::default();
        conn.execute(
            "INSERT INTO mcp_workspace_enablements (server_id, workspace_id) VALUES ('server-a', 'personal')",
            [],
        )
        .unwrap();
        replace_env(&mut conn, &store, "server-a", &[var("TOKEN", "first")]).unwrap();
        let first_ref: String = conn
            .query_row(
                "SELECT env_ref FROM mcp_servers WHERE id = 'server-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let grants: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mcp_workspace_enablements",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(grants, 0);

        replace_env(&mut conn, &store, "server-a", &[var("TOKEN", "second")]).unwrap();
        let second_ref: String = conn
            .query_row(
                "SELECT env_ref FROM mcp_servers WHERE id = 'server-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_ne!(first_ref, second_ref);
        assert!(!store.values.lock().unwrap().contains_key(&first_ref));
        assert_eq!(
            read_effective_env(&mut conn, &store, "server-a").unwrap(),
            vec![var("TOKEN", "second")]
        );
    }

    #[test]
    fn failed_old_blob_deletion_is_queued_without_restoring_old_reference() {
        let mut conn = database("");
        let store = MemoryStore::default();
        replace_env(&mut conn, &store, "server-a", &[var("TOKEN", "first")]).unwrap();
        let old_ref: String = conn
            .query_row(
                "SELECT env_ref FROM mcp_servers WHERE id = 'server-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        store.fail_delete.store(true, Ordering::SeqCst);
        replace_env(&mut conn, &store, "server-a", &[var("TOKEN", "second")]).unwrap();
        let active_ref: String = conn
            .query_row(
                "SELECT env_ref FROM mcp_servers WHERE id = 'server-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_ne!(active_ref, old_ref);
        let queued: String = conn
            .query_row("SELECT env_ref FROM mcp_credential_gc", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(queued, old_ref);
        store.fail_delete.store(false, Ordering::SeqCst);
        reap_old_references(&conn, &store).unwrap();
        assert!(!store.values.lock().unwrap().contains_key(&old_ref));
    }

    #[test]
    fn failed_replacement_write_preserves_active_reference_and_grants() {
        let mut conn = database("");
        let store = MemoryStore::default();
        replace_env(&mut conn, &store, "server-a", &[var("TOKEN", "first")]).unwrap();
        let active_ref: String = conn
            .query_row(
                "SELECT env_ref FROM mcp_servers WHERE id = 'server-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO mcp_workspace_enablements (server_id, workspace_id) VALUES ('server-a', 'personal')",
            [],
        )
        .unwrap();
        store.fail_write.store(true, Ordering::SeqCst);
        assert!(replace_env(&mut conn, &store, "server-a", &[var("TOKEN", "second")]).is_err());
        let after: String = conn
            .query_row(
                "SELECT env_ref FROM mcp_servers WHERE id = 'server-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(after, active_ref);
        assert_eq!(
            read_effective_env(&mut conn, &store, "server-a").unwrap(),
            vec![var("TOKEN", "first")]
        );
        let grants: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mcp_workspace_enablements",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(grants, 1);
    }

    #[test]
    fn vault_delete_failure_preserves_reference_and_grants() {
        let mut conn = database("");
        let store = MemoryStore::default();
        replace_env(&mut conn, &store, "server-a", &[var("TOKEN", "first")]).unwrap();
        conn.execute(
            "INSERT INTO mcp_workspace_enablements (server_id, workspace_id) VALUES ('server-a', 'personal')",
            [],
        )
        .unwrap();
        store.fail_delete.store(true, Ordering::SeqCst);
        assert!(delete_env(&mut conn, &store, "server-a").is_err());
        assert_eq!(
            read_effective_env(&mut conn, &store, "server-a").unwrap(),
            vec![var("TOKEN", "first")]
        );
        let grants: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mcp_workspace_enablements",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(grants, 1);
    }

    #[test]
    fn legacy_parser_rejects_malformed_lines_and_redacts_debug_output() {
        assert!(parse_legacy_env("TOKEN=one\nnot-a-pair").is_err());
        assert_eq!(
            parse_legacy_env("TOKEN=one\nTOKEN=two").unwrap(),
            vec![var("TOKEN", "two")]
        );
        assert!(!format!("{:?}", var("TOKEN", "highly-sensitive")).contains("highly-sensitive"));
        assert!(validate_env_vars(&[var("TOKEN", " a ")]).is_err());
    }
}

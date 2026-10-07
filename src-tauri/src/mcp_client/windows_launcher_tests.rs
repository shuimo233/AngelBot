//! Windows launcher regressions: exercise the actual MCP manager through a
//! batch wrapper, not just `Command::new` in isolation.

use super::{resolve_mcp_executable, McpProcessManager};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::{Builder, TempDir};

struct LauncherFixture {
    // Keep the directory alive until the process tree has been terminated.
    _directory: TempDir,
    manager: McpProcessManager,
}

impl Drop for LauncherFixture {
    fn drop(&mut self) {
        self.manager.stop_all();
    }
}

#[test]
fn only_bare_npx_uses_the_windows_batch_alias() {
    assert_eq!(resolve_mcp_executable("npx"), "npx.cmd");
    assert_eq!(resolve_mcp_executable("  NpX  "), "npx.cmd");
    assert_eq!(resolve_mcp_executable("node"), "node");
    assert_eq!(resolve_mcp_executable("npx.cmd"), "npx.cmd");

    // Outer settings quotes are removed, but an explicit path is not
    // rewritten to an unrelated executable on PATH.
    assert_eq!(
        resolve_mcp_executable(r#""C:\Program Files\nodejs\npx.cmd""#),
        r"C:\Program Files\nodejs\npx.cmd"
    );
    assert_eq!(
        resolve_mcp_executable(r#""C:\Program Files\nodejs\npx""#),
        r"C:\Program Files\nodejs\npx"
    );
}

#[test]
fn manager_launches_cmd_wrapper_from_path_with_spaces() {
    // The repository's existing MCP stdio fixtures use Node. Confirm the
    // executable is available before the batch file depends on PATH too.
    let node = Command::new("node.exe")
        .arg("--version")
        .output()
        .expect("Node executable is required for MCP stdio tests");
    assert!(node.status.success(), "Node executable did not start");

    let directory = Builder::new()
        .prefix("angelbot mcp cmd launcher ")
        .tempdir()
        .unwrap();
    assert!(
        directory
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(' '),
        "the temporary directory must exercise a path with spaces"
    );

    let script = directory.path().join("mcp server.cjs");
    fs::write(
        &script,
        r#"
const readline = require('readline');
if (process.argv[2] !== 'launch argument with spaces') process.exit(2);
readline.createInterface({ input: process.stdin }).on('line', line => {
  const request = JSON.parse(line);
  let result;
  if (request.method === 'initialize') {
    result = {
      protocolVersion: '2024-11-05', capabilities: { tools: {} },
      serverInfo: { name: 'cmd-fixture', version: '1.0.0' }
    };
  } else if (request.method === 'tools/list') {
    result = { tools: [{ name: 'echo', description: 'Offline fixture',
      inputSchema: { type: 'object', properties: { message: { type: 'string' } } }
    }] };
  } else if (request.method === 'tools/call') {
    result = { content: [{ type: 'text', text: request.params.arguments.message }] };
  } else {
    return;
  }
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result }) + '\n');
});
"#,
    )
    .unwrap();

    let wrapper = directory.path().join("launch server.cmd");
    fs::write(
        &wrapper,
        b"@echo off\r\nnode.exe \"%~dp0mcp server.cjs\" %*\r\n",
    )
    .unwrap();

    let fixture = LauncherFixture {
        _directory: directory,
        manager: McpProcessManager::new(),
    };
    let command = wrapper.to_str().expect("batch path must be Unicode");
    let args = serde_json::to_string(&["launch argument with spaces"]).unwrap();
    fixture
        .manager
        .start("cmd-fixture", "CMD fixture", command, &args, "")
        .unwrap();
    assert_eq!(
        fixture.manager.get_status("cmd-fixture").unwrap().status,
        "running"
    );

    let tools = fixture.manager.refresh_tools("cmd-fixture").unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(
        fixture
            .manager
            .get_status("cmd-fixture")
            .unwrap()
            .tools_count,
        Some(1)
    );
    assert_eq!(
        fixture
            .manager
            .call_tool_if_current_snapshot(
                "cmd-fixture",
                "echo",
                &tools[0].snapshot_for("cmd-fixture"),
                &serde_json::json!({ "message": "through cmd" }),
                || Ok(()),
            )
            .unwrap(),
        "through cmd"
    );

    fixture.manager.stop("cmd-fixture").unwrap();
    assert_eq!(
        fixture.manager.get_status("cmd-fixture").unwrap().status,
        "stopped"
    );
    assert!(fixture.manager.get_cached_tools("cmd-fixture").is_empty());
}

#[test]
fn manager_launches_real_npx_offline_fixture() {
    let directory = Builder::new()
        .prefix("angelbot mcp npx ")
        .tempdir()
        .unwrap();
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the Rust crate must be inside the repository")
        .join("e2e/fixtures/mcp-stdio.cjs");
    assert!(
        fixture_path.is_file(),
        "the local MCP stdio fixture must exist"
    );
    let node_call = format!(r#"node "{}""#, fixture_path.display());
    let args = serde_json::to_string(&["--offline", "--yes=false", "--call", &node_call]).unwrap();
    let env = format!(
        "NPM_CONFIG_CACHE={}\n\
         NPM_CONFIG_USERCONFIG={}\n\
         NPM_CONFIG_GLOBALCONFIG={}\n\
         NPM_CONFIG_PREFIX={}\n\
         NPM_CONFIG_OFFLINE=true\n\
         NPM_CONFIG_YES=false\n\
         NPM_CONFIG_UPDATE_NOTIFIER=false\n\
         NPM_CONFIG_AUDIT=false\n\
         NPM_CONFIG_FUND=false",
        directory.path().join("cache").display(),
        directory.path().join("user.npmrc").display(),
        directory.path().join("global.npmrc").display(),
        directory.path().join("prefix").display(),
    );
    let fixture = LauncherFixture {
        _directory: directory,
        manager: McpProcessManager::new(),
    };

    fixture
        .manager
        .start("npx-fixture", "npx fixture", "npx", &args, &env)
        .unwrap();
    assert_eq!(
        fixture.manager.get_status("npx-fixture").unwrap().status,
        "running"
    );

    let tools = fixture.manager.refresh_tools("npx-fixture").unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "angelbot_e2e_ping");

    fixture.manager.stop("npx-fixture").unwrap();
    assert_eq!(
        fixture.manager.get_status("npx-fixture").unwrap().status,
        "stopped"
    );
    assert!(fixture.manager.get_cached_tools("npx-fixture").is_empty());
}

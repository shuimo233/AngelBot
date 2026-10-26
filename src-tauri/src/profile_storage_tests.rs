use super::*;

fn fixture_paths(root: &Path) -> Paths {
    Paths {
        legacy: root.join("angelbot"),
        current: root.join(DIRECTORY),
        config: root.join("roaming/AngelBot/app_config.json"),
    }
}

fn fixture() -> (tempfile::TempDir, Paths) {
    let temp = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temp.path());
    fs::create_dir_all(paths.legacy.join("results")).unwrap();
    fs::create_dir_all(paths.legacy.join("skills/local")).unwrap();
    fs::create_dir_all(paths.config.parent().unwrap()).unwrap();
    fs::write(
        paths.config.clone(),
        br#"{"provider":"openai","model":"gpt-6-luna","credential_ref":"opaque-fixture"}"#,
    )
    .unwrap();
    fs::write(
        paths.legacy.join("results/清单.csv"),
        "事项,状态\n验收,完成\n",
    )
    .unwrap();
    fs::write(
        paths.legacy.join("skills/local/SKILL.md"),
        "# Fixture skill",
    )
    .unwrap();
    fs::write(paths.legacy.join("chatgpt-plan.credentials"), [7; 80]).unwrap();
    // Installer payload/unknown files are not profile-owned migration entries.
    fs::write(paths.legacy.join("angelbot.exe"), b"not-a-program").unwrap();
    fs::write(
        paths.legacy.join("keep-user-file.txt"),
        b"not-migration-owned",
    )
    .unwrap();
    fs::create_dir_all(paths.current.join("EBWebView")).unwrap();
    fs::write(paths.current.join("EBWebView/cache"), b"existing-webview").unwrap();
    unsafe {
        crate::vector_store::load_extension_globally();
    }
    let conn = crate::db::init(&paths.legacy.join("data.db")).unwrap();
    crate::db::migrate(&conn).unwrap();
    conn.execute("INSERT OR IGNORE INTO sessions(id,title,created_at,updated_at) VALUES('migration-main','Fixture',1,1)", []).unwrap();
    conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES('migration-reply','migration-main','assistant','old-history-canary',1)", []).unwrap();
    let args = serde_json::json!([
        "-y",
        "@playwright/mcp@0.0.83",
        "--isolated",
        "--output-dir",
        paths.legacy.join("results").to_string_lossy()
    ])
    .to_string();
    conn.execute("INSERT INTO mcp_servers(id,name,command,args,enabled,created_at) VALUES('browser','Fixture','npx.cmd',?1,1,1)", [&args]).unwrap();
    conn.execute("INSERT INTO mcp_tool_catalog(server_id,config_fingerprint,tools_json,updated_at) VALUES('browser',?1,'[]',1)", [crate::commands::mcp_catalog::configuration_fingerprint("npx.cmd", &args)]).unwrap();
    let custom_args =
        serde_json::json!([paths.legacy.join("results").to_string_lossy()]).to_string();
    conn.execute("INSERT INTO mcp_servers(id,name,command,args,enabled,created_at) VALUES('custom','Custom fixture','custom.exe',?1,1,1)", [&custom_args]).unwrap();
    let extra_args = serde_json::json!([
        "@playwright/mcp@0.0.83",
        "--output-dir",
        paths.legacy.join("results").to_string_lossy(),
        "--config",
        paths.legacy.join("browser.json").to_string_lossy()
    ])
    .to_string();
    conn.execute("INSERT INTO mcp_servers(id,name,command,args,enabled,created_at) VALUES('browser-extra','Custom browser fixture','npx.cmd',?1,1,1)", [&extra_args]).unwrap();
    conn.execute("INSERT INTO mcp_servers(id,name,command,args,enabled,created_at) VALUES('unrelated','Other fixture','other.exe','[]',1,1)", []).unwrap();
    let output = crate::browser_storage::prepare_output(&conn, "managed-browser").unwrap();
    fs::write(output.join("keep-runtime.yml"), "runtime canary").unwrap();
    let managed_args = serde_json::json!([
        "-y",
        "@playwright/mcp@0.0.83",
        "--output-dir",
        crate::commands::file::display_work_dir(&output)
    ])
    .to_string();
    conn.execute("INSERT INTO mcp_servers(id,name,command,args,enabled,created_at) VALUES('managed-browser','Managed browser','npx.cmd',?1,1,1)", [&managed_args]).unwrap();
    drop(conn);
    (temp, paths)
}

#[test]
fn cutover_preserves_data_config_key_binding_and_only_rebases_owned_browser_output() {
    let (_temp, paths) = fixture();
    let source_hash = fingerprint(&paths.legacy.join("data.db")).unwrap();
    let config = fs::read(&paths.config).unwrap();
    let lease = open(&paths, false, false).unwrap();
    let (borrowed, read_guard) = borrow_saved_for(&paths).unwrap();
    assert_eq!(borrowed, paths.current);
    drop(read_guard);
    assert_eq!(lease.root, paths.current);
    assert_eq!(saved_root_for(&paths), paths.current);
    assert_eq!(
        config_directory_for(&paths),
        paths.current.join("model-config")
    );
    assert!(lease.migrated);
    assert!(!paths.legacy.join("data.db").exists());
    assert_eq!(fingerprint(&paths.backup("data.db")).unwrap(), source_hash);
    assert_eq!(
        fs::read(paths.current.join("model-config/app_config.json")).unwrap(),
        config
    );
    assert_eq!(
        fs::read_to_string(paths.current.join("results/清单.csv")).unwrap(),
        "事项,状态\n验收,完成\n"
    );
    assert_eq!(
        fs::read_to_string(paths.current.join("skills/local/SKILL.md")).unwrap(),
        "# Fixture skill"
    );
    assert!(paths.legacy.join("angelbot.exe").exists());
    assert!(paths.legacy.join("keep-user-file.txt").exists());
    assert_eq!(
        fs::read(paths.current.join("EBWebView/cache")).unwrap(),
        b"existing-webview"
    );
    assert_ne!(
        fs::read(paths.current.join("chatgpt-plan.credentials")).unwrap(),
        [7; 80]
    );
    // Both profile guards remain live, but model renewal locks must be usable.
    assert!(crate::runtime_instance::acquire(&paths.legacy).is_err());
    assert!(crate::runtime_instance::acquire(&paths.current).is_err());
    let renew = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(paths.current.join("chatgpt-plan.lock"))
        .unwrap();
    renew.try_lock_exclusive().unwrap();
    FileExt::unlock(&renew).unwrap();
    let conn = crate::db::init(&paths.current.join("data.db")).unwrap();
    assert_eq!(
        crate::commands::file::personal_files_root(&conn).unwrap(),
        paths.current.canonicalize().unwrap().join("results")
    );
    assert_eq!(
        conn.query_row(
            "SELECT enabled FROM mcp_servers WHERE id='browser-extra'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0,
    );
    assert_eq!(
        conn.query_row(
            "SELECT enabled FROM mcp_servers WHERE id='custom'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT enabled FROM mcp_servers WHERE id='unrelated'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let history: String = conn
        .query_row(
            "SELECT content FROM messages WHERE id='migration-reply'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(history, "old-history-canary");
    let managed_args: String = conn
        .query_row(
            "SELECT args FROM mcp_servers WHERE id='managed-browser' AND enabled=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let managed_output =
        crate::browser_storage::configured_output(&conn, "managed-browser", &managed_args)
            .unwrap()
            .unwrap();
    assert!(managed_output.starts_with(paths.current.canonicalize().unwrap()));
    assert_eq!(
        fs::read_to_string(managed_output.join("keep-runtime.yml")).unwrap(),
        "runtime canary"
    );
    let args: String = conn
        .query_row(
            "SELECT args FROM mcp_servers WHERE id='browser'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let args: Vec<String> = serde_json::from_str(&args).unwrap();
    assert_eq!(
        Path::new(args.last().unwrap()),
        paths.current.join("results")
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM mcp_tool_catalog", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(conn);
    drop(lease);
    let again = open(&paths, false, false).unwrap();
    assert!(!again.migrated);
}

#[test]
fn rollback_and_interrupted_cutover_never_overwrite_changed_data() {
    if let Some(config) = std::env::var_os("ANGELBOT_PRIVATE_VERIFY_CONFIG_DIR").map(PathBuf::from)
    {
        if let Ok(mode) = fs::read_to_string(config.join("storage-child")) {
            unsafe {
                crate::vector_store::load_extension_globally();
            }
            let paths = fixture_paths(config.parent().unwrap());
            if mode == "exit-after-wal" {
                let conn = crate::db::init(&paths.legacy.join("data.db")).unwrap();
                conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0")
                    .unwrap();
                conn.execute("INSERT INTO messages(id,session_id,role,content,created_at) VALUES('wal-reply','migration-main','assistant','wal-only-canary',2)", []).unwrap();
                std::process::exit(94);
            }
            let _ = open_observed(&paths, false, mode == "exit-during-rollback", |stage| {
                if mode == "exit-after-installed" && stage == "installed" {
                    std::process::exit(93);
                }
                if mode == "exit-during-rollback" && stage == "rollback-restored" {
                    std::process::exit(92);
                }
                Ok(())
            });
            panic!("child did not reach interruption checkpoint");
        }
    }
    for fail_at in ["prepared", "retired", "installed", "config-backed-up"] {
        let (_temp, paths) = fixture();
        let before = fingerprint(&paths.legacy.join("data.db")).unwrap();
        let error = open_observed(&paths, false, false, |stage| {
            if stage == fail_at {
                Err(failure("injected"))
            } else {
                Ok(())
            }
        });
        assert!(error.is_err());
        assert_eq!(fingerprint(&paths.legacy.join("data.db")).unwrap(), before);
        assert_eq!(read_journal(&paths).unwrap().unwrap().phase, "rolled_back");
        let retried = open(&paths, true, false).unwrap();
        drop(retried);
        let rolled_back = open(&paths, false, true).unwrap();
        assert!(rolled_back.legacy_recovery);
        assert_eq!(saved_root_for(&paths), paths.legacy);
        assert_eq!(config_directory_for(&paths), paths.config.parent().unwrap());
        drop(rolled_back);
        assert_eq!(fingerprint(&paths.legacy.join("data.db")).unwrap(), before);
    }
    let (_temp, paths) = fixture();
    drop(open(&paths, false, false).unwrap());
    fs::write(
        paths.current.join("results/new.txt"),
        b"new-data-must-survive",
    )
    .unwrap();
    assert!(open(&paths, false, true).is_err());
    assert_eq!(read_journal(&paths).unwrap().unwrap().phase, "complete");
    drop(open(&paths, false, false).unwrap());
    assert_eq!(
        fs::read(paths.current.join("results/new.txt")).unwrap(),
        b"new-data-must-survive"
    );
    assert!(!paths.legacy.join("data.db").exists());
    assert!(paths.backup("data.db").exists());
    let (_temp, paths) = fixture();
    let panic = std::panic::catch_unwind(|| {
        let _ = open_observed(&paths, false, false, |stage| {
            if stage == "installed" {
                panic!("simulated process loss");
            }
            Ok(())
        });
    });
    assert!(panic.is_err());
    // A new owner recovers the filesystem journal, then attempts one cutover.
    drop(open(&paths, false, false).unwrap());
    assert_eq!(read_journal(&paths).unwrap().unwrap().phase, "complete");
    for (mode, exit_code) in [
        ("exit-after-installed", 93),
        ("exit-after-wal", 94),
        ("exit-during-rollback", 92),
    ] {
        let (_temp, paths) = fixture();
        if mode == "exit-during-rollback" {
            drop(open(&paths, false, false).unwrap());
        }
        let root = paths.legacy.parent().unwrap();
        let config = root.join("process-config");
        fs::create_dir(&config).unwrap();
        fs::write(config.join("storage-child"), mode).unwrap();
        let output = File::create(root.join("child.log")).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "profile_storage::tests::rollback_and_interrupted_cutover_never_overwrite_changed_data",
            "--exact",
            "--test-threads=1",
        ])
        .env("ANGELBOT_PRIVATE_VERIFY_CONFIG_DIR", &config)
        .stdin(std::process::Stdio::null())
        .stdout(output.try_clone().unwrap())
        .stderr(output)
        .spawn()
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("owned storage child timed out");
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        };
        assert_eq!(
            status.code(),
            Some(exit_code),
            "{}",
            fs::read_to_string(root.join("child.log")).unwrap()
        );
        if mode == "exit-during-rollback" {
            assert_eq!(read_journal(&paths).unwrap().unwrap().phase, "rolling_back");
            let recovered = open(&paths, false, false).unwrap();
            assert!(recovered.legacy_recovery);
            drop(recovered);
            assert!(paths.legacy.join("data.db").exists());
            drop(open(&paths, true, false).unwrap());
        } else {
            if mode == "exit-after-wal" {
                assert!(
                    fs::metadata(paths.legacy.join("data.db-wal"))
                        .unwrap()
                        .len()
                        > 0
                );
            } else {
                assert_eq!(read_journal(&paths).unwrap().unwrap().phase, "prepared");
            }
            drop(open(&paths, false, false).unwrap());
        }
        assert_eq!(read_journal(&paths).unwrap().unwrap().phase, "complete");
        assert_eq!(
            fs::read_to_string(paths.current.join("results/清单.csv")).unwrap(),
            "事项,状态\n验收,完成\n"
        );
        if mode == "exit-after-wal" {
            assert!(paths.backup("data.db-wal").exists());
            let conn = crate::db::init(&paths.current.join("data.db")).unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT content FROM messages WHERE id='wal-reply'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
                "wal-only-canary"
            );
        }
    }
}

#[test]
fn conflicts_busy_roots_bad_journals_and_links_fail_without_touching_sources() {
    assert!(Lease::isolated(PathBuf::from("relative-profile-must-not-be-created")).is_err());
    let fresh = tempfile::tempdir().unwrap();
    let fresh_paths = fixture_paths(fresh.path());
    let new_profile = open(&fresh_paths, false, false).unwrap();
    assert_eq!(new_profile.root, fresh_paths.current);
    drop(new_profile);
    let (_temp, paths) = fixture();
    fs::create_dir_all(paths.legacy.join("delegated/task")).unwrap();
    fs::write(
        paths.legacy.join("delegated/task/.git"),
        b"gitdir: outside-managed-root",
    )
    .unwrap();
    assert!(open(&paths, false, false).is_err());
    assert!(paths.legacy.join("data.db").exists());
    let (_temp, paths) = fixture();
    let before = fingerprint(&paths.legacy.join("data.db")).unwrap();
    let (_, reader) = borrow_saved_for(&paths).unwrap();
    assert!(open(&paths, false, false).is_err());
    assert_eq!(fingerprint(&paths.legacy.join("data.db")).unwrap(), before);
    drop(reader);
    let busy = crate::runtime_instance::acquire(&paths.legacy).unwrap();
    assert!(open(&paths, false, false).is_err());
    drop(busy);
    fs::write(paths.current.join("data.db"), b"existing-new-profile").unwrap();
    assert!(open(&paths, false, false).is_err());
    assert!(borrow_saved_for(&paths).is_err());
    assert_eq!(
        fs::read(paths.current.join("data.db")).unwrap(),
        b"existing-new-profile"
    );
    assert_eq!(fingerprint(&paths.legacy.join("data.db")).unwrap(), before);
    let (_temp, paths) = fixture();
    fs::create_dir(paths.journal()).unwrap();
    fs::write(paths.journal().join("state.json"), br#"{"version":1,"phase":"complete","entries":[{"name":"../outside","source_hash":"x","target_hash":"x"}]}"#).unwrap();
    assert!(open(&paths, false, false).is_err());
    assert!(paths.legacy.join("data.db").exists());
    assert!(borrow_saved_for(&paths).is_err());
    let (_temp, paths) = fixture();
    let error = open_observed(&paths, false, false, |stage| {
        if stage == "retired" {
            fs::write(paths.current.join("data.db"), b"new-foreign-data")?;
        }
        Ok(())
    });
    assert!(error.is_err());
    assert!(borrow_saved_for(&paths).is_err());
    assert_eq!(
        fs::read(paths.current.join("data.db")).unwrap(),
        b"new-foreign-data"
    );
    assert!(paths.backup("data.db").exists());
    #[cfg(windows)]
    {
        let (_temp, paths) = fixture();
        // Junctions need no symlink privilege; never follow one during copy.
        let external = paths.current.join("EBWebView");
        let link = paths.legacy.join("results").join("redirect");
        let status = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&external)
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );
        assert!(open(&paths, false, false).is_err());
        assert_eq!(
            fs::read(external.join("cache")).unwrap(),
            b"existing-webview"
        );
    }
}

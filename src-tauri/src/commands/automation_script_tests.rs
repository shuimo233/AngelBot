use super::{
    create_automation_impl, execute_owned_automation, execute_script_command,
    get_automation_runs_impl, get_automations_impl, script_admission, script_command,
    stop_automation_run_impl, ScriptRunResult, MAX_CAPTURED_OUTPUT_BYTES,
};
use std::{
    fs,
    path::Path,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

fn run_node_script(source: &str, args: &[String], timeout_seconds: u64) -> ScriptRunResult {
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("fixture.cjs");
    fs::write(&script, source).unwrap();
    let command = script_command(&script, args, directory.path()).unwrap();
    execute_script_command(command, timeout_seconds, &|| false).unwrap()
}

#[test]
fn script_exit_and_stream_identity_are_preserved() {
    // Closing stderr before stdout used to make the anonymous mpsc messages
    // switch their labels depending on reader completion order.
    let result = run_node_script(
        "const fs = require('fs'); fs.writeSync(2, 'early error'); fs.closeSync(2); \
         setTimeout(() => fs.writeSync(1, 'late output'), 100);",
        &[],
        30,
    );
    assert_eq!(result.status, "completed");
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.summary, "脚本执行完成");
    assert_eq!(result.output, "late output\n[stderr]\nearly error");

    let failed = run_node_script("process.exitCode = 7; console.error('failed');", &[], 30);
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.exit_code, Some(7));
    assert_eq!(failed.output, "[stderr]\nfailed");
}

#[test]
fn output_flood_is_bounded_per_stream_and_fully_drained() {
    let result = run_node_script(
        "const fs = require('fs'); fs.writeSync(1, Buffer.alloc(2 * 1024 * 1024, 'O')); \
         fs.writeSync(2, Buffer.alloc(2 * 1024 * 1024, 'E'));",
        &[],
        30,
    );
    assert_eq!(result.status, "completed");
    assert_eq!(
        result.output.matches('O').count(),
        MAX_CAPTURED_OUTPUT_BYTES
    );
    assert_eq!(
        result.output.matches('E').count(),
        MAX_CAPTURED_OUTPUT_BYTES
    );
    assert!(result.output.contains("[stdout 输出已截断]\n[stderr]\n"));
    assert!(result.output.ends_with("[stderr 输出已截断]"));
    assert!(result.output.len() <= 2 * MAX_CAPTURED_OUTPUT_BYTES + 128);
}

#[test]
fn script_timeout_stops_before_joining_output_readers() {
    let started = Instant::now();
    let result = run_node_script(
        "process.stdout.write('running'); setInterval(() => {}, 1000);",
        &[],
        1,
    );
    assert_eq!(result.status, "timed_out");
    // Interpreter readiness is not a promise: a cold host can legitimately
    // time out before its first output. Only termination is the contract here.
    assert!(started.elapsed().as_secs() < 30);

    #[cfg(windows)]
    {
        // Cross the real owned-process seam: the child holds both output handles,
        // so returning from Stop requires reclaiming descendants before drain.
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("stop-fixture.cjs");
        let ready = directory.path().join("child-ready");
        fs::write(&script, "const {spawn}=require('child_process'); \
        spawn(process.execPath, ['-e', \
        \"const fs=require('fs'); fs.writeSync(1,'child-ready'); fs.writeFileSync(process.argv[1], 'ready'); setInterval(()=>{},1000);\", \
        process.argv[2]], {stdio:'inherit',windowsHide:true}); setInterval(()=>{},1000);").unwrap();
        let state = Arc::new(crate::tests::test_app_state());
        {
            let conn = state.db.lock().unwrap();
            crate::db::migrate(&conn).unwrap();
            conn.execute(
            "INSERT OR REPLACE INTO settings(key,value,updated_at) VALUES('work_directory',?1,1)",
            [directory.path().to_string_lossy().as_ref()],
        )
        .unwrap();
        }
        let automation = create_automation_impl(
            &state,
            "Owned stop fixture".into(),
            String::new(),
            "schedule".into(),
            "每天 09:00".into(),
            String::new(),
            Some("script".into()),
            None,
            Some(script.to_string_lossy().into_owned()),
            Some(vec![ready.to_string_lossy().into_owned()]),
            None,
            Some(30),
        )
        .unwrap();
        let script_admission::Admission::Started(ticket) = script_admission::admit(
            &state,
            &automation.id,
            script_admission::Origin::Manual,
            15,
            &|| {},
        )
        .unwrap() else {
            panic!("expected live script ticket");
        };
        let run_id = ticket.run_id.clone();
        let worker_state = state.clone();
        let worker = thread::spawn(move || {
            script_admission::execute_ticket(
                &worker_state,
                ticket,
                &|| {},
                &execute_owned_automation,
            )
        });
        let started = Instant::now();
        while !ready.is_file()
            && !worker.is_finished()
            && started.elapsed() < Duration::from_secs(15)
        {
            thread::sleep(Duration::from_millis(20));
        }
        let child_started = ready.is_file();
        let stopped = stop_automation_run_impl(&state, &automation.id, &run_id);
        worker.join().unwrap().unwrap();
        assert!(
            stopped.is_ok(),
            "fixture stop failed: {stopped:?}; owned receipts: {:?}",
            get_automation_runs_impl(&state, &automation.id).unwrap()
        );
        assert!(
            child_started,
            "owned child must start before requesting Stop"
        );
        assert!(started.elapsed() < Duration::from_secs(25));
        let runs = get_automation_runs_impl(&state, &automation.id).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, run_id);
        assert_eq!(runs[0].status, "cancelled");
        assert!(runs[0].stop_control.is_none());
        assert!(runs[0].output.contains("child-ready"));
        let saved = get_automations_impl(&state).unwrap().remove(0);
        assert!(saved.enabled);
        assert_eq!(saved.next_run_at, automation.next_run_at);
    }
}

#[test]
fn direct_script_arguments_are_literal_and_child_environment_is_minimal() {
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("fixture.cjs");
    fs::write(
        &script,
        "console.log(JSON.stringify({args: process.argv.slice(2), \
         secret: process.env.ANGELBOT_MODEL_TEST_SECRET ?? null}));",
    )
    .unwrap();
    let args = vec![
        "one two".to_string(),
        "& literal".to_string(),
        "%PATH%".to_string(),
        "\"quoted\"".to_string(),
        "!literal!".to_string(),
        r"\\?\C:\keep literal".to_string(),
    ];
    let script = fs::canonicalize(script).unwrap();
    let working_dir = fs::canonicalize(directory.path()).unwrap();
    let mut command = script_command(&script, &args, &working_dir).unwrap();
    // A synthetic credential on this Command avoids mutating the global test
    // environment or reading any real user's model credentials.
    command.env("ANGELBOT_MODEL_TEST_SECRET", "must-not-leak");
    let result = execute_script_command(command, 30, &|| false).unwrap();
    assert_eq!(result.status, "completed");
    let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();
    assert_eq!(output["args"], serde_json::json!(args));
    assert!(output["secret"].is_null());
}

#[test]
fn batch_support_rejects_shell_text_without_removing_literal_arguments() {
    for extension in ["cmd", "bat"] {
        let script = format!("literal.{extension}");
        let args = vec!["--mode".to_string(), "game name".to_string()];
        let command = script_command(Path::new(&script), &args, Path::new(".")).unwrap();
        assert_eq!(command.get_program(), "cmd");
        for argument in [
            "a&b", "a|b", "a>b", "a<b", "a^b", "a(b)", "%PATH%", "!PATH!", "\"a\"", "a\nb", "a\rb",
            "a\tb",
        ] {
            assert!(
                script_command(Path::new(&script), &[argument.to_string()], Path::new("."))
                    .is_err(),
                "accepted batch shell text: {argument:?}"
            );
        }
        assert!(script_command(
            Path::new(&format!("unsafe%path.{extension}")),
            &[],
            Path::new(".")
        )
        .is_err());
    }
    assert!(script_command(
        Path::new("literal.cjs"),
        &["a\0b".to_string()],
        Path::new(".")
    )
    .is_err());
}

#[test]
fn invalid_script_definition_is_rejected_before_persistence() {
    let directory = tempfile::tempdir().unwrap();
    let state = crate::tests::test_app_state();
    {
        let connection = state.db.lock().unwrap();
        crate::db::migrate(&connection).unwrap();
        connection
            .execute(
                "INSERT OR REPLACE INTO settings (key,value,updated_at) VALUES ('work_directory',?1,1)",
                [directory.path().to_string_lossy().as_ref()],
            )
            .unwrap();
    }
    for (filename, arguments) in [
        ("unsupported.exe", vec![]),
        ("unsafe.cmd", vec!["& unexpected".to_string()]),
    ] {
        let script = directory.path().join(filename);
        fs::write(&script, "must not execute").unwrap();
        let result = create_automation_impl(
            &state,
            "Invalid script".to_string(),
            String::new(),
            "schedule".to_string(),
            "每天 09:00".to_string(),
            String::new(),
            Some("script".to_string()),
            None,
            Some(script.to_string_lossy().into_owned()),
            Some(arguments),
            None,
            None,
        );
        assert!(result.is_err(), "persisted invalid script: {filename}");
    }
    let count: i64 = state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM automations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[cfg(windows)]
#[test]
fn literal_batch_arguments_and_paths_with_spaces_still_execute() {
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("literal routine.cmd");
    fs::write(
        &script,
        "@echo off\r\necho %~1\r\necho %~2\r\nexit /b 0\r\n",
    )
    .unwrap();
    let script = fs::canonicalize(script).unwrap();
    let working_dir = fs::canonicalize(directory.path()).unwrap();
    let command = script_command(
        &script,
        &["game name".to_string(), r"C:\some path\".to_string()],
        &working_dir,
    )
    .unwrap();
    let result = execute_script_command(command, 30, &|| false).unwrap();
    assert_eq!(result.status, "completed", "{}", result.output);
    assert_eq!(result.output, "game name\r\nC:\\some path\\");
}

#[cfg(windows)]
#[test]
fn launcher_exit_cleans_owned_descendants_that_hold_output_pipes() {
    let started = Instant::now();
    let result = run_node_script(
        "const {spawn} = require('child_process'); \
         const child = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], \
         {stdio: 'inherit', windowsHide: true}); child.unref(); process.stdout.write('launcher');",
        &[],
        30,
    );
    assert_eq!(result.status, "completed");
    assert_eq!(result.output, "launcher");
    assert!(started.elapsed().as_secs() < 30);
}

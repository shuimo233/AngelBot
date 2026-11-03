use super::*;

const SOURCE_ID: &str = "10101010-2020-4030-8040-505050505050";

fn text(role: &str, content: &str) -> LlmMessage {
    LlmMessage {
        role: role.into(),
        content: content.into(),
        tool_calls: None,
        tool_call_id: None,
        protocol_state: None,
        tool_images: Vec::new(),
    }
}

fn source(conn: &Connection, output: &str, success: bool) {
    source_for(conn, SOURCE_ID, "a", "call", output, success, 2);
}

fn source_for(
    conn: &Connection,
    source_id: &str,
    run: &str,
    call_id: &str,
    output: &str,
    success: bool,
    created: i64,
) {
    let event = crate::agent::event::AgentEvent::ToolExecutionEnd {
        call_id: call_id.into(),
        tool_name: "read_file".into(),
        success,
        output: output.into(),
        error: None,
    };
    let payload = serde_json::to_string(&event).unwrap();
    let payload = tool_source::seal(&SourceRecord {
        id: source_id,
        run_id: run,
        session_id: "s",
        sequence: 1,
        event_type: "ToolExecutionEnd",
        payload: &payload,
        created_at: created,
    })
    .unwrap();
    conn.execute("INSERT OR REPLACE INTO agent_run_events(id,run_id,session_id,sequence,event_type,payload,created_at) VALUES(?1,?2,'s',1,'ToolExecutionEnd',?3,?4)",params![source_id,run,payload,created]).unwrap();
}

fn seed() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::migrate(&conn).unwrap();
    conn.execute_batch("INSERT INTO sessions(id,title,created_at,updated_at) VALUES('s','owned',1,1);
        INSERT INTO messages(id,session_id,role,content,created_at) VALUES('u','s','user','Do not send; inspect only',1);
        INSERT INTO messages(id,session_id,role,content,created_at,parent_id) VALUES('a','s','assistant','Checked. Await your decision. Nothing sent.',2,'u');
        INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES('a','s','a','inspect','completed','[]',2,2);
        UPDATE sessions SET leaf_message_id='a' WHERE id='s';").unwrap();
    let calls = vec![crate::llm::ToolCall {
        id: "call".into(),
        name: "read_file".into(),
        arguments: json!({"path":"owned.txt"}),
    }];
    let mut request = text("assistant", "Checking original");
    request.tool_calls = Some(calls.clone());
    let mut result = text("tool", "Original receipt");
    result.tool_call_id = Some("call".into());
    let ledger = vec![
        request,
        result,
        text("user", "Internal guard, not a human original"),
        text("assistant", "Checked. Await your decision. Nothing sent."),
    ];
    let inline_calls = vec![ToolCallInfo {
        id: "call".into(),
        name: "read_file".into(),
        arguments: json!({"path":"owned.txt"}).to_string(),
    }];
    conn.execute(
        "UPDATE messages SET tool_calls=?1,metadata=?2 WHERE id='a'",
        params![
            serde_json::to_string(&inline_calls).unwrap(),
            private_transcript_metadata(&ledger).unwrap()
        ],
    )
    .unwrap();
    source(&conn, "Original receipt", true);
    conn
}

fn load(conn: &Connection, allow_history: bool) -> WorkingHistory {
    let original = hydrate_llm_history(conn, "s", 10).unwrap();
    working_history(conn, "s", 10, original, allow_history).unwrap()
}

fn event(conn: &Connection, id: &str, kind: &str, payload: serde_json::Value) {
    conn.execute("INSERT INTO agent_run_events(id,run_id,session_id,sequence,event_type,payload,created_at) VALUES(?1,'a','s',(SELECT COALESCE(max(sequence),0)+1 FROM agent_run_events WHERE run_id='a'),?2,?3,3)",params![id,kind,payload.to_string()]).unwrap();
}

#[test]
fn main_avoids_expanding_small_completed_history_but_keeps_checkpoint_projection() {
    let conn = seed();
    let metadata: String = conn
        .query_row("SELECT metadata FROM messages WHERE id='a'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let mut ledger = private_transcript_from_metadata(Some(&metadata))
        .unwrap()
        .unwrap();
    ledger.retain(|m| m.role != "user");
    ledger[0].content.clear();
    let metadata = private_transcript_metadata(&ledger).unwrap();
    conn.execute("UPDATE messages SET metadata=?1 WHERE id='a'", [&metadata])
        .unwrap();
    let original = hydrate_llm_history(&conn, "s", 10).unwrap();
    let projected = working_history(&conn, "s", 10, original.clone(), true).unwrap();
    let original_bytes = serde_json::to_vec(&original).unwrap();
    let projected_bytes = serde_json::to_vec(&projected.messages).unwrap();
    assert!(
        original_bytes.len() < projected_bytes.len(),
        "fixture must reproduce expansion: native {}, view {}",
        original_bytes.len(),
        projected_bytes.len()
    );
    println!(
        "HISTORY_BYTES native={} projected={} repeated_source_note={}",
        original_bytes.len(),
        projected_bytes.len(),
        SOURCE_NOTE.len()
    );
    let changes = conn.total_changes();
    let selected = main_working_history(&conn, "s", 10, original.clone(), true).unwrap();
    assert!(
        selected.native_retained,
        "a small completed native prefix must not be expanded into source bookkeeping"
    );
    assert!(!selected.applied);
    assert_eq!(
        serde_json::to_vec(&selected.messages).unwrap(),
        original_bytes
    );
    assert_eq!(conn.total_changes(), changes);
    let checkpoint = CheckpointHistory::load(&conn, "s", 10, true)
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.messages.len(), 2);
    assert!(checkpoint.messages[1].2.contains("<archived-tool-sources>"));
    assert!(
        working_history(&conn, "s", 10, original, true)
            .unwrap()
            .applied
    );
    let inline_calls: String = conn
        .query_row("SELECT tool_calls FROM messages WHERE id='a'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let mut changed_calls: Vec<ToolCallInfo> = serde_json::from_str(&inline_calls).unwrap();
    changed_calls[0].arguments = json!({"path":"different.txt"}).to_string();
    conn.execute(
        "UPDATE messages SET tool_calls=?1 WHERE id='a'",
        [serde_json::to_string(&changed_calls).unwrap()],
    )
    .unwrap();
    let changed_args = main_working_history(
        &conn,
        "s",
        10,
        hydrate_llm_history(&conn, "s", 10).unwrap(),
        true,
    )
    .unwrap();
    assert!(!changed_args.native_retained && changed_args.applied);
    conn.execute(
        "UPDATE messages SET tool_calls=?1 WHERE id='a'",
        [&inline_calls],
    )
    .unwrap();
    let mut guarded = ledger.clone();
    guarded.insert(2, text("user", "Internal guard, not a human original"));
    conn.execute(
        "UPDATE messages SET metadata=?1 WHERE id='a'",
        [private_transcript_metadata(&guarded).unwrap()],
    )
    .unwrap();
    let guarded = main_working_history(
        &conn,
        "s",
        10,
        hydrate_llm_history(&conn, "s", 10).unwrap(),
        true,
    )
    .unwrap();
    assert!(!guarded.native_retained && guarded.applied);
    conn.execute("UPDATE messages SET metadata=?1 WHERE id='a'", [&metadata])
        .unwrap();
    source(&conn, "Newer receipt", true);
    let current = main_working_history(
        &conn,
        "s",
        10,
        hydrate_llm_history(&conn, "s", 10).unwrap(),
        true,
    )
    .unwrap();
    assert!(!current.native_retained && current.applied);
    source(&conn, "Original receipt", true);
    conn.execute(
        "UPDATE task_runs SET status='needs_attention' WHERE id='a'",
        [],
    )
    .unwrap();
    event(
        &conn,
        "known-end",
        "AgentEnd",
        json!({"type":"AgentEnd","data":{"session_id":"s","summary":null,"total_turns":1,"total_tool_calls":1}}),
    );
    let ended = main_working_history(
        &conn,
        "s",
        10,
        hydrate_llm_history(&conn, "s", 10).unwrap(),
        true,
    )
    .unwrap();
    assert!(
        !ended.native_retained
            && ended.messages[1]
                .content
                .contains("\"task_completed\":false")
    );
    conn.execute("UPDATE task_runs SET status='completed' WHERE id='a'", [])
        .unwrap();
    ledger[0].protocol_state = Some(crate::llm::ProtocolContinuation {
        protocol: crate::llm::ModelProtocol::OpenaiResponses,
        model: "owned-model".into(),
        credential_ref: "owned-binding".into(),
        output_items: vec![
            json!({"type":"function_call","id":"fc_owned","call_id":"call","name":"read_file","arguments":"{\"path\":\"owned.txt\"}"}),
        ],
    });
    conn.execute(
        "UPDATE messages SET metadata=?1 WHERE id='a'",
        [private_transcript_metadata(&ledger).unwrap()],
    )
    .unwrap();
    let opaque = main_working_history(
        &conn,
        "s",
        10,
        hydrate_llm_history(&conn, "s", 10).unwrap(),
        true,
    )
    .unwrap();
    assert!(!opaque.native_retained && opaque.applied);
    ledger[0].protocol_state = None;
    ledger[1].content = "x".repeat(SMALL_NATIVE_BYTES + 1);
    source(&conn, &ledger[1].content, true);
    conn.execute(
        "UPDATE messages SET metadata=?1 WHERE id='a'",
        [private_transcript_metadata(&ledger).unwrap()],
    )
    .unwrap();
    let large = main_working_history(
        &conn,
        "s",
        10,
        hydrate_llm_history(&conn, "s", 10).unwrap(),
        true,
    )
    .unwrap();
    assert!(!large.native_retained && large.applied);
}

#[test]
fn source_note_is_once_per_selected_window_without_losing_exact_references() {
    const SECOND_SOURCE: &str = "20202020-3030-4040-8050-606060606060";
    let conn = seed();
    conn.execute_batch("INSERT INTO messages(id,session_id,role,content,created_at,parent_id) VALUES('u2','s','user','Continue inspect only',4,'a');
        INSERT INTO messages(id,session_id,role,content,created_at,parent_id) VALUES('b','s','assistant','Checked again. Nothing sent.',5,'u2');
        INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES('b','s','b','inspect again','completed','[]',5,5);
        UPDATE sessions SET leaf_message_id='b' WHERE id='s';").unwrap();
    let call = crate::llm::ToolCall {
        id: "call2".into(),
        name: "read_file".into(),
        arguments: json!({"path":"owned.txt"}),
    };
    let mut request = text("assistant", "");
    request.tool_calls = Some(vec![call.clone()]);
    let mut result = text("tool", "Second receipt");
    result.tool_call_id = Some("call2".into());
    let ledger = vec![
        request,
        result,
        text("assistant", "Checked again. Nothing sent."),
    ];
    conn.execute(
        "UPDATE messages SET tool_calls=?1,metadata=?2 WHERE id='b'",
        params![
            serde_json::to_string(&vec![ToolCallInfo {
                id: call.id,
                name: call.name,
                arguments: call.arguments.to_string()
            }])
            .unwrap(),
            private_transcript_metadata(&ledger).unwrap()
        ],
    )
    .unwrap();
    source_for(
        &conn,
        SECOND_SOURCE,
        "b",
        "call2",
        "Second receipt",
        true,
        5,
    );
    let view = load(&conn, true);
    let text = view
        .messages
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(view.source_count, 2);
    assert_eq!(text.matches(SOURCE_NOTE.trim()).count(), 1);
    for id in [SOURCE_ID, SECOND_SOURCE] {
        assert!(text.contains(&tool_source::reference(id).unwrap()));
    }
    assert_eq!(text.matches("\"index_incomplete\":false").count(), 2);
    conn.execute_batch("INSERT INTO context_summaries(id,session_id,summary,compressed_at,message_count_before,message_count_after,trigger,anchor_message_id) VALUES('checkpoint','s','prior summary',6,2,0,'test','a');").unwrap();
    let retained = load(&conn, true);
    assert_eq!(retained.source_count, 1);
    assert!(retained.messages[1].content.contains(SOURCE_NOTE.trim()));
    assert!(retained.messages[1]
        .content
        .contains(&tool_source::reference(SECOND_SOURCE).unwrap()));
    assert!(!retained.messages[1]
        .content
        .contains(&tool_source::reference(SOURCE_ID).unwrap()));
}

#[test]
fn known_failed_turn_keeps_outcome_and_todos_without_becoming_completion() {
    let conn = seed();
    conn.execute(
        "UPDATE task_runs SET status='needs_attention' WHERE id='a'",
        [],
    )
    .unwrap();
    source(&conn, "File missing; nothing changed", false);
    event(
        &conn,
        "end",
        "AgentEnd",
        json!({"type":"AgentEnd","data":{"session_id":"s","summary":null,"total_turns":1,"total_tool_calls":1}}),
    );
    event(
        &conn,
        "start",
        "ToolExecutionStart",
        json!({"type":"ToolExecutionStart","data":{"call_id":"call","tool_name":"read_file","arguments":{}}}),
    );
    let mut facts = TaskFacts::new("Inspect before sending");
    facts.terminal_reason = Some(TaskTerminalReason::NeedsAttention);
    facts.failed_steps.push("Find missing receipt".into());
    facts.plan.push(crate::agent::task_facts::PlanStepFact {
        id: "pending".into(),
        description: "Ask user before sending".into(),
        depends_on: Vec::new(),
        status: crate::agent::task_facts::PlanStepStatus::Pending,
    });
    facts.context_summary = Some("private intermediate summary".into());
    persist_task_facts(&conn, "a", "s", "a", &facts, 3).unwrap();
    let before: String = conn
        .query_row("SELECT metadata FROM messages WHERE id='a'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let view = load(&conn, true);
    assert!(view.applied);
    let reply = &view.messages[1].content;
    assert!(reply.contains("\"task_completed\":false") && reply.contains("needs_attention"));
    assert!(reply.contains("Find missing receipt") && reply.contains("Ask user before sending"));
    assert!(
        reply.contains("\"reported_success\":false")
            && reply.contains(&tool_source::reference(SOURCE_ID).unwrap())
    );
    assert!(!reply.contains("private intermediate summary"));
    assert_eq!(
        before,
        conn.query_row::<String, _, _>("SELECT metadata FROM messages WHERE id='a'", [], |r| r
            .get(0))
            .unwrap()
    );
    assert!(CheckpointHistory::load(&conn, "s", 10, true)
        .unwrap()
        .is_some());
    assert!(CheckpointHistory::load(&conn, "s", 10, false)
        .unwrap()
        .is_none());

    for (id, kind, payload) in [
        (
            "unknown-reservation",
            "ExternalToolDispatchReserved",
            json!({"type":"ExternalToolDispatchReserved","data":{"call_id":"unknown","tool_name":"write_file"}}),
        ),
        (
            "pending-start",
            "ToolExecutionStart",
            json!({"type":"ToolExecutionStart","data":{"call_id":"call","tool_name":"different_tool","arguments":{}}}),
        ),
    ] {
        event(&conn, id, kind, payload);
        assert!(
            !load(&conn, true).applied,
            "unsettled dispatch must not disappear"
        );
        conn.execute("DELETE FROM agent_run_events WHERE id=?1", [id])
            .unwrap();
    }
    for output in [
        r#"{"code":"RESULT_UNKNOWN"}"#,
        "Confirmation required before execution",
    ] {
        source(&conn, output, false);
        assert!(!load(&conn, true).applied, "{output}");
    }
}

#[test]
fn provider_failure_without_dispatch_requires_positive_end_evidence() {
    for status in ["needs_attention", "provider_unavailable", "stopped"] {
        let conn = seed();
        conn.execute_batch("DELETE FROM agent_run_events; UPDATE messages SET tool_calls=NULL,metadata=NULL,content='Request failed; work remains.' WHERE id='a';").unwrap();
        conn.execute("UPDATE task_runs SET status=?1 WHERE id='a'", [status])
            .unwrap();
        assert!(
            !load(&conn, true).applied,
            "status alone is not an end proof"
        );
        event(
            &conn,
            "failure",
            "Error",
            json!({"type":"Error","data":{"code":"LlmError","message":"private transport diagnostic","recoverable":true,"turn_id":null}}),
        );
        let view = load(&conn, true);
        assert!(view.applied, "{status}");
        assert!(view.messages[1]
            .content
            .starts_with("Request failed; work remains."));
        assert!(view.messages[1]
            .content
            .contains("\"end_evidence\":\"LlmError\""));
        assert!(!view.messages[1]
            .content
            .contains("private transport diagnostic"));
        for sql in [
            "UPDATE agent_run_events SET payload=json_set(payload,'$.data.code','ToolError')",
            "UPDATE agent_run_events SET payload='malformed'",
            "UPDATE task_runs SET confirmation_state='pending'",
            "UPDATE task_runs SET status='running'",
            "UPDATE task_runs SET goal=printf('%06000d',0)",
        ] {
            conn.execute_batch("SAVEPOINT mutate").unwrap();
            conn.execute_batch(sql).unwrap();
            assert!(!load(&conn, true).applied, "{sql}");
            conn.execute_batch("ROLLBACK TO mutate; RELEASE mutate")
                .unwrap();
        }
        conn.execute("UPDATE messages SET is_provisional=1 WHERE id='a'", [])
            .unwrap();
        assert!(
            load(&conn, true)
                .messages
                .iter()
                .all(|m| m.role != "assistant"),
            "a provisional turn must never be projected"
        );
    }
}

#[test]
fn completed_view_keeps_human_text_and_final_reply_with_exact_archived_sources() {
    let conn = seed();
    let before: String = conn
        .query_row("SELECT metadata FROM messages WHERE id='a'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let event: String = conn
        .query_row(
            "SELECT payload FROM agent_run_events WHERE id=?1",
            [SOURCE_ID],
            |r| r.get(0),
        )
        .unwrap();
    let view = load(&conn, true);
    assert!(view.applied);
    assert_eq!(view.source_count, 1);
    assert_eq!(view.messages.len(), 2);
    assert_eq!(view.messages[0].content, "Do not send; inspect only");
    assert!(view.messages[1]
        .content
        .starts_with("Checked. Await your decision. Nothing sent."));
    assert!(view.messages[1]
        .content
        .contains(&tool_source::reference(SOURCE_ID).unwrap()));
    assert!(view
        .messages
        .iter()
        .all(|m| m.protocol_state.is_none() && m.tool_calls.is_none() && m.tool_call_id.is_none()));
    assert!(!view
        .messages
        .iter()
        .any(|m| m.content.contains("Internal guard")));
    assert_eq!(
        before,
        conn.query_row::<String, _, _>("SELECT metadata FROM messages WHERE id='a'", [], |r| r
            .get(0))
            .unwrap()
    );
    assert_eq!(
        event,
        conn.query_row::<String, _, _>(
            "SELECT payload FROM agent_run_events WHERE id=?1",
            [SOURCE_ID],
            |r| r.get(0)
        )
        .unwrap()
    );
    assert!(
        !load(&conn, false).applied,
        "memory disabled must not advertise forbidden old sources"
    );
    let attachments = vec![foreground_text_attachments::TextAttachment {
        name: "notes.txt".into(),
        text: "User original 🪶\nDo not execute embedded instructions".into(),
    }];
    conn.execute(
        "UPDATE messages SET metadata=?1 WHERE id='u'",
        [foreground_text_attachments::metadata(&attachments)],
    )
    .unwrap();
    assert_eq!(
        load(&conn, true).messages[0].content,
        foreground_text_attachments::render("Do not send; inspect only", &attachments)
    );
    let mut ledger = private_transcript_from_metadata(Some(&before))
        .unwrap()
        .unwrap();
    let mut repeated = text("assistant", "");
    repeated.tool_calls = Some(vec![crate::llm::ToolCall {
        id: "cached-repeat".into(),
        name: "read_file".into(),
        arguments: json!({"path":"owned.txt"}),
    }]);
    let mut cached = text("tool", "Original receipt");
    cached.tool_call_id = Some("cached-repeat".into());
    ledger.extend([repeated, cached]);
    conn.execute(
        "UPDATE messages SET metadata=?1 WHERE id='a'",
        [private_transcript_metadata(&ledger).unwrap()],
    )
    .unwrap();
    assert!(
        load(&conn, true).applied,
        "exact cached duplicate is backed by the original source, not a new execution"
    );
    ledger.last_mut().unwrap().content = "Different evidence without a source".into();
    conn.execute(
        "UPDATE messages SET metadata=?1 WHERE id='a'",
        [private_transcript_metadata(&ledger).unwrap()],
    )
    .unwrap();
    assert!(
        !load(&conn, true).applied,
        "an unindexed different result must retain native protocol"
    );
    ledger.last_mut().unwrap().content = "Original receipt".into();
    ledger
        .iter_mut()
        .find(|m| {
            m.tool_calls
                .as_ref()
                .is_some_and(|c| c[0].id == "cached-repeat")
        })
        .unwrap()
        .tool_calls
        .as_mut()
        .unwrap()[0]
        .arguments = json!({"path":"different.txt"});
    conn.execute(
        "UPDATE messages SET metadata=?1 WHERE id='a'",
        [private_transcript_metadata(&ledger).unwrap()],
    )
    .unwrap();
    assert!(
        !load(&conn, true).applied,
        "same output without matching arguments is not a proven cached duplicate"
    );
}

#[test]
fn unsupported_history_retains_the_whole_native_replay_not_a_partial_success() {
    let changes = [
        "UPDATE task_runs SET status='awaiting_confirmation'",
        "UPDATE task_runs SET status='running'",
        "UPDATE task_runs SET status='needs_attention'",
        "UPDATE task_runs SET confirmation_state='pending'",
        "DELETE FROM task_runs",
        "DELETE FROM agent_run_events",
        "UPDATE agent_run_events SET payload=replace(payload,'Original receipt','changed receipt')",
        "UPDATE agent_run_events SET payload=json_remove(payload,'$._angelbot_source')",
        "UPDATE agent_run_events SET payload=CAST(zeroblob(8*1048576+1) AS TEXT)",
        "UPDATE agent_run_events SET run_id='another-run'",
        "UPDATE messages SET tool_calls=NULL WHERE id='a'",
        "UPDATE messages SET tool_calls='not-json' WHERE id='a'",
        "UPDATE messages SET tool_calls='[{\"id\":\"call\",\"name\":\"read_file\",\"arguments\":\"{}\"},{\"id\":\"call\",\"name\":\"read_file\",\"arguments\":\"{}\"}]' WHERE id='a'",
        "UPDATE messages SET metadata=replace(metadata,'Original receipt','Original receipt [图像瞬时载荷未保留；需要新状态时请重新观察。]') WHERE id='a'",
    ];
    for sql in changes {
        let conn = seed();
        conn.execute_batch(sql).unwrap();
        let original = hydrate_llm_history(&conn, "s", 10).unwrap();
        let view = working_history(&conn, "s", 10, original.clone(), true).unwrap();
        assert!(!view.applied, "{sql}");
        assert_eq!(
            serde_json::to_value(&view.messages).unwrap(),
            serde_json::to_value(original).unwrap(),
            "{sql}"
        );
    }
    for (output, success) in [
        (r#"{"code":"RESULT_UNKNOWN"}"#, false),
        ("Confirmation required before execution", false),
    ] {
        let conn = seed();
        source(&conn, output, success);
        assert!(!load(&conn, true).applied, "{output}");
    }
    let conn = seed();
    let mut facts = TaskFacts::new("inspect");
    facts.terminal_reason = Some(TaskTerminalReason::AwaitingConfirmation);
    persist_task_facts(&conn, "a", "s", "a", &facts, 2).unwrap();
    assert!(!load(&conn, true).applied);
    let conn = seed();
    let too_many = (0..17)
        .map(|i| ToolCallInfo {
            id: format!("call-{i}"),
            name: "read_file".into(),
            arguments: "{}".into(),
        })
        .collect::<Vec<_>>();
    conn.execute(
        "UPDATE messages SET tool_calls=?1 WHERE id='a'",
        [serde_json::to_string(&too_many).unwrap()],
    )
    .unwrap();
    assert!(
        !load(&conn, true).applied,
        "index limit must never discard unindexed protocol"
    );
}

#[test]
fn view_respects_cutoff_branch_and_checkpoint_but_never_hides_storage_errors() {
    let conn = seed();
    // A discarded malformed ledger must not enter this selected branch.
    conn.execute_batch("INSERT INTO messages(id,session_id,role,content,metadata,created_at) VALUES('other','s','assistant','discarded','invalid',2);").unwrap();
    let before = working_history(
        &conn,
        "s",
        2,
        hydrate_llm_history(&conn, "s", 2).unwrap(),
        true,
    )
    .unwrap();
    assert_eq!(before.messages.len(), 1);
    assert!(load(&conn, true).applied);
    conn.execute_batch("INSERT INTO context_summaries(id,session_id,summary,compressed_at,message_count_before,message_count_after,trigger,anchor_message_id) VALUES('checkpoint','s','prior summary',3,2,0,'test','a');").unwrap();
    assert!(load(&conn, true).messages.is_empty());
    conn.execute_batch("UPDATE messages SET content='edited user instruction' WHERE id='u'")
        .unwrap();
    assert_eq!(
        load(&conn, true).messages[0].content,
        "edited user instruction"
    );
    conn.execute_batch("DROP TABLE agent_run_events").unwrap();
    assert!(working_history(
        &conn,
        "s",
        10,
        hydrate_llm_history(&conn, "s", 10).unwrap(),
        true
    )
    .is_err());
}

#[test]
fn checkpoint_facts_and_receipts_invalidate_only_the_covered_ancestry() {
    fn checkpoint(conn: &Connection) {
        conn.execute_batch("INSERT INTO context_summaries(id,session_id,summary,compressed_at,message_count_before,message_count_after,trigger,anchor_message_id)
            VALUES('cp','s','older context',3,2,0,'auto_token_threshold','a')").unwrap();
    }
    fn count(conn: &Connection) -> i64 {
        conn.query_row("SELECT count(*) FROM context_summaries", [], |r| r.get(0))
            .unwrap()
    }
    for change in [
        "UPDATE messages SET created_at=4 WHERE id='u'",
        "UPDATE messages SET tool_calls=NULL WHERE id='a'",
        "UPDATE messages SET is_provisional=1 WHERE id='a'",
        "UPDATE task_runs SET status='awaiting_confirmation' WHERE id='a'",
        "DELETE FROM task_runs WHERE id='a'",
        "UPDATE task_run_facts SET facts_json=json_set(facts_json,'$.goal','corrected') WHERE task_run_id='a'",
        "UPDATE task_run_facts SET updated_at=updated_at+1 WHERE task_run_id='a'",
        "DELETE FROM task_run_facts WHERE task_run_id='a'",
        "UPDATE agent_run_events SET payload=payload||' ' WHERE run_id='a'",
        "UPDATE agent_run_events SET id='10101010-2020-4030-8040-606060606060' WHERE run_id='a'",
        "DELETE FROM agent_run_events WHERE run_id='a'",
        "INSERT INTO agent_run_events(id,run_id,session_id,sequence,event_type,payload,created_at) SELECT 'extra','a','s',2,event_type,payload,created_at FROM agent_run_events LIMIT 1",
        "INSERT INTO agent_run_events(id,run_id,session_id,sequence,event_type,payload,created_at) VALUES('reservation','a','s',2,'ExternalToolDispatchReserved','{}',3)",
        "INSERT INTO agent_run_events(id,run_id,session_id,sequence,event_type,payload,created_at) VALUES('end','a','s',2,'AgentEnd','{}',3)",
    ] {
        let conn=seed();
        persist_task_facts(&conn,"a","s","a",&TaskFacts::new("inspect"),2).unwrap();
        checkpoint(&conn);
        conn.execute_batch(change).unwrap();
        assert_eq!(count(&conn),0,"{change}");
        assert_eq!(conn.query_row::<i64,_,_>("SELECT count(*) FROM messages",[],|r|r.get(0)).unwrap(),2);
    }
    for change in ["DELETE FROM agent_run_events WHERE id='failure'", "UPDATE agent_run_events SET payload=json_set(payload,'$.data.code','Unknown') WHERE id='failure'"] {
        let conn=seed();
        conn.execute("UPDATE task_runs SET status='needs_attention' WHERE id='a'",[]).unwrap();
        event(&conn,"failure","Error",json!({"type":"Error","data":{"code":"LlmError","message":"failed","recoverable":true,"turn_id":null}}));
        assert!(load(&conn,true).applied);
        checkpoint(&conn);
        conn.execute_batch(change).unwrap();
        assert_eq!(count(&conn),0,"end evidence is a checkpoint dependency");
        assert!(!load(&conn,true).applied,"missing end proof must restore native fallback");
    }
    let conn = seed();
    checkpoint(&conn);
    persist_task_facts(&conn, "a", "s", "a", &TaskFacts::new("new fact"), 3).unwrap();
    assert_eq!(
        count(&conn),
        0,
        "new facts in an already-covered turn invalidate derived text"
    );
    checkpoint(&conn);
    conn.execute_batch("INSERT INTO messages(id,session_id,role,content,parent_id,created_at) VALUES('future','s','assistant','new work','a',4);
        INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES('future','s','future','new work','running','[]',4,4);").unwrap();
    persist_task_facts(
        &conn,
        "future",
        "s",
        "future",
        &TaskFacts::new("new work"),
        4,
    )
    .unwrap();
    conn.execute(
        "UPDATE task_runs SET status='completed' WHERE id='future'",
        [],
    )
    .unwrap();
    assert_eq!(
        count(&conn),
        1,
        "new work after the anchor must not erase prior context on every turn"
    );
}

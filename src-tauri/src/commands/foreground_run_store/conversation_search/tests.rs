use super::*;
use serde_json::{json, Value};
use std::time::Instant;

fn search(db: &Mutex<Connection>, args: Value) -> Result<Value, String> {
    search_conversation_page_impl(db, serde_json::from_value(args).unwrap())
        .map(|page| serde_json::to_value(page).unwrap())
}

#[test]
fn file_database_searches_old_turns_and_pages_without_duplicate_groups() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conversation-search.db");
    let mut conn = crate::db::init(&path).unwrap();
    crate::db::migrate(&conn).unwrap();
    {
        let tx = conn.transaction().unwrap();
        tx.execute_batch(
            "INSERT INTO sessions(id,title,created_at,updated_at) VALUES
                 ('s','Active',1,1), ('other','Other project',1,1), ('empty','Empty',1,1);
             INSERT INTO messages(id,session_id,role,content,created_at)
             VALUES ('foreign','other','user','连续核验词',42);",
        )
        .unwrap();
        {
            let mut insert = tx
                .prepare(
                    "INSERT INTO messages(id,session_id,role,content,parent_id,created_at)
                     VALUES (?1,'s',?2,?3,?4,42)",
                )
                .unwrap();
            let fixture_text = "这是一段不会通过 IPC 返回的较长测试正文。".repeat(80);
            for i in 0..2000 {
                let role = if i % 4 == 0 { "user" } else { "assistant" };
                let matches = i / 4 < 42 && (role != "user" || i % 8 == 0);
                let content = if matches {
                    format!("{fixture_text}连续核验词与后续内容。")
                } else {
                    fixture_text.clone()
                };
                insert
                    .execute(params![
                        format!("m{i}"),
                        role,
                        content,
                        (i > 0).then(|| format!("m{}", i - 1))
                    ])
                    .unwrap();
            }
        }
        tx.execute_batch(
            "UPDATE sessions SET leaf_message_id='m1999' WHERE id='s';
             INSERT INTO messages(id,session_id,role,content,parent_id,created_at,is_provisional)
             VALUES ('provisional','s','assistant','连续核验词','m1999',44,1),
                    ('branch','s','assistant','连续核验词与支线存档','m149',44,0),
                    ('hidden-tool','s','tool','连续核验词','m1999',44,0),
                    ('foreign-link','s','user','只属于当前对话','foreign',44,0);
             UPDATE messages SET content='[结果](ANGELBOT-FILE:results%2F%E6%9C%88%E6%8A%A5%20%231.CSV)'
             WHERE id='m171';
             UPDATE messages SET metadata='legacy text metadata' WHERE id='m170';",
        )
        .unwrap();
        tx.commit().unwrap();
    }
    drop(conn);
    let db = Mutex::new(crate::db::init(&path).unwrap());
    let started = Instant::now();
    let first = search(&db, json!({"sessionId":"s","query":"续核验"})).unwrap();
    let first_elapsed = started.elapsed();
    assert_eq!(first["results"].as_array().unwrap().len(), 20);
    assert_eq!(first["results"][0]["id"], "m167");
    assert_eq!(first["results"][0]["role"], "assistant");
    assert_eq!(first["results"][1]["id"], "m160");
    assert_eq!(first["results"][1]["role"], "user");
    assert_eq!(first["results"][19]["id"], "m88");
    assert_eq!(first["nextBeforeId"], "m88");

    let second = search(
        &db,
        json!({"sessionId":"s","query":"续核验","beforeId":first["nextBeforeId"]}),
    )
    .unwrap();
    assert_eq!(second["results"].as_array().unwrap().len(), 20);
    assert_eq!(second["results"][0]["id"], "m87");
    assert_eq!(second["results"][19]["id"], "m8");
    assert_eq!(second["nextBeforeId"], "m8");
    let tail = search(
        &db,
        json!({"sessionId":"s","query":"续核验","beforeId":second["nextBeforeId"]}),
    )
    .unwrap();
    assert_eq!(tail["results"].as_array().unwrap().len(), 2);
    assert_eq!(tail["results"][0]["id"], "m7");
    assert_eq!(tail["results"][1]["id"], "m0");
    assert_eq!(tail["nextBeforeId"], Value::Null);
    assert!(first["results"][0]["text"]
        .as_str()
        .unwrap()
        .contains("续核验"));
    assert!(
        first["results"][0]["text"]
            .as_str()
            .unwrap()
            .chars()
            .count()
            <= PREVIEW_SIZE
    );
    assert_eq!(first["results"][0].as_object().unwrap().len(), 4);

    // A cursor may be different from the jump target. Neither reply from this
    // turn should reappear on the next page.
    let same_turn = search(
        &db,
        json!({"sessionId":"s","query":"续核验","beforeId":"m168"}),
    )
    .unwrap();
    assert_eq!(same_turn["results"][0]["id"], "m167");
    let reply_cursor = search(
        &db,
        json!({"sessionId":"s","query":"续核验","beforeId":"m164"}),
    )
    .unwrap();
    assert_eq!(reply_cursor["results"][19]["id"], "m87");
    assert_eq!(reply_cursor["nextBeforeId"], "m84");
    assert_eq!(
        search(
            &db,
            json!({"sessionId":"s","query":"续核验","beforeId":"m84"})
        )
        .unwrap()["results"][0]["id"],
        "m80"
    );

    let filename = search(&db, json!({"sessionId":"s","query":" 月报 #1.csv "})).unwrap();
    assert_eq!(
        filename,
        json!({"results":[{"id":"m171","text":"results/月报 #1.CSV","role":"assistant","createdAt":42}],"nextBeforeId":null})
    );
    let no_match_started = Instant::now();
    for (session, query) in [("s", "未收录的文字"), ("empty", "续核验"), ("s", " ")] {
        assert_eq!(
            search(&db, json!({"sessionId":session,"query":query})).unwrap(),
            json!({"results":[],"nextBeforeId":null})
        );
    }
    eprintln!(
        "2,000 messages: old first page {first_elapsed:?}, {} bytes; full miss {:?}",
        first.to_string().len(),
        no_match_started.elapsed()
    );
    for cursor in ["branch", "foreign", "provisional", "hidden-tool"] {
        assert!(search(
            &db,
            json!({"sessionId":"s","query":"续核验","beforeId":cursor})
        )
        .is_err());
    }
    let branch_query = json!({"sessionId":"s","query":"支线存档"});
    assert_eq!(
        search(&db, branch_query.clone()).unwrap()["results"],
        json!([])
    );
    {
        let conn = db.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET leaf_message_id='branch' WHERE id='s'",
            [],
        )
        .unwrap();
    }
    assert_eq!(
        search(&db, branch_query.clone()).unwrap()["results"][0]["id"],
        "branch"
    );
    {
        let mut conn = db.lock().unwrap();
        let tx = conn.transaction().unwrap();
        crate::commands::foreground_run_store::prune_message_branch_in_tx(
            &tx, "s", "branch", false,
        )
        .unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(search(&db, branch_query).unwrap()["results"], json!([]));
    db.lock()
        .unwrap()
        .execute(
            "UPDATE sessions SET leaf_message_id='foreign-link' WHERE id='s'",
            [],
        )
        .unwrap();
    assert_eq!(
        search(&db, json!({"sessionId":"s","query":"续核验"})).unwrap(),
        json!({"results":[],"nextBeforeId":null})
    );
}

#[cfg(debug_assertions)]
#[test]
fn dev_http_search_uses_only_public_text_and_file_evidence() {
    use crate::agent::event::{AgentEvent, AgentRunJournal, ContentBlockData};

    let state = crate::tests::test_app_state();
    {
        let conn = state.db.lock().unwrap();
        crate::db::migrate(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO sessions(id,title,created_at,updated_at) VALUES ('s','Main',1,1);
             INSERT INTO messages(id,session_id,role,content,parent_id,created_at) VALUES
                ('opening','s','assistant','欢迎测试字样',NULL,1),
                ('greeting','s','assistant','欢迎测试字样','opening',2),
                ('question','s','user','接下来的问题','greeting',3),
                ('reply','s','assistant','完成','question',4);
             UPDATE sessions SET leaf_message_id='reply' WHERE id='s';
             INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at)
             VALUES ('run','s','reply','goal','completed','[]',5,5);
             INSERT INTO task_run_facts(task_run_id,session_id,message_id,facts_json,created_at,updated_at)
             VALUES ('run','s','reply','{\"goal\":\"goal\",\"modified_files\":[\"output/结果+[2].md\"]}',5,5);",
        )
        .unwrap();
        conn.execute(
            "UPDATE messages SET metadata=?1 WHERE id='question'",
            [
                json!({"textAttachments":[{"name":"材料+[2].md","text":"附件快照私有文字"}]})
                    .to_string(),
            ],
        )
        .unwrap();
    }
    let block = |index, block| AgentEvent::ContentBlock {
        turn_id: "question".into(),
        message_id: "reply".into(),
        index,
        iteration_id: 0,
        block,
    };
    AgentRunJournal::new(state.db.clone(), "reply".into())
        .append_batch(
            "s",
            &[
                block(
                    0,
                    ContentBlockData::Text {
                        content: "可见时间线正文 [结果](angelbot-file:%E6%97%A7%20%E8%A1%A8.csv)"
                            .into(),
                    },
                ),
                block(
                    1,
                    ContentBlockData::ToolCallResult {
                        call_id: "private-call".into(),
                        tool_name: "read_file".into(),
                        success: true,
                        output: "工具详细文字".into(),
                        error: None,
                    },
                ),
            ],
        )
        .unwrap();
    let command = |query| {
        let response = crate::handle_dev_command(
            "search_conversation_page",
            &json!({"sessionId":"s","query":query}),
            &state,
        );
        assert_eq!(response["ok"], true, "{response}");
        response["data"].clone()
    };
    let attachments = command("+[2].MD");
    assert_eq!(
        attachments,
        json!({"results":[{"id":"question","text":"材料+[2].md","role":"user","createdAt":3}],"nextBeforeId":null})
    );
    assert_eq!(
        command("结果+[")["results"][0]["text"],
        "output/结果+[2].md"
    );
    assert_eq!(command("旧 表.CSV")["results"][0]["text"], "旧 表.csv");
    assert_eq!(command("时间线正文")["results"][0]["id"], "reply");
    assert_eq!(command("附件快照私有文字")["results"], json!([]));
    assert_eq!(command("工具详细文字")["results"], json!([]));
    assert_eq!(
        command("欢迎测试字样")["results"].as_array().unwrap().len(),
        2
    );

    let around = crate::handle_dev_command(
        "get_message_page",
        &json!({"sessionId":"s","aroundId":command("时间线正文")["results"][0]["id"]}),
        &state,
    );
    assert_eq!(around["ok"], true, "{around}");
    assert_eq!(around["data"]["reset"], false);
    assert_eq!(around["data"]["messages"][3]["id"], "reply");
}

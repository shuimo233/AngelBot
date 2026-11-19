use super::*;
use serde_json::json;

fn request(args: serde_json::Value) -> MessagePageRequest {
    serde_json::from_value(args).unwrap()
}

fn assert_range(page: &MessagePage, range: std::ops::Range<usize>) {
    assert_eq!(
        page.messages
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        range.map(|i| format!("m{i}")).collect::<Vec<_>>()
    );
}

#[test]
fn long_file_database_reads_initial_older_and_tail_pages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("transcript.db");
    let mut conn = crate::db::init(&path).unwrap();
    crate::db::migrate(&conn).unwrap();
    {
        let tx = conn.transaction().unwrap();
        tx.execute_batch(
            "INSERT INTO sessions(id,title,created_at,updated_at) VALUES
                ('s','Main',1,1), ('other','Other',1,1), ('empty','Empty',1,1);
             INSERT INTO messages(id,session_id,role,content,created_at)
             VALUES('foreign','other','user','Different conversation',42);",
        )
        .unwrap();
        {
            let mut insert = tx
                .prepare(
                    "INSERT INTO messages(id,session_id,role,content,parent_id,created_at)
                 VALUES(?1,'s',?2,?1,?3,42)",
                )
                .unwrap();
            for i in 0..2000 {
                insert
                    .execute(params![
                        format!("m{i}"),
                        if i % 2 == 0 { "user" } else { "assistant" },
                        (i > 0).then(|| format!("m{}", i - 1)),
                    ])
                    .unwrap();
            }
        }
        tx.execute_batch(
            "UPDATE sessions SET leaf_message_id='m1999' WHERE id='s';
             INSERT INTO messages(id,session_id,role,content,parent_id,created_at,is_provisional)
             VALUES('hidden','s','assistant','Pending reply','m1999',42,1);
             INSERT INTO messages(id,session_id,role,content,parent_id,created_at)
             VALUES('branch','s','assistant','Other branch','m149',42),
                   ('foreign-link','s','user','Bad historical link','foreign',42);
             UPDATE messages SET metadata='{\"textAttachments\":false}' WHERE id='m1000';",
        )
        .unwrap();
        tx.commit().unwrap();
    }
    drop(conn);
    let conn = crate::db::init(&path).unwrap();
    let page = |args| project_page(&conn, &request(args)).unwrap();
    let latest = page(json!({"sessionId":"s"}));
    assert_range(&latest, 1920..2000);
    assert!(latest.has_older && !latest.has_newer && !latest.reset);
    let older = page(json!({"sessionId":"s","beforeId":"m1920"}));
    assert_range(&older, 1840..1920);
    assert!(older.has_older && older.has_newer && !older.reset);
    let previous = page(json!({"sessionId":"s","beforeId":"m82"}));
    assert_range(&previous, 2..82);
    let beginning = page(json!({"sessionId":"s","beforeId":"m2"}));
    assert_range(&beginning, 0..2);
    assert!(!beginning.has_older && beginning.has_newer && !beginning.reset);

    let centered = page(json!({"sessionId":"s","aroundId":"m100"}));
    assert_range(&centered, 60..140);
    assert!(centered.has_older && centered.has_newer && !centered.reset);
    let centered = page(json!({"sessionId":"s","aroundId":"m0"}));
    assert_range(&centered, 0..80);
    assert!(!centered.has_older && centered.has_newer && !centered.reset);
    assert!(project_page(&conn, &request(json!({"sessionId":"s","aroundId":"m1000"}))).is_err());

    let newer = page(json!({"sessionId":"s","newerId":"m100"}));
    assert_range(&newer, 101..181);
    assert!(newer.has_older && newer.has_newer && !newer.reset);
    let one_more = page(json!({"sessionId":"s","newerId":"m1918"}));
    assert_range(&one_more, 1919..1999);
    assert!(one_more.has_older && one_more.has_newer && !one_more.reset);
    let last_full = page(json!({"sessionId":"s","newerId":"m1919"}));
    assert_range(&last_full, 1920..2000);
    assert!(last_full.has_older && !last_full.has_newer && !last_full.reset);
    let last_one = page(json!({"sessionId":"s","newerId":"m1998"}));
    assert_range(&last_one, 1999..2000);
    assert!(last_one.has_older && !last_one.has_newer && !last_one.reset);
    let no_newer = page(json!({"sessionId":"s","newerId":"m1999"}));
    assert!(
        no_newer.messages.is_empty()
            && no_newer.has_older
            && !no_newer.has_newer
            && !no_newer.reset
    );

    let empty = page(json!({"sessionId":"empty"}));
    assert!(empty.messages.is_empty() && !empty.has_older && !empty.has_newer && !empty.reset);
    let unchanged = page(json!({"sessionId":"s","afterId":"m1999"}));
    assert!(
        unchanged.messages.is_empty()
            && unchanged.has_older
            && !unchanged.has_newer
            && !unchanged.reset
    );
    let exact = page(json!({"sessionId":"s","afterId":"m1919"}));
    assert_range(&exact, 1920..2000);
    assert!(!exact.reset);
    let overflow = page(json!({"sessionId":"s","afterId":"m1918"}));
    assert_range(&overflow, 1920..2000);
    assert!(overflow.reset);

    conn.execute_batch(
        "UPDATE messages SET metadata='{\"textAttachments\":false}' WHERE id='m1999';
         INSERT INTO messages(id,session_id,role,content,parent_id,created_at)
         VALUES('m2000','s','user','New question','m1999',42),
               ('m2001','s','assistant','New answer','m2000',42);
         UPDATE sessions SET leaf_message_id='m2001' WHERE id='s';",
    )
    .unwrap();
    let tail = page(json!({"sessionId":"s","afterId":"m1999"}));
    assert_range(&tail, 2000..2002);
    assert!(tail.has_older && !tail.has_newer && !tail.reset);
    conn.execute(
        "UPDATE sessions SET leaf_message_id='branch' WHERE id='s'",
        [],
    )
    .unwrap();
    for (direction, id) in [
        ("beforeId", "m1999"),
        ("afterId", "m2001"),
        ("newerId", "m2001"),
        ("aroundId", "foreign"),
        ("beforeId", "hidden"),
        ("aroundId", "missing"),
    ] {
        let reset = page(json!({"sessionId":"s",(direction):id}));
        assert!(reset.reset && reset.has_older && !reset.has_newer);
        assert_eq!(reset.messages.len(), 80);
        assert_eq!(reset.messages.last().unwrap().id, "branch");
    }
    conn.execute(
        "UPDATE sessions SET leaf_message_id='foreign-link' WHERE id='s'",
        [],
    )
    .unwrap();
    let cross_session = page(json!({"sessionId":"s"}));
    assert_eq!(cross_session.messages.len(), 1);
    assert_eq!(cross_session.messages[0].id, "foreign-link");
}

#[cfg(debug_assertions)]
#[test]
fn dev_http_page_keeps_attachment_and_legacy_goal_with_matching_approval() {
    let state = crate::tests::test_app_state();
    {
        let conn = state.db.lock().unwrap();
        crate::db::migrate(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO sessions(id,title,created_at,updated_at) VALUES ('s','Main',1,1);
             INSERT INTO messages(id,session_id,role,content,parent_id,created_at)
             VALUES('root','s','user','Authoritative goal',NULL,4),
                   ('a','s','assistant','Request approval','root',5),
                   ('unselected','s','assistant','Sibling request','root',5),
                   ('file','s','user','Read this','a',5);
             UPDATE sessions SET leaf_message_id='file' WHERE id='s';
             INSERT INTO agent_steps(id,call_id,session_id,tool_name,tool_output,success,seq,created_at)
             VALUES('step-a','call-a','s','run_project_command','Confirmation required: safe fixture',0,1,5),
                   ('step-b','call-b','s','run_project_command','Unselected branch receipt',1,2,5);",
        ).unwrap();
        conn.execute(
            "UPDATE messages SET tool_calls=?1 WHERE id='a'",
            [json!([
                {"id":"call-a","name":"run_project_command","arguments":{}}
            ])
            .to_string()],
        )
        .unwrap();
        conn.execute(
            "UPDATE messages SET metadata=?1 WHERE id='file'",
            [json!({
                "textAttachments":[{"name":"readme.txt","text":"Snapshot"}]
            })
            .to_string()],
        )
        .unwrap();
    }
    let response = crate::handle_dev_command(
        "get_message_page",
        &json!({"sessionId":"s","newerId":"root"}),
        &state,
    );
    assert_eq!(response["ok"], true, "{response}");
    let page = &response["data"];
    assert_eq!(page["hasOlder"], true);
    assert_eq!(page["hasNewer"], false);
    assert_eq!(page["reset"], false);
    let messages = page["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["id"], "a");
    assert_eq!(messages[0]["taskRun"]["goal"], "Authoritative goal");
    assert_eq!(messages[0]["taskRun"]["confirmationState"], "pending");
    assert_eq!(messages[0]["toolResults"].as_array().unwrap().len(), 1);
    assert_eq!(messages[0]["toolResults"][0]["callId"], "call-a");
    assert_eq!(
        messages[0]["toolResults"][0]["confirmationStatus"],
        "pending"
    );
    assert_eq!(messages[1]["textAttachments"][0]["text"], "Snapshot");
    let response = crate::handle_dev_command(
        "get_message_page",
        &json!({"sessionId":"s","newerId":"a","afterId":"a"}),
        &state,
    );
    assert_eq!(response["ok"], false);
    assert!(response.get("data").is_none());
}

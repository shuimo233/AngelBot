//! Owned SQLite + production journal/reader/projection, including a cold reopen.
//! No normal app, desktop, external action or new credential is involved.
use crate::agent::{
    config::AgentConfig,
    event::{DurableEventEmitter, NullEventEmitter},
    tool::{Tool, ToolHandler, ToolResult},
    AgentRunner, NativeAgentSliceResult, ToolRegistry,
};
use crate::commands::{
    foreground_history,
    foreground_tool_sources::{self, ForegroundToolSources},
};
use crate::llm::{LlmProvider, Message};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

pub(super) struct Journey {
    path: PathBuf,
    receipt: String,
    inspections: Arc<AtomicUsize>,
}
struct Inspect {
    output: String,
    calls: Arc<AtomicUsize>,
}
impl ToolHandler for Inspect {
    fn name(&self) -> &str {
        "inspect_task_receipt"
    }
    fn execute(&self, _: &Value, _: &Path) -> ToolResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        ToolResult::success(self.name(), &self.output)
    }
}
impl Journey {
    pub(super) fn new(root: &Path, id: &str, arm: &str, receipt: &Value) -> Self {
        assert!(id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
        let path = root.join(format!("{id}-{arm}.db"));
        let conn = Connection::open(&path).unwrap();
        crate::db::migrate(&conn).unwrap();
        conn.execute_batch("INSERT INTO sessions(id,title,created_at,updated_at) VALUES('owned','Synthetic source journey',1,1);
          INSERT INTO messages(id,session_id,role,content,parent_id,created_at,is_provisional) VALUES('u','owned','user','Inspect original task receipt',NULL,1,0),('r','owned','assistant','','u',2,1);
          INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES('r','owned','r','Inspect original task receipt','running','[]',2,2);
          UPDATE sessions SET leaf_message_id='u' WHERE id='owned';").unwrap();
        Self {
            path,
            receipt: receipt.to_string(),
            inspections: Arc::new(AtomicUsize::new(0)),
        }
    }
    pub(super) fn runner(
        &self,
        provider: Box<dyn LlmProvider>,
        config: AgentConfig,
        arm: &str,
        turn: usize,
    ) -> AgentRunner {
        // A fresh connection and adapter every turn; no fixture receipt cache.
        let db = Arc::new(Mutex::new(Connection::open(&self.path).unwrap()));
        let sources = Arc::new(ForegroundToolSources::new(
            db.clone(),
            "owned".into(),
            (turn == 0).then(|| "r".into()),
            true,
        ));
        let mut registry = ToolRegistry::empty();
        foreground_tool_sources::register(&mut registry, sources.clone());
        if turn == 0 {
            registry.register(
                Tool::new(
                    "inspect_task_receipt",
                    "Read the original synthetic task receipt, not rerun its operation",
                    json!({"type":"object","additionalProperties":false,"properties":{}}),
                )
                .with_read_only_budget_hint(true),
                Arc::new(Inspect {
                    output: self.receipt.clone(),
                    calls: self.inspections.clone(),
                }),
            );
        }
        let mut runner =
            AgentRunner::with_registry(provider, config, registry).with_session_id("owned".into());
        if turn == 0 {
            runner = runner.with_emitter(Arc::new(DurableEventEmitter::new(
                Arc::new(NullEventEmitter),
                db,
                "r".into(),
            )));
            if arm == "compact" {
                runner = runner.with_tool_context_projector(sources);
            }
        }
        runner
    }
    pub(super) fn settle_and_reopen(&self, result: &NativeAgentSliceResult) -> Vec<Message> {
        let conn = Connection::open(&self.path).unwrap();
        let metadata = foreground_history::private_transcript_metadata(&result.protocol_transcript)
            .unwrap()
            .unwrap();
        conn.execute("UPDATE messages SET metadata=?1,is_provisional=0,content='Inspected original receipt' WHERE id='r'",[metadata]).unwrap();
        conn.execute_batch("UPDATE task_runs SET status='completed' WHERE id='r'; UPDATE sessions SET leaf_message_id='r' WHERE id='owned';").unwrap();
        drop(conn);
        let reopened = Connection::open(&self.path).unwrap();
        foreground_history::hydrate_llm_history(&reopened, "owned", 10).unwrap()
    }
    pub(super) fn annotate(&self, row: &mut Value, turn: usize) {
        let calls = self.inspections.load(Ordering::SeqCst);
        row["inspectionExecutions"] = json!(calls);
        row["coldReopen"] = json!(turn > 0);
        assert!(
            calls <= 1,
            "original operation observation must not be replayed"
        );
        assert!(
            turn == 0 || calls == 1,
            "cold turn requires the original durable source"
        );
    }
}

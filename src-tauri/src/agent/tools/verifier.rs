// Issue #102: Post-call verification — bridge from tool declarations to disk truth
//
// The agent trace records whatever the tool handler returned (e.g. "已创建
// AgentTestA.txt") but never crosses-checks that claim against the filesystem.
// `verify_result` (#104) helps the agent judge its own claims, but it still
// relies on the agent to invoke it. PostCallVerifier runs automatically after
// every tool call that declared one and records the evidence to the trace so
// the next request — usually the "complete" step — can surface the gap.
//
// Scope: read-only. We deliberately do NOT extend `ToolResult` (that would
// ripple through the OpenAPI schema and every frontend trace consumer). The
// evidence travels in `agent_step.metadata` only. The completion gate stays
// a soft warning (the agent can still call `complete`; the user sees the
// red mark in the trace and decides whether to undo).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

/// A read-only verifier that runs after a tool completes. It does not modify
/// the returned `ToolResult`; it only emits `VerificationEvidence` that the
/// runner records into the trace.
pub trait PostCallVerifier: Send + Sync {
    /// `args` is the post-`prepare_arguments` JSON the handler ran with.
    /// `result` is what the handler returned (in case the verifier wants to
    /// inspect the success flag or scrape a path out of the content).
    fn verify(&self, args: &Value, result: &ToolResult) -> VerificationEvidence;
}

/// Where the verifier looked for the artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationSource {
    /// Path was resolved against the work_dir sandbox.
    WorkDir,
    /// Path was resolved against a supplemental read directory (declared
    /// by the session — fallback paths that should not normally write).
    Supplemental,
    /// Path could not be resolved under any known sandbox.
    Unresolved,
}

/// One post-call observation. The runner serializes these into the
/// `agent_step.metadata.verification` JSON column.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationEvidence {
    /// Stable identifier for the verifier kind, e.g. `"file_exists"`.
    pub kind: &'static str,
    /// The declared artifact path (relative to whichever source).
    pub target: String,
    /// Whether the tool itself declared the artifact exists (true for
    /// write/edit/create; false for read).
    pub declared_existence: bool,
    /// What the filesystem actually says right now.
    pub actual_existence: bool,
    /// Where the verifier looked.
    pub source: VerificationSource,
    /// `true` iff the disk agrees with the tool's claim. For deletes,
    /// `matched` is `true` when the artifact is gone.
    pub matched: bool,
    /// Human-readable note for the trace; empty when matched.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

// Forward re-export so we don't take a hard dependency on tool.rs at the
// module level. `ToolResult` is the only type we need from the runtime.
use super::tool::ToolResult;

/// Verifier for `write_file` / `edit_file` / `create_directory`. After the
/// handler reports success, we stat the disk to confirm the artifact is
/// actually there. If the handler said "success" but the file is missing,
/// the trace gets a red evidence row.
pub struct FileExistenceVerifier;

impl PostCallVerifier for FileExistenceVerifier {
    fn verify(&self, args: &Value, _result: &ToolResult) -> VerificationEvidence {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let actual = !path.is_empty() && Path::new(&path).exists();
        let matched = actual; // write/edit/create: tool says success ⇒ file should exist
        VerificationEvidence {
            kind: "file_exists",
            target: path,
            declared_existence: true,
            actual_existence: actual,
            source: VerificationSource::WorkDir,
            matched,
            note: if matched {
                String::new()
            } else {
                "工具声称成功，但磁盘上未找到产物".to_string()
            },
        }
    }
}

/// Verifier for `delete_file`. The handler reports success ⇒ the file should
/// be gone at the moment we verify.
pub struct FileAbsenceVerifier;

impl PostCallVerifier for FileAbsenceVerifier {
    fn verify(&self, args: &Value, _result: &ToolResult) -> VerificationEvidence {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let actual = !path.is_empty() && Path::new(&path).exists();
        VerificationEvidence {
            kind: "file_absent",
            target: path,
            declared_existence: false,
            actual_existence: actual,
            source: VerificationSource::WorkDir,
            matched: !actual,
            note: if actual {
                "工具声称已删除，但磁盘上仍存在".to_string()
            } else {
                String::new()
            },
        }
    }
}

/// Wrap a verifier so it can be stored in `Tool::post_call_verifier`.
pub fn arc<T: PostCallVerifier + 'static>(v: T) -> Arc<dyn PostCallVerifier> {
    Arc::new(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result_success() -> ToolResult {
        ToolResult::success("write_file", "ok")
    }

    #[test]
    fn file_existence_verifier_matches_when_path_is_real() {
        let tmp = std::env::temp_dir().join("angelbot-verifier-exists.txt");
        std::fs::write(&tmp, "hello").unwrap();
        let args = serde_json::json!({"path": tmp.to_string_lossy()});
        let ev = FileExistenceVerifier.verify(&args, &result_success());
        assert!(ev.matched);
        assert!(ev.actual_existence);
        assert!(ev.note.is_empty());
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn file_existence_verifier_flags_missing_path() {
        let path = std::env::temp_dir().join("angelbot-verifier-missing-xyz.txt");
        // ensure it really does not exist
        let _ = std::fs::remove_file(&path);
        let args = serde_json::json!({"path": path.to_string_lossy()});
        let ev = FileExistenceVerifier.verify(&args, &result_success());
        assert!(!ev.matched);
        assert!(!ev.actual_existence);
        assert!(!ev.note.is_empty());
    }

    #[test]
    fn file_existence_verifier_handles_empty_path() {
        let args = serde_json::json!({"path": ""});
        let ev = FileExistenceVerifier.verify(&args, &result_success());
        assert!(!ev.matched);
        assert!(ev.target.is_empty());
    }

    #[test]
    fn file_absence_verifier_matches_when_path_is_gone() {
        let path = std::env::temp_dir().join("angelbot-verifier-absent.txt");
        let _ = std::fs::remove_file(&path);
        let args = serde_json::json!({"path": path.to_string_lossy()});
        let ev = FileAbsenceVerifier.verify(&args, &result_success());
        assert!(ev.matched);
        assert!(ev.note.is_empty());
    }

    #[test]
    fn arc_helper_wraps_in_dyn() {
        let v: Arc<dyn PostCallVerifier> = arc(FileExistenceVerifier);
        let args = serde_json::json!({"path": "/nonexistent"});
        let ev = v.verify(&args, &result_success());
        assert!(!ev.matched);
    }
}

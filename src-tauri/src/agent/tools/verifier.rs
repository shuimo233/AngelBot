// Read-only post-call observations attached to AgentStep. This module changes
// neither ToolResult, task-completion policy nor access permissions. Resolve
// targets under the tool's execution work_dir, never the process cwd.

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
    /// `work_dir` is the same current scope used by the tool execution.
    fn verify(&self, args: &Value, result: &ToolResult, work_dir: &Path) -> VerificationEvidence;
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

/// None is an unresolved/failed observation, not evidence of absence.
fn observe_existence(args: &Value, work_dir: &Path) -> (String, Option<bool>) {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let observed = if path.is_empty() {
        None
    } else {
        crate::commands::file::resolve_in_existing_root(work_dir, Path::new(&path), false)
            .ok()
            .and_then(|resolved| resolved.try_exists().ok())
    };
    (path, observed)
}

impl PostCallVerifier for FileExistenceVerifier {
    fn verify(&self, args: &Value, _result: &ToolResult, work_dir: &Path) -> VerificationEvidence {
        let (path, observed) = observe_existence(args, work_dir);
        let matched = observed == Some(true);
        VerificationEvidence {
            kind: "file_exists",
            target: path,
            declared_existence: true,
            actual_existence: observed.unwrap_or(false),
            source: if observed.is_some() {
                VerificationSource::WorkDir
            } else {
                VerificationSource::Unresolved
            },
            matched,
            note: if matched {
                String::new()
            } else if observed.is_none() {
                "无法在当前工作目录内确认产物状态".to_string()
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
    fn verify(&self, args: &Value, _result: &ToolResult, work_dir: &Path) -> VerificationEvidence {
        let (path, observed) = observe_existence(args, work_dir);
        VerificationEvidence {
            kind: "file_absent",
            target: path,
            declared_existence: false,
            actual_existence: observed.unwrap_or(false),
            source: if observed.is_some() {
                VerificationSource::WorkDir
            } else {
                VerificationSource::Unresolved
            },
            matched: observed == Some(false),
            note: if observed.is_none() {
                "无法在当前工作目录内确认产物状态".to_string()
            } else if observed == Some(true) {
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
        let root = tempfile::tempdir().unwrap();
        let tmp = root.path().join("exists.txt");
        std::fs::write(&tmp, "hello").unwrap();
        for path in ["exists.txt".to_owned(), tmp.to_string_lossy().into_owned()] {
            let ev = FileExistenceVerifier.verify(
                &serde_json::json!({"path":path}),
                &result_success(),
                root.path(),
            );
            assert!(ev.matched);
            assert!(ev.actual_existence);
            assert_eq!(ev.source, VerificationSource::WorkDir);
            assert!(ev.note.is_empty());
        }
        let other = tempfile::tempdir().unwrap();
        let ev = FileExistenceVerifier.verify(
            &serde_json::json!({"path":tmp}),
            &result_success(),
            other.path(),
        );
        assert!(!ev.matched);
        assert_eq!(ev.source, VerificationSource::Unresolved);
    }

    #[test]
    fn file_existence_verifier_flags_missing_path() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("missing.txt");
        let args = serde_json::json!({"path": path.to_string_lossy()});
        let ev = FileExistenceVerifier.verify(&args, &result_success(), root.path());
        assert!(!ev.matched);
        assert!(!ev.actual_existence);
        assert!(!ev.note.is_empty());
        let absent_root = root.path().join("removed-workspace");
        let ev = FileExistenceVerifier.verify(
            &serde_json::json!({"path":"report.txt"}),
            &result_success(),
            &absent_root,
        );
        assert!(!ev.matched);
        assert!(
            !absent_root.exists(),
            "a read-only observation must not recreate the workspace"
        );
    }

    #[test]
    fn file_existence_verifier_handles_empty_path() {
        let root = tempfile::tempdir().unwrap();
        let args = serde_json::json!({"path": ""});
        let ev = FileExistenceVerifier.verify(&args, &result_success(), root.path());
        assert!(!ev.matched);
        assert!(ev.target.is_empty());
    }

    #[test]
    fn file_absence_verifier_matches_when_path_is_gone() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("absent.txt");
        let args = serde_json::json!({"path": path.to_string_lossy()});
        let ev = FileAbsenceVerifier.verify(&args, &result_success(), root.path());
        assert!(ev.matched);
        assert!(ev.note.is_empty());
        let other = tempfile::tempdir().unwrap();
        for path in [
            "".to_owned(),
            other
                .path()
                .join("absent.txt")
                .to_string_lossy()
                .into_owned(),
        ] {
            let ev = FileAbsenceVerifier.verify(
                &serde_json::json!({"path":path}),
                &result_success(),
                root.path(),
            );
            assert!(
                !ev.matched,
                "unknown or out-of-scope is not verified absence"
            );
            assert_eq!(ev.source, VerificationSource::Unresolved);
        }
    }

    #[test]
    fn arc_helper_wraps_in_dyn() {
        let v: Arc<dyn PostCallVerifier> = arc(FileExistenceVerifier);
        let root = tempfile::tempdir().unwrap();
        let args = serde_json::json!({"path": "/nonexistent"});
        let ev = v.verify(&args, &result_success(), root.path());
        assert!(!ev.matched);
    }
}

//! Soft termination guards (Issue #096 — Phase 10).
//!
//! Guards observe the runner loop state and emit one of three verdicts:
//!
//! - `Continue` — let the next iteration proceed normally.
//! - `PauseForUser` — yield control back to the user (no AgentEnd).
//! - `Terminate` — hard stop with a `TerminalBranchKind` for audit.
//!
//! Design constraints (per AngelBot design philosophy):
//!
//! - Guards never mutate state directly; the runner owns state mutation.
//! - `WallClockGuard` is the only hard terminator in this set.
//! - NetProgressGuard is a soft pause. SideEffectAutoPauseGuard adds a
//!   count-based pause only outside the user's full-access task authorization.
//! - Side-effect counting uses the registry's conservative predicate. Explicit
//!   MCP read-only hints affect this soft budget, not execution authorization.
//!
//! Pi-inspired naming: `shouldStopAfterTurn` is a hook today; these guards
//! are an extension layer that catches the cases Pi leaves to user abort.

use crate::agent::lifecycle::TerminalBranchKind;
use crate::agent::registry::is_side_effect_tool;
use std::time::Duration;

/// Snapshot of loop state given to each guard on every evaluation.
#[derive(Debug, Clone)]
pub struct GuardContext {
    /// Zero-indexed main iteration count.
    pub iteration: usize,
    /// Elapsed wall-clock time since the current turn started.
    pub turn_wall_clock_elapsed: Duration,
    /// Count of side-effect tool calls executed in the current turn.
    pub side_effect_count: usize,
    /// Selected by the runtime, never supplied by model/tool arguments. This
    /// changes the count fuse only, not scope, approval or unknown-result rules.
    pub full_access: bool,
    /// Number of consecutive iterations without new evidence.
    pub no_progress_streak: usize,
    /// Whether the latest assistant response added new facts or tool results.
    pub last_response_was_progress: bool,
}

/// What a guard decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardVerdict {
    Continue,
    PauseForUser {
        reason: String,
    },
    Terminate {
        reason: String,
        branch: TerminalBranchKind,
    },
}

/// Trait every guard implements. Guards must be cheap, sync, and side-effect free.
pub trait LoopGuard: Send + Sync {
    fn name(&self) -> &'static str;
    fn evaluate(&self, ctx: &GuardContext) -> GuardVerdict;
}

// ── WallClockGuard ────────────────────────────────────────────────────────────

/// Hard-terminates when the current turn exceeds the budget.
pub struct WallClockGuard {
    budget: Duration,
}

impl WallClockGuard {
    pub fn new(budget: Duration) -> Self {
        Self { budget }
    }
}

impl LoopGuard for WallClockGuard {
    fn name(&self) -> &'static str {
        "wall_clock"
    }

    fn evaluate(&self, ctx: &GuardContext) -> GuardVerdict {
        if ctx.turn_wall_clock_elapsed >= self.budget {
            GuardVerdict::Terminate {
                reason: format!(
                    "wall clock budget exceeded: {:?} (iteration {})",
                    ctx.turn_wall_clock_elapsed, ctx.iteration
                ),
                branch: TerminalBranchKind::WallClockTimeout,
            }
        } else {
            GuardVerdict::Continue
        }
    }
}

// ── NetProgressGuard ──────────────────────────────────────────────────────────

/// Pauses for user input after `threshold` consecutive iterations without
/// new evidence. This is the soft alternative to the existing
/// `repeated_tool_batch_count` hard stop.
pub struct NetProgressGuard {
    threshold: usize,
}

impl NetProgressGuard {
    pub fn new(threshold: usize) -> Self {
        Self { threshold }
    }
}

impl LoopGuard for NetProgressGuard {
    fn name(&self) -> &'static str {
        "net_progress"
    }

    fn evaluate(&self, ctx: &GuardContext) -> GuardVerdict {
        if !ctx.last_response_was_progress && ctx.no_progress_streak >= self.threshold {
            GuardVerdict::PauseForUser {
                reason: format!(
                    "no progress for {} consecutive iterations; user input needed",
                    ctx.no_progress_streak
                ),
            }
        } else {
            GuardVerdict::Continue
        }
    }
}

// ── SideEffectAutoPauseGuard ──────────────────────────────────────────────────

/// Pauses (never terminates) after the configured number of side-effect
/// tool calls outside full access. Full access already authorizes continuing
/// the task; time, progress, cancellation and tool-specific controls still apply.
pub struct SideEffectAutoPauseGuard {
    threshold: usize,
}

impl SideEffectAutoPauseGuard {
    pub fn new(threshold: usize) -> Self {
        Self { threshold }
    }
}

impl LoopGuard for SideEffectAutoPauseGuard {
    fn name(&self) -> &'static str {
        "side_effect_auto_pause"
    }

    fn evaluate(&self, ctx: &GuardContext) -> GuardVerdict {
        if !ctx.full_access && ctx.side_effect_count >= self.threshold {
            GuardVerdict::PauseForUser {
                reason: format!(
                    "side effect limit reached ({}); user confirmation needed before more",
                    ctx.side_effect_count
                ),
            }
        } else {
            GuardVerdict::Continue
        }
    }
}

/// Helper used by the runner to know whether a tool name counts as a
/// side effect for the soft budget. Unknown MCP tools remain conservative;
/// names are never used to infer read-only behavior. Hints do not affect the
/// registry's independent confirmation policy or durable dispatch reservations.
pub fn is_side_effect(registry: &crate::agent::registry::ToolRegistry, name: &str) -> bool {
    is_side_effect_tool(name)
        && !(name.starts_with("mcp_")
            && registry
                .get(name)
                .is_some_and(|tool| tool.read_only_budget_hint))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_side_effect_delegates_to_registry() {
        let registry = crate::agent::registry::ToolRegistry::empty();
        assert!(is_side_effect(&registry, "write_file"));
        assert!(is_side_effect(&registry, "create_directory"));
        assert!(is_side_effect(&registry, "mcp_anything"));
        assert!(!is_side_effect(&registry, "read_file"));
        assert!(!is_side_effect(&registry, "search_files"));
    }
}

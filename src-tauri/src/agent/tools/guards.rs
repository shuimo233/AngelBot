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
//! - `NetProgressGuard` and `SideEffectAutoPauseGuard` are *soft* pauses
//!   that preserve user sovereignty (philosophy §2.3).
//! - Side-effect counting uses the centralised `is_side_effect_tool` from
//!   the registry so the two lists cannot drift.
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
/// tool calls. Per design philosophy §2.3, side-effect auto-pause must be
/// soft — only the user may authorise further side effects.
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
        if ctx.side_effect_count >= self.threshold {
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
/// side effect for guard accounting. Re-exports the registry-level predicate
/// to keep the single source of truth.
pub fn is_side_effect(name: &str) -> bool {
    is_side_effect_tool(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_side_effect_delegates_to_registry() {
        assert!(is_side_effect("write_file"));
        assert!(is_side_effect("create_directory"));
        assert!(is_side_effect("mcp_anything"));
        assert!(!is_side_effect("read_file"));
        assert!(!is_side_effect("search_files"));
    }
}

//! Tests for the LoopGuard system (Phase 10, Issue #096).
//!
//! Each guard decides whether the agent loop should continue, pause for
//! user input, or terminate. Guards are evaluated at every main iteration
//! boundary and again after a tool batch — they observe state but do not
//! mutate it directly.
//!
//! Reference: docs/agent-architecture-v2.md §11 + issues/096-pi-style-soft-terminate-loop.md

#[cfg(test)]
mod tests {
    use crate::agent::guards::{
        GuardContext, GuardVerdict, LoopGuard, NetProgressGuard, SideEffectAutoPauseGuard,
        WallClockGuard,
    };
    use crate::agent::lifecycle::TerminalBranchKind;
    use std::time::Duration;

    fn ctx_no_progress(iter: usize) -> GuardContext {
        GuardContext {
            iteration: iter,
            turn_wall_clock_elapsed: Duration::ZERO,
            side_effect_count: 0,
            no_progress_streak: 0,
            last_response_was_progress: false,
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // WallClockGuard
    // ────────────────────────────────────────────────────────────────────────

    #[test]
    fn wall_clock_guard_continues_within_budget() {
        let guard = WallClockGuard::new(Duration::from_secs(600));
        let mut ctx = ctx_no_progress(0);
        ctx.turn_wall_clock_elapsed = Duration::from_secs(120);
        match guard.evaluate(&ctx) {
            GuardVerdict::Continue => {}
            other => panic!("expected Continue, got {:?}", other),
        }
    }

    #[test]
    fn wall_clock_guard_terminates_at_budget() {
        let guard = WallClockGuard::new(Duration::from_secs(600));
        let mut ctx = ctx_no_progress(5);
        ctx.turn_wall_clock_elapsed = Duration::from_secs(601);
        match guard.evaluate(&ctx) {
            GuardVerdict::Terminate {
                branch: TerminalBranchKind::WallClockTimeout,
                ..
            } => {}
            other => panic!("expected Terminate WallClockTimeout, got {:?}", other),
        }
    }

    #[test]
    fn wall_clock_guard_terminates_well_over_budget() {
        let guard = WallClockGuard::new(Duration::from_secs(60));
        let mut ctx = ctx_no_progress(2);
        ctx.turn_wall_clock_elapsed = Duration::from_secs(900);
        match guard.evaluate(&ctx) {
            GuardVerdict::Terminate {
                branch: TerminalBranchKind::WallClockTimeout,
                reason,
            } => {
                assert!(reason.contains("wall clock"));
            }
            other => panic!("expected Terminate WallClockTimeout, got {:?}", other),
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // NetProgressGuard
    // ────────────────────────────────────────────────────────────────────────

    #[test]
    fn net_progress_guard_continues_when_iteration_makes_progress() {
        let guard = NetProgressGuard::new(5);
        let mut ctx = ctx_no_progress(3);
        ctx.last_response_was_progress = true;
        match guard.evaluate(&ctx) {
            GuardVerdict::Continue => {}
            other => panic!("expected Continue, got {:?}", other),
        }
    }

    #[test]
    fn net_progress_guard_pauses_for_user_after_threshold() {
        let guard = NetProgressGuard::new(5);
        let mut ctx = ctx_no_progress(7);
        ctx.no_progress_streak = 5;
        ctx.last_response_was_progress = false;
        match guard.evaluate(&ctx) {
            GuardVerdict::PauseForUser { reason } => {
                assert!(reason.to_lowercase().contains("progress"));
            }
            other => panic!("expected PauseForUser, got {:?}", other),
        }
    }

    #[test]
    fn net_progress_guard_continues_below_threshold() {
        let guard = NetProgressGuard::new(5);
        let mut ctx = ctx_no_progress(7);
        ctx.no_progress_streak = 4;
        ctx.last_response_was_progress = false;
        match guard.evaluate(&ctx) {
            GuardVerdict::Continue => {}
            other => panic!("expected Continue at streak 4, got {:?}", other),
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // SideEffectAutoPauseGuard
    // ────────────────────────────────────────────────────────────────────────

    #[test]
    fn side_effect_guard_continues_below_threshold() {
        let guard = SideEffectAutoPauseGuard::new(5);
        let mut ctx = ctx_no_progress(3);
        ctx.side_effect_count = 4;
        match guard.evaluate(&ctx) {
            GuardVerdict::Continue => {}
            other => panic!("expected Continue at 4, got {:?}", other),
        }
    }

    #[test]
    fn side_effect_guard_pauses_at_threshold() {
        let guard = SideEffectAutoPauseGuard::new(5);
        let mut ctx = ctx_no_progress(3);
        ctx.side_effect_count = 5;
        match guard.evaluate(&ctx) {
            GuardVerdict::PauseForUser { reason } => {
                assert!(reason.to_lowercase().contains("side effect"));
            }
            other => panic!("expected PauseForUser at 5, got {:?}", other),
        }
    }

    #[test]
    fn side_effect_guard_does_not_terminate_ever() {
        // SideEffectAutoPause must always be a soft pause, never a hard terminate.
        // Hard termination would violate design philosophy §2.3 (user sovereignty).
        let guard = SideEffectAutoPauseGuard::new(2);
        let mut ctx = ctx_no_progress(10);
        ctx.side_effect_count = 1000;
        match guard.evaluate(&ctx) {
            GuardVerdict::PauseForUser { .. } => {}
            other => panic!("expected PauseForUser even at 1000, got {:?}", other),
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // LoopGuard trait surface
    // ────────────────────────────────────────────────────────────────────────

    #[test]
    fn each_guard_has_a_distinct_name() {
        let a = WallClockGuard::new(Duration::from_secs(10));
        let b = NetProgressGuard::new(5);
        let c = SideEffectAutoPauseGuard::new(5);
        let names = vec![a.name(), b.name(), c.name()];
        let unique: std::collections::HashSet<_> = names.iter().copied().collect();
        assert_eq!(
            unique.len(),
            3,
            "guards must have unique names, got {:?}",
            names
        );
    }

    #[test]
    fn guard_names_are_stable_strings() {
        // Names feed into agent_steps audit + telemetry — changing them breaks dashboards.
        assert_eq!(
            WallClockGuard::new(Duration::from_secs(1)).name(),
            "wall_clock"
        );
        assert_eq!(NetProgressGuard::new(1).name(), "net_progress");
        assert_eq!(
            SideEffectAutoPauseGuard::new(1).name(),
            "side_effect_auto_pause"
        );
    }

    #[test]
    fn guard_context_construction_does_not_clone_messages() {
        // `GuardContext` carries `Duration`, `usize`, `bool` only — no
        // `Vec` or `String`. The runner must NOT push the full message
        // list into the guard context; guards observe aggregate counters,
        // not the conversation body. This anchors the cheap-evaluate
        // contract documented on the trait.
        let ctx = GuardContext {
            iteration: 0,
            turn_wall_clock_elapsed: Duration::ZERO,
            side_effect_count: 0,
            no_progress_streak: 0,
            last_response_was_progress: false,
        };
        // If we ever add `messages: Vec<...>` the struct size doubles
        // and this assertion catches the regression.
        assert!(
            std::mem::size_of::<GuardContext>() <= 64,
            "GuardContext must stay tiny, got {} bytes",
            std::mem::size_of::<GuardContext>()
        );
        // Sanity: the field used in tests is `last_response_was_progress`.
        let _ = ctx.last_response_was_progress;
    }

    #[test]
    fn guards_are_evaluated_independently() {
        // The runner calls each guard in registration order and takes
        // the first non-Continue verdict. The guards themselves must
        // not depend on each other — verify by running them out of order.
        let wall = WallClockGuard::new(Duration::from_secs(0));
        let side = SideEffectAutoPauseGuard::new(1);
        let progress = NetProgressGuard::new(1);

        let mut ctx = GuardContext {
            iteration: 0,
            turn_wall_clock_elapsed: Duration::from_secs(10), // past budget
            side_effect_count: 0,
            no_progress_streak: 0,
            last_response_was_progress: false,
        };

        // Order 1: wall-clock fires first.
        let mut verdicts = vec![
            wall.evaluate(&ctx),
            side.evaluate(&ctx),
            progress.evaluate(&ctx),
        ];
        assert!(matches!(verdicts[0], GuardVerdict::Terminate { .. }));

        // Order 2: side-effect guard sees a saturated counter.
        ctx.side_effect_count = 5;
        verdicts = vec![
            side.evaluate(&ctx),
            progress.evaluate(&ctx),
            wall.evaluate(&ctx),
        ];
        assert!(matches!(verdicts[0], GuardVerdict::PauseForUser { .. }));
    }
}

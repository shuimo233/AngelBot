---
name: angelbot-verify
description: Validate AngelBot changes with the repository's canonical Python test flow. Use when modifying AngelBot code, preparing a handoff, or checking CI readiness.
---

# AngelBot verification

Use `python scripts/verify.py` as the canonical integration/acceptance runner.
Follow `AGENTS.md` for scope and continuation; a passing check is not a reason
to stop approved development or ask for routine confirmation.

## Choose evidence for the changed contract

- During implementation, run the smallest relevant regression using the existing
  npm/Cargo runners. Route focused commands through
  `scripts.verify.run_check(Check(...), fail_fast=True)` to reuse its isolated
  environment and temporary test-configuration directory; propagate the returned
  boolean as exit status 0/1. Command examples are `npm test -- --run <test-file>`
  and `cargo test --manifest-path src-tauri/Cargo.toml --lib <filter> -- --test-threads=1`.
  Use temporary fixtures, never the owner's configuration. Confirm the intended
  cases actually ran; a targeted pass proves only those cases, not a profile.
- Use `python scripts/verify.py frontend` or `backend` when broader layer coverage
  is needed. Use `quick` at a coherent cross-layer integration point. Do not
  automatically run quick, a layer profile and full for the same unchanged input.
- Resolve intended edits and review findings before `python scripts/verify.py full`.
  Require a full pass for code milestone acceptance and before a code handoff,
  commit or CI-readiness claim. Continue the next approved item if work remains;
  verification does not authorize deployment, merge or release.
- For documentation/instruction-only work with no executable, dependency, build
  or CI change, check diff hygiene, referenced paths and any applicable validator.
  Reuse valid code evidence without claiming a new full run or accepting unrelated
  pre-existing changes. If this exception does not fit, use the code gates above.

## Preserve evidence and safety

- Reuse results while their inputs and acceptance conditions remain valid; rerun
  affected checks after a relevant change. Stage intended files before final
  verification and keep manual logs in ignored `logs/` or `src-tauri/target/`.
- Keep full deterministic and offline: no live credentials, network or user data.
  Serialize Cargo builds. Native and live-model acceptance remain separate and
  subject to their existing authorization; waiting for them need not block
  independent approved implementation.
- Diagnose failures and fix the affected contract; do not weaken assertions,
  bypass checks, count ignored cases as passed, or loop unknown side effects.
  A command's exit status and actual assertions, not dispatch, determine its result.

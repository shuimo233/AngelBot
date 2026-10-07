---
name: angelbot-verify
description: Validate AngelBot changes with the repository's canonical Python test flow. Use when modifying AngelBot code, preparing a handoff, or checking CI readiness.
---

# AngelBot verification

Use the Python runner as the single source of truth for local and CI validation.

1. After a focused edit, run `python scripts/verify.py quick`.
   Completion: diff hygiene, frontend compilation, and Rust type checking pass.
2. Before handing off, committing, or enabling a workflow, run `python scripts/verify.py full`.
   Completion: every check passes; do not treat a skipped or failed check as a successful validation.
3. When working in one layer, use `frontend` or `backend` only while iterating. Run `full` before completion.
4. Keep tests deterministic: use mock model/network providers and temporary databases. Never require API keys, a live model, or user runtime data for automated verification.

If a check fails, report its command and failure category, then fix the affected code or test expectation. Do not weaken assertions merely to make the runner green.

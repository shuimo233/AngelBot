# AngelBot agent guidance

## Development scope

- Work toward the current user-visible result; include necessary callers, data,
  tests and documentation, but do not expand unrelated modules.
- Reuse suitable existing modules, dependencies and Windows capabilities. Add a
  mechanism or continue research only to close a concrete current gap.
- Collect review findings and finish the intended edits before final verification.
  Reuse evidence that remains valid; do not repeat reads, tests or reviews without
  a changed input, acceptance condition or unresolved finding.
- Put manual build/test logs in ignored `logs/` or existing `src-tauri/target/`
  evidence paths, not the repository root. Use task-owned temporary locations;
  do not sweep runtime data or delete files solely because their names look temporary.

## Execution cadence

- Use the latest local `docs/current-roadmap.md` and the user's current request
  to choose the next approved milestone. Do not substitute an older public copy
  or expand the product scope to keep working. Choose ordinary implementation
  details within the agreed goal without requiring a new plan approval.
- For sustained development requests, continue implementation, focused checks,
  review and the next approved actionable item without asking for routine
  confirmation. A small fix, passing test or progress update is not a handoff;
  completing one milestone is not a reason to stop if approved work remains.
- Ask when a material architecture/product decision or new authority is needed,
  or a genuine blocker cannot be resolved safely. If only native/live acceptance
  is waiting on access or user availability, record the unverified boundary and
  continue independent approved work; do not replay unknown side effects.
- Keep progress updates brief and non-blocking. End the turn when the requested
  scope is complete, no safe approved work remains, user input is necessary to
  proceed, or the user asks to pause. Report outcomes and use-affecting limits,
  not a diary of internal checks. Continuing development never authorizes push,
  merge, release, permission changes or use of the user's desktop by itself.

## Implementation contracts

- Keep Rust IPC, corresponding Dev HTTP paths, typed frontend wrappers and callers
  aligned. Make development/test-only exceptions explicit; do not scatter raw
  invocation or silently translate errors into empty/successful results.
- Use forward migrations for schema changes; preserve existing data and check
  defaults, constraints, backup/restore and affected callers. Do not rewrite
  published migrations or weaken foreign keys to satisfy a fixture.
- Tool schemas describe bounded actions. Recheck current scope and approval at
  execution, preserve stable tool identity, and never infer business completion
  from dispatch or automatically replay unknown side effects.
- Reuse relevant tests and add the smallest regression for changed behavior.
  There is no per-issue quota for new tests or E2E cases; choose evidence for the
  affected contract and keep offline, native and live-model results distinct.

## Verification

Use `python scripts/verify.py` as the canonical test entry point.

- During iteration, run the smallest relevant regression with the existing
  isolated runner (see the verification skill); use `frontend` or `backend` when
  broader layer coverage is needed.
  Passing local checks means continue the approved work, not request approval.
- Use `quick` at a coherent integration point, especially after caller/contract
  changes across layers; do not stack it after every edit or repeat equivalent
  checks already covered by a current broader result.
- Run `full` after intended code edits and review findings are resolved, for
  milestone acceptance and before a code handoff, commit or CI-readiness claim.
  A full pass is a verification gate, not an instruction to end ongoing work.
- Documentation/instruction-only work that does not change executable code,
  dependencies, build or CI configuration uses diff hygiene, reference checks
  and applicable document/skill validators instead of rebuilding the app. Do not
  use this exception to claim unrelated pre-existing code changes are verified.

The full profile must run without live model credentials, network access, or user runtime data. Keep test doubles deterministic and fix failed assertions rather than bypassing them.

The reusable workflow is documented in `.codex/skills/angelbot-verify/SKILL.md`.

## Git and pull requests

- Work on a short-lived `codex/<change>` branch based on `main`; target `main` in
  the PR. Do not commit or push directly to integration/stable branches.
- Use a Conventional-Commit-style PR title, for example
  `fix(desktop): reject stale window targets`. Intermediate working commits need
  not be rewritten just to satisfy a title convention.
- Inspect active local hooks before committing. A commit must not implicitly
  publish changes; push/create a PR only when the user requests that workflow.
- Keep changes focused and buildable, reuse existing modules, and remove replaced
  paths rather than retaining duplicate implementations. Review the diff from an
  independent reviewer perspective; passing tests alone is not a code review.
- Stage the intended files before final verification so new files are included
  in staged diff hygiene. Never stage runtime data, credentials or generated files.
- Follow `.github/RELEASING.md` for review, promotion and merge rules. Do not
  merge a PR, rewrite published history, change repository visibility or create
  release tags without explicit user authorization.

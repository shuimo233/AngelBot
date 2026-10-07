# AngelBot agent guidance

## Verification

Use `python scripts/verify.py` as the canonical test entry point.

- `quick` after a focused change.
- `frontend` or `backend` while iterating within one layer.
- `full` before handoff, commit, or CI readiness claims.

The full profile must run without live model credentials, network access, or user runtime data. Keep test doubles deterministic and fix failed assertions rather than bypassing them.

The reusable workflow is documented in `.codex/skills/angelbot-verify/SKILL.md`.

## Git and pull requests

- Work on a short-lived `codex/<change>` branch based on `dev`; target `dev` in
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

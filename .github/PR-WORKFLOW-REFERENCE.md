# GitHub PR workflow reference

Verified against official GitHub documentation on 2026-10-08. This is a reference,
not proof that remote repository settings have been configured.

## Public repository bootstrap

The original private repository returned HTTP 403 for protection/ruleset APIs.
The owner then chose MIT and approved preserving that predecessor as a private
archive while creating a separate public repository. On 2026-10-08 the new
`shuimo233/AngelBot` was created with default branch `dev` and one clean root from
the verified source tree; the predecessor remains private and archived at
`shuimo233/AngelBot-private-archive-20261007`. No old refs/history were uploaded.

The public repository has one active `Integration PR gates` ruleset for `dev`,
with PRs, resolved conversations, no force pushes/deletions and no bypass actors.
Required check sources must be observed in this new repository, not inherited
from the archive. Bootstrap those requirements only after successful runs, then
read back the actual ruleset. The absent stable `main` is protected by extending
the same ruleset when it is deliberately established; do not publish old local
`main` or add a creation exception just to bypass unavailable checks.

Protected branches and branch/tag rulesets are available for public repositories
on GitHub Free; private repositories require an eligible paid plan. This matches
the observed limitation, but does not establish the account's exact subscription.
[Protected branches](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches),
[Rulesets](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/about-rulesets).

Checked-in workflows and templates alone do not establish remote merge gates.
Verify the ruleset's enforcement, conditions, bypass actors and check sources;
do not equate a policy example or local full test with GitHub enforcement.

Renaming plus a new repository keeps predecessor objects out of the public
object graph. Rewriting a branch in the same repository alone does not remove
all cached or SHA-addressable content. Keep the archive and recovery bundle
private; an ancestry hook cannot detect sensitive content cherry-picked into a
new commit. [Sensitive-history limitations](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/removing-sensitive-data-from-a-repository).

## Lightweight branch and review policy

Repository convention: short-lived `codex/<change>` branches target integration
branch `dev`; promote tested integration changes to stable `main` through a
separate PR. Prefer merge commits for long-lived `dev` to `main` promotions, not
repeated squash merges; keep merge commits enabled and do not require linear
history on `main` if using this policy.
[Long-running branches](https://docs.github.com/en/pull-requests/reference/pull-request-merges#squashing-and-merging-a-long-running-branch).

When protection is available, require PRs and successful checks on both branches,
block force pushes/deletions, and resolve review conversations. Avoid maintaining
overlapping classic protections and rulesets: applicable rules are combined.
[Rule layering](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/about-rulesets#about-rulesets-and-protected-branches).

A PR author cannot approve their own PR. For a sole maintainer, use zero required
approvals rather than an impossible self-approval requirement; disable mandatory
code-owner and last-push approval until another eligible reviewer exists. This is
a workflow recommendation, not a replacement for an independent review.
[Required reviews](https://docs.github.com/en/pull-requests/how-tos/review-pull-requests/approving-a-pull-request-with-required-reviews),
[Approval settings](https://docs.github.com/en/rest/branches/branch-protection#update-branch-protection).

Classic protections exempt admins by default. Enable their admin-enforcement
option if enforcement is intended; for rulesets, inspect bypass actors explicitly.
[Admin bypass](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches#do-not-allow-bypassing-the-above-settings).

## Required checks and minimal CI

Use unique, stable Actions **job/check names**, not an assumed workflow display
name. Select the exact names after a successful run; selectable required checks
must have succeeded in the repository within the past seven days. Where offered,
select GitHub Actions as their expected source rather than accepting any sender.
[Check names and sources](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches#require-status-checks-before-merging),
[Check selection](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks).

The check-run API exposes the exact job name and producing `app.id`. Ruleset
requirements use `integration_id`; classic protection uses `app_id`, neither a
check-run ID nor an installation ID. For this repository the three names are
`Windows full verification`, `Windows desktop smoke test`, and `PR policy`.
Read their actual runs before attaching them to the single active ruleset with
`strict_required_status_checks_policy: true` and no branch-creation exemption.
[Check runs](https://docs.github.com/en/rest/checks/runs#list-check-runs-for-a-git-reference),
[Ruleset API](https://docs.github.com/en/rest/repos/rules#create-a-repository-ruleset).

Run the canonical `python scripts/verify.py full` gate without credentials, live
model requests, or user runtime data. Do not path-filter a required workflow:
skipped workflows can leave checks Pending. A skipped conditional job can count
as success; a required aggregate job needs `always()` plus explicit dependency
result checks so a failed prerequisite cannot silently pass.
[Skipped required checks](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks#handling-skipped-but-required-checks).

For CI, declare `permissions: {contents: read}`; unspecified permission scopes
become `none`. Use a concurrency group containing `github.workflow` and
`github.ref`, with `cancel-in-progress: true`, to cancel obsolete runs without
canceling another workflow's work.
[Permissions](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#permissions),
[Concurrency](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/control-workflow-concurrency#example-only-cancel-in-progress-jobs-or-runs-for-the-current-workflow).

Use `pull_request`, not `pull_request_target`, for testing contributor code.
Fork PRs normally receive a read-only token and no repository secrets; privileged
target workflows become dangerous when they run untrusted PR code. Do not add
secret/write-token exceptions or a self-hosted runner just to make CI work.
[Fork PR security](https://docs.github.com/en/actions/reference/security/securely-using-pull_request_target).

## Titles and merge messages

Recommendation: validate one Conventional-Commit-style **PR title** with a small
repository script or inline standard-library check; do not add multiple commit
linting tools or require every intermediate feature-branch commit to match.
Treat the title as untrusted input: read event JSON or pass it through an
environment variable, never interpolate it into executable script text.
[Script injection prevention](https://docs.github.com/en/actions/reference/security/secure-use#use-an-intermediate-environment-variable).

Title validation must also run on `edited`; the default PR trigger covers only
`opened`, `synchronize`, and `reopened`. Branch filters match the **base** branch.
[PR events](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#pull_request).

For short-lived feature PRs, squash merging can produce one meaningful commit.
Configure its default message to use the PR title (optionally its description),
then check the actual message before merging: GitHub allows the merger to edit
the generated message, so a title check alone does not enforce commit format.
[Squash configuration](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/configuring-commit-squashing-for-pull-requests),
[Merge behavior](https://docs.github.com/en/pull-requests/reference/pull-request-merges).

# Define the Daily-Use Release acceptance scenarios

Status: closed
Type: grilling
Parent: [Personal Assistant Daily-Use Release](../MAP.md)

## Question

Which concrete novice and heavy-user workflows must succeed end to end before
AngelBot can be called a Daily-Use Release, and what observable result makes each
workflow pass or fail?

## Resolution

Daily use is an outcome claim, not a count of implemented modules. A release
candidate must pass every core scenario below on a clean, non-administrator
Windows 11 x64 account. Deterministic tests cover contracts and recovery; an
installed-build trial covers operating-system integration that mocks cannot
prove.

### Core novice journeys

| Journey | Observable pass condition |
| --- | --- |
| First conversation | The app opens without demanding an API key. A person can ask a question, receives actionable setup guidance only when a model is needed, configures one compatible provider in Settings, tests it, and returns to the unchanged conversation. |
| Start project work | `新建项目` opens the Windows folder picker. The selected folder becomes one Project Workspace with one Main-Agent conversation; cancelling creates nothing. |
| Complete a file task | From a Project Workspace, the Main Agent reads only the selected root, performs an approved edit, verifies the result, reports the outcome, and can reveal the cited file in Explorer. Traversal and reparse-point escape fail closed. |
| Perform a simple computer action | The Main Agent can open an allowlisted Windows setting or registered trusted app and accurately reports `verified`, `dispatched`, or `unavailable`; it never claims completion from dispatch alone. |
| Understand permission | The composer clearly shows `请求批准`, Workspace automatic access, or `完全访问权限`. A pending side effect has a redacted semantic summary and `允许一次`/`拒绝`; either choice survives reload and removes the pending controls. |
| Create routine automation | A person can describe a reminder or bounded automation, review its schedule and permission, enable or disable it, run it once, and see the latest durable outcome without understanding cron syntax. |
| Recover from common failure | Missing credentials, unavailable executable, denied file access, model timeout, and failed verification produce a specific next action. Retrying does not duplicate the prior side effect. |

### Core heavy-user journeys

| Journey | Observable pass condition |
| --- | --- |
| Move across work | Personal and Project Workspaces appear in one index. Switching restores the correct single conversation, context, tools, permission, and active work without leaking another Workspace's data. |
| Sustain a long conversation | Context compaction keeps recent turns plus structured goal, decisions, constraints, actions, evidence, open questions, and active task facts. The user can continue without replaying the whole history. |
| Delegate bounded work | The Main Agent may run at most two independent delegated attempts. The user sees what each worker is doing and its state, never a second chat. Explorer output is read-only; implementer changes stay in a lifecycle-bound temporary worktree until review and Main-Agent acceptance. |
| Review and materialize changes | Every implementer result receives an independent reviewer result. The Main Agent presents the outcome and risk; only an authorized exact candidate is materialized. Conflict, stale base, rejection, restart, and cleanup are recoverable and idempotent. |
| Backtrack safely | Editing or branching from an earlier user turn preserves the old history and creates a navigable continuation. Project files and durable task state agree with the selected branch; no hidden worker transcript becomes user history. |
| Extend one Workspace | A local MCP server can be installed and enabled for one Workspace, exposes only discovered tools there, follows the active execution permission, and can be revoked without affecting other Workspaces. Secrets never enter model context or logs. |
| Run concurrent bounded tasks | Independent tasks make progress concurrently within the configured limit; conflicting writes and external side effects serialize. Restart recovery neither loses accepted work nor replays an unknown side effect. |

### Connected-service and release journeys

| Journey | Observable pass condition |
| --- | --- |
| Email and calendar | The user connects a supported provider through system-browser PKCE, grants staged scopes, can read and draft, and can send or modify events only under the External Action policy. Revocation removes usable tokens and leaves a clear reconnect state. |
| Background attention | Enabled automations catch up after AngelBot restarts, respect quiet/notification settings, and create durable Attention State before attempting a notification. Duplicates are suppressed. |
| Install, update, and restore | A signed per-user NSIS build installs without elevation, preserves local data across update, verifies signed updater artifacts, checkpoints before exit, and offers safe recovery/export after a failed start. |

### Evidence rule

No scenario passes solely because a unit test exists or a UI element rendered.
Each scenario names one canonical automated path where deterministic simulation
is possible, plus an installed-build checklist for OS-dependent behavior.
Failures remain failures until their assertions are fixed; live credentials,
network access, and user runtime data are forbidden in the canonical full test
profile.

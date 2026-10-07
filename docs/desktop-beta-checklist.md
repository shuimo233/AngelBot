# Desktop beta checklist

This checklist is a release gate for the desktop application. It complements
deterministic tests; it does not replace `python scripts/verify.py full`.

Run it with a fresh local profile and one disposable project directory. Record
the app version, OS version, and any failure before changing settings or files.

## First use and daily conversation

- [ ] Launch without configured credentials: the personal workspace opens normally, with a clear model-setup notice rather than a blocking wizard.
- [ ] Configure a model in Settings; a first personal-space message receives a reply, and the API key is not shown in chat.
- [ ] Relaunch with credentials configured: the conversation remains accessible without setup interruption.
- [ ] Open the personal workspace: no project path or file access is implied.

## Project work and file safety

- [ ] Create or open one project workspace using the disposable directory; the sidebar shows one main conversation for it.
- [ ] Verify File Access settings show that project's root and explain its scope.
- [ ] Ask for a read-only inspection: the result identifies the active project without exposing another project.
- [ ] Request a file modification: the pending confirmation clearly identifies the change; deny it and verify the target is unchanged.
- [ ] Approve a disposable change: verify only the project directory changes and the workbench can locate the resulting file.

## Trusted Windows applications

- [ ] Add an application from Settings > Computer control with the Windows executable picker; its status becomes available without exposing its path in chat.
- [ ] Move or uninstall a configured application, reopen settings, and verify AngelBot marks it unavailable instead of attempting to launch the stale path.
- [ ] Ask the Main Agent to open an available application: the confirmation names the intended action and the application opens only after the current permission policy allows it.
- [ ] Add a disposable application to the trusted-app list and verify the Main Agent can inspect only that app's current unique main window. The snapshot contains bounded control roles/names/available patterns, not field values or passwords; a dense window is explicitly marked partial. A program outside the list must remain inaccessible.
- [ ] On an upgraded installation, keep an old launch/draft-only app unchanged: window inspection must remain unavailable. Confirm its displayed name and exact path in Settings, approve the one-time observation-scope upgrade, then verify inspection becomes available. Replacing the app path between display and confirmation must reject the whole batch.
- [ ] Open multiple main windows or disable the app after registration: observation must refuse the ambiguous or revoked target instead of choosing an unrelated window.
- [ ] Stop an in-progress window inspection or disable/remove that trusted app while it runs: the helper exits promptly and no late control snapshot reaches the Main Agent.
- [ ] With a disposable, non-sensitive app open, inspect draft targets in Settings: only eligible field names appear, never field values. Selecting one only edits the pending configuration; save it explicitly before it becomes usable.
- [ ] Open a second instance or make the target field ambiguous, then inspect again: no target is silently chosen and no draft is written.
- [ ] For an application explicitly configured for drafts, review the exact disposable text in the confirmation, approve, and check the target field. AngelBot must not click Send; the target application may auto-save or sync its own changes.
- [ ] In “full access”, a draft that may auto-save or sync still requires its own exact-content confirmation. Switch execution permission from full access to ask and immediately inspect both Settings and the composer: neither may show the new mode before the backend confirms it, and rapid selections must not save out of order.
- [ ] Stop a draft while it is running: if the field may have changed, the result says it is unknown, asks for inspection, and neither the UI nor Main Agent automatically retries.

## MCP tool services

- [ ] Add a disposable local stdio service in Settings, including a quoted argument path with spaces; connect it and verify its tool list appears only after the explicit check.
- [ ] Approve an MCP tool call whose schema rejects unknown properties: the service must receive exactly the arguments the user approved, without AngelBot's internal confirmation markers.
- [ ] Enable that service for one project and verify neither personal space nor another project acquires its tool; revoke the project scope and verify a pending call cannot run.
- [ ] Edit the service command or arguments, verify the running child stops and workspace grants reset, then reconnect and stop it; no service process should remain after app exit.
- [ ] Connect a disposable MCP fixture that keeps initialization pending; verify Settings shows “正在连接” and “取消连接”, cancel promptly, and confirm a late handshake never returns the service to “运行中”.
- [ ] On a throwaway profile, connect a trusted third-party MCP package with an empty local npx cache. Confirm the first install can finish within the bounded wait or be cancelled promptly; verify failures give a useful category without exposing stderr or credentials.
- [ ] With a disposable local MCP fixture, confirm the child receives only launch-required environment variables and explicitly configured benign values, never AngelBot's model API key. Do not use a real credential for this check; MCP environment scoping is not an OS sandbox.
- [ ] Set a disposable MCP environment variable in Settings: its value must never be shown again, changing or removing it must stop the service and revoke project grants, and a missing system-credential entry must prevent startup while offering a reset path.
- [ ] Export ordinary and password-encrypted backups with that disposable variable. Only the encrypted backup may restore its value on a fresh local profile; ordinary restore must leave a credential-dependent service disabled. Never use a real credential for this check.

## Delegation, automation, and recovery

- [ ] Ask the Main Agent for a delegable analysis task: the user sees a concise Main-Agent status, never a child-agent conversation or raw tool transcript.
- [ ] Interrupt or redirect a pending task: the next user message is accepted and the prior task reaches a recoverable terminal state.
- [ ] For one confirmed project network scope, open Settings > Project network permissions, verify only its approved host and actions are shown, revoke it, reopen the page, and verify a later delegation asks for a new scope instead of reusing the revoked one.
- [ ] Create a script automation and an Agent automation: the former runs independently; the latter completes through exactly one Main-Agent reply in its selected workspace, without a synthetic user message or a child-agent chat.
- [ ] Restart the app after a queued task or automation: an item that never started is safely queued again; an item that already entered Main-Agent execution is visible as completed, awaiting confirmation, or resumable—never silently replayed or left indefinitely “running”.

## Release evidence

- [ ] `python scripts/verify.py full` passes without credentials, network access, or user runtime data.
- [ ] Attach screenshots or issue links for every failed checklist item; do not mark the release ready until each item is resolved or explicitly waived.

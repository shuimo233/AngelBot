# Desktop beta checklist

This checklist is a release gate for the desktop application. It complements
deterministic tests; it does not replace `python scripts/verify.py full`.

Run it with a fresh local profile and one disposable project directory. Record
the app version, OS version, and any failure before changing settings or files.

Release acceptance uses a clean, non-administrator Windows 11 x64 account and
an installed build. Signed per-user installation, signed update verification,
data preservation and recovery/export after failed startup require OS-level
evidence; a debug executable or mocked relaunch does not satisfy these goals.

Unchecked items require current-release live/manual acceptance. Scoped native
fixture and deterministic evidence is recorded in [the current roadmap](current-roadmap.md);
it does not prove real model understanding or third-party business completion.

## First use and daily conversation

- [ ] Launch without configured credentials: the personal workspace opens normally, with a clear model-setup notice rather than a blocking wizard.
- [ ] Configure a model in Settings; a first personal-space message receives a reply, and the API key is not shown in chat.
- [ ] Test the current unsaved model form: a completed text response and a synthetic native tool/receipt round trip must both pass before full success. Text-only, malformed calls/results, tool-request failure and timeout must retain the successful chat check but visibly leave tools unverified, without a green full-success state. Changing the form invalidates the result; testing must not save settings, send history or invoke real tools. Run this with the actual selected model/protocol; deterministic tests alone do not establish live compatibility or complex-task success.
- [ ] Relaunch with credentials configured: the conversation remains accessible without setup interruption.
- [ ] Open the personal workspace: no project path or file access is implied.
- [ ] While a reply is running, history edit/delete and an already-open editor cannot resend; the editor retains its local draft and can still be cancelled. After the run, edit/delete become available again. Copy remains usable for assistant replies, reports success only after the clipboard write, and shows a safe manual-copy hint on failure; changing the displayed reply must clear old feedback. Verify the actual Windows clipboard separately from DOM mocks.
- [ ] Delete a disposable user turn only after the confirmation names its subsequent branch and explains that external actions are not undone. The refreshed transcript must match the backend and remain correct after relaunch. Failed edit/resend must read saved history instead of restoring the old branch: a committed replacement remains visible, while an unadopted edit returns to its editor without overwriting a separate composer draft. On an unknown edit/delete/refresh result, history editing and sending stay unavailable until read-only reload succeeds; reloading must not issue another edit or delete. Cancelling a recovered editor discards that editor draft, and switching workspaces must isolate late results. Also verify that an occupied backend rejects edit/delete/update without changing the branch, independent of UI disabling. A successful history read does not prove whether external work finished; the notice must ask the user to check task state rather than claim rollback.
- [ ] Select a disposable DOCX in the personal workspace: body paragraphs and table text are visible in the sent snapshot, the model-sharing/omitted-content notice is clear, and reopening/editing the message preserves that text without granting original-file or folder access. Corrupt, oversized, field/revision-heavy and unsupported files must visibly fail, not appear accepted or be truncated. Test real documents in addition to the isolated native fixture.
- [ ] Select disposable XLSX and BOM UTF-16 LE/BE log files: sheet names, sparse coordinates, raw values, merge ranges and decoded text survive sending/reopening/editing without folder access. The XLSX raw-value/formatting notice must remain visible; formulas, hidden content, corruption and budget violations must fail without partial attachments. Check representative real workbooks separately from the deterministic native fixture; dates, percentages and currency are not display-formatted.
- [ ] Drag disposable files from Windows Explorer into the composer; they must use the same bounded loader as selection/paste, never navigate away or send automatically. Expand a filename and keyboard-focus its native disclosure; Enter/Space must expand/collapse and Tab must reach its scrollable text and remove control. Confirm preview text matches sent history, stays readable in dark/short windows, and disappears when switching workspace or removing its file. The embedded WebDriver uses synthetic key events and does not prove native disclosure keyboard defaults or Explorer OLE delivery; these require manual acceptance.
- [ ] In both personal and project space, use “保存回复…” on a durable assistant reply and choose a disposable Markdown/text file in the native Windows picker. Cancel first and verify no file/success claim; save next and compare exact UTF-8 bytes, including Unicode/newlines. Existing files and concurrent destination creation must never be overwritten. The app must not export hidden/tool/private-history rows, offer saving the active streamed reply, persist late feedback into another message, or grant the Agent directory access. Unknown outcomes require inspecting the destination before retrying. Verify the real picker separately from deterministic IPC/filesystem tests; plain text export is not formatted Word/PDF output.

## Project work and file safety

- [ ] Create or open one project workspace using the native folder picker and disposable directory; the sidebar shows one main conversation for it. Cancelling creates no workspace.
- [ ] Verify File Access settings show that project's root and explain its scope.
- [ ] Ask for a read-only inspection: the result identifies the active project without exposing another project. Traversal and reparse-point escape must fail closed.
- [ ] Request a file modification: the pending confirmation clearly identifies the change; deny it and verify the target is unchanged.
- [ ] Approve a disposable change: verify only the project directory changes and the workbench can locate the resulting file.

## Trusted Windows applications

- [ ] Add an application from Settings > Computer control with the Windows executable picker; its status becomes available without exposing its path in chat.
- [ ] Move or uninstall a configured application, reopen settings, and verify AngelBot marks it unavailable instead of attempting to launch the stale path.
- [ ] Ask the Main Agent to open an available application: the confirmation names the intended action and the application opens only after the current permission policy allows it.
- [ ] Add a disposable application to the trusted-app list and verify the Main Agent can inspect only that app's current unique main window. The snapshot contains bounded control roles/names/available patterns and optional toggle/selection/expansion/vertical-scroll state, not field values or passwords; missing state is unknown, not false. Independently re-observe after an approved change and compare the actual state, not only its action receipt. A dense window or failed state read is explicitly marked partial without actionable refs. A program outside the list must remain inaccessible.
- [ ] On an upgraded installation, keep an old launch/draft-only app unchanged: window inspection must remain unavailable. Confirm its displayed name and exact path in Settings, approve the one-time observation-scope upgrade, then verify inspection becomes available. Replacing the app path between display and confirmation must reject the whole batch.
- [ ] Open multiple main windows or disable the app after registration: observation must refuse the ambiguous or revoked target instead of choosing an unrelated window.
- [ ] Stop an in-progress window inspection or disable/remove that trusted app while it runs: the helper exits promptly and no late control snapshot reaches the Main Agent.
- [ ] With a disposable, non-sensitive app open, inspect draft targets in Settings: only eligible field names appear, never field values. Selecting one only edits the pending configuration; save it explicitly before it becomes usable.
- [ ] Open a second instance or make the target field ambiguous, then inspect again: no target is silently chosen and no draft is written.
- [ ] For an application explicitly configured for drafts, review the exact disposable text in the confirmation, approve, and check the target field. AngelBot must not click Send; the target application may auto-save or sync its own changes.
- [ ] With a disposable ordinary field, replace its contents, then ask to clear it. Both live and recovered confirmation cards must explicitly name clearing, warn that original content is not retained and require one approval. Refuse stale targets, modified pending content, missing/null text, and empty message drafts. Verify the actual field is empty and neighboring fields unchanged; an unknown result must never be replayed automatically.
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

- [ ] Ask the Main Agent for bounded work with at most two independent delegated attempts: each has an understandable state, never a child-agent conversation or raw tool transcript. Explorer remains read-only; implementer output remains isolated until independent review and authorized exact-candidate materialization. Conflicting writes serialize, and rejecting or stopping one task must not stop the other or accept a late result.
- [ ] Interrupt or redirect a pending task: the next user message is accepted and the prior task reaches a recoverable terminal state.
- [ ] In a disposable multi-operation task, let one operation succeed and a later one fail: Main must retain the unresolved operation, distinguish partial results from completion, and not erase it when the same tool succeeds for another target. A non-coding failure must not ask for a build or verification command; unknown external effects must never be automatically replayed.
- [ ] For one confirmed project network scope, open Settings > Project network permissions, verify only its approved host and actions are shown, revoke it, reopen the page, and verify a later delegation asks for a new scope instead of reusing the revoked one.
- [ ] Create a script automation and an Agent automation: the former runs independently; the latter completes through exactly one Main-Agent reply in its selected workspace, without a synthetic user message or a child-agent chat.
- [ ] Ask the Main Agent about existing automations and pause one by its exact ID: only the current workspace is accessible, querying never runs a task, and pause clearly affects future scheduling rather than the current execution.
- [ ] Check Main-chat recall after a failed or unknown local run, including one from yesterday: the unresolved result remains visible, newer skipped occurrences do not replace its evidence, and the automation page is one visible action away.
- [ ] With explicitly authorized MAA 6.18 and its existing queue, verify one scheduled business run at the agreed time before enabling the daily plan; distinguish queue completion from game objectives and verify owned-process/configuration cleanup. Do not rerun real game tasks early just for testing.
- [ ] Restart the app after a queued task or automation: an item that never started is safely queued again; an item that already entered Main-Agent execution is visible as completed, awaiting confirmation, or resumable—never silently replayed or left indefinitely “running”. Catch-up must respect notification/quiet settings, persist attention before notification delivery and suppress duplicates; verify real delivery separately from stored attention.
- [ ] Use the runtime-health restart and signed-update entries on a disposable profile: each must explain interruption/unsaved-data risks and focus cancellation before any native restart/install call. Cancelling must leave tasks and drafts untouched. Ephemeral storage must explicitly warn to copy new content first; a failed native relaunch must show a safe retry message, not raw diagnostics. An update whose install returned successfully but whose relaunch failed must offer restart only, without repeating installation. Verify actual Windows installer exit/new-process startup separately; deterministic mocks do not establish either, and the warning is not a global idle/shutdown barrier.

## Release evidence

- [ ] `python scripts/verify.py full` passes without credentials, network access, or user runtime data.
- [ ] Attach screenshots or issue links for every failed checklist item; do not mark the release ready until each item is resolved or explicitly waived.

Before declaring an email/calendar connector supported, additionally verify its
system-browser PKCE flow, staged read/draft/send or calendar-mutation scopes,
the active external-action policy, and revocation/reconnect behavior. This is a
capability-specific gate, not a reason to redefine AngelBot as a mail assistant
or infer that an unimplemented connector is available.

## Continuous-use trial: acceptance procedure, not a release waiver

Use the normal Windows build with the owner's selected model and a disposable
project. Do not substitute the scripted E2E profile for a connected account.
Keep real mail, purchases, game routines and other existing scheduled actions
outside the trial unless the owner authorizes that specific run. The configured
model/process-restart entry in [Desktop E2E](../e2e/README.md) supplies an isolated
preflight, not these manual results.

1. In Personal, hold an ordinary conversation and express a lightweight style
   preference. Replies should remain useful without turning every sentence into
   a persistent-memory confirmation or a new task card. Do not infer success
   merely because a request returned HTTP/IPC success.
2. In the disposable project, ask for one concrete local-file result, inspect
   the proposed change, approve it, and compare actual file contents with Main's
   report. Reject a second change; it must not appear on disk.
3. Leave a harmless proposed write awaiting confirmation, exit and reopen the
   app, then stop that task. Verify the old confirmation cannot execute and a
   new request still completes in the same project's sole conversation. Personal
   must not obtain that project's file contents just by switching workspaces.
4. Inspect Settings → Personality → learned habits after enough ordinary use.
   A short conversation's repeated wording is one observation, not multi-day
   evidence. Tentative records must not be treated as established facts. Correct
   or stop one habit, reopen the app, and verify it remains stopped. Pausing
   learning/long-term memory must not erase the owner's manual inspection controls.
5. Return on later actual days, without editing timestamps or injecting history.
   Check conversational continuity, useful preferences and the absence of stale
   or intrusive adaptation. Open an existing project and continue a bounded task.
   Authorization expiry or connection failure must preserve prior results and
   offer a clear next step without replaying uncertain actions.

Record date/build, Windows version, model identity, permission mode, workspace
kind, expected/actual outcome and a minimal evidence reference for each step.
Do not put credentials, authorization URLs, full conversations or personal file
contents in public evidence. Label results **offline**, **scripted native**,
**live model** or **natural multi-day**; never merge these into a single pass.
Lost history, unapproved effects, replayed stopped actions or unexplained blank
replies stop the trial until diagnosed. A correct single-day preflight makes the
trial reproducible; it does not prove the multi-day step or the release gates.

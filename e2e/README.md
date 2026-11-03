# Desktop E2E

Run `npm run test:desktop` to build the real Tauri desktop binary and validate non-blocking first use, Main-Agent reminder creation and recall, workspace continuity, automation handoff, workbench, and trusted-application status flow.

The runner uses a temporary application-data directory and enables the `desktop-e2e` Cargo feature. That feature registers the embedded WebDriver only in the test binary; the normal application build has neither the feature nor the test server. While the test is running, credential reads return empty values, so it never uses a developer's system keychain data.

Test and `desktop-e2e` startup also skip the repository `.env`, including the
debug HTTP thread. A sanitized launcher must not reload developer credentials
or deployment opt-ins inside the application. Main-conversation titles belong
to the workspace; no background foreign-provider title request is made.

The initial connect-model notice is intentional first-use coverage, not a readback
of the developer's saved Luna account. Neither this fixture nor the browser-only
development preview proves a live ChatGPT connection in the normal desktop app.

The personal-space journey sends in-memory DOCX, XLSX and BOM UTF-16 log fixtures
made with Python's standard library in the same existing reminder turn. Real Tauri
extraction must visibly reject corrupt DOCX/XLSX and formula-containing XLSX without
a chip or user turn. Accepted snapshots must preserve body/table text, worksheet
coordinates/raw values/merge ranges and decoded log text in both the model request
and durable history, omitting headers/comments and never granting directory access.
The XLSX raw-value/formatting limitation is visible before and after sending.
Draft/sent screenshots (including `xlsx-sent.png`) are saved under
`src-tauri/target/e2e`. Later text-attachment editing/resend checks reuse the same
snapshot contract. No user's document, live model or Office installation is required.

The same turn exercises the composer HTML drop handler, read-only pre-send preview,
collapse/reopen, native summary focus, and exact equality with durable text. Light,
dark and short-window screenshots are saved as `attachment-preview*.png`; short
windows must keep the input and Send button visible. This is WebView DOM-drop
coverage plus a regression guard for `dragDropEnabled: false`, not proof of Windows
Explorer OLE delivery. The embedded driver's key actions dispatch synthetic events
without browser disclosure defaults, so Enter/Space acceptance remains manual;
the product uses native `<details>/<summary>`, without a test-specific key handler.
An out-of-composer file drop must be prevented without reading an attachment,
creating a message, or navigating the WebView; the composer drop still propagates
to its existing loader exactly once.

The automation journey starts one temporary Node script in the isolated project,
waits for its independent PID marker, then asks the Main Agent to stop this run.
The scripted provider obtains dynamic automation/run IDs only through the exact
`list_automations` call/result pair. Rejecting consent must leave the real process
running; approving the next request must yield `requested`, not a fabricated
terminal result. The host checks the process has exited before the native
`cancelled` receipt, recalls **本次已停止** without a misleading forced exit code
in the same conversation, and verifies the future schedule is unchanged.
Consent scope must fit inside the reading viewport after WebView-native scrolling;
page navigation uses the visible menu and normal WebDriver clicks. The consent and
result screenshots are written to a dedicated temporary evidence directory printed
by the runner. Failure cleanup requests a stop only for this fixture's exact owned
run. This covers owned-process cancellation, not rollback of external work, real
model understanding, MAA game completion, or stopping pre-existing applications.

Test builds always use `src-tauri/target/desktop-e2e`, overriding an inherited
`CARGO_TARGET_DIR`. The executable is `debug/angelbot.exe` within that directory;
the Python runner passes its absolute path to WDIO, which has no normal-build
fallback. The E2E Tauri configuration also builds and embeds the frontend from
`src-tauri/target/desktop-e2e/frontend`, leaving normal `dist` and desktop binaries
untouched. These generated test artifacts stay ignored by Git.

The Python launcher selects an available `127.0.0.1` port for the embedded
driver and its readiness check. It does not rely on fixed port 4445, which
Windows may reserve, or change firewall/port-exclusion settings. The socket
probe is released before the driver starts, so allocation is best-effort;
startup/handshake failures still fail the journey without changing OS settings.

`python scripts/desktop_e2e.py --skip-build` explicitly reuses that isolated
cached executable and fails if it is missing. It does not check source freshness:
after frontend, Rust, dependency or build-configuration changes, run without
`--skip-build` to rebuild before testing. A failed build stops before any desktop
fixture or WDIO launch; the runner never substitutes the daily-use binary.

## Native reply-save acceptance (opt-in)

Run `python scripts/desktop_e2e.py --reply-export` to select only the native
reply-save journey. It reuses the isolated binary/profile and scripted provider,
but does not start the UIA fixture, Explorer work or existing user automations.
The default desktop journey and offline `full` profile do not enter this mode.
`--skip-build` has the same freshness caveat above; `--keep-data` retains only
this test's temporary profile and outputs for diagnosis.

This is interactive acceptance, not a headless CI test. A person or supported
Windows Computer Use controls the actual system picker. The printed `stage.json`
asks first to cancel, then to save at each exact temporary Markdown/text path.
The WebDriver verifies cancellation, exact durable-reply bytes, clean output
directory, UI receipt and unchanged conversation after reload. It never passes
a path to application IPC or replaces the picker with a mock. Each picker has
a bounded wait; unreliable target binding must stop UI input, not trigger a
coordinate/PowerShell fallback or acceptance of an overwrite warning.

`src-tauri/target/e2e/reply-export-desktop.json` is invalidated before building or starting the driver and
sealed after each test. Only `status: passed` together with all four checks is
complete evidence; `failed`/`running`, a partial cancellation check or a stale
checkpoint is not a passed native-save journey. Reports exclude raw errors and
private paths. The offline runner regressions cover mode isolation and failed
evidence sealing, not the real picker. Current live limitations are recorded in
[the roadmap](../docs/current-roadmap.md).

## Configured-model continuous-use acceptance (opt-in)

This is separate from offline `full` and the scripted desktop journey. It
borrows the already saved ChatGPT model/authorization; it may consume shared
plan allowance and refresh that authorization. It does not sign in, change
providers, use a paid API fallback, start the daily-use app or its automations,
or copy personal history into the test. Requires an existing valid connection.

```powershell
$env:ANGELBOT_LIVE_MODEL_TEST = '1'
$env:ANGELBOT_LIVE_MODEL_PERMISSION = 'ask'
$env:ANGELBOT_LIVE_MODEL_JOURNEY = 'continuous'
cargo test --manifest-path src-tauri/Cargo.toml --lib commands::message::live_tests::configured_model_completes_bounded_file_task -- --exact --ignored --nocapture --test-threads=1
Remove-Item Env:ANGELBOT_LIVE_MODEL_TEST, Env:ANGELBOT_LIVE_MODEL_PERMISSION, Env:ANGELBOT_LIVE_MODEL_JOURNEY
```

One parent owns temporary files/database and starts independent test processes.
The `ask` journey checks local CSV → exact JSON report → readback, approval,
duplicate-approval refusal, rejection, restart with a pending write, stop and
blocked replay, then a new real task using the prior conversation's unique
marker. It also submits ten real Main turns, waits for the production automatic
review hook, checks observations remain tentative in this one short conversation,
dismisses one through the owner command, pauses learning and reopens the DB in
another process. No candidates or fake historical timestamps are inserted.
Each child seals its own receipt: a successful process with zero matched tests
cannot count as acceptance. `workspace_auto` remains a smaller file/restart
journey and does not count as approval/cancellation/learning acceptance.
Successful temporary profiles are removed. A failed child prints and retains
only its synthetic fixture for diagnosis; do not publish the database or treat
its partial receipts as a pass. Remove that exact owned directory after diagnosis.

These are synthetic, non-private acceptance requests with a real model, not
evidence of natural multi-day companionship, desktop restart, installer recovery,
notification delivery or arbitrary application control. See the continuous-use
trial below the [desktop beta checklist](../docs/desktop-beta-checklist.md).

### Native Responses checkpoint and fresh Main

The same ignored entry accepts `ANGELBOT_LIVE_MODEL_JOURNEY=native-context`
with `ANGELBOT_LIVE_MODEL_PERMISSION=full_access`. It requires the already saved
ChatGPT/Luna authorization; it does not change the normal model configuration.
Run the exact command above with those two values and clear the flags afterwards.
Offline `full` never runs this journey.

One temporary profile/project spans three independent processes. The first calls
the production Responses provider with synthetic filler and a unique marker,
lowering the soft threshold only in that provider instance. An actual encrypted
compaction item is required; accepting the parameter alone is not evidence.
The response and original transcript are committed through `ForegroundRunStore`,
not copied into a second context format. A fresh process uses the production
context assembler and codec to check that its initial Main window includes the
checkpoint but not the old plaintext marker. The normal Main must then recall
the marker, read `amounts.csv`, write `{marker,total:42}` to `receipt.json` and
read it back. A third process performs read-only verification in the same Main.
Host checks require exact files, unchanged CSV and exactly one successful write.
Private state is confined to the parent-owned temporary DB, removed on exit;
printed evidence contains shape/counts only, never encrypted continuation data.

This is live provider + durable storage + headless Main acceptance, not native
UI, installer, natural multi-day use or repeated natural compaction quality.
Full history is retained for branches/recovery; only the model replay window is
pruned. See the current [roadmap](../docs/current-roadmap.md) for evidence.

### Real delegated producer and Reviewer

The same ignored entry point has a separate, bounded journey. It uses the saved
model for both the Implementer and an independent Reviewer through the production
factory, Supervisor, pump, file host and review coordinator. All task data,
Git configuration, database, sandbox/worktree and materialization remain in one
owned temporary fixture; normal startup, user automations, MCP and delegated
network exploration do not run. This may consume the same plan allowance as the
continuous-use journey. No new authorization or provider fallback is performed.

```powershell
$env:ANGELBOT_LIVE_MODEL_TEST = '1'
$env:ANGELBOT_LIVE_MODEL_JOURNEY = 'delegation'
try {
    cargo test --offline --manifest-path src-tauri/Cargo.toml --lib commands::message::live_tests::configured_model_completes_bounded_file_task -- --exact --ignored --nocapture --test-threads=1
} finally {
    Remove-Item Env:ANGELBOT_LIVE_MODEL_TEST, Env:ANGELBOT_LIVE_MODEL_JOURNEY
}
```

The producer must discover a randomly named input, read it, write one isolated
candidate, then reread both files after the final write and return structured
evidence. Host-generated read receipts carry paths, byte counts and SHA-256,
not file text or tool logs; producer-authored fields cannot forge these receipts.
A distinct real Reviewer
must pass before the normal Main control handler may materialize it. The fixture
explicitly approves only its own candidate, checks exact output and unchanged
input, repeats the receipt without rewriting, and verifies terminal resource
cleanup. This tests real worker/review quality for this small task, not a
model-driven Main delegation decision, automatic Main summary, native approval,
parallel tasks or crash recovery. Failed phases remain failures and retain only
synthetic diagnostics; the parent requires a completed child receipt.

### Main-issued delegation and confirmed continuation

Use the same ignored command above with
`ANGELBOT_LIVE_MODEL_JOURNEY='main-delegation'` to cover the foreground callers.
The saved real model must issue exactly one `delegate_work`; the normal isolated
delegation pump performs the producer/independent review and wakes the existing
Main follow-up pump. Change summaries are text-only and must leave the canonical
file absent. A second real user turn asks to apply it; `ask` permission must
persist a confirmation before any canonical write. The normal confirmation
command resumes the linked Main reply, which must read back the actual file.
Exact output, unchanged input, no synthetic user turns, duplicate-approval refusal
and sandbox/worktree cleanup are checked. This is headless command/runner
acceptance, not native UI clicking, parallel-task or crash-recovery evidence.
Both delegation modes reuse one fixture and production graph; they never start
the user's daily application, automation scheduler or MCP services. The desktop
E2E feature continues to prohibit real credential access.

## Default desktop journey checks

Child environments exclude credential-like names and force offline Cargo/npm
resolution. Model and Explorer responses are scripted, not live service evidence.
The reminder receipt check suppresses OS notifications and manually triggers the
saved definition; focus refresh verifies its durable receipt in an existing chat.
Actual due-time claiming/cancellation is covered by the offline backend tests.
The same journey reads the completed Agent automation through the native
workspace-result projection and recalls it in the existing chat. It checks
that prompts, paths, arguments and raw process output are absent, and that a
new project cannot see personal results (`daily-automation-result.png`). The daily
automation card retains its next schedule after completion; its result link opens
the same Main conversation without adding a turn or another run
(`daily-automation-result-link.png`).
The approved-file check waits for the final, non-streaming citation and resolves
its visible node on each poll, then uses a normal WebDriver click and verifies
the selected file and preview content. A failure records bounded visibility
diagnostics and `file-locator-failure.png`; it does not force-click a hidden link.
The final consent check shows the MAA unattended queue/emulator/timeout scope,
then rejects it before execution and verifies that no timer was created
(`maa-consent-scope.png`). A separate fixture revokes Notepad before approval,
checks the disabled state before permitting the attempt, and verifies the durable
failure and visible safe result (`desktop-failed-result.png`). No MAA, emulator
or Notepad is launched by these checks; they are not a game-business acceptance.
Rejection and a failed approval settle without resuming the model. These checks
assert durable receipts rather than inventing a follow-up model response. Read
fixture configuration again after a WebView reload; window globals do not survive.
The window-image regression starts one owned temporary WPF window after the
desktop build, with observation-only access. The real confirmation UI rejects
one request and approves another. Before either decision, the enabled image-only
button's own card must show the backend-attested app name and executable basename,
current-model transfer and privacy scope, without exposing the executable path.
Image preflight checks the stored app authorization only, not the live window;
window selection and capture occur after approval. The ordinary desktop parent's
capture helper must deliver both fixture color markers to the scripted provider
on the exact approved tool call. The following turn must contain no image. Public messages
and the isolated database must contain no PNG payload, and independent fixture
values and action counters must stay unchanged. The temporary window is cleaned
up even on failure or `--keep-data`. `desktop-window-image-consent.png` and
`desktop-window-image-result.png` record only AngelBot's consent/result UI,
not the captured source image. A consent timeout records boolean-only diagnostics
and `desktop-window-image-consent-failure.png`, not raw card text or pixels from
the observed app. This verifies transport and lifecycle, not real visual-model
understanding or third-party app compatibility.
Before the same owned window gains `interact`, the Node runner independently
checks the image-phase snapshot against its original PID/title and state baseline.
One subsequent user task then observes region A, approves check, obtains fresh
root and region references, approves uncheck, and observes again. Both real cards
must identify the same app/region/control. The scripted provider resolves actions
only from exact current observation/tool-call pairs; it cannot invent references
or infer state from action receipts. Read-only UIA observations must report
`state.toggle` as `off` → `on` → `off`, with no fallback for absent or contradictory
state. The host checks the full
durable parent/call chain, fresh references and approved native verified receipts
across all confirmation slices, including rejection cases derived from that ledger. Independent
fixture reads prove check/uncheck occurred once each and all other values/counters
remain unchanged. Consent/result UI is saved as `desktop-window-task-second-consent.png`
and `desktop-window-task-result.png`; failures retain boolean-only card checks and
bounded owned-fixture observation metadata, never reference values or control values.
This covers Main-Agent continuation and native control effects, not third-party
business completion. No extra user turn, window, scheduler or model service is added.
Structured checkpoint requests use their real compression protocol and a separate
deterministic test response, without advancing the interactive script cursor or
disabling production compaction. This checks protocol/continuation, not summary quality.
For this sequential script, durable parent links and assistant call position
determine order; tool results pair uniquely by call ID regardless of array order.
The final model response checks only its latest root/scoped pair: compaction may
remove earlier confirmation slices from the model input, not the durable ledger.
Every control slice must retain the original user task in both its durable goal
and structured facts; continuation instructions must not recursively replace it.
Validation errors retain the current scripted reply rather than consuming later steps.
The expanded multi-reminder check preserves at least 120 CSS pixels of reading
space and an unclipped composer at the tested small-window size. Screenshots are
generated under `src-tauri/target/e2e`, including `compact-reminder-recall.png`.

The same journey first rejects an unsupported image with a visible error and
rejects an unsupported request before persistence. Four long attachment names
must preserve at least 120 CSS pixels of reading space and an unclipped composer
at the tested small-window size (`daily-file-draft.png`). It then selects a bounded
CSV `File` through the real Composer reader, sends an attachment-only Personal
turn, follows up, switches spaces,
and edits/resends the original turn. The scripted provider checks the file name
and unique content marker in its input and rejects a Personal surface containing
`read_file` or `write_file`; persisted messages must retain the unchanged snapshot.
This proves transport/context continuity, not a live model's understanding of a
document. The fixture contains no user file or private data. Its sent snapshot is
`daily-file-snapshot.png`.

This suite deliberately stays separate from `python scripts/verify.py full`: the latter is the deterministic unit/integration gate, while desktop E2E is a narrow Windows smoke test. Add a desktop scenario only for a regression that cannot be covered at a lower layer.

The canonical runner strips model credentials and live-acceptance flags from each
check's environment, resolves dependencies offline, and owns a temporary model
configuration directory until that check exits, including failures. The Rust
directory override exists only under `cfg(test)`; ordinary application builds
ignore it. Install dependencies with `npm ci` and
`cargo fetch --locked --manifest-path src-tauri/Cargo.toml` before offline
verification (CI already has those separate bootstrap steps). Direct Cargo
invocations are not a replacement for this canonical isolation contract.

For the actual Windows UI Automation observation/preflight/confirmed text-write
path, run `python scripts/native_uia_smoke.py`. It uses an owned temporary WPF
window and the production desktop adapter, without model keys or user app data.
See [the native fixture guide](../scripts/fixtures/README.md) for prerequisites,
safety assertions, cleanup, and the limits of this backend-only smoke.

## Opt-in live webpage recovery

`python scripts/browser_form_acceptance.py --mcp-cli "D:\Tools\playwright-mcp\node_modules\@playwright\mcp\cli.js" --hold-for-inspection`

Replace the example CLI path with an already approved, cached official
`@playwright/mcp@0.0.83` installation. This command does not install packages or
log in. It uses the existing saved/authorized model path, so it consumes that
model's allowance; the ordinary offline `verify.py full` never runs it.

The same ignored Main journey used by the checklist accepts a synthetic
cross-page equipment draft, dropdown, text fields, textarea and checkbox. The
owned loopback application commits the first save and deliberately loses its
HTTP receipt. A second save would create a real duplicate in this fixture:
Main must inspect the draft box, verify all fields and deliver the result.
A fresh independent Main process then reopens it read-only via the durable MCP
catalog; no manual reconnect or repeated save is allowed. Only isolated headless
Edge is controlled, not the user's browser, accounts or real applications.

Add `--workspace-report` to extend that same journey across project files and
the webpage. It creates one temporary General Project through the production
workspace command; its single Main reads synthetic `request.json`, verifies
the persisted draft, writes `receipt.json` and reads it back. A fresh process
opens the same project Main and checks both deliveries read-only. Host assertions
check exact JSON fields, unchanged source bytes, one successful file write,
one server save and matching session identity across processes. This does not
add a second harness or an offline test case. The project and MCP processes are
temporary; retained markers attest the actual files before cleanup, not a saved
user project or native workbench inspection.

The command prints its task-owned `logs/browser-form-*` directory, live log,
acceptance JSON and URL. Failure remains failure; the server records exact save
attempts and mutations, not just successful tool dispatch. With inspection
enabled, success keeps the page alive until a `stop` file appears in that owned
directory or ten minutes elapse; otherwise it closes immediately. Inspect the
known URL independently before signing off visible business state. This is a
bounded backend/live-model journey, not native Main UI or arbitrary-site proof.

## Opt-in MCP disconnect continuity

Use the existing ignored saved-model command above with
`ANGELBOT_LIVE_MODEL_JOURNEY='mcp-disconnect'` and
`ANGELBOT_LIVE_MODEL_PERMISSION='full_access'`. Node must already be installed;
no package download, browser or foreground input is required. This consumes the
saved model's allowance, outside offline verification.

The existing synthetic MCP fixture commits exactly one local receipt, then exits
without returning its tool result while AngelBot stays alive. The normal Main
reply must preserve a non-resumable unknown result; continuing that reply must
not dispatch it again. A new user request performs one fresh read through the
cached catalog and lazy connection, without a Settings reconnect. A subsequent
independent Main task must complete without tools; the original unknown reply
remains intact, the external receipt count remains one, no turn is left running,
and the owned MCP is stopped. This is a backend/native-process continuity check,
not model-network failure, arbitrary services, a native UI journey or multi-day
reliability. The separate `mcp-interrupt` journey still kills the host process
after the same durable side effect and verifies recovery in a new host.

On Windows, let an owned library-test process finish before rebuilding the same
target: the running `.exe` cannot be replaced by the linker. Serialize focused
Cargo runs and `verify.py full`; a link-lock failure is not a test result.

### Model-boundary recovery

The same saved-model entry also accepts `ANGELBOT_LIVE_MODEL_JOURNEY='model-fault'`
with `full_access`. It reuses the owned MCP fixture, but returns an acknowledged
receipt before injecting an error through the existing `LlmProvider` seam. The
first real-model slice runs through the production context assembler, model
factory and coordinator; only the provider is decorated in this test module.
No production fault switch, proxy, endpoint change or credential copy is added.

Visible events, reply and tool summaries must exclude the synthetic private
error. The confirmed operation stays recorded once; the failed slice cannot be
blindly continued. A fresh request through the normal Main command must read the
actual count, deliver it, then complete another independent task without tools.
This consumes the saved model's allowance before/after the injected error. It
proves model-boundary failure handling, not an actual Internet outage, expired
authorization, native Main UI clicks or natural multi-day reliability.

For actual public API admission rejection, also set
`ANGELBOT_LIVE_MODEL_FAILURE='auth-reject'` and clear it afterwards. This test-only
variant decorates the same model seam with an intentionally invalid, owned static
credential source only after the first acknowledged effect. It does not revoke,
replace or copy the user's saved credentials, and its different binding excludes
opaque account continuation data. The normal public Responses route must return
HTTP 401 exactly once. The ordinary saved Luna provider is used before and after
that rejection; the same Main must perform the fresh read and independent task,
with no duplicate effect, unsafe continuation or visible canary. This proves an
actual 401 boundary plus Main recovery, not genuine expiry/revocation or OAuth
reauthorization. The original synthetic-error variant remains available; offline
`full` ignores both through the existing single ignored entry.

### Native Main conversation recovery

`python scripts/desktop_e2e.py --model-recovery`

This opt-in Windows journey uses the existing scripted provider, not credentials
or a live model. It launches the isolated desktop build without taking focus,
submits an ordinary composer message, completes one read-only `list_automations`
call, and deliberately fails the next model response's existing context guard.
The safe failure reply must retain its receipt and release the composer. A second
ordinary message supplies the missing context marker and must complete without
refreshing or replaying the tool. A second application process opens the same
isolated data and must retain both turns, then complete one further message using
the prior user context. No real automation, other app or user browser is involved.

The Python runner checks durable receipt counts and absence of active tasks or
automation runs. Reports and screenshots are in
`src-tauri/target/e2e/model-recovery/`; a failed build/run invalidates the aggregate
report rather than retaining an older pass. `--keep-data` retains the isolated
runtime for diagnosis. `--skip-build` requires a separately rebuilt current
desktop-E2E binary. Optional `--inspect` holds the final window for up to 120
seconds; independently inspect only that owned window, then create the reported
evidence directory's `inspection-done` file to release it. No foreground input
is needed. This covers the native frontend/IPC recovery path with a deterministic
model error, not an Internet outage, expired account or multi-day reliability.

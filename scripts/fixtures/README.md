# Native Windows UIA smoke

Run explicitly from an interactive Windows desktop:

```powershell
python scripts/native_uia_smoke.py
```

For a focused control-helper change:

```powershell
python scripts/native_uia_smoke.py --controls-only
```

This runs the existing real confirmation, scoped control operations and readback
journey against the same owned WPF fixture, without image/capture checks. It builds
only the library-test executable; it does not rebuild or replace the ordinary
`src-tauri/target/debug/angelbot.exe`. Both focused modes can reuse an explicitly
rebuilt current library test with `--test-binary <absolute-path>`; never reuse a
binary built before the changed sources. Capture-only and controls-only cannot
be combined. The default command continues to cover both journeys.

The control journey also opens a test-owned, ownerless non-activating tool HWND and
then a genuine second window. It must still find the main controls with the
auxiliary present, refuse the second independent window, and close both before
continuing. No title, foreground-window guess, or first-window choice is used.
The outer control-test allowance is 150 seconds for this additional read-only
discovery phase; production action timeouts and approval checks are unchanged.

The journey exercises the existing independent ListBox `select`, plus the bounded
ComboBox `choose`: one approval binds the exact dropdown and short option label;
execution expands, requires a unique visible matching item in that container,
selects at most once, verifies selection, then collapses and verifies again.
The neighbouring ListBox contains a deliberately identical option name. Separate
counters prove no cross-container selection and no repeated selection when the
requested option is already selected. It requires fresh actionable references,
not metadata-only aliases with the same Name. Visible ListItem state identifies
its selection-container type without collecting another Name or field value.
The verified choice receipt echoes the approved dropdown and option, not hidden
content; neither its receipt nor a control label proves business completion.

The original two-approval approach safely failed when returning to Main closed
the transient dropdown (`logs/native-combo-selection-20261021/`). New action
evidence lives under `logs/bounded-choice-20261021/`; the direct native journey
alone does not attest foreground switching, real model understanding or Main
delivery. Those acceptance results and limitations are tracked independently in
`docs/current-roadmap.md`. No hidden-option, coordinate or keyboard fallback,
unbounded macro or unknown-effect retry is introduced.

For a focused capture regression without the preceding control mutations:

```powershell
python scripts/native_uia_smoke.py --capture-only
```

This uses the same owned fixture in its dense layout and the same production
capture helper, process-tree host, pixel assertions and wrong-PID refusal. It
does not run or attest the confirmation/control-action chain. The default
command runs that chain first, then the independent capture test against the
same owned window; a failure stops the runner rather than retrying an operation.

The runner builds the AngelBot library test binary, and (unless controls-only)
the current normal AngelBot executable, with Cargo's `--offline` flag. It compiles
`NativeUiaFixture.cs` with the installed .NET Framework C#
compiler, and launches a uniquely located owned WPF executable. A small
non-activating fixture window is visible during the test. No model credentials,
network, existing trusted-app settings, or user runtime database are needed.
Cached Cargo dependencies and Windows .NET Framework/WPF are prerequisites.
Credential-like API key/token, secret, password, and credential environment variables are
removed from compiler, fixture, and test child environments; Cargo and npm
offline flags are explicitly enforced.
This is child-environment hygiene, not a Windows security sandbox or a block on
all possible filesystem/network access by arbitrary local binaries.

The ignored `commands::message::tests::native_uia_fixture_confirmation` test
uses the production Windows desktop adapter and existing pending-confirmation,
preflight, preview binding, and confirmation resolver. It checks:

- Observed ordinary fields receive a live reference without exposing values.
- Password fields are omitted; read-only and duplicate-ID fields receive no
  writable reference.
- Approval without a backend preview cannot write.
- A field made read-only after preflight rejects the confirmed write.
- Fresh observation and preflight lead to a `verified` write result and an
  independent fixture-side readback; protected fields remain unchanged.
- The observed fixture-owned button receives a short-lived control reference.
  Live preflight and confirmation lead to `dispatched` Invoke, independently
  observed exactly once by the fixture. Re-observation sees its new label;
  reusing the consumed reference or settled approval cannot invoke again.
- Named radio selection and parent menu expansion/collapse share that exact
  production confirmation path. Each operation reads back the requested
  control state; the independently recorded fixture state must match.
  Repeating Select, Expand or Collapse with a fresh observation is a verified
  no-op: selection/open/close event counters remain exactly one each. All
  prior field/control refs for the app expire after every control attempt.
- A single ordinary checkbox shares that path for desired-state Check/Uncheck.
  Check, Check again, Uncheck and Uncheck again produce exactly one checked
  and one unchecked event; the repeated desired states are verified no-ops.
  Observation, preflight and approval without a live preview cannot toggle it;
  consumed references and settled approvals cannot replay either operation.
  The fixture then changes only that checkbox to an initial indeterminate
  tri-state. Both actions refuse it during live preflight without advancing
  its state or any event counter. Its final indeterminate event comes solely
  from the owned fixture's command, not an AngelBot operation.
- A named vertically scrollable pane uses the same one-step confirmation path.
  Small down/up operations are independently checked against fixture offsets;
  repeating either direction at its edge is a verified no-op. Exactly two
  vertical-change events occur, with no horizontal movement. Observation,
  preflight and an approval without a live preview never change either offset.
- The same owned window then replaces its simple panel with four named regions,
  each containing 12 small visible controls. The partial whole-window scan
  exposes read-only region refs but no field/control refs; selecting region A
  produces a complete observation under the unchanged budget. Duplicate field
  and checkbox IDs/names in the other regions cannot redirect the confirmed
  write or Check. Independent per-region values and counters prove only A
  changes. Reordering the actual region child collection after preflight
  invalidates its absolute UIA sibling path before the write; a fresh root and
  scoped observation can then write A in its new position. A final separately
  observed, preflighted and confirmed empty replacement clears A. Independent
  values and change counters must report an empty string and exactly three
  field changes in A; all other regions remain unchanged.
- After the final Check/Uncheck, both fresh scoped observation and independent
  fixture values must report A off, with exactly one Check and one Uncheck;
  other dense regions remain unchanged.

The ignored `commands::message::tests::native_uia_fixture_capture` test then
checks capture independently. With the password-control tree removed, the dense
groups provide two distinctive opaque background colors without adding UIA
nodes. Production target selection must identify the exact fixture PID. The
current normal binary's existing single-window capture helper must return a
bounded, nonblank RGBA PNG containing both colors; all pixels remain in memory.
A deliberately wrong PID must return `TARGET_CHANGED` and zero PNG bytes.
The fixture then minimizes only its own window: both the production target
probe and the bound helper must refuse it as `TARGET_MINIMIZED`, with no PNG
bytes and no automatic restoration. After an explicit fixture-side restore,
the same target must produce a valid PNG again. This exercises a real window
state transition, not a simulated capture result.
Independent fixture values and counters remain unchanged by capture. This is
the existing capture phase split out for a shorter diagnosis loop, not a second
capture implementation or an extra offline scenario.

Frame/window dimension mismatches are separately refused as `FRAME_CHANGED`,
with no image returned. Offline tests cover that fixed helper category and its
safe recovery message; the native smoke does not force a resizing race or prove
the cause of historical intermittent capture failures.

The dense-panel transition can reset the removed ScrollViewer's layout metrics.
Its final assertions therefore use an explicitly named `legacyScrollCheckpoint`
recorded while the old panel was still visible, after native scroll assertions.
Other legacy values and event counters continue reading their original control
objects. Dense-region values/counters are separate live readbacks; no user app
or unrelated window is involved.

Failure diagnostics also include counts of pointer movement, button, wheel and
key-down events received by this owned window. They do not record keys,
positions or global input, block input, change event routing, or replace any
state/counter assertion. Existing state snapshots publish the counts without
extra per-input disk writes. Nonzero counts are a diagnostic clue, not proof
that input caused a failure; zero counts do not exclude every OS/provider cause.

The same runtime adapter serializes UIA observation, preflight, settings-only
draft-target discovery and writes with
a nonblocking gate; overlapping calls return `TARGET_BUSY` without queuing or
consuming refs. Deterministic two-thread/barrier and poisoned-lock regressions
cover this offline. The native fixture itself does not simulate two user tasks.
Settings discovery now uses the supplied runtime adapter rather than creating
a new adapter, and grants no observation or action capability by itself.

Only the exact fixture child is terminated, and its resolved temporary directory
is removed on exit. The fixture also self-expires after three minutes. Helpers
have no console window. To reuse a current build, pass
`--test-binary <absolute-path-to-angelbot-library-test.exe>`; the runner checks
that each requested exact test exists before launching the fixture. Even with that option,
the normal AngelBot binary is rebuilt from current sources before the fixture
starts. Its path is read only by the ignored test, not a production override.
The exact existing helper argument exits through `main` before normal app startup;
the existing process-tree host hides its console and reaps helper descendants.

This smoke intentionally stays outside `python scripts/verify.py full`, whose
automated checks must work without a live desktop. It is backend/native-path
coverage, not a real model conversation or a frontend confirmation-click test.
It does not cover process restart, off-screen controls, or every Windows app's
UIA provider.

The WGC increment verifies production target selection, the normal executable's
helper entry point, native single-window pixels, bounded process handling and
wrong-PID refusal on this owned WPF window. A libtest executable cannot enter
the normal helper entry point, so the test supplies the separately built normal
binary explicitly; it does **not** attest the ordinary `DesktopAdapter` parent's
`current_exe()` launch path, a frontend image-confirmation click, real visual-model
input, Chromium/Win32 compatibility, lock-screen/occlusion behavior or a completed
business task. No image is saved or printed, and no normal AngelBot UI, user app,
network request or user runtime database is opened. Older recorded native passes
below do not attest this new WGC increment until the updated smoke runs.

The separate `python scripts/desktop_e2e.py` journey reuses `owned_fixture` in
dense mode after building its isolated desktop binary. It exercises the ordinary
parent's `current_exe()` helper path and the real image-confirmation UI, then
checks exact tool-call delivery to a scripted image-aware provider, next-turn
release, unchanged fixture counters and absence of persisted PNG data. Its
temporary window is removed independently of retained E2E application data.
See [the desktop E2E guide](../../e2e/README.md); neither journey proves real
visual-model understanding or arbitrary third-party application compatibility.

## Recorded verification

On 2026-10-01 the earlier text-only chain and eight fresh-window repetitions passed
consecutively. Password/read-only/duplicate controls remained unchanged; the
confirmed ordinary field matched the requested Unicode text. The repository's
offline full profile also passed (286 frontend tests, 934 Rust tests; this
interactive test is intentionally ignored in that profile). The current smoke
extends that same fixture and test with the confirmed Invoke chain above;
these earlier repetition counts do not attest the newer button chain.

The extended text/Invoke chain passed four independently started owned
windows on 2026-10-01. Each fixture reported exactly one invocation; fresh UIA
observation saw the changed button label. Consumed references and settled
approvals could not be reused, and protected fields remained unchanged.
The preceding offline full profile passed 327 frontend tests and 941 Rust tests;
six frontend tests remain skipped and the one interactive smoke is separately
opt-in, not counted as passed by the offline profile.

An earlier fixture readback wait timed out once and was not reproduced in
those eight subsequent runs. Expected-state and last-snapshot diagnostics are
retained for recurrence; the timeout is not considered explained or fixed by
the later passes. This is bounded WPF-fixture evidence, not universal Windows
compatibility or business-task completion.

As of 2026-10-02, the extended text/Invoke/Select/Expand/Collapse chain has passed
five fresh owned-window runs in this implementation phase. One additional run
returned `RESULT_UNKNOWN` on Expand; three later fresh-window runs passed.
The Expand failure remains unexplained, not fixed by those passes. On a failed
run, the runner now reports only the owned fixture's boolean control state and
event counters, never raw user-app values. An unknown result stops that run;
the runner does not replay an operation against that window.

The 2026-10-02 `python scripts/verify.py full` passed 381 frontend tests and 947 Rust
library tests without model credentials or online dependency resolution.
Six frontend tests remain skipped; the one interactive test remains separately
opt-in. Neither is counted as passed by that offline profile. Native evidence
still does not cover a frontend approval click, real model journey, tab switch,
or a completed business task.

The 2026-10-03 scrolling increment extends the existing fixture and ignored test,
not another parallel harness. Deterministic provider tests separately cover
delayed state publication, wrong direction, unusable positions, state/identity
loss and single-effect counters. Read-only settling never repeats the action;
its sleep budget is bounded but provider calls remain subject to the existing
helper deadline. This addresses a reproduced delayed-readback failure pattern,
not proof of the historical sporadic native Expand failure's cause. The latest
run results are recorded in [the current roadmap](../../docs/current-roadmap.md).

The Check/Uncheck increment extends the same fixture, ignored native test and
Python readback rather than adding another harness. The one checkbox is kept
within the existing observation budget; its checked, unchecked and indeterminate
events are recorded independently by WPF. This scope attests control state only,
not a completed application workflow. Native execution remains opt-in; recorded
passes of the older chain do not attest this increment until the updated smoke
has run.

# Native Windows UIA smoke

Run explicitly from an interactive Windows desktop:

```powershell
python scripts/native_uia_smoke.py
```

The runner builds the AngelBot library test binary with Cargo's `--offline`
flag, compiles `NativeUiaFixture.cs` with the installed .NET Framework C#
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
- A named vertically scrollable pane uses the same one-step confirmation path.
  Small down/up operations are independently checked against fixture offsets;
  repeating either direction at its edge is a verified no-op. Exactly two
  vertical-change events occur, with no horizontal movement. Observation,
  preflight and an approval without a live preview never change either offset.

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
that the exact test exists before launching the fixture.

This smoke intentionally stays outside `python scripts/verify.py full`, whose
automated checks must work without a live desktop. It is backend/native-path
coverage, not a real model conversation or a frontend confirmation-click test.
It does not cover process restart, off-screen controls, or every Windows app's
UIA provider.

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

# Delegated Agent Framework Specification

> **Status legend.** Every requirement below is labelled **Implemented** (enforced
> by the named code), **Foundation-not-wired** (durable or policy building block
> exists but no real worker path enforces it), **Target** (agreed product/design
> behaviour not implemented), or **Needs-user-decision** (must not be silently
> chosen). This is the normative integration index for delegated work. Research
> documents remain evidence; the network protocol is defined separately in
> [Delegated Agent Network Capability Specification](delegated-network-capability-spec.md).

> **Current-state override (2026-08-30).** The implementation update below is
> historical. `delegate_work` now creates a Workspace Supervisor work record
> before issuance and links it to the delegation/outbox inside the issuance
> transaction. `SupervisorDelegationDispatcher` is the sole adapter that opens
> that record's execution gate; `DelegationRuntime` cannot claim a linked
> outbox before authorization and the existing pump can recover a pending
> authorization. This closes the initial control-plane wiring slice only. It
> does **not** make worker completion, reviewer acceptance, real-workspace
> materialization, or the user-facing Main Agent work surface complete. The
> current priority and verification status live in
> [current-roadmap.md](current-roadmap.md).

> **Implementation update (2026-08-04).** `DelegatedAttemptScheduler` now
> performs conservative startup recovery, WorkPackage/queued-lease gating, and
> durable outbox dispatch through `DelegationRuntime`. It is an injected module
> only: there is still no application-host scheduler, automatic delegation
> service, Materializer path, or user-facing child-agent surface. A real
> `DelegatedWorkerAdapter` factory/host binding therefore remains **Target**.
> Migration `050` now captures an immutable, non-secret `DelegatedModelBinding`
> for each WorkPackage at Main-Agent issuance. The keychain-backed host factory
> remains injected and unwired; restart recovery must not fall back to the
> current foreground provider.

## 1. Product boundary and authority

- **Target** The user converses with exactly one Main Agent. It owns relational
  context, decisions, confirmations, recovery explanations, and every
  user-visible status. A child is never a user-operated conversation endpoint.
- **Target** The Main Agent directly completes simple work. It delegates only
  independently deliverable, specialised, long-running, or parallelisable work.
- **Target** A child cannot recursively delegate. Only the Main Agent creates a
  `WorkPackage`, delegation, attempt, lease, or confirmation request.

Implementation status: WorkPackage lifecycle v1 is now minimal and durable:
`draft|active|frozen|accepted|declined|cancelled|expired`. Only `active`
packages can issue a lease or change candidates. Creating a ChangeSet freezes
the package; its single confirmation records the separate decision and moves
it to `accepted` or `declined`.
- **Implemented** Foreground terminal UI events are held until their audit write
  succeeds; failure suppresses the completion event. See
  `src-tauri/src/agent/foreground_lifecycle_control.rs`.

## 2. Ownership seams

| Owner | Responsibility | Status |
|---|---|---|
| `ForegroundConversationState` | Select personalised history, compaction and compact task facts for a foreground turn. | **Implemented** |
| `ExecutionKernel` + foreground host ports | Shared multi-turn model/tool orchestration seam; foreground has controlled batch/lifecycle ports. | **Implemented** for foreground; **Foundation-not-wired** for delegated host. |
| Main Agent / `DelegationRuntime` | Create durable delegation records, choose profile/shape, supervise, accept/reject delivery, decide retry/cancel/recovery. | **Partially implemented**: the foreground-only internal `delegate_work` tool captures a provisional parent identity, validates an approved source-policy reference, issues a lease and writes the queued outbox atomically. No scheduler/WorkerAdapter dispatch loop is live. |
| `DelegatedWorkerAdapter` + `ExplorerNetworkToolHost` | Fresh worker host consumes a persisted brief; the Explorer host can offer bounded `network.search`/`network.fetch` evidence through the shared Kernel settlement seam. | **Implemented host seam, not wired**: no Tauri command, automatic dispatch, or real workspace writes. |
| `DelegatedModelBinding` | Bind one WorkPackage to immutable provider/model/credential-handle/policy references; resolve a host through an injected keychain factory. | **Foundation-not-wired**: migration `050` and typed safe failure mapping exist; no application factory or worker launch consumes it yet. |
| `ExecutionGateway` | Fail-closed authorization for every child file/tool/network/materialization operation and effect key. | **Foundation-not-wired** policy validation exists; actual gateway enforcement is **Target**. |
| `DelegatedSandbox` | Fresh per-attempt candidate workspace, seal/revoke/quarantine/cleanup. | **Partially implemented**: `delegate_work` allocates the fresh candidate sandbox before queueing; runtime dispatch, sealing, and cleanup scheduling remain unwired. |
| Materializer | Sole holder of real-workspace write handle; apply an approved immutable ChangeSet serially. | **Target** |
| UI projection | Show Main-Agent work-package milestones, not atomic child tools or child controls. | **Target** |

## 3. Identity, lifecycle and durable records

- **Implemented** `delegationId` is the stable unit; `attemptId` is disposable
  per execution/retry. Migration `041_delegation_runtime.sql` prevents two
  queued/running attempts for one delegation and binds parent run/session/message.
- **Implemented** Delegation statuses are `queued`, `running`,
  `awaiting_confirmation`, `awaiting_summary`, `completed`, `failed`,
  `cancelled`, `needs_decision`. A completion requires a parent-accepted
  delivery; child submission cannot complete a delegation.
- **Implemented** Attempt records, leases, ordered events and an outbox are
  durable (`041`); `DelegationRuntime` provides admission, heartbeat expiry,
  cancel/pause/recovery and delivery acceptance primitives.
- **Target** The only live transition flow is:

```text
WorkPackage -> delegationId -> queued attemptId -> running
  -> [awaiting_confirmation | awaiting_summary | needs_decision]
  -> completed | failed | cancelled
```

  Pausing, cancelling, timeout or a missed heartbeat revokes the lease before
  stopping the worker and sealing its sandbox. Resuming/retrying creates a new
  attempt, lease and sandbox, never reuses the old one.
- **Target** One safe automatic retry is allowed only for a proven no-effect,
  transient failure. Partial/unknown effects, permission denial, verification
  failure or a changed strategy become `needs_decision` for the Main Agent.

## 4. Task and worker classification

`TaskShape` and `WorkerProfile` are independent; profiles are data-driven
policies, not special privileged code paths. `TaskShape` is always the hard
ceiling: every profile may be selected for either shape, but `explore` permits
only project reading and mediated `search`/`fetch`, never candidate access or
writes, scratch output, builds/tests, confirmation, or materialization.

| Profile | `explore` | `change` | sandbox candidate write | real workspace write | network | Status |
|---|---:|---:|---:|---:|---|---|
| Explorer | project read + issued `search` / `fetch` | project read + issued `search` / `fetch` | no | no | issued `search` / `fetch` only | **Foundation-not-wired** policy |
| Implementer | constrained by `explore` ceiling | sandbox candidates + temporary build/test output | yes for `change` only | no | approved docs; lockfile-bound build registry only | **Foundation-not-wired** policy |
| Verifier | constrained by `explore` ceiling | reads candidate + temporary test output | no | no | default off; build registry only if issued | **Foundation-not-wired** policy |

- **Foundation-not-wired** `agent::worker_policy` is an implemented pure,
  no-privilege compiler for the initial profile matrix and immutable v1 target
  `CapabilityPolicy`; it never grants materialization. Migration `045` persists
  `worker_profile` and `worker_policy_version` on `WorkPackage` (it adds no
  index; the work-package scope index already exists in migration `044`).
- **Target** Explorer results may be automatically accepted as structured
  evidence by the Main Agent. Implementer output is always a sandbox candidate,
  never a real-workspace change.
- **Needs-user-decision** The first production `WorkerProfile` catalogue and
  any future profile that adds a new capability require an explicit product
  decision; this specification fixes only the three initial profiles above.

## 5. Capability, sandbox and network policy

- **Implemented** `CapabilityLease` persists read/write roots, tool allowlist,
  network hosts, budget, expiry and revocation state; `ExecutionGateway`
  provides validation for expiry, allowlists, root containment and effect keys.
  **Target:** actual lease issuance and gateway enforcement by a worker.
- **Foundation-not-wired** `DelegatedSandbox` provides nonce-manifested
  lifecycle and cleanup primitives for active/sealed/revoked/quarantined data.
  **Target:** actual per-attempt sandbox creation and worker confinement.
- **Target** Children start with no filesystem, process, network, credential,
  browser-state or real-workspace authority. Every operation traverses the
  Gateway and must satisfy global ceiling ∩ WorkPackage policy ∩ profile ∩
  lease. A model instruction, URL, path or UI claim never grants authority.
- **Target** Fresh sandbox and lease per attempt; accepted output is explicitly
  materialized, everything else is quarantined for seven days then safely
  cleaned. Diagnostic/raw evidence is not eligible for later briefs.
- **Needs-user-decision** Sandbox base location, encryption-at-rest requirement,
  cleanup retry horizon and user-facing notification for blocked cleanup remain
  unspecified. The seven-day retention duration is decided.
- **Target** Network is `gateway-only search/fetch`; its exact allowed methods,
  source policy, redirect/DNS/MIME/size checks, prompt-injection handling and
  permanent prohibitions are normative in the linked network specification and
  must not be duplicated or weakened here.

## 6. Context, contracts, evidence and attention

- **Implemented** Versioned delegation brief/delivery validators,
  constrained artifacts and acceptance gates exist in
  `delegation_contract.rs`. Parent context assembly selects structured facts,
  rather than raw conversation/tool/model output.
- **Target** A child receives only task goal, selected background constraints,
  lease/profile policy and authorised evidence references. It returns a bounded
  delivery: milestones/effects, verification records, artefact references, and
  at most three decision questions/options/risks. It never returns raw chain of
  thought, tool logs or an unbounded transcript.
- **Implemented** `AttentionCard.v1` projects open actionable attention as
  compact structured cards, at most three cards and 512 bytes; diagnostic
  payload is isolated in `attention_diagnostic_evidence`.
- **Implemented** Cross-session attention requires both local profile and
  canonical workspace key; missing scope falls back to the originating session.
  Resolved, superseded and expired cards are excluded.
- **Target** Worker deliveries and failures enter Main-Agent context only as
  validated summaries plus authorised evidence references; untrusted web text,
  raw diagnostics and child reasoning never become long-term chat context.

## 7. Low-interruption change approval

```text
WorkPackage (goal + frozen scope/policy)
  -> CandidateSet (sandbox candidates + verification evidence)
  -> ChangeSet vN (merged deterministic diff + risks + validation + rollback)
  -> Confirmation(ChangeSetHash, scopeDigest, baseRevision, TTL)
  -> Materializer -> terminal audit barrier -> Main-Agent result
```

- **Implemented (contract)** Confirmation is one user decision per understandable work package,
  not per child, file or normal documentation fetch. Explorer needs no write
  confirmation; Verifier rechecks the same candidate set without adding one.
- **Target** The Materializer is the sole real-workspace writer and serializes
  writes. It must verify the immutable confirmation binding before applying.
- **Implemented (contract)** A confirmation becomes invalid on CandidateSet/diff change,
  scope/workspace/profile/network-policy expansion, base-revision drift,
  verification/strategy change, increased risk or any external side effect.
- **Implemented (contract)** Migration `044` and `agent::work_package` provide
  durable `WorkPackage`, `CandidateSet`, `ChangeSet`, and `Confirmation`
  bindings with scope isolation, optimistic candidate versioning, expiry and
  pre-materialization revalidation. **Target** A Materializer is still absent.
- **Needs-user-decision** Exact conflict-resolution UX when two candidates
  modify the same file has not been chosen; first implementation must surface a
  Main-Agent `needs_decision`, not invent a merge policy.

## 8. Concurrency, recovery, verification and visibility

- **Target** At most two independent delegations run concurrently initially.
  One delegation has at most one active attempt; same-workspace materialization,
  confirmation and high-risk effects serialize. Candidate conflicts are detected
  before ChangeSet creation.
- **Target** Heartbeats are structured task milestones with stage, budget and
  redacted evidence references, not per-tool chatter. A missed heartbeat or
  exhausted budget follows the revoke-stop-seal path.
- **Target** Worker completion is only *submitted* until schema/evidence/budget
  validation and Main-Agent acceptance. Only explicit verification evidence may
  be displayed as verified.
- **Implemented** Foreground terminal durability barriers prevent a persisted
  failure from being shown as successful completion. **Target** The same barrier
  applies to worker acceptance, ChangeSet materialization and user projection.
- **Target** UI exposes aggregated Main-Agent task milestones and outcomes;
  it never exposes child controls or atomic tool activity.

## 9. Implementation gates

1. **Implemented, P0 contract:** Persist `WorkPackage -> CandidateSet ->
   ChangeSet -> Confirmation`, including immutable hash/scope/base/TTL and invalidation.
2. **Foundation-not-wired, P0 policy:** The pure no-privilege
   `TaskShape`/`WorkerProfile` compiler and v45 WorkPackage persistence exist;
   a future adapter must validate the frozen plan before **Target** lease
   issuance.
3. **Target, P0:** Implement the gateway-only worker host and its first
   Explorer slice (local read + issued controlled network) with no direct
   `ToolRegistry`, real-workspace handle or conversation context.
4. **Target, P0:** Implement fresh Implementer sandbox candidates and Verifier
   delivery; detect CandidateSet conflicts. Do not materialize yet.
5. **Target, P0:** Implement Materializer, confirmation validation, serial
   apply/rollback semantics and terminal durability/recovery tests.
6. **Target, P1:** Add work-package-level projection after the internal
   acceptance path is trustworthy.

It is **not safe to claim delegated agents are usable** before gates 1--4. It
is safe to begin gate 1 because its contracts do not grant any child authority.

## 10. Traceability matrix

| Concern | Code / migration evidence | Test evidence | Primary documentation | Status |
|---|---|---|---|---|
| durable identity, lease, outbox, delivery acceptance | `migrations/041_delegation_runtime.sql`, `delegation.rs`, `delegation_runtime.rs` | `session_delete_cascades_all_delegation_records`, `duplicate_completion_acceptance_is_rejected` | [runtime boundary](controlled-delegation-runtime-boundary-report.md) | **Implemented / Foundation-not-wired** |
| policy validation | `execution_gateway.rs` | `effect_keys_are_stable_and_scoped_to_attempt` | [permissions threat model](agent-delegation-permissions-threat-model.md) | **Foundation-not-wired** |
| sandbox lifecycle | `delegated_sandbox.rs` | `cleanup_refuses_active_or_forged_paths` | [sandbox audit](delegated-sandbox-lifecycle-audit.md) | **Foundation-not-wired** |
| structured brief/delivery | `delegation_contract.rs` | `rejects_schema_identity_secret_and_absolute_path_leaks` | [worker integration design](real-worker-adapter-integration-design.md) | **Implemented** |
| delegated model identity | `migrations/050_delegated_model_binding.sql`, `delegated_model_binding.rs` | `durable_scope_and_immutability_are_enforced_without_secrets`, `migration_recovers_when_v50_journal_entry_is_missing` | this specification | **Foundation-not-wired** |
| attention and cross-session scope | `attention.rs`, migrations `042`, `043` | `same_profile_and_workspace_cards_cross_session_but_never_cross_scope` | [compact attention research](research-firstmate-compact-attention-context.md) | **Implemented** |
| foreground loop seam/durability | `execution_kernel.rs`, `foreground_*`, `event.rs` | `terminal_event_is_persisted_before_it_is_exposed` | [kernel design](execution-kernel-multi-turn-tool-orchestration-design.md), [runner decomposition](agent-runner-decomposition-design.md) | **Implemented** foreground only |
| work-package confirmation contract | `migrations/044_work_package_confirmation.sql`, `work_package.rs` | `work_package` focused tests | this specification | **Implemented (no privilege / not wired)** |
| worker profile policy contract | `migrations/045_work_package_worker_policy.sql`, `worker_policy.rs` | `worker_policy` focused tests | this specification | **Foundation-not-wired**: implemented pure no-privilege compiler; v45 persists `worker_profile` and `worker_policy_version`; not wired to lease/gateway enforcement |
| worker/runtime integration | no adapter/materializer | no end-to-end worker test | [worker integration design](real-worker-adapter-integration-design.md) | **Target** |
| network capability | no search/fetch gateway implementation | no gateway test | [network capability specification](delegated-network-capability-spec.md) | **Target** |
| safety/usability rationale | n/a | research evidence, not runtime tests | [mainstream research](research-mainstream-subagent-safety-usability.md), [FirstMate comparison](research-firstmate-safety-usability.md) | **Evidence** |
# Implementation status update (2026-08-03)

The first no-privilege runtime slices are implemented: durable, versioned
`WorkPackage -> CandidateSet -> ChangeSet -> Confirmation` records with
ownership/session/workspace scoping, candidate optimistic locking, immutable
hash bindings, expiry, and invalidation before materialization; and the pure
`TaskShape`/`WorkerProfile` policy compiler. Migration `045` persists
`worker_profile` and `worker_policy_version` on `WorkPackage` without adding an
index (the scope index is from migration `044`). These remain
**Foundation-not-wired**: no WorkerAdapter, actual lease issuance or gateway
enforcement, network execution, per-attempt sandbox creation, direct tools, or
real-workspace Materializer is implemented.

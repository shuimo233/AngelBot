# Delegated Agent Framework Specification

> **Status legend.** Every requirement below is labelled **Implemented** (enforced
> by the named code), **Foundation-not-wired** (durable or policy building block
> exists but no real worker path enforces it), **Target** (agreed product/design
> behaviour not implemented), or **Needs-user-decision** (must not be silently
> chosen). This is the normative integration index for delegated work. Research
> documents remain evidence; the network protocol is defined separately in
> [Delegated Agent Network Capability Specification](delegated-network-capability-spec.md).

> **Current-state override (2026-10-14).** The production factory now composes
> the Supervisor-authorized outbox, local model worker, independent Reviewer,
> candidate capture, Main-Agent follow-up and confirmed Materializer through one
> service/pump. Model replies contain business fields only; trusted runtime
> identity/version fields are attached by the host. Producer completion revokes
> its execution lease without accepting or discarding the submitted delivery.
> Prepared Change and failed-review summaries are text-only, use the existing foreground queue,
> and cannot accept, discard or apply a candidate. Real-model and native desktop
> task acceptance remain separate gates in [current-roadmap.md](current-roadmap.md).
> Before-delivery runtime failures remain host facts, not fabricated deliveries or
> review verdicts. The existing Main conversation activity projection and ordinary
> foreground context share fixed, non-diagnostic failure reasons bound to the latest
> attempt; retries never inherit earlier result/review/heartbeat state. Ordinary
> context reserves at most two of the existing four delegation slots for failures
> within the same 12 KB budget. A source-bound delivery-summary turn excludes these
> unrelated failures. No extra model turn, automatic retry or transcript message is
> manufactured for a runtime failure; the Main Agent can explain it on the next turn.
>
> **Historical control-plane update (2026-08-30).** The implementation update below is
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
| `ExecutionKernel` + host ports | Shared multi-turn model/tool orchestration seam; hosts retain lifecycle/settlement policy. | **Implemented** for foreground and the production change-worker adapter. |
| Main Agent / `DelegationRuntime` | Create durable delegation records, choose profile/shape, supervise, accept/reject delivery, decide retry/cancel/recovery. | **Partially implemented**: the foreground-only internal `delegate_work` tool captures a provisional parent identity, validates an approved source-policy reference, issues a lease and writes the queued outbox atomically. No scheduler/WorkerAdapter dispatch loop is live. |
| `DelegatedWorkerAdapter` + `ExplorerNetworkToolHost` | Fresh worker host consumes a persisted brief; the Explorer host can offer bounded `network.search`/`network.fetch` evidence through the shared Kernel settlement seam. | **Implemented host seam, not wired**: no Tauri command, automatic dispatch, or real workspace writes. |
| `DelegatedModelBinding` | Bind one WorkPackage to immutable provider/model/credential-handle/policy references; resolve a host through an injected keychain factory. | **Foundation-not-wired**: migration `050` and typed safe failure mapping exist; no application factory or worker launch consumes it yet. |
| `ExecutionGateway` | Fail-closed authorization for every child file/tool/network/materialization operation and effect key. | **Foundation-not-wired** policy validation exists; actual gateway enforcement is **Target**. |
| `DelegatedSandbox` | Fresh per-attempt candidate workspace, seal/revoke/quarantine/cleanup. | **Partially implemented**: `delegate_work` allocates the fresh candidate sandbox before queueing; runtime dispatch, sealing, and cleanup scheduling remain unwired. |
| Materializer | Sole holder of real-workspace write handle; apply an approved immutable ChangeSet serially. | **Target** |
| UI projection | Show Main-Agent work-package milestones, not atomic child tools or child controls. | **Target** |

**Model-error policy (2026-10-21).** A provider-returned error ends the current
kernel model turn immediately. The kernel must not infer retryability from
untrusted error text or blindly resend authentication, invalid-body or stream
errors. A new host-owned turn can recover; existing timeout-attempt budgets,
explicit protocol-correction turns and Supervisor attempt policy remain separate.
This is shared by Main and workers, not an account sign-out or billing fallback.

**Stop contract (2026-10-21).** Every foreground admission reserves its run slot
and cancellation signal together; that signal is mandatory input to the runner,
including confirmation continuations. Native IPC uses the registered `AppState`;
the four typed frontend controls share their implementation with Dev HTTP.
The kernel observes cancellation during inference and timeout backoff, and rejects
late responses before host settlement. A stopped Main slice persists `stopped`,
not a fabricated model failure, retaining settled receipts without blind resume.
Stopping cannot undo external effects or guarantee server-side inference/billing
has ended. Only an already-owned credential resolution may outlive its waiter:
it retains the existing refresh locks and persists rotating replacements, never
detaching inference, deltas or tools. This follows the token-persistence boundary
in [OpenAI profiles and sessions](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions);
process death during rotation is not claimed crash-safe. No new authority,
automatic replay, worker retry policy, dependency or schema is introduced.

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
- **Implemented** A recovered, interrupted producer with no submitted delivery
  retains `needs_decision` and the `recovery_unknown_effects` host event. Main
  context and the existing activity projection share the fixed
  `execution_interrupted` explanation: effects/completion are unverified, and
  there was no automatic retry. Only the latest attempt can supply it; raw
  diagnostics stay private. A submitted delivery uses its normal review path,
  not this before-delivery failure projection. No extra transcript turn or
  recovery dispatcher is created.
- **Implemented** A terminal worker with a submitted delivery is sealed and its
  capability lease revoked before waiting for review/confirmation. Its delegation
  remains `awaiting_summary`; a frozen candidate is not an ineligible running
  producer. Recovery retains the same delivery and never replays the producer.
- **Implemented** An interrupted running review is requeued and claimed with
  a fresh Reviewer execution identity through the existing coordinator/service.
  Its job, delivery revision and review subject remain unchanged; an old claim
  cannot record an outcome for the new execution. Only the side-effect-free
  review is rerun, never the producer. A new verdict still requires the normal
  Main follow-up and canonical-write confirmation. Real-model recovery evidence
  is recorded separately in [current-roadmap.md](current-roadmap.md).
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

- **Implemented** Main preparation and `check_constraint` share the saved-global-rule
  reader in `agent/extensions/constraint.rs`. Its JSON projection carries source
  IDs and an explicit omitted count, excludes forgotten/superseded/unattributable
  session rows, and is independent of similarity recall and summary replacement.
  It is disabled with long-term memory, bounded to 10 whole rules / 8192 JSON
  bytes, and never authorizes actions or automatically enters a worker's memory.
  See [ADR-0004](architecture-decisions.md#adr-0004) for the authority boundary.
- **Implemented** Versioned delegation brief/delivery validators,
  constrained artifacts and acceptance gates exist in
  `delegation_contract.rs`. Parent context assembly selects structured facts,
  rather than raw conversation/tool/model output.
- **Implemented** The local model result contains only `status`,
  `executive_summary`, `key_facts`, `milestones`, `verifications`,
  `candidate_artifacts`, `open_questions`, `risks`, `evidence_refs`, and `usage`.
  The prompt supplies typed item shapes, enums and existing contract limits.
  The host supplies schema version, delivery ID/revision and trusted delegation
  identity; model-supplied routing fields are rejected. Durable delivery format
  is unchanged. Both worker and Reviewer text require a complete `EndTurn`;
  truncated/filtered/unknown responses cannot become accepted results.
- **Implemented** Reviewer input preserves verification status, not only the
  producer's conclusion. Its no-tool prompt specifies every `ReviewOutcome`
  field and verdict. Review and verification remain distinct evidence sources.
- **Implemented** Automatic summary context uses only the delivery named by
  the trusted queue binding, selected before the ordinary context count limit.
  A missing source stops preparation; it does not yield an ungrounded success.
  User/model text cannot select this internal context scope or widen its budget.
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
- **Implemented** For delegated Change, the Materializer is the sole canonical
  writer. It checks immutable confirmation, scope/base and the reviewed artifact,
  serializes application and records an independently checked applied receipt.
- **Implemented (contract)** A confirmation becomes invalid on CandidateSet/diff change,
  scope/workspace/profile/network-policy expansion, base-revision drift,
  verification/strategy change, increased risk or any external side effect.
- **Implemented (contract)** Migration `044` and `agent::work_package` provide
  durable `WorkPackage`, `CandidateSet`, `ChangeSet`, and `Confirmation`
  bindings with scope isolation, optimistic candidate versioning, expiry and
  pre-materialization revalidation. `changes/materializer.rs` and
  `change_pipeline.rs` implement confirmed application; this is not arbitrary
  tool, UI or external-side-effect rollback.
- **Implemented** Only a bound, durable `applied` materialization receipt permits
  re-entry to finish parent acknowledgement/cleanup without another write.
  Incomplete/unknown effects do not qualify. A cleanup-pending receipt does not
  turn already-accepted application into a failed operation.
- **Implemented** Passed Explorer results and prepared Change candidates return
  through the same source-bound Main follow-up queue. Explorer retains its
  exact-delivery acknowledgement tool; automatic Change summaries have no tools
  and leave the candidate pending for an interactive confirmed turn. Provider
  failure is not summary success; completed nonempty text with no tool activity
  settles only the notification, not the delivery or its resources.
- **Implemented** Failed/needs-decision reviews of submitted results use that
  same queue with no tools; Main explains missing evidence/decisions without
  accepting, discarding or retrying the result. Unknown or missing source records
  never receive an automatic acknowledgement tool. Producer failures without a
  structured delivery still use the existing task-state projection; full
  automatic conversational explanation of those failures remains **Target**.
- **Needs-user-decision** Exact conflict-resolution UX when two candidates
  modify the same file has not been chosen; first implementation must surface a
  Main-Agent `needs_decision`, not invent a merge policy.

## 8. Concurrency, recovery, verification and visibility

- **Implemented, Main stop interface:** Ordinary Main tools may call
  `list_delegated_work` for at most 12 recent unfinished tasks in the trusted
  session, then `stop_delegated_work` with an exact delegation/attempt pair.
  The service rejects cross-session and superseded attempts, revokes execution
  authority, invalidates obsolete Reviewer claims and unapplied approvals, and
  uses the existing terminal cleanup/recovery path. Repeated requests distinguish
  already-stopped and already-finished work; pending cleanup is explicit.
  An applying/unknown materialization retains recovery resources; an applied
  result is never rolled back. Automatic delivery-summary turns have neither
  tool. This does not prove live-model intent selection or native desktop
  acceptance; those remain separate gates in the current roadmap.
- **Implemented, interrupted materialization:** Before the service accepts new
  work, a persisted `applying` receipt becomes `unknown`. Recovery neither
  replays Git nor aborts or overwrites the canonical workspace. Only an already
  verified `applied` receipt may repair its exact confirmation/artifact/change
  handoff; this is journal repair, not a new content read or user acceptance.
  Unknown effects block retry, ordinary decision/decline, another approval and
  terminal cleanup. Main context and activity show bounded host uncertainty
  instead of presenting the producer's delivery as canonical-write success.
  Candidate resources remain available for inspection; automatic reconciliation
  of partial writes or conflicts is not implemented. Offline acceptance kills
  owned processes before writing, after a partial write, after Git commits and
  after receipt persistence; this is not a power-loss/filesystem-corruption test.
- **Implemented, bounded concurrency** At most two independent delegations run concurrently initially.
  One delegation has at most one active attempt; workspace admission excludes
  a second writer in the same workspace, and the shared service serializes
  canonical materialization. Real-model acceptance covers two separate Project
  workspaces with overlapping workers, scoped cancellation of one, and the
  survivor's independent review, Main follow-up and confirmed/read-back apply.
  It does not establish arbitrary same-project concurrent writes or native UI
  acceptance; those remain separate from headless model evidence.
- **Target:** Broader high-risk-effect/confirmation concurrency policy and
  candidate-conflict resolution before ChangeSet creation. Current canonical
  drift checks refuse a stale apply; they do not automatically rebase or merge.
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

## 9. Implementation gates (current, 2026-10-14)

1. **Implemented, P0 contract:** Persist `WorkPackage -> CandidateSet ->
   ChangeSet -> Confirmation`, including immutable hash/scope/base/TTL and invalidation.
2. **Implemented, P0 policy:** Issuance validates frozen task/profile policy and
   model binding before lease creation; the Supervisor authorizes the outbox.
3. **Implemented, P0 bounded workers:** Local discovery/read/candidate-write and controlled
   Explorer network hosts do not inherit the Main tool registry or canonical
   write authority. The production factory owns their single scheduler/pump.
   `worker_list` performs non-recursive metadata reads under the existing
   `file.read` capability and exact worktree read roots; it does not grant an
   ancestor directory, shell or new network access. It returns relative entries
   (directories end in `/`), excluding Git metadata and links/junctions, with
   limits of 128 entries / 32 KiB and 1024 scanned children. `truncated` explicitly
   marks an incomplete inventory. Manifest identity, epoch, granted capability
   and lifecycle are rechecked; released worktrees are not discoverable.
   Model-facing local names use `worker_list`, `worker_read` and
   `worker_write_candidate` for protocol compatibility. Durable lease names,
   delivery identities and confirmation authority do not change; dotted legacy
   aliases are not retained. Worker batch validation rejects malformed discovery
   references before any operation in that batch executes.
   File reads are bounded before allocation and during the read, not only by a
   post-read length assertion; a growing/oversized file cannot force an unbounded
   read into the model host.
4. **Implemented, P0 review:** Independent semantic Reviewer, bounded structured
   results and deterministic artifact/materialization checks are wired. These
   mechanics do not establish real-model review quality or arbitrary build/test
   execution inside a worker.
   The ReviewSubject retains typed `candidate_artifacts` from the validated
   delivery (kind, relative reference, description and optional evidence ref),
   using the same domain validator rather than discarding output identity into
   generic evidence URIs. Older queued subjects default this field to empty,
   never invented evidence. The Reviewer returns the actual five-field
   ReviewOutcome object, not a `template` envelope; wrappers, malformed fields
   and abnormal completion remain failures. Declared candidate metadata is not
   an independent file-content read or materialization authority.
   The runtime additionally attaches `file_read_evidence` after actual successful
   reads: an opaque evidence URI, relative file reference, byte count and SHA-256,
   never file text or raw tool logs. The model-facing business DTO rejects this
   host-only field. The same domain validator/count ceiling is used for delivery
   and Reviewer projection, with empty defaults for historical records. Repeated
   reads of the same path/bytes reuse a receipt; overflow is explicit and marks
   context truncation. A successful write invalidates all earlier receipts, so
   final equality checks require rereading the relevant files after the last
   write. Receipt equality supports observed-byte equality only, not code quality,
   arbitrary business correctness or automatic approval.
   Each asynchronous review has a fixed 180-second local deadline. Timeout
   emits a non-passing execution outcome through the existing review service;
   it does not retry or approve a candidate. Shutdown drops in-flight reviewer
   futures, while already accepted remote requests may still run or be billed.
   Review claims, outcome settlement and recovery require the still-submitted
   delivery of the newest producer attempt/revision while Main is awaiting its
   summary. Cancellation, acceptance or supersession invalidates unfinished
   jobs; the existing pump cancels only their model requests. Finished verdicts
   are retained, not rewritten. Recovery uses a fresh opaque reviewer claim, so
   late results from an earlier claim cannot settle the new execution. These
   rules do not create an automatic retry or canonical-write authority.
   The executor also tracks the claim identity: a stale result cannot remove a
   current request from cancellation tracking. Repeated enqueue of the same
   claim is idempotent; a newly claimed execution replaces only that job's old
   local request. Activity projection selects the current delivery revision,
   not an obsolete review with a later cancellation timestamp.
5. **Implemented, P0 confirmed writes:** Candidate capture, frozen ChangeSet,
   confirmation revalidation, serial application, durable receipts and cleanup
   are wired. Unknown external effects stop; they are not automatically replayed.
   Local model workers reuse the attempt's one-way ToolCancellation signal for
   both Kernel and run lifetime. Stop drops the in-flight local provider future
   or retry wait and prevents further tool settlement; existing terminal
   sealing, lease revocation and resource cleanup remain authoritative. It
   cannot recall a request already accepted by a remote model service.
   Foreground-issued sandbox roots follow the current file-backed database's
   directory, including confirmation reconstruction, so a disposable instance
   cannot issue resources into the owner's normal application directory. The
   normal in-memory test fallback is unchanged. No new runtime environment
   override or access to protected credentials is enabled for desktop E2E.
   Main control receipts serialize host decision and scope before producer
   context, retaining them under bounded continuation truncation. A canonical
   materialization receipt proves commit application, not a fresh content read;
   producer evidence stays marked as producer delivery. Requested post-apply
   content verification uses the normal Main read tool. Pending review context
   explicitly projects whether the canonical workspace has been written, using
   the existing change-handoff status rather than a second state ledger.
   Confirmation in progress, needs-decision, cancelled and unfamiliar handoff
   states project an unknown effect, never proof that nothing was written.
6. **Target, P0 acceptance:** Complete real-model/native desktop tasks, including
   rejection, cancellation, revoked permission and interrupted-result recovery,
   before claiming daily unattended reliability. Work-package projections and
   isolated fixtures alone do not satisfy this gate.

## 10. Implementation and acceptance evidence

This specification defines the delegated contract, not a second progress ledger.
Current production wiring, remaining gaps and acceptance evidence live only in
[current-roadmap.md](current-roadmap.md). Original dated outcomes are preserved
in the [fixed evidence archive](archive/implementation-evidence-2026-10-17.md).
Product authority and rationale are maintained in [design principles](design-philosophy.md)
and [architecture decisions](architecture-decisions.md), not unpublished local
reports. Passing deterministic tests does not establish native or real-model
business completion; keep those acceptance gates distinct.

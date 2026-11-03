# AngelBot glossary

AngelBot is a local-first Personal Assistant for one person, reducing the burden
of switching between applications, conversations, and tools. It presents one
continuous Main Agent across personal life and project work, and completes real
tasks through workspace-scoped conversations and controlled external actions.
AngelBot is distributed as an open-source application and has no AngelBot cloud
account or hosted control plane.

## Product Identity

**Personal Assistant**:
A long-lived assistant serving one person across everyday and project work, combining conversation, retained user context, and controlled action to carry work across tools. It is neither a disposable chat nor a coding-only agent or execution-tool launcher.
_Avoid_: chatbot, coding agent, multi-user assistant

**Main Agent**:
The single user-facing assistant responsible for understanding a request, coordinating its execution, following up, and explaining the delivered result. Delegating work changes the executor, not the assistant the user must manage.
_Avoid_: coding agent, worker manager (when referring to the user's assistant)

**Daily Workspace**:
The Personal workspace and usual entry point for everyday conversation and affairs that do not need their own sustained project context; computer, browser and file capabilities do not by themselves make a task a Project. It is not a reduced-capability chat mode and does not own project files. It has one host-owned durable `results/` directory beside the application database, consumed through the existing file tools and workbench. This grants neither project-command nor delegated-workspace authority, nor access to other user directories. Results are retained by default; native Save As authorizes one new destination, not its parent. Existing backups carry the bounded file payload; restore is additive and refuses differing same-name files even in replace mode.
_Avoid_: simple mode, chat-only mode

**Professional Executor**:
An optional specialized execution capability, such as an external coding agent, to which the Main Agent delegates scoped work while retaining responsibility for follow-up and result delivery. It is not another user-facing assistant or a prerequisite for everyday use.
_Avoid_: second assistant, deep mode

**Daily-Use Release**:
A product milestone at which a person can rely on AngelBot as a general assistant for bounded local-file, application, browser, connected-service, automation, and project workflows, including authorization, execution, recovery, and result delivery. Email and calendar are initial connected-service examples, not the product identity.
_Avoid_: demo, feature-complete framework, agent playground

**Connected Service**:
An external account or application, such as email or calendar, that the user explicitly connects so AngelBot can perform bounded reads and actions on their behalf.
_Avoid_: plugin (when referring to the user's account), integration token

**Service Connector**:
A provider-specific boundary that translates a Connected Service into stable personal-assistant capabilities while preserving provider consent and revocation semantics. Mail and calendar are examples rather than privileged capability categories.
_Avoid_: provider SDK, MCP server (unless it is specifically the transport)

**Local Owner**:
The person who controls an AngelBot installation, its local data, policies, and connected third-party services. AngelBot does not introduce a separate product account or remote identity.
_Avoid_: tenant, AngelBot account, workspace member

**User-Owned Data**:
Conversation, memory, task, project, policy, and configuration data controlled by the Local Owner and stored locally by default, with explicit export, deletion, and user-directed backup paths.
_Avoid_: cloud profile, platform data

**Windows Release**:
The first fully supported AngelBot distribution, including installation, updates, secure credential storage, background execution, desktop interaction, and recovery on Windows. Other operating systems are not release targets until this experience is complete.
_Avoid_: desktop release, cross-platform release

**Bounded Task**:
A user objective with a concrete deliverable and a scope suitable for normal personal use or a focused project change. It may use tools, background work, or delegated workers, but it is not an open-ended research programme or autonomous management of a very large project.
_Avoid_: long-running mission, epic, research programme

**General Project**:
A durable Workspace for a sustained, bounded objective with its own history, materials and local agreements, whether or not its files form a Git repository. It retains the same Main Agent and admitted kinds of capability as Daily, but its detailed context is not merged into everyday conversation.
_Avoid_: repository, coding project (when referring to every Project)

**Project Handoff**:
The transfer of a current request into its unambiguous owning Project, rather than a response to merely mentioning that Project. It changes the request's context, not the assistant's identity or access grants; returning to Daily does not move or cancel project work already in progress.
_Avoid_: new agent, permission escalation, copying the Daily conversation

**Task Queue**:
The durable, user-comprehensible ordering of Bounded Tasks owned by one Workspace. Independent work may run with limited concurrency, while conflicting writes and External Actions remain serialized by policy.
_Avoid_: worker queue, message queue, agent pool

**External Action**:
An action that changes state outside AngelBot, such as sending mail, creating a calendar event, or submitting a form. Its authorization is determined by impact and user policy, not by how many internal tool calls it requires.
_Avoid_: tool call, automation step

**Model Profile**:
A replaceable description of a model provider, protocol, model identity, capabilities, cost class, and credential reference. Product behavior may depend on declared capabilities but never on a hard-coded provider or model name.
_Avoid_: model setting, API configuration

**Capability Tier**:
A provider-independent class of model work, such as economical routine work or high-capability complex work. A tier may resolve to a cloud model, a local compatible endpoint, or another user-configured provider.
_Avoid_: provider, model name, reasoning level

**Model Compatibility**:
The ability to preserve the same user workflow across supported providers by negotiating capabilities, selecting fallbacks, and reporting unavailable features without corrupting task state.
_Avoid_: OpenAI-compatible (when only the transport shape is meant), interchangeable output

**Proactive Work**:
Background observation, preparation, or execution initiated from a user-approved trigger or standing policy rather than a current chat message. Its result returns through the same Main Agent and never creates another user-facing agent.
_Avoid_: unsolicited chat, background agent, autonomous mode

**Standing Authorization**:
A revocable user policy allowing a defined class of low-risk External Actions to proceed without repeated confirmation. It is bounded by service, action, scope, and impact rather than granting general autonomy.
_Avoid_: always allow, full access, blanket approval

**Prepared Action**:
A draft, recommendation, or proposed state change that AngelBot may produce proactively but has not yet committed to a Connected Service.
_Avoid_: completed action, pending tool call

## Workspace Supervision

**Workspace**:
A durable user-facing scope: either Personal, which has no project files, or Project, which owns one canonical project root and one visible Main Agent conversation. Restoring a Project preserves its directory as information, not authority: new restored projects have `directory_confirmed=0` until the owner confirms one batch of local paths. History remains readable; file, command and delegated-workspace roots remain unavailable before confirmation. Conversation backups retain Main/tree pointers and inert receipts, not execution queues or external project files.
_Avoid_: Project (when referring to Personal), session, chat

**Main Conversation**:
The sole conversation projected for a Workspace. Its internal session tree supports recovery but is never a second user-facing agent or parallel project chat.
_Avoid_: active session, primary thread

**Supervisor**:
The Workspace-owned authority that serializes state and write decisions while presenting itself as the Main Agent. Workers report to it and never communicate with the user directly.
_Avoid_: orchestrator (when it means a worker), subagent manager

**Work Package**:
A bounded, durable unit of delegated work issued by a Supervisor, with objective, scope, capability lease, state anchor, and structured result contract.
_Avoid_: task (when the user-facing objective is meant), delegation

**Attempt**:
One execution of a Work Package. A retry is a new Attempt, preserving the old one as evidence rather than replaying unknown side effects.
_Avoid_: rerun, resume

**Candidate Change**:
An isolated proposed project mutation and its evidence. It is not a user-project write until the Supervisor verifies its anchor and materializes it.
_Avoid_: completed change, merge

**Capability Lease**:
A minimum, expiring grant of tool and resource authority for exactly one Work Package or automation definition.
_Avoid_: permission, full access

**Activity Projection**:
A safe, concise Main-Agent-level description of delegated work for the user. It excludes raw worker conversation, tool traces, timing estimates, and worker identities.
_Avoid_: worker log, progress percentage

**State Anchor**:
The recorded project state against which a Candidate Change, branch restoration, or materialization is checked.
_Avoid_: base revision (unless specifically a Git revision)

## Project Memory and Automation

**Workspace Repository**:
The Git-backed substrate for an eligible Project Workspace, separating user project contents from AngelBot-managed checkpoints, worktrees, and metadata.
_Avoid_: worktree, project folder

**Project Index**:
A deliberately minimal Personal-space reference to a Project Workspace: name, recent activity, state, and bounded summary, never its task contents or file data.
_Avoid_: shared project context

**Lesson**:
Evidence-backed, project-scoped knowledge derived from an expired branch or failed Attempt; it records the failure condition, reason, correct path, and evidence.
_Avoid_: memory (when referring to global user memory), error log

**Automation Definition**:
A persisted trigger, exact action, Workspace owner, and fixed capability set. It may be deterministic, a restricted script, or Supervisor-mediated agent work.
_Avoid_: scheduled task, agent job

## Delegated Agent Framework

The canonical integration contract for Main-Agent-only delegated work is the
[Delegated Agent Framework Specification](docs/delegated-agent-framework-spec.md).
It labels every rule as Implemented, Foundation-not-wired, Target, or
Needs-user-decision. The network-specific protocol remains in the
[Delegated Agent Network Capability Specification](docs/delegated-network-capability-spec.md).

## Network Capability

A lease-bound, gateway-only permission for a worker to perform a narrowly defined network operation. It is constrained by a versioned Source Policy and never includes credentials, browser state, or direct HTTP access. The normative definition is in [Delegated Agent Network Capability Specification](docs/delegated-network-capability-spec.md).

## Source Policy

An immutable, main-agent-issued policy that specifies allowed public source classes, exact domains or registries, operations, budgets, and expiry for a work package. A worker cannot create or expand it.

## Network Evidence

Untrusted, gateway-produced evidence from search, document fetch, or lockfile-bound dependency retrieval. It is delivered by reference and structured summary, never as authority or automatic long-term context.

## Attention State

A durable indication that the main assistant has work requiring follow-up.

## Attention Card

A compact, structured representation of an actionable attention state.

## Evidence Reference

An opaque pointer to protected supporting information; it is not the information itself.

## Visible Projection

A safe, user-facing explanation derived from structured state.

## Cross-Session Scope

The local identity and workspace boundary used to determine whether unresolved attention may be carried into another conversation.

## Foreground Delegation Tool

`delegate_work` is an internal-only Main Agent tool. It receives only a bounded
goal, evidence references, task shape, worker profile, and approved capability
scope reference. The parent identity, workspace, sandbox, hosts, and effective
permissions are captured by the foreground runtime and are never model input.

## Delegated Model Binding

An immutable, non-secret model identity attached to one WorkPackage. It stores
only provider-profile, model, credential-handle, and policy-version references.
The scheduler must resolve the credential handle through an injected keychain
factory; it never reuses whichever foreground provider is current.

## Personal Memory

**Explicit Memory**:
Information the user has chosen to retain, including facts, events, knowledge or preferences. A saved statement is not by itself current truth or action authority.
_Avoid_: verified truth, permission

**User-Stated Preference**:
A stable preference directly expressed by the user and retained with its original evidence, including an explicitly saved preference. It is distinct from an inferred habit and remains subject to current requests, scope and later corrections.
_Avoid_: inferred habit, automatically confirmed current default

**Observed Habit**:
A tentative pattern inferred from observations rather than directly stated by the user. Inspecting it is an explicit choice and does not make it a confirmed preference or action authority.
_Avoid_: user-stated preference, fact

## Foreground Context Assembler

The single preparation seam for one Main-Agent foreground turn. It assembles
profile/memory configuration, skills, best-effort compaction, selected archived
history, workspace references, and bounded identifier-only attention cards. It
does not resolve providers, execute the runner, write terminal state, emit UI
events, or expose diagnostic evidence payloads.

The shared `PromptBuilder` renders each existing layer once and drops empty
results; renderers own their visibility gates. Layer order, provider roles and
capability admission are unchanged. The learned-preference policy uses compact
Priority/Defaults/Time/History/Authority clauses, with original conditions,
corrections, historical evidence and no-authority rules retained. Explicit and
inferred values are both JSON-encoded data, while their source labels and priority
remain distinct. Exact user-specified formats are requested verbatim, not repaired
by a host postprocessor. Layer `cache_key` labels are incomplete identities and
must not be used to memoize prompts or permissions; no new cache is introduced.

Main omits the text-echo `think` and `extract_entities` helpers; reasoning and
entity extraction remain native model work, not extra host calls or evidence.
Delegation controls are admitted only when their service exists. A present
service keeps its entire turn contract, including controls for newly created
work. No intent classifier, mid-turn tool expansion or new permission is added;
real tool schemas and confirmation rules remain unchanged. Supplemental file
and automation guidance does not repeat facts already carried by native schemas.

Best-effort text compaction borrows the already-bound turn provider and its
non-secret window/output limits. It does not reload settings or resolve another
credential. Incomplete, empty or non-shrinking checkpoints retain the original
history; a summary remains a projection, never new authorization or task truth.
The working window follows the selected message ancestry, not all rows sharing
a session ID. A persisted text checkpoint cuts that window only at its own
same-session message anchor; source edits/deletions invalidate derived summaries.
Legacy unanchored summaries do not hide originals. Summarization includes only
task facts owned by its covered prefix, preserves host system/developer policy,
and never rewrites opaque provider continuation into a text summary. Raw history
and durable tool sources remain separately readable under the current scope.

For safely ended turns, Main derives a lightweight view in that same read snapshot:
verbatim visible user text/attachments, final replies and verified tool-source
references. Known failed turns additionally retain a bounded outcome (not task
completion), goal, failure/unfinished steps and structural end evidence. They
require a non-running final status plus an AgentEnd or whitelisted provider-error
record; status alone, including recovery normalization, is not enough. Every
durable dispatch reservation/start needs a sealed matching result. Private error
messages and intermediate summaries are not copied, and no retry is authorized.
Current-turn protocol and original storage are unchanged. Pending,
unknown, image-bearing, legacy/unreadable or incompletely indexed history falls
back to the entire native replay, as does disabled historical memory reading.
Every actual call needs a sealed source in the reader's current branch reach;
native duplicate-batch cached results need identical arguments and the exact
already-sealed original output. Storage/decoding errors remain errors. There is
no extra memory store, scheduler, provider option or background process. This
view does not replace native in-turn compaction or promise bounded growth of
all historical user attachments. Main retains a small transparent native history
only when every ended turn is completed, sealed receipts match native arguments
and outputs, user originals match the view, and projection would not shrink its
bounded encoding (16 KiB maximum). Opaque protocol is ineligible for this size
comparison. This avoids expanding tiny receipts; it is not a token or cache-hit
estimate. Source warnings appear once per selected window; every exact reference
and completeness marker remains. Explicit unverified native-replay controls
exist only in protocol evaluation.

At the existing token threshold, `CheckpointHistory` can feed that ended-turn
view into the existing anchored text-checkpoint path. It never feeds opaque
items to the summarizer or rewrites originals. Exact conversation references
include revision and explicit user-text attachment indexes; the reader returns
only the selected stored snapshot, never arbitrary metadata or disk contents.
Discovery remains a bounded recent search; exact known references revalidate
the full selected ancestry so older sources do not become unreachable merely
because that search window moved. Summary inference rechecks both its source
view and task facts before commit. Migration 084 invalidates covered checkpoints
on task/fact/receipt/end-proof changes, but leaves unrelated and later work alone. Restore
installs derived context last and excludes summaries whose sources were normalized.
Persisted summaries are explicitly labelled historical data, not instructions,
authorization or proof of completion. These mechanics do not prove long-term
retention quality, bounded cost in every fallback case or multi-model reliability.
Checkpoint task facts identify their own run/message and are not a complete
conversation ledger. A new task does not cancel unrelated user agreements;
empty verification/approval fields do not negate earlier receipts or open work.

Local profile/memory/skill configuration is loaded before provider resolution,
then handed to preparation without re-reading it, so local configuration errors
still take precedence over opening model credentials.

Saved global user constraints are a separate, source-labelled projection from
durable storage, not similarity recall or compressed history. Main preparation
and `check_constraint` share the bounded reader; omissions are explicit. This
context never authorizes an action or implicitly becomes a child's full memory.

Explicit stable interaction preferences reuse `adaptive_constraints`, not a new
memory tier. With memory and learning enabled, the existing bounded background
review may nominate a short verbatim user quote with its source index and a
zero-based memory index (not a second, independently paraphrased content key). The host
checks the original selected user message, current switches and no-save scope;
one explicit statement stays one evidence item, independently ready, with no
inferred-habit lease. Ambiguous, quoted, task-local or authority-changing claims
are not promoted by this path. A direct stable declaration, including dislikes,
does not require a separate "remember this" command. Inferred habits still require repeated evidence.
An effective start date does not by itself make a continuing personal default
task-local. The nominated quote must retain that date and before/after conditions;
when correcting or cancelling a preference, it must also retain the affected
object and correction/cancellation wording, even without a replacement default.
A withdrawal remains sourced preference evidence, not just a review summary or
permission to delete a record. This is a model
extraction contract, not a host calendar parser or automatic semantic erasure.
Main interprets those originals against the current clock: future changes do
not apply before their start, later withdrawals cover scheduled changes within
their scope, and one-turn exceptions do not persist. This remains model reasoning,
not stored applicability state. Restricted-value replies must not recite other
matched memories; unsupported details remain unknown.
Directness is nominated by the semantic extraction, not decided by a phrase
allowlist. Keyword cues only affect review scheduling; original-source, scope,
no-save and control checks remain mandatory. Main frames user-stated records as
saved defaults to apply, and inferred habits as fallible suggestions; neither
category authorizes an action. Exact generative compliance remains fallible.
Explicit records are projected newest first by original message time and source
insertion order, not review completion time; existing evidence supplies this order
without rewriting old records. Both old and new statements remain auditable data:
this is not host-side semantic conflict resolution or automatic erasure.
Main labels both types and gives current requests/settings priority; neither
grants tool authority. The existing owner list/forget path shows provenance type
and clears a dismissed record plus its evidence; the disabled key blocks late
reinsertion of the same record. A paused-learning interval is not backfilled.
Explicitly requested habit lookup filters/ranks before the owner-list cutoff:
the recent list stays at 50 and automatic Main projection at 12. The latter
now also carries an omission count from the same scope/readiness/expiry-filtered
query into Main. A positive count labels the working view incomplete and points
to existing authorized `recall_memories` when the current request needs older
user-stated preferences; the inference flag is only for observed habits. It does not force lookup, expose hidden
values, widen scope or bypass the learning/memory switches. Zero omitted is not
a claim that the user has never saved other information. The test-only Vec view
remains for existing fixtures; production consumes records plus omission data.
When that view has omissions, Main's current-request preparation may reuse the
existing preference lookup to select at most two direct global user-stated
candidates: the strongest lexical match and the newest valid source statement
within its bounded pool. Only global interaction-preference slots are replaced;
session agreements and other rules keep their original positions. The same read
snapshot checks retained verbatim user sources and computes the original count;
it neither commits a caller's transaction nor edits stored preferences. Empty,
unmatched or character-only queries retain the recent view. Edited, deleted,
assistant or provisional originals cannot acquire a new task-match marker.
Existing legacy records are not deleted by this selection. A small host-owned
count labels the query scope as global explicit preferences, not a complete
archive or facts search. The original recall/correction guidance remains; the
experimental completed-lookup directive is not a production policy. The
selection implementation is shared with evaluation, not duplicated there.
Nonempty lookup
streams eligible records into bounded top results using the existing memory
tokenizer/scorer, retaining literal substring compatibility. This is an on-demand
scan, not a new index, background process or larger automatic context. User-stated
preferences are included by default; `include_learned_habits=true` only opts into
observed habits. Only an explicit user exclusion calls for
`exclude_preferences=true`, which excludes both preference kinds even when the
inference flag is true. Ordinary questions do not require source classification.
A miss is not proof of absence.
Labels, opt-out and dismissal remain unchanged.
When no literal/word match exists, opt-in inspection may return labelled partial
Han-character candidates. It reuses the same tokenizer and bounds, ranks rare
overlaps ahead of common noise, and never displaces a direct saved-memory hit.
This is not semantic/synonym search: an unrelated record may share characters,
so Main must check the original quote. Automatic task selection does not promote
these partial matches; the 12-record cap remains. No embedding provider or extra
model call is enabled.
`recall_memories.alternatives` accepts at most five short reformulations from
the same Main that already has the current user intent. Eligible preference
lookup also honors these requested hints after a preference hit, so an old option
does not suppress newer corrections. Ordinary factual hits still expand only
on a miss; scope/opt-in, dismissal and the combined result cap are unchanged.
No hints means no automatic expansion. Generated hints cannot promote character-only matches. The
tool returns original records labelled as candidates for semantic checking,
never new facts or write permission. There is no nested runtime, helper model,
automatic preference expansion or extra background work.
Do not automatically append source-neighbor preferences or create episode/
topic graphs: the 2026-10-29 isolated screens did not establish a quality gain.
They remain explicit test-only experiments, not production memory policies.
Explicit lookup shows each habit's existing source-prioritized record time
alongside provenance, so a delayed review does not look like a newer statement.
Preference results are selected by keyword relevance, then presented with explicit
preferences first and record time newest-first within each class. The bounded
set is not a complete current state. Current defaults require checking
newer same-topic corrections and applicable conditions; list position and a
historical option hit do not by themselves establish the current preference.
It does not automatically resolve semantic contradictions or delete old sources.
Foreground model usage is recorded per successful request with reported usage
and the bound model identity, including compaction through the same provider.
It is not the last request's usage recorded once per user turn. Missing usage,
failed or cancelled requests are not invented as zero-cost calls; these records
are reported token observations, not complete billing or subscription quotas.
PromptBuilder keeps workspace/behavior/tool guidance ahead of changing profile,
memory and time, with the existing turn policy last. It rebuilds live time on
every production build; only paired evaluation freezes its system text. This
preserves a stable text prefix without adding a cache or provider-specific
breakpoints; it is not proof of cache hits or ChatGPT-plan savings.
Known no-save/control markers normalize apostrophe typography only, without
changing original quotes or replacing semantic preference classification.
Ordinary learning shares the existing review cooldown. New correction/withdrawal
cues may wake that same review early and wait a bounded time for its existing
single slot; the host rechecks sources and switches after acquiring it. A durable
attempt cursor prevents the same failed source from being retried every turn.
Cues schedule review only; they do not decide preference meaning or effective
time. This does not promise immediate recall of every formulation, natural
multi-day retention, or new protocol compatibility.

## Conversation Source Recall

Main-only, on-demand read access to the host-selected session's visible active
parent chain. `recall_conversation` returns matched source windows and adjacent
user/assistant messages; `read_conversation` pages exact original bodies using
message ID, revision and Unicode-character offsets. Every call rechecks branch
membership in one read snapshot; edits, deletion or branch changes invalidate
references. Neither tool reads attachment metadata, private provider protocol,
tool receipts, other sessions or delegated worker context.

This reuses SQLite and the existing message FTS index. Continuous Han text uses
a bounded literal scan instead of relying on unicode61 word boundaries. Output,
source and ancestry limits have explicit omission markers, never negative proof
that a conversation did not happen. The existing memory-reading opt-out removes
handlers, schemas and guidance together. There is no second memory database,
background indexer, arbitrary executor, automatic learning or authorization from
historical text. Task facts and current user corrections remain authoritative.

## Durable Tool Sources

Main's optional context projection reuses `agent_run_events`, not a second
receipt store. New result records bind the complete event and owner fields to
a full SHA-256 revision; short source references identify those records. Only
durably matched, large, successful text results receive a bounded model view.
Raw steps, frontend events, verifiers and original handler output stay intact;
images, failures, legacy/unavailable sources and ineffective reductions retain
the original path. Private replay saves the model view without rewriting opaque
provider items. The journal codec owns identity; the pure view renderer owns
paging; the foreground adapter owns branch admission and database reads.

`read_tool_result` rechecks the host-selected Main branch and stored revision,
then returns a bounded JSON subtree or Unicode text window with omissions and
continuation offsets. Empty arguments list at most three verified recent
sources; an empty/partial index is not absence proof. Memory-reading opt-out
blocks prior-run sources but permits the current running Main's own results.
References confer no action authority, business success or replay permission.
There is no new database, migration, background service or delegated capability.

The existing `read_file` also accepts optional `offset`/`limit`/`tail` for direct
plain-text windows, reusing the same Unicode selection and 5000-byte output
budget (including host provenance). Offsets start at zero and count characters,
not lines. Partial results keep `resolvedPath`, `total_chars`, omissions and
continuation metadata; each call reads the current file, not a frozen paging
session. The journal seals that returned window, not an unseen full-file receipt.
No-window calls retain their original result. Office windows are rejected using
the original reader's document-format decision, so extraction limitations cannot
be hidden by slicing off their header. File scope and IPC remain unchanged;
this reduces model input/round trips, not the underlying full-file disk read.

## Foreground Run Store

The transcript/run consistency module for a Main-Agent foreground turn. It
creates hidden provisional assistant parents with matching running task runs,
atomically promotes successful replies with their tool/task protocol, records
recoverable run status, and projects only visible transcript rows. Terminal
event publication remains owned by `ForegroundLifecycleControl`. Startup also
settles abandoned already-visible running replies without rewriting their text,
receipts or selected branch, and without enabling replay. A per-data-directory
Windows instance handle is acquired before opening the database or recovering
runs; native helper processes and distinct isolated data roots remain separate.

## Foreground Model Factory

The single provider-resolution seam for a Main-Agent foreground turn. It
reuses the existing config-file/environment, keychain, thinking-settings, and
provider-construction helpers, then binds the resolved provider/model to the
session. Its output is a provider, thinking settings, non-secret identity and
context/output limits captured from that same configuration snapshot;
API keys and raw provider/keychain errors never cross the seam. It does not
persist provider profiles, create delegations, schedule workers, or assemble
conversation context.

Main acceptance uses the same requested-effort resolver: an omitted effort
follows the production default, not an implicit test-only `low` override.
Completion review uses that same selection. Explicit low-effort benchmark
results remain a separate condition, not evidence of default-mode behavior.
Natural-learning acceptance keeps exact source snapshots as well as before/after
fingerprints. Test-only file scopes admit both write and edit for an authorized
file, never arbitrary paths; recovery and explicit repair retain old receipts
and do not silently replay unknown operations.

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
The Personal workspace for everyday conversation and affairs that do not need a durable Project. It is not a reduced-capability chat mode and does not own project files.
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
A durable Workspace for a bounded body of user work, whether or not its files form a Git repository. Coding projects may gain Git and worktree capabilities, while non-code projects retain the same Main Agent, memory, permissions, and result-oriented experience.
_Avoid_: repository, coding project (when referring to every Project)

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
A durable user-facing scope: either Personal, which has no project files, or Project, which owns one canonical project root and one visible Main Agent conversation.
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

## Foreground Context Assembler

The single preparation seam for one Main-Agent foreground turn. It assembles
profile/memory configuration, skills, best-effort compaction, visible native
history, workspace references, and bounded identifier-only attention cards. It
does not resolve providers, execute the runner, write terminal state, emit UI
events, or expose diagnostic evidence payloads.

## Foreground Run Store

The transcript/run consistency module for a Main-Agent foreground turn. It
creates hidden provisional assistant parents with matching running task runs,
atomically promotes successful replies with their tool/task protocol, records
recoverable run status, and projects only visible transcript rows. Terminal
event publication remains owned by `ForegroundLifecycleControl`.

## Foreground Model Factory

The single provider-resolution seam for a Main-Agent foreground turn. It
reuses the existing config-file/environment, keychain, thinking-settings, and
provider-construction helpers, then binds the resolved provider/model to the
session. Its output is a provider, thinking settings, and non-secret identity;
API keys and raw provider/keychain errors never cross the seam. It does not
persist provider profiles, create delegations, schedule workers, or assemble
conversation context.

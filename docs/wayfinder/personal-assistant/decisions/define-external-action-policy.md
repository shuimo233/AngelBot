# Define the External Action authorization policy

Status: closed
Type: grilling
Parent: [Personal Assistant Daily-Use Release](../MAP.md)

## Question

Which observable impact classes determine whether an External Action may run
automatically, use Standing Authorization, or require immediate confirmation?

## Resolution

Authorization follows the user's visible execution permission. AngelBot does
not add an unrelated confirmation system for MCP, desktop control, automation,
or connected services. Tool admission, Workspace scope, and execution
permission are separate gates and all three must pass.

### Impact classes

| Class | Examples | Authorization |
| --- | --- | --- |
| Observe | Read an authorized file, search local/project state, inspect project state, list reminders | Runs automatically inside the current capability scope |
| External query | Native Web Search against a configured trusted endpoint | `ask` confirms the outgoing query by default with a redacted semantic summary; enabled capability and trusted-endpoint configuration are separate admission gates |
| Bounded local change | Edit an authorized project file, create a folder, run an admitted project command, prepare a draft | `ask` confirms the exact action; `workspace_auto` covers admitted Workspace changes; `full_access` covers admitted session tools |
| External action | Send a message, create or change a calendar event, upload, publish, submit a form, invoke a mutating MCP tool | `ask` confirms the exact action; `full_access` is explicit session-scoped Standing Authorization and must remain visibly indicated |
| Protected action | Reveal or replace credentials, change security or authorization policy, make a purchase, perform an irreversible deletion, elevate privileges | Always requires immediate confirmation, or remains unsupported when a safe semantic adapter does not exist |

`workspace_auto` is intentionally narrower than `full_access`: it covers
Workspace writes and explicitly enabled Workspace MCP capabilities, not general
desktop or connected-service actions. The user's selection is a persistent
default across Workspaces and may be reduced at any time; each Workspace still
admits its own files, MCP servers, and trusted applications. Delegated workers
never receive this preference; they receive a separate expiring capability
lease and return a candidate or structured result to the Main Agent.

Native Web Search is an external query, not an `Observe` operation. Its Settings
switch and trusted endpoint determine whether the tool is present at all; a
confirmation or broader execution permission cannot revive a disabled or
unconfigured capability. Its completion returns only bounded, structured,
untrusted evidence, never credentials or an unbounded page response.

### Confirmation contract

A confirmation must identify the semantic action, target, affected account or
Workspace, and whether the result is reversible. It must not expose secrets,
raw message bodies, complete file contents, or opaque MCP arguments. Approval is
single-use for the stored call identity; changed arguments require a new
confirmation. Rejection is terminal for that call and must clear the pending
state durably.

Execution success is not inferred from dispatch. Adapters return a structured
outcome (`verified`, `dispatched`, `denied`, `unavailable`, or `failed`) and
attach bounded evidence when verification is possible. An unknown result after
a crash or timeout becomes Attention State and is never silently replayed.

### Product presentation

The composer always shows the active permission. Selecting `full_access` must
explain that admitted local, desktop, MCP, and connected-service actions across
Workspaces may run without another prompt; protected actions are excluded.
Confirmation cards use
`允许一次` and `拒绝`, present a redacted semantic summary, and disappear once a
durable decision is recorded.

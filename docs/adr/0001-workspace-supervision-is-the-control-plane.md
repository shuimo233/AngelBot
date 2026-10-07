# Workspace supervision is the control plane

**Status: accepted.** AngelBot exposes one Main Agent conversation per Workspace, while a durable Workspace Supervisor alone owns delegation, state transitions, and project-write decisions. This rejects user-addressable child agents and a general-purpose shared queue: workers produce bounded evidence or candidates, and the Supervisor serializes the only authority that can change Workspace state.

## Considered Options

- A single global agent queue would blur Personal and Project isolation and make recovery ownership ambiguous.
- User-visible subagent conversations would expose implementation detail and split the relationship with AngelBot.

## Consequences

Per-workspace input steering follows a Pi-style queue, but durable Work Packages, Attempts, leases, and candidates form a separate recoverable state machine. SQLite persists the control plane; in-flight work is inspected after restart rather than blindly resumed.

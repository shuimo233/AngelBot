# Define the daily-use product boundary

Status: closed
Type: grilling
Parent: [Personal Assistant Daily-Use Release](../MAP.md)

## Question

What product boundary distinguishes AngelBot as a broadly usable personal
assistant without turning it into a hosted service, coding-only agent, or
long-horizon autonomous research system?

## Resolution

AngelBot is an open-source, local-first assistant for one person and has no
AngelBot cloud account. Its first fully supported release targets Windows and
presents one continuous Main Agent across Personal and Project Workspaces.

The Daily-Use Release must complete real workflows spanning local files,
applications, browser use, connected services, automation, and focused project
changes. AngelBot is a general personal assistant: email and calendar are the
first connected-service examples, not a privileged vertical or the definition
of the product. Coding and general work are equal product capabilities: Git
worktrees and review remain available where appropriate, but projects need not
be repositories and the interface is organized around user goals and outcomes.

Model access remains provider-independent. Users may bring low-cost cloud API
credentials or compatible local endpoints; routine work should be routable to
economical models and complex work may use a stronger configured tier. No core
workflow may depend on one provider, model name, or proprietary response shape.

AngelBot may observe, prepare, and execute work proactively under revocable,
scoped standing authorization. High-impact external actions remain governed by
the user's action policy, and all results return through the Main Agent.

Tasks are bounded rather than open-ended. Independent tasks may run with limited
concurrency and survive application restart; conflicting writes and side effects
are serialized. The product does not target autonomous multi-day research or
very-large-project management.

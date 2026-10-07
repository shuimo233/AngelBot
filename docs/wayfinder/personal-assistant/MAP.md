# Personal Assistant Daily-Use Release

## Destination

Reach a decision-complete specification for an open-source Windows release that
one person can rely on as a general daily assistant for bounded file,
application, browser, connected-service, automation, and focused project work.
Email and calendar are initial connector examples, not the product identity.
The release must keep one continuous Main Agent, local user ownership, provider
compatibility, controlled proactive work, and recoverable task execution.

## Notes

- Product scope and terminology are governed by [CONTEXT.md](../../../CONTEXT.md).
- Architecture should reuse existing conversation, workspace, supervision,
  isolation, permission, automation, and verification modules before adding new
  machinery.
- The map records decisions, not an implementation backlog. Once the frontier is
  resolved, implementation work is planned against the resulting acceptance
  scenarios and contracts.
- First release target: Windows only. AngelBot has no hosted account or control
  plane.
- Use primary sources for connected-service and Windows-platform research.

## Decisions so far

- [Define the daily-use product boundary](decisions/define-daily-use-product-boundary.md) — AngelBot serves one local owner through one Main Agent as a general Windows assistant; connected services such as email/calendar, low-cost model compatibility, bounded proactive execution, and focused project work are capabilities rather than separate product identities.
- [Select the email and calendar connection strategy](decisions/select-connected-service-strategy.md) — Use provider-native delegated APIs by default, standards-based fallbacks behind the same boundary, desktop PKCE authorization, local token protection, and staged scopes that treat Gmail reading as a release-compliance risk.
- [Define the Windows capability boundary](decisions/define-windows-capability-boundary.md) — Target current Windows 11 x64 through seven narrow platform ports, with user-enabled residency, protected credentials, signed per-user delivery, and deliberately limited desktop automation.
- [Define desktop capability routing](decisions/define-desktop-capability-routing.md) — Reuse the foreground tool surface as the single policy seam; route each task through native file/application capabilities, workspace-scoped MCP, browser semantics, or bounded UI Automation in that order, without exposing a generic shell or coordinate-click tool.
- [Define the External Action authorization policy](decisions/define-external-action-policy.md) — Use the visible session execution permission as the single authorization model, retain an always-confirmed protected class, and require redacted single-use confirmations plus durable outcomes.
- [Define the Daily-Use Release acceptance scenarios](decisions/define-daily-use-acceptance.md) — Gate the release on concrete novice, heavy-user, connected-service, recovery, and installed-Windows journeys rather than feature counts.

## Not yet specified

- Whether user-directed backup should be file-based only or support user-owned
  remote storage in the first release.
- Which extension packaging boundary should follow the first built-in connected
  services without creating a marketplace or AngelBot account dependency.
- Which accessibility and localization guarantees belong to the first stable
  Windows release after the core daily workflows are fixed.

## Out of scope

- Multi-user, team, tenant, or hosted AngelBot account features.
- macOS and Linux release parity before the Windows experience is complete.
- Open-ended autonomous research programmes, multi-day missions, and management
  of very large projects.
- Exposing worker conversations or treating delegated workers as user-facing
  assistants.

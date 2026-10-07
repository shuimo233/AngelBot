# Define desktop capability routing

## Decision

AngelBot will not treat MCP as a universal desktop driver. MCP is one adapter
format inside the existing Main-Agent tool surface. For each requested action,
the foreground surface exposes the narrowest available semantic capability in
this order:

1. Built-in workspace file operation or application-native interface.
2. Fixed URI or fixed-command adapter owned by AngelBot.
3. User-installed, workspace-enabled local MCP tool.
4. Browser semantic automation for web applications.
5. Bounded Windows UI Automation for a configured trusted application.

Raw shell execution, unrestricted PowerShell, arbitrary executable arguments,
screen coordinates, UAC interaction, and automatic privilege elevation are not
general personal-assistant capabilities.

## Existing seams

- `ForegroundToolSurface` remains the single place that assembles capabilities
  for a Main-Agent session and applies workspace-scoped MCP enablement.
- `DesktopAdapter` remains the platform seam for native Windows launch, reveal,
  settings, and verified UI Automation actions.
- `McpProcessManager` remains the adapter for local stdio MCP servers; server
  tools are visible only in workspaces where the user enabled them.
- File paths continue through the canonical workspace resolver before any
  mutating or desktop-visible action.

No parallel `CapabilityRouter` module is introduced while these existing seams
can express the routing policy. The deletion test would otherwise move the same
selection and permission logic back into these modules.

## Permission and result contract

- Read-only inspection may run under the active workspace grant.
- Opening applications, revealing paths, browser interaction, UI Automation,
  and MCP calls remain visible tool activity and follow the user's execution
  permission.
- Sending, submitting, uploading, deleting, installing, purchasing, or changing
  system state always requires a semantic action with an explicit confirmation
  policy; a low-level click is never accepted as a substitute.
- Success means the adapter returned a structured result and, where possible,
  verified the resulting state. Dispatch alone must be labelled `dispatched`,
  not `completed`.

## First implementation slice

- The Main Agent can reveal an existing item from the active workspace in
  Windows File Explorer; canonical path validation rejects traversal and
  junction/symlink escape.
- Trusted application setup uses the Windows file picker instead of requiring a
  manually typed executable path.
- Trusted application tool schemas contain only the enabled application IDs and
  disappear when no matching capability is configured, reducing invalid calls
  and prompt cost.

## Evidence

The primary-source comparison and follow-on constraints are recorded in
[Windows application control research](../../../research/windows-application-control.md).

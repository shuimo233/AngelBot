# Capabilities are leased and mediated

**Status: accepted.** Tool, MCP, skill, filesystem, network, and automation authority is issued as the minimum expiring Capability Lease needed by a Work Package or fixed Automation Definition. A Tool Capability Gateway constructs the allowed surface and normalizes untrusted tool output; neither worker text nor tool output may expand authority or trigger new delegation.

## Considered Options

- A persistent `full_access` switch makes scope drift and MCP updates unsafe.
- Treating MCP registration as authorization confuses availability with permission.

## Consequences

Workspace enablement and task leases are separate. Tool manifest or command changes revoke the affected workspace grant until renewed approval; all project mutations, including MCP-originated ones, enter the Candidate Change path.

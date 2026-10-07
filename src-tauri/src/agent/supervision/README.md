# Workspace supervision

This directory is the single control-plane module for a Workspace. Its public
interface accepts Workspace input and control actions, persists scheduling
state, emits safe Activity Projections, and delegates through the existing
`delegation/`, `changes/`, and `isolation/` adapters.

It must not host worker prompts, direct filesystem writes, MCP execution, or
React/Tauri transport concerns. Those remain behind their existing seams.

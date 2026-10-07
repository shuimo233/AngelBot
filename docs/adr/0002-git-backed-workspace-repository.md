# Project mutation uses a Git-backed Workspace Repository

**Status: accepted.** Every Project Workspace that AngelBot may mutate uses a Git-backed Workspace Repository, including non-code projects. Existing Git projects retain their own workflow; a non-Git project can opt into an application-managed Git base whose metadata, worktrees, checkpoints, and AngelBot records live outside the project root except for the required standard `.git` pointer.

## Considered Options

- Direct writes in a selected project directory are simpler but cannot isolate concurrent candidates or provide reliable rollback.
- AngelBot-specific files in every project root would make the project noisy and leak product internals into user material.

## Consequences

Candidate work occurs in isolated worktrees from private checkpoint refs. Materialization checks a State Anchor and is serialized per Workspace; drift triggers re-read, rebase, and review rather than a silent overwrite.

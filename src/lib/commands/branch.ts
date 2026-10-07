/**
 * Session Branching Commands (Issue #41)
 * 
 * Provides commands for creating, listing, switching, and merging session branches.
 */

import { invoke } from '../invoke';

/**
 * Branch information
 */
export interface BranchInfo {
  id: string;
  session_id: string;
  parent_branch_id: string | null;
  name: string;
  description: string | null;
  created_at: number;
  updated_at: number;
  is_active: boolean;
  message_count: number;
}

/**
 * Session with branch info and children
 */
export interface SessionWithBranch {
  id: string;
  title: string;
  parent_id: string | null;
  branch_name: string | null;
  branch_id: string | null;
  created_at: number;
  updated_at: number;
  message_count: number;
  children: SessionWithBranch[];
}

/**
 * Create a new branch from an existing session
 */
export async function createSessionBranch(
  sessionId: string,
  branchName: string,
  description?: string
): Promise<BranchInfo> {
  return invoke<BranchInfo>('create_session_branch', {
    sessionId,
    branchName,
    description,
  });
}

/**
 * Get all branches for a session
 */
export async function getSessionBranches(sessionId: string): Promise<BranchInfo[]> {
  return invoke<BranchInfo[]>('get_session_branches', { sessionId });
}

/**
 * Get the branch tree for a session
 */
export async function getSessionBranchTree(sessionId: string): Promise<SessionWithBranch> {
  return invoke<SessionWithBranch>('get_session_branch_tree', { sessionId });
}

/**
 * Switch to a different branch
 */
export async function switchToBranch(branchId: string): Promise<string> {
  return invoke<string>('switch_to_branch', { branchId });
}

/**
 * Delete a branch (keeps messages)
 */
export async function deleteBranch(branchId: string): Promise<void> {
  return invoke<void>('delete_branch', { branchId });
}

/**
 * Get all branches across all sessions
 */
export async function getAllBranches(): Promise<BranchInfo[]> {
  return invoke<BranchInfo[]>('get_all_branches');
}

/**
 * Format branch name for display
 */
export function formatBranchName(branch: BranchInfo): string {
  return branch.name;
}

/**
 * Get branch status indicator
 */
export function getBranchStatus(branch: BranchInfo): 'active' | 'inactive' {
  return branch.is_active ? 'active' : 'inactive';
}

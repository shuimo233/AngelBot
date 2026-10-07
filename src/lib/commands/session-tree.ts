/**
 * Session Tree Commands (Message-Level Branching)
 *
 * Provides commands for managing message-level branches within a session.
 */

import { invoke } from '../invoke';

/**
 * A message node in the branch tree
 */
export interface MessageNode {
  id: string;
  role: 'user' | 'assistant' | 'tool';
  content: string;
  parent_id: string | null;
  created_at: number;
}

/**
 * A branch in the session tree
 */
export interface Branch {
  id: string;
  message_id: string;
  name: string | null;
  is_active: boolean;
}

/**
 * The full branch tree for a session
 */
export interface BranchTree {
  branches: Branch[];
  messages: MessageNode[];
}

/**
 * Get the full branch tree for a session
 */
export async function getBranchTree(sessionId: string): Promise<BranchTree> {
  return invoke<BranchTree>('get_branch_tree', { sessionId });
}

/**
 * Switch to a different branch (change leaf pointer)
 */
export async function switchToMessageBranch(
  sessionId: string,
  branchMessageId: string
): Promise<void> {
  return invoke<void>('switch_to_message_branch', {
    sessionId,
    branch_message_id: branchMessageId,
  });
}

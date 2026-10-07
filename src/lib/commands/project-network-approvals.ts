import { invoke } from '$lib/invoke';

/**
 * A deliberately narrow, user-safe projection of one project network approval.
 *
 * The opaque reference is only sent back to the backend when revoking an
 * approval. It must never be displayed, logged, or used as a project identity.
 */
export interface ProjectNetworkApproval {
  approvalRef: string;
  grantedAt: number;
  hosts: string[];
  actions: Array<'search' | 'fetch'>;
  maxResponseBytes: number;
  maxRedirects: number;
}

export interface ProjectNetworkApprovalList {
  approvals: ProjectNetworkApproval[];
}

export interface ProjectNetworkApprovalRevocation {
  status: 'revoked' | 'already_inactive';
}

/** Read active delegated-network approvals for one trusted project workspace. */
export function getProjectNetworkApprovals(
  workspaceId: string,
): Promise<ProjectNetworkApprovalList> {
  return invoke<ProjectNetworkApprovalList>('get_project_network_approvals', { workspaceId });
}

/**
 * Remove an approval from one trusted project workspace.
 *
 * Revocation only reduces capability, so the user action is applied directly;
 * the backend makes repeated clicks safe and idempotent.
 */
export function revokeProjectNetworkApproval(
  workspaceId: string,
  approvalRef: string,
): Promise<ProjectNetworkApprovalRevocation> {
  return invoke<ProjectNetworkApprovalRevocation>('revoke_project_network_approval', {
    workspaceId,
    approvalRef,
  });
}

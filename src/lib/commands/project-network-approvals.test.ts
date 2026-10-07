import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '$lib/invoke';
import {
  getProjectNetworkApprovals,
  revokeProjectNetworkApproval,
  type ProjectNetworkApprovalList,
} from './project-network-approvals';

vi.mock('$lib/invoke', () => ({ invoke: vi.fn() }));

describe('Project network approval command boundary', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it('reads only the current project workspace approvals', async () => {
    const result: ProjectNetworkApprovalList = { approvals: [] };
    vi.mocked(invoke).mockResolvedValue(result);

    await expect(getProjectNetworkApprovals('project-1')).resolves.toEqual(result);

    expect(invoke).toHaveBeenCalledWith('get_project_network_approvals', {
      workspaceId: 'project-1',
    });
  });

  it('uses the opaque approval reference only for the revoke command', async () => {
    vi.mocked(invoke).mockResolvedValue({ status: 'revoked' });

    await expect(revokeProjectNetworkApproval('project-1', 'opaque-approval-ref')).resolves.toEqual({
      status: 'revoked',
    });

    expect(invoke).toHaveBeenCalledWith('revoke_project_network_approval', {
      workspaceId: 'project-1',
      approvalRef: 'opaque-approval-ref',
    });
  });
});

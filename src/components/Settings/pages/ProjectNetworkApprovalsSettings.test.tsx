import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  getProjectNetworkApprovals,
  revokeProjectNetworkApproval,
  type ProjectNetworkApproval,
} from '$lib/commands/project-network-approvals';
import { useWorkspacesStore } from '$stores/workspaces';
import { ProjectNetworkApprovalsSettings } from './ProjectNetworkApprovalsSettings';

vi.mock('$lib/commands/project-network-approvals', () => ({
  getProjectNetworkApprovals: vi.fn(),
  revokeProjectNetworkApproval: vi.fn(),
}));

const approval: ProjectNetworkApproval = {
  approvalRef: 'opaque-policy-reference-that-must-not-render',
  grantedAt: 1_725_000_000,
  hosts: ['docs.example.test', 'api.example.test'],
  actions: ['search', 'fetch'],
  maxResponseBytes: 524_288,
  maxRedirects: 3,
};

describe('ProjectNetworkApprovalsSettings', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getProjectNetworkApprovals).mockResolvedValue({ approvals: [] });
    vi.mocked(revokeProjectNetworkApproval).mockResolvedValue({ status: 'revoked' });
  });

  it('explains that personal space has no project approval scope without fetching one', () => {
    useWorkspacesStore.setState({
      workspaces: [{ id: 'personal', name: 'AngelBot 日常', kind: 'personal', createdAt: 1, updatedAt: 1, activeSessionId: 'session-1' }],
      activeWorkspaceId: 'personal', loading: false, lastError: null,
    });

    render(<ProjectNetworkApprovalsSettings />);

    expect(screen.getByText('个人空间没有项目联网授权')).toBeInTheDocument();
    expect(getProjectNetworkApprovals).not.toHaveBeenCalled();
  });

  it('renders only the safe approval projection for the active project', async () => {
    useWorkspacesStore.setState({
      workspaces: [{ id: 'project-1', name: 'AngelBot', kind: 'project', rootPath: 'D:\\Projects\\AngelBot', createdAt: 1, updatedAt: 1, activeSessionId: 'session-1' }],
      activeWorkspaceId: 'project-1', loading: false, lastError: null,
    });
    const backendOnlyFields = {
      ...approval,
      rawQuery: 'never render this query',
      fullUrl: 'https://credentials.example.test/search?q=never-render',
      credential: 'never-render-secret',
    };
    vi.mocked(getProjectNetworkApprovals).mockResolvedValue({ approvals: [backendOnlyFields] });

    render(<ProjectNetworkApprovalsSettings />);

    expect(await screen.findByText('docs.example.test')).toBeInTheDocument();
    expect(screen.getByText('api.example.test')).toBeInTheDocument();
    expect(screen.getByText('搜索、读取')).toBeInTheDocument();
    expect(screen.getByText('512 KB')).toBeInTheDocument();
    expect(screen.getByText('3 次')).toBeInTheDocument();
    expect(screen.queryByText(approval.approvalRef)).not.toBeInTheDocument();
    expect(screen.queryByText('never render this query')).not.toBeInTheDocument();
    expect(screen.queryByText('https://credentials.example.test/search?q=never-render')).not.toBeInTheDocument();
    expect(screen.queryByText('never-render-secret')).not.toBeInTheDocument();
    expect(getProjectNetworkApprovals).toHaveBeenCalledWith('project-1');
  });

  it('reloads the approvals for the newly selected project', async () => {
    const secondApproval = { ...approval, approvalRef: 'opaque-second-ref', hosts: ['status.example.test'] };
    useWorkspacesStore.setState({
      workspaces: [
        { id: 'project-1', name: 'First', kind: 'project', createdAt: 1, updatedAt: 1, activeSessionId: 'session-1' },
        { id: 'project-2', name: 'Second', kind: 'project', createdAt: 1, updatedAt: 1, activeSessionId: 'session-2' },
      ],
      activeWorkspaceId: 'project-1', loading: false, lastError: null,
    });
    vi.mocked(getProjectNetworkApprovals)
      .mockResolvedValueOnce({ approvals: [approval] })
      .mockResolvedValueOnce({ approvals: [secondApproval] });

    render(<ProjectNetworkApprovalsSettings />);
    expect(await screen.findByText('docs.example.test')).toBeInTheDocument();

    act(() => useWorkspacesStore.setState({ activeWorkspaceId: 'project-2' }));

    expect(await screen.findByText('status.example.test')).toBeInTheDocument();
    expect(getProjectNetworkApprovals).toHaveBeenLastCalledWith('project-2');
  });

  it('revokes from the active project and removes the approval immediately', async () => {
    const user = userEvent.setup();
    useWorkspacesStore.setState({
      workspaces: [{ id: 'project-1', name: 'AngelBot', kind: 'project', createdAt: 1, updatedAt: 1, activeSessionId: 'session-1' }],
      activeWorkspaceId: 'project-1', loading: false, lastError: null,
    });
    vi.mocked(getProjectNetworkApprovals).mockResolvedValue({ approvals: [approval] });

    render(<ProjectNetworkApprovalsSettings />);

    await user.click(await screen.findByRole('button', { name: '撤销 docs.example.test 的联网授权' }));

    await waitFor(() => expect(revokeProjectNetworkApproval).toHaveBeenCalledWith(
      'project-1',
      approval.approvalRef,
    ));
    expect(screen.getByText('尚无联网授权。需要联网探索时，AngelBot 会先向你说明要访问的范围。')).toBeInTheDocument();
  });
});

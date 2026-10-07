import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { WorkspaceActivityCapsule } from '$components/WorkspaceActivityCapsule';
import { useWorkspaceActivityStore } from '$stores/workspaceActivity';
import { useWorkspacesStore } from '$stores/workspaces';

describe('WorkspaceActivityCapsule', () => {
  beforeEach(() => {
    useWorkspacesStore.setState({
      workspaces: [{ id: 'workspace-1', name: 'Demo', kind: 'project', rootPath: 'D:\\Demo', createdAt: 1, updatedAt: 1, activeSessionId: 'internal-1' }],
      activeWorkspaceId: 'workspace-1',
      loading: false,
      lastError: null,
    });
    useWorkspaceActivityStore.setState({
      workspaceId: 'workspace-1',
      isLoading: false,
      error: null,
      projection: {
        workspaceId: 'workspace-1',
        cursor: 1,
        pendingDecisions: [],
        attention: null,
        work: [{
          id: 'delegation-1',
          goal: '检查委派运行时',
          status: 'running',
          summary: null,
          keyFacts: [],
          milestonesCompleted: 0,
          milestonesTotal: 0,
          verificationsPassed: 0,
          verificationsTotal: 0,
          evidenceCount: 0,
          activity: '正在执行',
          reviewerStatus: null,
          updatedAt: 1,
        }],
      },
      refresh: vi.fn().mockResolvedValue(undefined),
    });
  });

  it('shows a trusted stage without an estimated completion percentage', async () => {
    render(<WorkspaceActivityCapsule />);

    expect(screen.getByText('检查委派运行时')).toBeInTheDocument();
    expect(screen.getByText('正在执行')).toBeInTheDocument();
    expect(screen.queryByText(/\d+%/)).not.toBeInTheDocument();
    await waitFor(() => {
      expect(useWorkspaceActivityStore.getState().refresh).toHaveBeenCalledWith('workspace-1');
    });
  });

  it('does not project a previous workspace’s delegated activity', () => {
    useWorkspacesStore.setState({ activeWorkspaceId: 'workspace-2' });

    render(<WorkspaceActivityCapsule />);

    expect(screen.queryByText('检查委派运行时')).not.toBeInTheDocument();
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('keeps a recoverable activity-read failure visible in the main conversation', () => {
    const refresh = vi.fn().mockResolvedValue(undefined);
    useWorkspaceActivityStore.setState({
      projection: null,
      isLoading: false,
      error: '任务状态暂时无法读取',
      refresh,
    });

    render(<WorkspaceActivityCapsule />);

    expect(screen.getByRole('alert')).toHaveTextContent('无法读取任务状态');
    expect(screen.getByRole('alert')).toHaveTextContent('任务状态暂时无法读取');
    fireEvent.click(screen.getByRole('button', { name: '重新检查' }));
    expect(refresh).toHaveBeenLastCalledWith('workspace-1');
  });

  it('surfaces a Main-Agent confirmation summary without child controls', () => {
    useWorkspaceActivityStore.setState({
      projection: {
        workspaceId: 'workspace-1',
        cursor: 2,
        work: [],
        attention: null,
        pendingDecisions: [{
          workId: 'delegation-2',
          goal: '审校后的变更',
          questions: ['是否将审校通过的候选修改写入工作目录？'],
          risks: ['会改变一个文件'],
          updatedAt: 2,
        }],
      },
    });

    render(<WorkspaceActivityCapsule />);

    expect(screen.getByText('审校后的变更')).toBeInTheDocument();
    expect(screen.getByText('是否将审校通过的候选修改写入工作目录？')).toBeInTheDocument();
    expect(screen.getByText('风险：会改变一个文件')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /允许|拒绝/i })).not.toBeInTheDocument();
  });

  it('reuses the same projection as a persistent workbench view with factual progress only', () => {
    useWorkspaceActivityStore.setState((state) => ({
      projection: state.projection && {
        ...state.projection,
        work: [{
          ...state.projection.work[0],
          milestonesCompleted: 2,
          milestonesTotal: 4,
          verificationsPassed: 1,
          verificationsTotal: 2,
          evidenceCount: 3,
        }],
      },
    }));

    render(<WorkspaceActivityCapsule mode="panel" />);

    expect(screen.getByRole('heading', { name: '任务与委派' })).toBeInTheDocument();
    expect(screen.getByText('步骤 2/4')).toBeInTheDocument();
    expect(screen.getByText('验证 1/2')).toBeInTheDocument();
    expect(screen.getByText('证据 3')).toBeInTheDocument();
    expect(screen.queryByText(/%/)).not.toBeInTheDocument();
  });

  it('shows a reviewer decision state instead of masking it as a Main-Agent summary wait', () => {
    useWorkspaceActivityStore.setState((state) => ({
      projection: state.projection && {
        ...state.projection,
        work: [{
          ...state.projection.work[0],
          status: 'awaitingSummary',
          activity: '等待主 Agent 汇总',
          reviewerStatus: 'needs_decision',
        }],
      },
    }));

    render(<WorkspaceActivityCapsule mode="panel" />);

    expect(screen.getByText('独立审校需要判断')).toBeInTheDocument();
    expect(screen.queryByText('等待主 Agent 汇总')).not.toBeInTheDocument();
  });

  it.each([
    ['completed', '已完成'],
    ['failed', '未完成'],
    ['cancelled', '已取消'],
  ] as const)('shows terminal status %s instead of an earlier reviewer stage', (status, label) => {
    useWorkspaceActivityStore.setState((state) => ({
      projection: state.projection && {
        ...state.projection,
        work: [{
          ...state.projection.work[0],
          status,
          activity: '正在整理结果',
          reviewerStatus: 'passed',
        }],
      },
    }));

    render(<WorkspaceActivityCapsule mode="panel" />);

    expect(screen.getByText(label)).toBeInTheDocument();
    expect(screen.queryByText('独立审校已通过')).not.toBeInTheDocument();
    expect(screen.queryByText('独立审校已通过，等待主 Agent 处理')).not.toBeInTheDocument();
    expect(screen.queryByText('正在整理结果')).not.toBeInTheDocument();
  });

  it('keeps the workbench useful when the workspace has no delegated work', () => {
    useWorkspaceActivityStore.setState({
      projection: { workspaceId: 'workspace-1', cursor: 0, work: [], pendingDecisions: [], attention: null },
    });

    render(<WorkspaceActivityCapsule mode="panel" />);

    expect(screen.getByText('当前没有进行中的工作')).toBeInTheDocument();
    expect(screen.getByText(/委派、审校、待确认和需要继续处理/)).toBeInTheDocument();
  });

  it('surfaces recoverable Main-Agent work without exposing a retry control', () => {
    const onReturnToConversation = vi.fn();
    useWorkspaceActivityStore.setState({
      projection: {
        workspaceId: 'workspace-1',
        cursor: 3,
        work: [],
        pendingDecisions: [],
        attention: {
          openCount: 2,
          message: '有工作需要继续处理，进度已安全保留。请回到主对话说明下一步。',
        },
      },
    });

    const { rerender } = render(<WorkspaceActivityCapsule />);

    expect(screen.getByText('AngelBot 有 2 项工作需要继续处理')).toBeInTheDocument();
    expect(screen.getByText('进度已安全保留，可直接在对话中继续。')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /继续|重试/i })).not.toBeInTheDocument();

    rerender(<WorkspaceActivityCapsule mode="panel" onReturnToConversation={onReturnToConversation} />);

    expect(screen.getByRole('heading', { name: '需要继续处理（2）' })).toBeInTheDocument();
    expect(screen.getByText('有工作需要继续处理，进度已安全保留。请回到主对话说明下一步。')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '回到对话' }));
    expect(onReturnToConversation).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('button', { name: /继续|重试/i })).not.toBeInTheDocument();
  });
});

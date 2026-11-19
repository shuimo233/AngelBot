import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { WorkspaceActivityCapsule } from '$components/WorkspaceActivityCapsule';
import * as workspaceActivityCommands from '$lib/commands/workspace-activity';
import { useMessagesStore } from '$stores/messages';
import { useWorkspaceActivityStore } from '$stores/workspaceActivity';
import { useWorkspacesStore } from '$stores/workspaces';
import type { Message, ToolCall, ToolResult } from '$types';

describe('WorkspaceActivityCapsule', () => {
  beforeEach(() => {
    useMessagesStore.setState({ messages: [] });
    useWorkspacesStore.setState({
      workspaces: [{ id: 'workspace-1', name: 'Demo', kind: 'project', rootPath: 'D:\\Demo', createdAt: 1, updatedAt: 1, activeSessionId: 'internal-1' }],
      activeWorkspaceId: 'workspace-1',
      loading: false,
      lastError: null,
    });
    useWorkspaceActivityStore.setState({
      workspaceId: 'workspace-1',
      sessionId: 'internal-1',
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
      observe: vi.fn().mockReturnValue(() => {}),
    });
  });

  it('shows a trusted stage without an estimated completion percentage', async () => {
    render(<WorkspaceActivityCapsule />);

    expect(screen.getByText('检查委派运行时')).toBeInTheDocument();
    expect(screen.getByText('正在执行')).toBeInTheDocument();
    expect(screen.queryByText(/\d+%/)).not.toBeInTheDocument();
    await waitFor(() => {
      expect(useWorkspaceActivityStore.getState().observe).toHaveBeenCalledWith('workspace-1', 'internal-1');
    });
  });

  it('shows an unknown delegation without implying execution or reusing an old stage', () => {
    const projection = useWorkspaceActivityStore.getState().projection!;
    const [running] = projection.work;
    const unknown = {
      ...running, id: 'delegation-unknown', goal: '核对资料归档', status: 'unknown' as const,
      activity: '正在整理结果', reviewerStatus: 'passed',
    };
    useWorkspaceActivityStore.setState({ projection: { ...projection, work: [unknown] } });
    const view = render(<WorkspaceActivityCapsule />);

    expect(screen.getByText('AngelBot 有 1 项工作状态未确认')).toBeInTheDocument();
    expect(screen.getByText('核对资料归档')).toBeInTheDocument();
    expect(screen.getByText('状态未确认')).toBeInTheDocument();
    expect(screen.queryByText(/正在处理|正在整理结果|独立审校已通过/)).not.toBeInTheDocument();

    act(() => useWorkspaceActivityStore.setState({ projection: { ...projection, work: [running, unknown] } }));
    expect(screen.getByText('AngelBot 正在处理 1 项工作')).toBeInTheDocument();
    expect(screen.getByText('另有 1 项工作状态未确认')).toBeInTheDocument();

    view.rerender(<WorkspaceActivityCapsule mode="panel" />);
    expect(screen.getByText('状态未确认')).toBeInTheDocument();
    expect(screen.getByText(/无法确认这项工作是否仍在执行/)).toBeInTheDocument();
    expect(screen.queryByText(/正在整理结果|独立审校已通过|结果待核对/)).not.toBeInTheDocument();
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
    ['needsDecision', '需要处理'],
  ] as const)('shows settled or blocked status %s instead of an earlier execution stage', (status, label) => {
    useWorkspaceActivityStore.setState((state) => ({
      projection: state.projection && {
        ...state.projection,
        work: [{
          ...state.projection.work[0],
          status,
          activity: '正在整理结果',
          reviewerStatus: status === 'needsDecision' ? null : 'passed',
          summary: status === 'needsDecision' ? '委派未能启动：此次任务的模型连接不可用，尚未形成可交付结果。' : null,
        }],
        pendingDecisions: status === 'needsDecision' ? [{
          workId: 'delegation-1', goal: '检查委派运行时',
          questions: ['委派未能启动：此次任务的模型连接不可用，尚未形成可交付结果。', '核对配置后再决定是否重新派发；没有自动重试。'],
          risks: [], updatedAt: 2,
        }] : [],
      },
    }));

    const view = render(<WorkspaceActivityCapsule mode="panel" />);

    expect(screen.getByText(label)).toBeInTheDocument();
    expect(screen.queryByText('独立审校已通过')).not.toBeInTheDocument();
    expect(screen.queryByText('独立审校已通过，等待主 Agent 处理')).not.toBeInTheDocument();
    expect(screen.queryByText('正在整理结果')).not.toBeInTheDocument();
    if (status === 'needsDecision') {
      expect(screen.getByText('委派未能启动：此次任务的模型连接不可用，尚未形成可交付结果。')).toBeInTheDocument();
      view.unmount();
      render(<WorkspaceActivityCapsule />);
      expect(screen.getByText('委派未能启动：此次任务的模型连接不可用，尚未形成可交付结果。')).toBeInTheDocument();
      expect(screen.getByText(/没有自动重试/)).toBeInTheDocument();
      expect(screen.queryByText('正在整理结果')).not.toBeInTheDocument();
    }
  });

  it('keeps the workbench useful when the workspace has no delegated work', () => {
    useWorkspaceActivityStore.setState({
      projection: { workspaceId: 'workspace-1', cursor: 0, work: [], pendingDecisions: [], attention: null },
    });

    render(<WorkspaceActivityCapsule mode="panel" />);

    expect(screen.getByText('当前没有进行中的工作')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '刷新' })).toBeEnabled();
  });

  it('shows a scoped unresolved result from a durable task receipt without changing aggregate attention', () => {
    const action: ToolCall = { id: 'external-action', name: 'run_project_command', arguments: '{}' };
    const pending: Message = {
      id: 'pending-result', sessionId: 'internal-1', role: 'assistant', content: '', createdAt: 1,
      taskRun: { id: 'run-1', goal: '准备项目报告', status: 'needs_attention', plan: [], confirmationState: 'none', resumable: false, stepCount: 1, completedStepCount: 0 },
      toolCalls: [action], toolResults: [{ callId: action.id, toolName: action.name, success: false, output: 'Error: {"code":"RESULT_UNKNOWN","detail":"PRIVATE"}' }],
    };
    const completed: Message = {
      ...pending, id: 'another-reply', createdAt: 2, toolCalls: [], toolResults: [],
      taskRun: { ...pending.taskRun!, id: 'another-run', status: 'completed' },
    };
    const projection = useWorkspaceActivityStore.getState().projection!;
    useWorkspaceActivityStore.setState({ projection: { ...projection, work: [], attention: { openCount: 1, message: '另有旧记录' } } });
    useMessagesStore.setState({ messages: [{ ...pending, id: 'foreign', sessionId: 'another-session', taskRun: { ...pending.taskRun!, goal: '其他项目目标' } }] });
    const returnToResult = vi.fn();
    render(<WorkspaceActivityCapsule mode="panel" onReturnToResultReview={returnToResult} />);
    expect(screen.queryByRole('button', { name: '回对话核对结果' })).not.toBeInTheDocument();

    act(() => useMessagesStore.setState({ messages: [pending, completed] }));
    fireEvent.click(screen.getByRole('button', { name: '回对话核对结果' }));
    expect(returnToResult).toHaveBeenCalledWith(pending.id);
    expect(screen.getByText('准备项目报告')).toBeInTheDocument();
    expect(screen.queryByText(/请先核对实际结果/)).not.toBeInTheDocument();
    expect(screen.queryByText(/PRIVATE/)).not.toBeInTheDocument();
    expect(screen.getByText('另有旧记录')).toBeInTheDocument();

    act(() => useMessagesStore.setState({ messages: [pending, { ...completed, taskRun: { ...completed.taskRun!, id: pending.taskRun!.id } }] }));
    expect(screen.queryByRole('button', { name: '回对话核对结果' })).not.toBeInTheDocument();
    expect(screen.getByText('另有旧记录')).toBeInTheDocument();
  });

  it('removes the redundant task shortcut while the workbench is open', () => {
    const projection = useWorkspaceActivityStore.getState().projection!;
    useWorkspaceActivityStore.setState({
      projection: { ...projection, work: [{ ...projection.work[0], status: 'unknown' }] },
    });
    const onOpenTasks = vi.fn();
    const view = render(<WorkspaceActivityCapsule reviewInConversation onOpenTasks={onOpenTasks} />);
    fireEvent.click(screen.getByRole('button', { name: /查看任务状态/ }));
    expect(onOpenTasks).toHaveBeenCalledOnce();
    view.rerender(<WorkspaceActivityCapsule reviewInConversation onOpenTasks={onOpenTasks} workbenchOpen />);
    expect(screen.queryByRole('button', { name: /查看任务状态/ })).not.toBeInTheDocument();
  });

  it('shows ordered file receipts from the completed chat and its approval continuations', () => {
    useWorkspaceActivityStore.setState({
      projection: { workspaceId: 'workspace-1', cursor: 0, work: [], pendingDecisions: [], attention: null },
    });
    const call = (id: string, name: string, path: string): ToolCall => ({ id, name, arguments: JSON.stringify({ path, content: 'PRIVATE_BODY' }) });
    const receipt = (tool: ToolCall, confirmationStatus?: string): ToolResult => ({
      callId: tool.id, toolName: tool.name, success: confirmationStatus !== 'pending', confirmationStatus, output: 'PRIVATE_OUTPUT',
    });
    const readSource = call('read-source', 'read_file', 'orders.csv');
    const readRules = call('read-rules', 'read_file', 'rules.md');
    const writeJson = call('write-json', 'write_file', 'summary.json');
    const writeReport = call('write-report', 'write_file', 'summary.md');
    const readReport = call('read-report', 'read_file', 'summary.md');
    const readJson = call('read-json', 'read_file', 'summary.json');
    const turn = (id: string, parentId: string | undefined, status: string, toolCalls: ToolCall[], toolResults: ToolResult[]): Message => ({
      id, parentId, sessionId: 'internal-1', role: 'assistant', content: '', createdAt: 1, toolCalls, toolResults,
      taskRun: { id, goal: '核算订单', status, plan: [], confirmationState: 'none', resumable: false, stepCount: toolCalls.length, completedStepCount: toolCalls.length },
    });
    const unrelated = call('other', 'write_file', 'another-project.md');
    useMessagesStore.setState({ messages: [
      turn('first', undefined, 'continue_suggested', [readSource, readRules, writeJson], [receipt(readSource), receipt(readRules), receipt(writeJson, 'approved'), receipt(writeJson, 'pending')]),
      turn('second', 'first', 'continue_suggested', [writeReport], [receipt(writeReport, 'approved')]),
      turn('final', 'second', 'completed', [readReport, readJson], [receipt(readReport), receipt(readJson)]),
      { ...turn('other-project', undefined, 'completed', [unrelated], [receipt(unrelated)]), sessionId: 'another-session' },
    ] });
    render(<WorkspaceActivityCapsule mode="panel" />);
    fireEvent.click(screen.getByText('最近完成的文件任务'));
    const rows = screen.getAllByRole('listitem');
    expect(rows).toHaveLength(6);
    for (const [index, label] of ['读取 orders.csv', '读取 rules.md', '写入 summary.json', '写入 summary.md', '读回 summary.md', '读回 summary.json'].entries()) {
      expect(rows[index]).toHaveTextContent(label);
    }
    expect(rows[2]).toHaveTextContent('批准后完成');
    expect(rows[3]).toHaveTextContent('批准后完成');
    expect(screen.queryByText(/PRIVATE_|another-project/)).not.toBeInTheDocument();
    expect(screen.queryByText('当前没有进行中的工作')).not.toBeInTheDocument();
  });

  it('keeps known work visible during a background refresh and labels stale results on failure', () => {
    useWorkspaceActivityStore.setState({ isLoading: true });
    const view = render(<WorkspaceActivityCapsule />);
    expect(screen.getByText('检查委派运行时')).toBeInTheDocument();
    expect(screen.queryByText('正在同步任务状态')).not.toBeInTheDocument();
    act(() => { useWorkspaceActivityStore.setState({ isLoading: false, error: '临时不可用' }); });
    view.rerender(<WorkspaceActivityCapsule mode="panel" />);
    expect(screen.getByText('检查委派运行时')).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('上次记录，不代表任务仍在执行');
    view.rerender(<WorkspaceActivityCapsule />);
    expect(screen.getByText(/上次记录：AngelBot/)).toBeInTheDocument();
  });

  it('keeps historical Main work in the workbench rather than a permanent composer card', () => {
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

    const { container, rerender } = render(<WorkspaceActivityCapsule />);

    expect(container).toBeEmptyDOMElement();
    expect(screen.queryByRole('button', { name: /继续|重试/i })).not.toBeInTheDocument();

    rerender(<WorkspaceActivityCapsule mode="panel" onReturnToConversation={onReturnToConversation} />);

    expect(screen.getByRole('heading', { name: '需要继续处理（2）' })).toBeInTheDocument();
    expect(screen.getByText('有工作需要继续处理，进度已安全保留。请回到主对话说明下一步。')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '回到对话' }));
    expect(onReturnToConversation).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('button', { name: /继续|重试/i })).not.toBeInTheDocument();
  });

  it('dismisses only the displayed failure reminder and refreshes the workbench badge source', async () => {
    const dismiss = vi.spyOn(workspaceActivityCommands, 'dismissWorkspaceTaskReminders').mockResolvedValue(undefined);
    try {
      const { projection, refresh } = useWorkspaceActivityStore.getState();
      useWorkspaceActivityStore.setState({
        projection: { ...projection!, work: [], attention: {
          openCount: 1, message: '旧记录', retryReminderIds: ['attn_previous'],
        } },
      });
      render(<WorkspaceActivityCapsule mode="panel" />);
      expect(screen.getByRole('heading', { name: '执行失败提醒（1）' })).toBeVisible();
      fireEvent.click(screen.getByRole('button', { name: '不再提醒' }));
      expect(dismiss).toHaveBeenCalledWith('workspace-1', 'internal-1', ['attn_previous']);
      await waitFor(() => expect(refresh).toHaveBeenCalledWith('workspace-1'));
      await waitFor(() => expect(screen.getByRole('button', { name: '不再提醒' })).toBeEnabled());
    } finally {
      dismiss.mockRestore();
    }
  });
});

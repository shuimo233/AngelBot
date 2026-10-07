import { render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ContextDrawerContent } from './ContextDrawerContent';
import { useMessagesStore } from '$stores/messages';
import { useSessionsStore } from '$stores/sessions';
import { useSettingsStore } from '$stores/settings';
import { getContextUsage } from '$lib/commands/session';
import type { Message } from '$types';

vi.mock('$lib/commands/session', () => ({
  getContextUsage: vi.fn(),
}));

const makeSession = (id: string, workDir?: string) => ({
  id,
  title: 'Demo',
  createdAt: Date.now(),
  updatedAt: Date.now(),
  contextVersion: 0,
  workDir,
});

const makeAssistantMessage = (overrides: Partial<Message>): Message => ({
  id: 'msg-1',
  sessionId: 'session-1',
  role: 'assistant',
  content: '好的',
  createdAt: Date.now(),
  ...overrides,
});

describe('ContextDrawerContent', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.mocked(getContextUsage).mockResolvedValue({
      activeMessageCount: 2,
      totalMessageCount: 2,
      estimatedTokens: 1200,
      contextLimit: 8192,
      isCompressed: false,
      measurementSource: 'local_estimate',
    });
    useMessagesStore.setState((state) => ({
      ...state,
      messages: [],
      toolExecuting: false,
      executingTool: null,
      liveStatus: '',
      toolProgress: {},
      liveToolSteps: {},
    }));
    useSessionsStore.setState({ sessions: [], activeSessionId: null, activeSession: null });
    useSettingsStore.setState({
      activeApiConfig: { provider: 'anthropic', model: 'claude-sonnet-4-20250514', baseUrl: '', apiKey: '', maxTokens: 4096, temperature: 0.7 },
      apiConfigLoaded: true,
    });
  });

  it('renders all five sections with quiet empty states when the round has no agent activity', () => {
    render(<ContextDrawerContent onClose={vi.fn()} />);

    expect(screen.getByRole('heading', { name: '本轮' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: '使用的记忆' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: '使用的资料' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: '工具与技能' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: '运行记录' })).toBeInTheDocument();

    expect(screen.getByText('本轮还没有召回记忆。')).toBeInTheDocument();
    expect(screen.getByText('本轮还没有读取资料。')).toBeInTheDocument();
    expect(screen.getByText('本轮还没有使用工具。')).toBeInTheDocument();
    expect(screen.getByText('本轮还没有运行记录。')).toBeInTheDocument();
    expect(screen.getByText('没有正在运行的任务')).toBeInTheDocument();
    expect(screen.getByText('无待确认事项')).toBeInTheDocument();
  });

  it('shows current model, provider, work directory, and context usage', async () => {
    useSettingsStore.getState().updateApiConfig({ provider: 'openai', model: 'unsaved-plan-model', authMode: 'chatgpt_plan' });
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: 'session-1',
      activeSession: makeSession('session-1', 'D:/Projects/demo'),
    });
    render(<ContextDrawerContent onClose={vi.fn()} />);

    expect(screen.getByText('Anthropic (Claude) · claude-sonnet-4-20250514')).toBeInTheDocument();
    expect(screen.getByText('D:/Projects/demo')).toBeInTheDocument();
    expect(await screen.findByText('1.2k / 8.2k')).toBeInTheDocument();
    await waitFor(() => expect(getContextUsage).toHaveBeenCalledWith('session-1'));
  });

  it('derives memories, materials, tools, and run summary from the latest assistant message', () => {
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: 'session-1',
      activeSession: makeSession('session-1'),
    });
    useMessagesStore.setState((state) => ({
      ...state,
      messages: [
        makeAssistantMessage({
          toolCalls: [
            { id: 'call-1', name: 'recall_memories', arguments: JSON.stringify({ query: '喜欢的饮料' }) },
            { id: 'call-2', name: 'read_file', arguments: JSON.stringify({ path: 'notes/todo.md' }) },
            { id: 'call-3', name: 'write_file', arguments: JSON.stringify({ path: 'notes/todo.md' }) },
          ],
          toolResults: [
            { callId: 'call-1', toolName: 'recall_memories', success: true, output: '[preference] 喜欢乌龙茶 (重要性:0.8)' },
            { callId: 'call-2', toolName: 'read_file', success: true, output: '文件内容' },
            { callId: 'call-3', toolName: 'write_file', success: false, output: '', error: '需要确认', confirmationRequired: true, confirmationStatus: 'pending' },
          ],
          taskRun: {
            id: 'run-1',
            goal: '整理待办笔记',
            status: 'awaiting_confirmation',
            plan: ['读取笔记', '更新笔记'],
            confirmationState: 'pending',
            resumable: true,
            stepCount: 2,
            completedStepCount: 1,
          },
        }),
      ],
    }));
    render(<ContextDrawerContent onClose={vi.fn()} />);

    // 使用的记忆：摘要、类别、召回原因
    expect(screen.getByText('喜欢乌龙茶')).toBeInTheDocument();
    expect(screen.getByText('preference')).toBeInTheDocument();
    expect(screen.getByText('因查询「喜欢的饮料」被召回')).toBeInTheDocument();

    // 使用的资料：路径与状态
    expect(screen.getByText('notes/todo.md')).toBeInTheDocument();

    // 工具与技能：本轮调用过的工具与状态
    expect(screen.getByText('查找记忆')).toBeInTheDocument();
    expect(screen.getByText('读取文件')).toBeInTheDocument();
    expect(screen.getByText('写入文件')).toBeInTheDocument();
    expect(screen.getAllByText('已完成').length).toBeGreaterThan(0);
    expect(screen.getAllByText('等待确认').length).toBeGreaterThan(0);

    // 本轮：待确认事项计数
    expect(screen.getByText('1 项待确认')).toBeInTheDocument();

    // 运行记录：目标、状态、步骤进度
    expect(screen.getByText('整理待办笔记')).toBeInTheDocument();
    expect(screen.getByText('已完成 1/2 步')).toBeInTheDocument();
  });

  it('surfaces taskFacts.modifiedFiles and pendingConfirmation on the run summary', () => {
    // After C, durable TaskFacts expose the full agent verdict to the UI.
    // The drawer must show modified files and any pending confirmation
    // so the user can audit what the agent changed without leaving the
    // chat surface.
    useMessagesStore.setState(() => ({
      messages: [
        { id: 'user-1', sessionId: 'session-1', role: 'user', content: '整理一下', createdAt: 1 },
        {
          id: 'assistant-1', sessionId: 'session-1', role: 'assistant', content: '好', createdAt: 2,
          taskRun: {
            id: 'run-1', goal: '整理', status: 'awaiting_confirmation', plan: [],
            confirmationState: 'pending', resumable: true, stepCount: 1, completedStepCount: 0,
          },
          taskFacts: {
            goal: '整理',
            modifiedFiles: ['notes/todo.md', 'README.md'],
            terminalReason: 'awaiting_confirmation',
            pendingConfirmation: {
              callId: 'call-x',
              toolName: 'write_file',
              arguments: { path: 'notes/todo.md' },
              requestedAt: 1,
            },
          },
        },
      ],
    }));
    render(<ContextDrawerContent onClose={vi.fn()} />);

    expect(screen.getByText('notes/todo.md')).toBeInTheDocument();
    expect(screen.getByText('等待 write_file 确认')).toBeInTheDocument();
  });

  it('shows live tool steps that are not yet persisted on the message', () => {
    useMessagesStore.setState((state) => ({
      ...state,
      toolExecuting: true,
      executingTool: 'read_file',
      liveStatus: '正在调用 read_file',
      liveToolSteps: {
        'live-1': { callId: 'live-1', toolName: 'read_file', arguments: {}, status: 'running' },
      },
    }));
    render(<ContextDrawerContent onClose={vi.fn()} />);

    expect(screen.getAllByText('正在调用 read_file').length).toBeGreaterThan(0);
    expect(screen.getByText('读取文件')).toBeInTheDocument();
    expect(screen.getByText('进行中')).toBeInTheDocument();
  });
});

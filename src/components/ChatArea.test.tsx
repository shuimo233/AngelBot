import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ChatArea, restoreRetryableLiveConfirmation, workspaceFilePathFromHref } from '$components/ChatArea';
import { ContentBlockRenderer } from '$components/ContentBlockRenderer';
import { AUTOMATIONS_UPDATED_EVENT } from '$components/WorkspaceReminders';
import { useSessionsStore } from '$stores/sessions';
import { useWorkspacesStore } from '$stores/workspaces';
import { useMessagesStore, type LiveBlock } from '$stores/messages';
import { useSettingsStore } from '$stores/settings';
import { useThinkingEffortStore } from '$stores/thinkingEffort';
import { usePreferencesStore } from '$stores/preferences';
import type { Message } from '$types';
import type { Automation } from '$lib/commands/automation';

const automationFixture = vi.hoisted(() => ({ items: [] as Automation[] }));

// The work capsule owns its own projection polling and has dedicated tests.
// ChatArea tests exercise transcript and composer behavior without consuming
// their ordered IPC fixtures through an unrelated child component.
vi.mock('./WorkspaceActivityCapsule', () => ({
  WorkspaceActivityCapsule: () => null,
}));

vi.mock('$lib/commands/session', () => ({
  updateSessionTitle: vi.fn(),
  getContextUsage: () => Promise.resolve({
    activeMessageCount: 0,
    totalMessageCount: 0,
    estimatedTokens: 0,
    contextLimit: 128000,
    isCompressed: false,
  }),
}));

// The empty-state card reads automations and knowledge; keep those off the
// fetch-based IPC path so existing call-count assertions stay valid. Plain
// functions survive this file's vi.restoreAllMocks() in beforeEach. The fixture
// also exercises real reminder recall without changing the ordered fetch mocks.
vi.mock('$lib/commands/automation', () => ({
  getAutomations: () => Promise.resolve(automationFixture.items),
}));
vi.mock('$lib/commands/memory', () => ({
  getMemories: () => Promise.resolve([]),
}));
vi.mock('$lib/commands/usage', () => ({
  getSessionUsage: () => Promise.resolve([]),
}));
// The branch navigator is a separate workspace-history concern. Keep its
// command transport out of ChatArea's message-loading fixtures so these tests
// continue to describe transcript behaviour rather than IPC call ordering.
vi.mock('$lib/commands/session-tree', () => ({
  getBranchTree: () => Promise.resolve({ branches: [] }),
  switchToMessageBranch: vi.fn(),
}));

const makeSession = (id: string, title: string) => ({
  id, title, createdAt: Date.now(), updatedAt: Date.now(), contextVersion: 0,
});

describe('Main Agent file citations', () => {
  it('accepts only project-relative angelbot-file links', () => {
    expect(workspaceFilePathFromHref('angelbot-file:src%2Fmain.ts')).toBe('src/main.ts');
    expect(workspaceFilePathFromHref('angelbot-file:/src%2Fmain.ts')).toBeNull();
    expect(workspaceFilePathFromHref('angelbot-file:..%2Fsecrets.txt')).toBeNull();
    expect(workspaceFilePathFromHref('angelbot-file:src%5Cmain.ts')).toBeNull();
    expect(workspaceFilePathFromHref('file:///D:/Projects/demo/main.ts')).toBeNull();
  });
});

describe('ChatArea', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    automationFixture.items = [];
    useMessagesStore.setState((state) => ({
      ...state,
      messages: [],
      input: '',
      toolExecuting: false,
      executingTool: null,
      streamingText: '',
      liveBlocks: [],
      liveMessageId: null,
      liveStatus: '',
      toolProgress: {},
    }));
    useSessionsStore.setState({ sessions: [], activeSessionId: null, activeSession: null });
    useWorkspacesStore.setState({ workspaces: [], activeWorkspaceId: null });
    useSettingsStore.setState({
      profile: { name: 'Test', avatar: '', bio: '', languageStyle: 'casual', tone: 'friendly', responseFormats: ['text'], keywords: [], greeting: '', personality: 'balanced', speechBubble: 'default' },
      activeApiConfig: { provider: 'anthropic', model: 'claude-sonnet-4-20250514', baseUrl: '', apiKey: '', maxTokens: 4096, temperature: 0.7 },
      apiConfigLoaded: true,
    });
    useThinkingEffortStore.setState({ effort: 'medium' });
    usePreferencesStore.setState({
      preferences: { communication: { preferredTone: [], dislikedWords: [], petPeeves: [] }, habits: { greetingStyle: '', responseLength: 'medium', responseLanguage: 'auto' }, topics: { interests: [], avoidTopics: [] }, learnedAt: Date.now(), evolutionEnabled: true },
    });
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ ok: true, data: [] }))
    );
  });

  it('keeps a locally failed live confirmation retryable instead of marking the action complete', () => {
    const blocks: LiveBlock[] = [{
      kind: 'tool_call',
      index: 0,
      iterationId: 0,
      status: 'completed',
      callId: 'confirmation-call',
      toolName: 'write_file',
      arguments: { path: 'notes.txt', content: 'hello' },
    }];
    const restored = restoreRetryableLiveConfirmation(blocks, 'confirmation-call');

    expect(restored).toMatchObject([{
      kind: 'tool_call',
      status: 'needs_approval',
      error: '确认处理失败，请重新检查后重试',
    }]);
    render(<ContentBlockRenderer blocks={restored} onResolveConfirmation={vi.fn()} />);
    expect(screen.getByRole('button', { name: '允许' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '拒绝' })).toBeInTheDocument();
  });

  it('shows first-use guidance when no session is selected', () => {
    render(<ChatArea />);

    expect(screen.getByText(/^(早上好|下午好|晚上好)$/)).toBeInTheDocument();
    expect(screen.getByText(/直接说出想处理的事/)).toBeInTheDocument();
    expect(screen.getByPlaceholderText('给 AngelBot 发消息…')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '发送' })).toBeDisabled();
  });

  it('renders selected session title', async () => {
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: 'session-1',
      activeSession: makeSession('session-1', 'Demo'),
    });
    render(<ChatArea />);
    expect(screen.getByRole('heading', { name: 'Demo' })).toBeInTheDocument();
    expect(screen.queryByText(/0 条消息/)).not.toBeInTheDocument();
  });

  it('sends file-only snapshots from Personal space without routing from file contents, and reloads compact history', async () => {
    const user = userEvent.setup();
    const personalSession = makeSession('personal-session', '日常');
    const otherSession = makeSession('other-session', '项目');
    const workspaces = [
      { id: 'personal', name: '日常', kind: 'personal' as const, activeSessionId: personalSession.id, createdAt: 1, updatedAt: 1 },
      { id: 'project', name: 'Named Project', kind: 'project' as const, activeSessionId: otherSession.id, createdAt: 1, updatedAt: 1 },
    ];
    let durable: Message[] = [];
    const submitted: Record<string, unknown>[] = [];
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd, args } = JSON.parse(String(init?.body ?? '{}'));
      if (cmd === 'get_messages') return new Response(JSON.stringify({ ok: true, data: args.sessionId === personalSession.id ? durable : [] }));
      if (cmd === 'send_message') {
        submitted.push(args.req);
        const reply: Message = { id: 'file-reply', sessionId: args.req.sessionId, role: 'assistant', content: '已读完整文本。', createdAt: 1 };
        durable = [{ id: args.req.clientMessageId, sessionId: args.req.sessionId, role: 'user', content: args.req.content,
          textAttachments: args.req.textAttachments, createdAt: 1 }, reply];
        return new Response(JSON.stringify({ ok: true, data: reply }));
      }
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    useWorkspacesStore.setState({ activeWorkspaceId: 'personal', workspaces });
    useSessionsStore.setState({ activeSessionId: personalSession.id, activeSession: personalSession, sessions: [personalSession, otherSession] });
    render(<ChatArea />);
    await user.upload(screen.getByLabelText('选择文本附件'), new File(['Named Project,不要改原文件'], 'daily.csv'));
    await screen.findByText('daily.csv');
    await user.click(screen.getByRole('button', { name: '发送' }));
    await screen.findByText('已读完整文本。');
    expect(submitted).toMatchObject([{ sessionId: personalSession.id, content: '', textAttachments: [{ name: 'daily.csv', text: 'Named Project,不要改原文件' }] }]);
    expect(useWorkspacesStore.getState().activeWorkspaceId).toBe('personal');
    expect(screen.queryByLabelText('待发送文本附件')).not.toBeInTheDocument();
    const history = screen.getByLabelText('已发送文本附件');
    expect(within(history).getByText('daily.csv')).toBeInTheDocument();
    await user.click(within(history).getByText('daily.csv'));
    expect(within(history).getByText('Named Project,不要改原文件')).toBeVisible();
    act(() => {
      useWorkspacesStore.setState({ activeWorkspaceId: 'project' });
      useSessionsStore.setState({ activeSessionId: otherSession.id, activeSession: otherSession });
    });
    await waitFor(() => expect(screen.queryByLabelText('已发送文本附件')).not.toBeInTheDocument());
    act(() => {
      useWorkspacesStore.setState({ activeWorkspaceId: 'personal' });
      useSessionsStore.setState({ activeSessionId: personalSession.id, activeSession: personalSession });
    });
    expect(await screen.findByText('daily.csv')).toBeInTheDocument();
  });

  it.each([false, true])('preserves an unsuccessful snapshot unless its durable user ID was accepted (%s)', async (persisted) => {
    const user = userEvent.setup();
    const session = makeSession('failure-session', 'Failure');
    let durable: Message[] = [];
    let sendCount = 0;
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd, args } = JSON.parse(String(init?.body ?? '{}'));
      if (cmd === 'get_messages') return new Response(JSON.stringify({ ok: true, data: durable }));
      if (cmd === 'send_message') {
        sendCount++;
        if (persisted) durable = [{ id: args.req.clientMessageId, sessionId: session.id, role: 'user',
          content: args.req.content, textAttachments: args.req.textAttachments, createdAt: 1 }];
        return new Response(JSON.stringify({ ok: false, error: 'Offline provider' }));
      }
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    useSessionsStore.setState({ activeSessionId: session.id, activeSession: session, sessions: [session] });
    render(<ChatArea />);
    await user.upload(screen.getByLabelText('选择文本附件'), new File(['important'], 'keep.txt'));
    await screen.findByText('keep.txt');
    await user.type(screen.getByRole('textbox'), '请处理这份文本');
    await user.click(screen.getByRole('button', { name: '发送' }));
    await screen.findByText(/Offline provider/);
    if (persisted) {
      expect(screen.queryByLabelText('待发送文本附件')).not.toBeInTheDocument();
      expect(screen.getByLabelText('已发送文本附件')).toHaveTextContent('keep.txt');
      expect(screen.getByRole('textbox')).toHaveValue('');
    } else {
      expect(await screen.findByRole('alert')).toHaveTextContent('附件仍保留');
      expect(screen.getByLabelText('待发送文本附件')).toHaveTextContent('keep.txt');
      expect(screen.queryByLabelText('已发送文本附件')).not.toBeInTheDocument();
      expect(screen.getByRole('textbox')).toHaveValue('请处理这份文本');
    }
    expect(sendCount).toBe(1);
  });

  it('does not clear a newer draft when delayed event subscription completes', async () => {
    const user = userEvent.setup();
    const session = makeSession('subscribe-session', 'Delayed subscribe');
    const originalSubscribe = useMessagesStore.getState().subscribeToAgentEvents;
    let release: () => void = () => {};
    const subscribe = vi.fn(() => new Promise<void>((resolve) => { release = resolve; }));
    let sentText = '';
    let durable: Message[] = [];
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd, args } = JSON.parse(String(init?.body ?? '{}'));
      if (cmd === 'get_messages') return new Response(JSON.stringify({ ok: true, data: durable }));
      if (cmd === 'send_message') {
        sentText = args.req.content;
        const reply: Message = { id: 'delayed-reply', sessionId: session.id, role: 'assistant', content: '原始请求已完成。', createdAt: 1 };
        durable = [reply];
        return new Response(JSON.stringify({ ok: true, data: reply }));
      }
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    useMessagesStore.setState({ subscribeToAgentEvents: subscribe });
    useSessionsStore.setState({ activeSessionId: session.id, activeSession: session, sessions: [session] });
    try {
      render(<ChatArea />);
      const input = screen.getByRole('textbox');
      await user.type(input, '原始请求');
      await user.click(screen.getByRole('button', { name: '发送' }));
      expect(subscribe).toHaveBeenCalledWith(session.id);
      expect(input).toHaveValue('');
      await user.type(input, '下一条草稿');
      await act(async () => release());
      await screen.findByText('原始请求已完成。');
      expect(sentText).toBe('原始请求');
      expect(input).toHaveValue('下一条草稿');
    } finally { useMessagesStore.setState({ subscribeToAgentEvents: originalSubscribe }); }
  });

  it.each([false, true])('routes prompt and files only to the exact bound project session, refusing target drift (%s)', async (drift) => {
    const user = userEvent.setup();
    const personalSession = makeSession('route-personal', '日常');
    const projectSession = makeSession('route-project', 'Named Project');
    const workspaces = [
      { id: 'personal', name: '日常', kind: 'personal' as const, activeSessionId: personalSession.id, createdAt: 1, updatedAt: 1 },
      { id: 'project', name: 'Named Project', kind: 'project' as const, activeSessionId: projectSession.id, createdAt: 1, updatedAt: 1 },
    ];
    const originalOpen = useWorkspacesStore.getState().openWorkspace;
    const sent: Record<string, unknown>[] = [];
    let durable: Message[] = [];
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd, args } = JSON.parse(String(init?.body ?? '{}'));
      if (cmd === 'get_messages') return new Response(JSON.stringify({ ok: true, data: durable }));
      if (cmd === 'send_message') {
        sent.push(args.req);
        const reply: Message = { id: 'routed-reply', sessionId: projectSession.id, role: 'assistant', content: '项目内已读附件。', createdAt: 1 };
        durable = [{ id: args.req.clientMessageId, sessionId: projectSession.id, role: 'user', content: args.req.content, textAttachments: args.req.textAttachments, createdAt: 1 }, reply];
        return new Response(JSON.stringify({ ok: true, data: reply }));
      }
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    const open = vi.fn(async () => {
      useSessionsStore.setState({ activeSessionId: drift ? 'wrong-session' : projectSession.id,
        activeSession: drift ? makeSession('wrong-session', 'Wrong') : projectSession });
      useWorkspacesStore.setState({ activeWorkspaceId: 'project' });
    });
    useWorkspacesStore.setState({ activeWorkspaceId: 'personal', workspaces, openWorkspace: open });
    useSessionsStore.setState({ activeSessionId: personalSession.id, activeSession: personalSession, sessions: [personalSession, projectSession] });
    try {
      render(<ChatArea />);
      await user.upload(screen.getByLabelText('选择文本附件'), new File(['Unrelated Project data'], 'input.md'));
      await screen.findByText('input.md');
      await user.type(screen.getByRole('textbox'), '请在 Named Project 检查附件');
      await user.click(screen.getByRole('button', { name: '发送' }));
      await waitFor(() => expect(open).toHaveBeenCalledWith('project'));
      if (drift) {
        await waitFor(() => expect(screen.getByRole('button', { name: '发送' })).toBeDisabled());
        expect(sent).toEqual([]);
        expect(screen.queryByText('input.md')).not.toBeInTheDocument();
      } else {
        await screen.findByText('项目内已读附件。');
        expect(sent).toMatchObject([{ sessionId: projectSession.id, content: '请在 Named Project 检查附件',
          textAttachments: [{ name: 'input.md', text: 'Unrelated Project data' }] }]);
        expect(screen.getByLabelText('已发送文本附件')).toHaveTextContent('input.md');
      }
    } finally { useWorkspacesStore.setState({ openWorkspace: originalOpen }); }
  });

  it('never restores a late failed project-route draft after leaving and returning to its source', async () => {
    const user = userEvent.setup();
    const personalSession = makeSession('route-source', '日常');
    const otherSession = makeSession('route-other', 'Other');
    const originalOpen = useWorkspacesStore.getState().openWorkspace;
    let rejectOpen: (reason: Error) => void = () => {};
    const open = vi.fn(() => new Promise<void>((_resolve, reject) => { rejectOpen = reject; }));
    useWorkspacesStore.setState({ activeWorkspaceId: 'personal', openWorkspace: open, workspaces: [
      { id: 'personal', name: '日常', kind: 'personal', activeSessionId: personalSession.id, createdAt: 1, updatedAt: 1 },
      { id: 'project', name: 'Named Project', kind: 'project', activeSessionId: 'target', createdAt: 1, updatedAt: 1 },
      { id: 'other', name: 'Other', kind: 'project', activeSessionId: otherSession.id, createdAt: 1, updatedAt: 1 },
    ] });
    useSessionsStore.setState({ activeSessionId: personalSession.id, activeSession: personalSession, sessions: [personalSession, otherSession] });
    try {
      render(<ChatArea />);
      await user.upload(screen.getByLabelText('选择文本附件'), new File(['private old request'], 'old.txt'));
      await screen.findByText('old.txt');
      await user.type(screen.getByRole('textbox'), '交给 Named Project 处理');
      await user.click(screen.getByRole('button', { name: '发送' }));
      expect(open).toHaveBeenCalledWith('project');
      act(() => {
        useWorkspacesStore.setState({ activeWorkspaceId: 'other' });
        useSessionsStore.setState({ activeSessionId: otherSession.id, activeSession: otherSession });
      });
      act(() => {
        useWorkspacesStore.setState({ activeWorkspaceId: 'personal' });
        useSessionsStore.setState({ activeSessionId: personalSession.id, activeSession: personalSession });
      });
      await user.type(screen.getByRole('textbox'), '新的日常草稿');
      await act(async () => rejectOpen(new Error('Project unavailable')));
      expect(screen.getByRole('textbox')).toHaveValue('新的日常草稿');
      expect(screen.queryByText('old.txt')).not.toBeInTheDocument();
      expect(screen.queryByText('交给 Named Project 处理')).not.toBeInTheDocument();
    } finally { useWorkspacesStore.setState({ openWorkspace: originalOpen }); }
  });

  it('recalls only the current workspace reminders in an existing conversation and refreshes trigger and cancellation changes', async () => {
    vi.useFakeTimers({ toFake: ['Date'] });
    vi.setSystemTime(new Date(2026, 9, 3, 15, 0, 0));
    try {
      const user = userEvent.setup();
      const session = makeSession('session-1', 'Current project');
      const now = Math.floor(Date.now() / 1000);
      const reminder: Automation = {
        id: 'triggered-reminder', title: '回看当前项目进展', prompt: '',
        triggerKind: 'once', triggerValue: '2026-10-03T16:00:00', enabled: true,
        permissionSummary: '', executorKind: 'notification', workspaceId: 'project-current',
        scriptArgs: [], timeoutSeconds: 300, nextRunAt: now + 3600,
      };
      const cancellable = { ...reminder, id: 'cancelled-reminder', title: '取消后不应再待提醒' };
      const otherWorkspace = { ...reminder, id: 'other-reminder', title: '其他空间的私有提醒', workspaceId: 'project-other' };
      automationFixture.items = [reminder, cancellable, otherWorkspace];
      let messages: Message[] = [{
        id: 'existing-reply', sessionId: session.id, role: 'assistant',
        content: '已有的主对话仍然可读。', createdAt: now,
      }];
      vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
        const { cmd } = JSON.parse(String(init?.body ?? '{}')) as { cmd?: string };
        if (cmd === 'get_messages') return new Response(JSON.stringify({ ok: true, data: messages }));
        if (cmd === 'send_message') {
          automationFixture.items = automationFixture.items.map((item) => item.id === cancellable.id
            ? { ...item, enabled: false, nextRunAt: undefined }
            : item);
          const reply: Message = {
            id: 'cancel-reply', sessionId: session.id, role: 'assistant',
            content: '已取消剩余提醒。', createdAt: now,
          };
          messages = [...messages, reply];
          return new Response(JSON.stringify({ ok: true, data: reply }));
        }
        return new Response(JSON.stringify({ ok: true, data: [] }));
      });
      useWorkspacesStore.setState({
        activeWorkspaceId: 'project-current',
        workspaces: [{
          id: 'project-current', name: 'Current project', kind: 'project',
          activeSessionId: session.id, createdAt: now, updatedAt: now,
        }],
      });
      useSessionsStore.setState({ sessions: [session], activeSessionId: session.id, activeSession: session });

      render(<ChatArea />);
      expect(await screen.findByText('已有的主对话仍然可读。')).toBeInTheDocument();
      expect(await screen.findByText('待执行 2 · 今日已触发 0')).toBeInTheDocument();
      await user.click(screen.getByText('事项回看'));
      expect(screen.getByText(reminder.title)).toBeVisible();
      expect(screen.getByText(cancellable.title)).toBeVisible();
      expect(screen.queryByText(otherWorkspace.title)).not.toBeInTheDocument();

      automationFixture.items = [
        { ...reminder, enabled: false, nextRunAt: undefined, lastRunAt: now - 1 },
        cancellable, otherWorkspace,
      ];
      act(() => fireEvent(window, new Event(AUTOMATIONS_UPDATED_EVENT)));
      expect(await screen.findByText('待执行 1 · 今日已触发 1')).toBeInTheDocument();
      expect(within(screen.getByRole('region', { name: '今日已触发' })).getByText(reminder.title)).toBeVisible();
      expect(within(screen.getByRole('region', { name: '待执行' })).queryByText(reminder.title)).not.toBeInTheDocument();

      // Completion of a real ChatArea send refreshes recall after the backend
      // cancels a reminder; no manual refresh event is supplied for this phase.
      await user.type(screen.getByPlaceholderText('给 AngelBot 发消息…'), '取消剩余提醒');
      await user.click(screen.getByRole('button', { name: '发送' }));
      expect(await screen.findByText('已取消剩余提醒。')).toBeInTheDocument();
      expect(await screen.findByText('待执行 0 · 今日已触发 1')).toBeInTheDocument();
      expect(screen.queryByText(cancellable.title)).not.toBeInTheDocument();
      expect(screen.queryByText(otherWorkspace.title)).not.toBeInTheDocument();
      expect(screen.getByText(reminder.title)).toBeVisible();
      expect(screen.getByText(/不代表任务已完成或系统通知已送达/)).toBeVisible();
    } finally {
      vi.useRealTimers();
    }
  });

  it('routes a safe Main-Agent file citation to the workbench locator', async () => {
    const session = makeSession('session-1', 'Demo');
    const assistantMessage: Message = {
      id: 'assistant-file-citation',
      sessionId: session.id,
      role: 'assistant',
      content: '[查看项目计划](angelbot-file:docs%2Fplan.md)',
      createdAt: Date.now(),
    };
    useSessionsStore.setState({ sessions: [], activeSessionId: session.id, activeSession: session });
    const located = vi.fn();
    window.addEventListener('angelbot:locate-file', located);

    render(<ChatArea />);
    act(() => useMessagesStore.setState((state) => ({ ...state, messages: [assistantMessage] })));
    fireEvent.click(screen.getByRole('button', { name: '在工作台中定位 docs/plan.md' }));

    expect(located).toHaveBeenCalledTimes(1);
    expect((located.mock.calls[0][0] as CustomEvent).detail).toEqual({ path: 'docs/plan.md' });
    window.removeEventListener('angelbot:locate-file', located);
  });

  it('does not render a stale message load after switching sessions', async () => {
    const firstSession = makeSession('session-1', 'First');
    const secondSession = makeSession('session-2', 'Second');
    let resolveFirstLoad: (response: Response) => void;
    const firstLoad = new Promise<Response>((resolve) => {
      resolveFirstLoad = resolve;
    });

    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation((_input, init) => {
      const { args } = JSON.parse(init?.body as string) as { args: { sessionId: string } };
      if (args.sessionId === firstSession.id) return firstLoad;
      return Promise.resolve(new Response(JSON.stringify({
        ok: true,
        data: [{
          id: 'second-message',
          sessionId: secondSession.id,
          role: 'assistant',
          content: 'Current session message',
          createdAt: Date.now(),
        }],
      })));
    });

    useSessionsStore.setState({
      sessions: [firstSession, secondSession],
      activeSessionId: firstSession.id,
      activeSession: firstSession,
    });
    render(<ChatArea />);
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));

    act(() => {
      useSessionsStore.setState({
        sessions: [firstSession, secondSession],
        activeSessionId: secondSession.id,
        activeSession: secondSession,
      });
    });
    expect(await screen.findByText('Current session message')).toBeInTheDocument();

    resolveFirstLoad!(new Response(JSON.stringify({
      ok: true,
      data: [{
        id: 'first-message',
        sessionId: firstSession.id,
        role: 'assistant',
        content: 'Stale session message',
        createdAt: Date.now(),
      }],
    })));

    await waitFor(() => {
      expect(screen.queryByText('Stale session message')).not.toBeInTheDocument();
      expect(screen.getByText('Current session message')).toBeInTheDocument();
    });
  });

  it('clears a file reference draft when switching sessions', async () => {
    const firstSession = makeSession('session-1', 'First');
    const secondSession = makeSession('session-2', 'Second');
    useSessionsStore.setState({
      sessions: [firstSession, secondSession],
      activeSessionId: firstSession.id,
      activeSession: firstSession,
    });
    render(<ChatArea />);

    act(() => useMessagesStore.getState().setInput('[引用文件: notes.md]'));
    expect(useMessagesStore.getState().input).toBe('[引用文件: notes.md]');

    act(() => {
      useSessionsStore.setState({
        sessions: [firstSession, secondSession],
        activeSessionId: secondSession.id,
        activeSession: secondSession,
      });
    });

    await waitFor(() => expect(useMessagesStore.getState().input).toBe(''));
  });

  it('does not send when input is empty', async () => {
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: 'session-1',
      activeSession: makeSession('session-1', 'Demo'),
    });
    render(<ChatArea />);
    // With no messages, no avatar 'U' renders; check the composer area exists instead
    await waitFor(() => expect(screen.queryByPlaceholderText('给 AngelBot 发消息…')).toBeInTheDocument());
    // Send button must be disabled when no input
    expect(screen.getByRole('button', { name: '发送' })).toBeDisabled();
  });

  it('sends without throwing when a session is active', async () => {
    const session = makeSession('session-1', 'Demo');
    const assistantReply: Message = {
      id: 'assistant-1',
      sessionId: session.id,
      role: 'assistant',
      content: 'Reply received',
      createdAt: Date.now(),
    };
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    const fetchMock = vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: [] })))
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: assistantReply })))
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: [
        { id: 'user-1', sessionId: session.id, role: 'user', content: 'hello', createdAt: Date.now() },
        assistantReply,
      ] })));

    useSessionsStore.setState({
      sessions: [session],
      activeSessionId: session.id,
      activeSession: session,
    });

    render(<ChatArea />);
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));

    await userEvent.type(screen.getByPlaceholderText('给 AngelBot 发消息…'), 'hello');
    await userEvent.click(screen.getByRole('button', { name: '发送' }));

    expect(await screen.findByText('Reply received')).toBeInTheDocument();
    const sendRequest = JSON.parse(fetchMock.mock.calls[1][1]?.body as string) as {
      cmd: string;
      args: { req: { clientMessageId?: string } };
    };
    expect(sendRequest.cmd).toBe('send_message');
    expect(sendRequest.args.req.clientMessageId).toEqual(expect.any(String));
    expect(consoleError).not.toHaveBeenCalled();
  });

  it('keeps the edited user message visible while its replacement reply is pending', async () => {
    const session = makeSession('session-1', 'Demo');
    const originalMessages: Message[] = [
      { id: 'user-1', sessionId: session.id, role: 'user', content: 'Original user message', createdAt: Date.now() },
      { id: 'assistant-1', sessionId: session.id, role: 'assistant', content: 'Original reply', createdAt: Date.now() + 1 },
    ];
    let resolveReply!: (response: Response) => void;
    const pendingReply = new Promise<Response>((resolve) => {
      resolveReply = resolve;
    });
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: originalMessages })))
      .mockReturnValueOnce(pendingReply);
    useSessionsStore.setState({
      sessions: [session],
      activeSessionId: session.id,
      activeSession: session,
    });

    render(<ChatArea />);
    await screen.findByText('Original user message');
    await userEvent.click(screen.getByRole('button', { name: '编辑' }));
    const editor = screen.getByRole('textbox', { name: '编辑后的消息内容' });
    await userEvent.clear(editor);
    await userEvent.type(editor, 'Edited user message');
    await userEvent.click(screen.getByRole('button', { name: '重新发送' }));

    expect(await screen.findByText('Edited user message')).toBeInTheDocument();

    resolveReply(new Response(JSON.stringify({
      ok: true,
      data: { id: 'assistant-2', sessionId: session.id, role: 'assistant', content: 'Replacement reply', createdAt: Date.now() + 2 },
    })));
  });

  it('does not erase the edited transcript when the post-resend reload is empty', async () => {
    const session = makeSession('session-1', 'Demo');
    const originalMessages: Message[] = [
      { id: 'user-1', sessionId: session.id, role: 'user', content: 'Original user message', createdAt: Date.now() },
      { id: 'assistant-1', sessionId: session.id, role: 'assistant', content: 'Original reply', createdAt: Date.now() + 1 },
    ];
    const replacementReply: Message = {
      id: 'assistant-2', sessionId: session.id, role: 'assistant', content: 'Replacement reply', createdAt: Date.now() + 2,
    };
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: originalMessages })))
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: replacementReply })))
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: [] })));
    useSessionsStore.setState({
      sessions: [session],
      activeSessionId: session.id,
      activeSession: session,
    });

    render(<ChatArea />);
    await screen.findByText('Original user message');
    await userEvent.click(screen.getByRole('button', { name: '编辑' }));
    const editor = screen.getByRole('textbox', { name: '编辑后的消息内容' });
    await userEvent.clear(editor);
    await userEvent.type(editor, 'Edited user message');
    await userEvent.click(screen.getByRole('button', { name: '重新发送' }));

    expect(await screen.findByText('Replacement reply')).toBeInTheDocument();
    expect(screen.getByText('Edited user message')).toBeInTheDocument();
    expect(screen.queryByText('Original user message')).not.toBeInTheDocument();
  });

  it('shows live status only in the run controls while a reply is pending', async () => {
    const session = makeSession('session-1', 'Demo');
    let resolveReply!: (response: Response) => void;
    const pendingReply = new Promise<Response>((resolve) => {
      resolveReply = resolve;
    });
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: [] })))
      .mockReturnValueOnce(pendingReply)
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: [
        { id: 'user-1', sessionId: session.id, role: 'user', content: 'stream this', createdAt: Date.now() },
        { id: 'assistant-1', sessionId: session.id, role: 'assistant', content: 'Done', createdAt: Date.now() },
      ] })));
    useSessionsStore.setState({
      sessions: [session],
      activeSessionId: session.id,
      activeSession: session,
    });

    render(<ChatArea />);
    await userEvent.type(await screen.findByPlaceholderText('给 AngelBot 发消息…'), 'stream this');
    await userEvent.click(screen.getByRole('button', { name: '发送' }));
    await waitFor(() => expect(screen.getByLabelText('运行控制')).toBeInTheDocument());

    act(() => useMessagesStore.getState().setLiveStatus('正在规划下一步'));

    expect(screen.getAllByText('正在执行')).toHaveLength(1);
    expect(screen.queryByText('正在规划下一步')).not.toBeInTheDocument();
    expect(screen.queryByTitle('停止生成 (Esc)')).not.toBeInTheDocument();

    resolveReply(new Response(JSON.stringify({
      ok: true,
      data: { id: 'assistant-1', sessionId: session.id, role: 'assistant', content: 'Done', createdAt: Date.now() },
    })));
    expect(await screen.findByText('Done')).toBeInTheDocument();
  });

  it('renders one approval and starts one preflight when persisted and live views share a pending call', async () => {
    const session = makeSession('session-1', 'Demo');
    let resolveReply!: (response: Response) => void;
    const pendingReply = new Promise<Response>((resolve) => { resolveReply = resolve; });
    let preflights = 0;
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd } = JSON.parse(String(init?.body ?? '{}')) as { cmd?: string };
      if (cmd === 'send_message') return pendingReply;
      if (cmd === 'preflight_pending_desktop_action') {
        preflights += 1;
        return new Response(JSON.stringify({ ok: true, data: {
          previewId: 'one-preview', operation: 'field', appDisplayName: '记事本',
          executableName: 'notepad.exe', windowTitle: '工作记录', controlName: '正文',
          text: '后端预检的内容', expiresAt: Date.now() + 60_000,
        } }));
      }
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    useSessionsStore.setState({ sessions: [session], activeSessionId: session.id, activeSession: session });

    render(<ChatArea />);
    await userEvent.type(await screen.findByPlaceholderText('给 AngelBot 发消息…'), '填写一个输入框');
    await userEvent.click(screen.getByRole('button', { name: '发送' }));
    await waitFor(() => expect(screen.getByLabelText('运行控制')).toBeInTheDocument());
    const assistant = useMessagesStore.getState().messages.find((message) => message.role === 'assistant');
    expect(assistant).toBeDefined();
    act(() => useMessagesStore.setState((state) => ({
      messages: state.messages.map((message) => message.id === assistant!.id ? {
        ...message,
        toolCalls: [{ id: 'field-call', name: 'set_trusted_app_text', arguments: '{"text":"model text"}' }],
        toolResults: [{
          callId: 'field-call', toolName: 'set_trusted_app_text', success: false,
          output: 'Confirmation required', confirmationRequired: true, confirmationStatus: 'pending',
        }],
      } : message),
      liveBlocks: [{
        kind: 'tool_call', index: 0, iterationId: 0, status: 'needs_approval',
        callId: 'field-call', toolName: 'set_trusted_app_text', arguments: { text: 'model text' },
      }],
      liveMessageId: assistant!.id,
    })));

    expect(await screen.findByText('后端预检的内容')).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: '允许' })).toHaveLength(1);
    expect(preflights).toBe(1);
    expect(screen.queryByText('model text')).not.toBeInTheDocument();

    resolveReply(new Response(JSON.stringify({ ok: true, data: {
      id: assistant!.id, sessionId: session.id, role: 'assistant', content: '已结束', createdAt: Date.now(),
    } })));
    await waitFor(() => expect(screen.queryByLabelText('运行控制')).not.toBeInTheDocument());
  });

  it('renders markdown content', async () => {
    const messages: Message[] = [{
      id: 'msg-1',
      sessionId: 'session-1',
      role: 'assistant',
      content: '```python\nprint("hello")\n```',
      createdAt: Date.now(),
    }];
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd } = JSON.parse(String(init?.body ?? '{}')) as { cmd?: string };
      return new Response(JSON.stringify({
        ok: true,
        data: cmd === 'get_messages' ? messages : [],
      }));
    });
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: 'session-1',
      activeSession: makeSession('session-1', 'Demo'),
    });
    render(<ChatArea />);
    expect(await screen.findByText(/hello/)).toBeDefined();
  });

  it('renders streaming markdown from each incoming delta before the reply completes', async () => {
    const session = makeSession('session-1', 'Demo');
    let resolveReply!: (response: Response) => void;
    const pendingReply = new Promise<Response>((resolve) => {
      resolveReply = resolve;
    });
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: [] })))
      .mockReturnValueOnce(pendingReply)
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: [
        { id: 'user-1', sessionId: session.id, role: 'user', content: 'stream this', createdAt: Date.now() },
        { id: 'assistant-1', sessionId: session.id, role: 'assistant', content: 'Done', createdAt: Date.now() },
      ] })));
    useSessionsStore.setState({
      sessions: [session],
      activeSessionId: session.id,
      activeSession: session,
    });

    render(<ChatArea />);
    await userEvent.type(screen.getByPlaceholderText('给 AngelBot 发消息…'), 'stream this');
    await userEvent.click(screen.getByRole('button', { name: '发送' }));

    await waitFor(() => expect(useMessagesStore.getState().messages).toHaveLength(2));
    act(() => useMessagesStore.getState().updateStreamingText('# Streaming title'));

    expect(await screen.findByRole('heading', { name: 'Streaming title' })).toBeInTheDocument();

    act(() => useMessagesStore.getState().updateStreamingText('# Streaming title\n\n**Second delta**'));
    expect(await screen.findByText('Second delta')).toHaveProperty('tagName', 'STRONG');
    expect(screen.getByLabelText('运行控制')).toBeInTheDocument();

    resolveReply(new Response(JSON.stringify({
      ok: true,
      data: { id: 'assistant-1', sessionId: session.id, role: 'assistant', content: 'Done', createdAt: Date.now() },
    })));
    expect(await screen.findByText('Done')).toBeInTheDocument();
  });

  it('keeps failed-run details folded until requested, then shows memory evidence and error output', async () => {
    const user = userEvent.setup();
    const messages: Message[] = [{
      id: 'msg-steps',
      sessionId: 'session-1',
      role: 'assistant',
      content: 'I checked the context.',
      createdAt: Date.now(),
      toolCalls: [
        {
          id: 'call-1',
          name: 'recall_memories',
          arguments: JSON.stringify({ query: 'Rust preference' }),
        },
        {
          id: 'call-2',
          name: 'write_file',
          arguments: JSON.stringify({ path: 'notes.txt' }),
        },
      ],
      toolResults: [
        {
          callId: 'call-1',
          toolName: 'recall_memories',
          success: true,
          output: '[preference] User likes Rust (importance 8)',
        },
        {
          callId: 'call-2',
          toolName: 'write_file',
          success: false,
          output: 'Path escapes sandbox',
          error: 'Path escapes sandbox',
        },
      ],
    }];
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ ok: true, data: messages }))
    );
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: 'session-1',
      activeSession: makeSession('session-1', 'Demo'),
    });
    // ChatArea reads from Zustand store, not from fetch response
    useMessagesStore.setState({ messages });

    render(<ChatArea />);

    const disclosure = await screen.findByRole('button', { name: /有步骤失败/ });
    expect(disclosure).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByText('查询记忆')).not.toBeInTheDocument();
    expect(screen.queryByText('[preference] User likes Rust (importance 8)')).not.toBeInTheDocument();

    await user.click(disclosure);

    expect(disclosure).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByText('查询记忆')).toBeInTheDocument();
    expect(screen.getByText('写入文件')).toBeInTheDocument();
    expect(screen.getByText('已检索的记忆')).toBeInTheDocument();
    expect(screen.getByText('[preference] User likes Rust (importance 8)')).toBeInTheDocument();
    expect(screen.queryByText('Path escapes sandbox', { selector: 'pre' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '查看工具输出' }));
    expect(screen.getByText('Path escapes sandbox', { selector: 'pre' })).toBeInTheDocument();
  });

  it('keeps completed persisted tool actions out of the conversation', async () => {
    const messages: Message[] = [{
      id: 'msg-tool-row',
      sessionId: 'session-1',
      role: 'assistant',
      content: 'I checked the directory.',
      createdAt: Date.now(),
      toolCalls: [{
        id: 'call-read',
        name: 'read_file',
        arguments: JSON.stringify({ path: 'README.md' }),
      }],
      toolResults: [{
        callId: 'call-read',
        toolName: 'read_file',
        success: true,
        output: 'contents',
      }],
    }];
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd } = JSON.parse(String(init?.body ?? '{}')) as { cmd?: string };
      return new Response(JSON.stringify({
        ok: true,
        data: cmd === 'get_messages' ? messages : [],
      }));
    });
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: 'session-1',
      activeSession: makeSession('session-1', 'Demo'),
    });
    useMessagesStore.setState({ messages });

    const { container } = render(<ChatArea />);

    await screen.findByText('I checked the directory.');
    expect(container.querySelector('.message-content--agent-turn')).toBeInTheDocument();
    expect(screen.queryByText('读取文件')).not.toBeInTheDocument();
    expect(container.querySelector('.agent-run-badge-row')).not.toBeInTheDocument();
    expect(container.querySelector('.agent-run-integrated')).not.toBeInTheDocument();
    expect(screen.queryByText(/动作已完成/)).not.toBeInTheDocument();
  });

  it('keeps completed tool audit out of the conversation while preserving the final answer', async () => {
    const messages: Message[] = [{
      id: 'msg-completed-timeline',
      sessionId: 'session-1',
      role: 'assistant',
      content: 'Final answer for the user.',
      createdAt: Date.now(),
      timelineBlocks: [
        { kind: 'text', index: 0, iterationId: 0, content: 'intermediate agent narration' },
        {
          kind: 'tool_call',
          index: 1,
          iterationId: 0,
          callId: 'call-read',
          toolName: 'read_file',
          arguments: {},
          status: 'completed',
        },
      ],
    }];
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd } = JSON.parse(String(init?.body ?? '{}')) as { cmd?: string };
      return new Response(JSON.stringify({
        ok: true,
        data: cmd === 'get_messages' ? messages : [],
      }));
    });
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: 'session-1',
      activeSession: makeSession('session-1', 'Demo'),
    });
    useMessagesStore.setState({ messages });

    render(<ChatArea />);

    expect(await screen.findByText('Final answer for the user.')).toBeInTheDocument();
    expect(screen.queryByText('已处理')).not.toBeInTheDocument();
    expect(screen.queryByText('intermediate agent narration')).not.toBeInTheDocument();
    expect(screen.queryByText('read_file')).not.toBeInTheDocument();
  });

  it('keeps a successful multi-action run focused on its final answer', async () => {
    const toolCalls = Array.from({ length: 4 }, (_, index) => ({
      id: `call-${index}`,
      name: 'read_file',
      arguments: JSON.stringify({ path: `file-${index}.txt` }),
    }));
    const messages: Message[] = [{
      id: 'msg-compact-steps', sessionId: 'session-1', role: 'assistant', content: 'Done', createdAt: Date.now(),
      toolCalls,
      toolResults: toolCalls.map((tool) => ({ callId: tool.id, toolName: tool.name, success: true, output: 'ok' })),
    }];
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({ ok: true, data: messages })));
    useSessionsStore.setState({ sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo') });
    useMessagesStore.setState({ messages });

    render(<ChatArea />);

    expect(await screen.findByText('Done')).toBeInTheDocument();
    expect(screen.queryByText('读取文件')).not.toBeInTheDocument();
    expect(screen.queryByText('read_file')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '查看调用参数' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /有步骤失败/ })).not.toBeInTheDocument();
  });

  it('renders side-effect confirmation steps as approval state', async () => {
    const messages: Message[] = [{
      id: 'msg-approval',
      sessionId: 'session-1',
      role: 'assistant',
      content: 'This action needs approval.',
      createdAt: Date.now(),
      toolCalls: [{
        id: 'call-approval',
        name: 'write_file',
        arguments: JSON.stringify({ path: 'notes.txt', content: 'hello' }),
      }],
      toolResults: [{
        callId: 'call-approval',
        toolName: 'write_file',
        success: false,
        output: "Confirmation required before executing side-effect tool 'write_file'.",
        error: "Confirmation required before executing side-effect tool 'write_file'.",
        confirmationRequired: true,
        confirmationStatus: 'pending',
      }],
      timelineBlocks: [{ kind: 'text', index: 0, iterationId: 0, content: 'This action needs approval.' }],
    }];
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ ok: true, data: messages }))
    );
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: 'session-1',
      activeSession: makeSession('session-1', 'Demo'),
    });
    useMessagesStore.setState({ messages });

    render(<ChatArea />);

    // Integrated badge shows the tool verb in Chinese
    expect(await screen.findByText('写入文件')).toBeInTheDocument();
    // Confirmation buttons are inline in the badge row
    expect(screen.getByText('写入项目文件：notes.txt')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '允许一次' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '拒绝' })).toBeInTheDocument();
    // No verbose "audit trail" intro text
    expect(screen.queryByText(/以下是 AngelBot/)).not.toBeInTheDocument();
  });

  it('keeps desktop dispatch and verified draft outcomes visible in a completed turn', async () => {
    const calls = [
      { id: 'app', name: 'open_trusted_app', arguments: '{"app_id":"notes"}' },
      { id: 'settings', name: 'open_windows_setting', arguments: '{"page":"sound"}' },
      { id: 'file', name: 'reveal_workspace_item', arguments: '{"path":"notes.txt"}' },
      { id: 'draft', name: 'prepare_message_draft', arguments: '{"app_id":"notes","text":"private draft"}' },
    ];
    const messages: Message[] = [{
      id: 'msg-desktop-results',
      sessionId: 'session-1',
      role: 'assistant',
      content: '已处理桌面操作。',
      createdAt: Date.now(),
      toolCalls: calls,
      toolResults: calls.map((call) => ({
        callId: call.id,
        toolName: call.name,
        success: true,
        output: JSON.stringify({ status: call.id === 'draft' ? 'verified' : 'dispatched', target: call.id }),
      })),
      timelineBlocks: [{ kind: 'text', index: 0, iterationId: 0, content: '已处理桌面操作。' }],
    }];
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({ ok: true, data: messages })));
    useSessionsStore.setState({ sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo') });
    useMessagesStore.setState({ messages });

    render(<ChatArea />);

    const receipts = await screen.findByRole('list', { name: '桌面操作结果' });
    expect(receipts.querySelectorAll('[role="listitem"]')).toHaveLength(4);
    expect(screen.getAllByText('已发出请求')).toHaveLength(3);
    expect(screen.getByText('已请求文件管理器定位；尚未确认窗口已显示。')).toBeInTheDocument();
    expect(screen.getByText('草稿已填写并校验；AngelBot 未点击发送，目标应用可能自动保存。')).toBeInTheDocument();
    expect(screen.queryByText('已完成并记录结果。')).not.toBeInTheDocument();
    expect(screen.queryByText('private draft')).not.toBeInTheDocument();
  });

  it('shows a successful desktop result even when an older turn has no durable timeline', async () => {
    const messages: Message[] = [{
      id: 'msg-legacy-desktop',
      sessionId: 'session-1',
      role: 'assistant',
      content: '已尝试打开应用。',
      createdAt: Date.now(),
      toolCalls: [{ id: 'open', name: 'open_trusted_app', arguments: '{"app_id":"notes"}' }],
      toolResults: [{ callId: 'open', toolName: 'open_trusted_app', success: true, output: '{"status":"dispatched"}' }],
    }];
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({ ok: true, data: messages })));
    useSessionsStore.setState({ sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo') });
    useMessagesStore.setState({ messages });

    render(<ChatArea />);

    expect(await screen.findByRole('list', { name: '桌面操作结果' })).toHaveTextContent('已发出请求');
    expect(screen.getByText('已向 Windows 发出打开请求；尚未确认目标窗口已就绪。')).toBeInTheDocument();
  });

  it('asks the user to inspect a stopped draft whose outcome is unknown', async () => {
    const messages: Message[] = [{
      id: 'msg-unknown-draft',
      sessionId: 'session-1',
      role: 'assistant',
      content: '请检查目标应用。',
      createdAt: Date.now(),
      toolCalls: [{ id: 'draft', name: 'prepare_message_draft', arguments: '{"app_id":"notes","text":"private draft"}' }],
      toolResults: [{
        callId: 'draft', toolName: 'prepare_message_draft', success: false,
        output: '', error: 'Error: {"code":"RESULT_UNKNOWN","message":"cancelled after dispatch"}',
      }],
    }];
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({ ok: true, data: messages })));
    useSessionsStore.setState({ sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo') });
    useMessagesStore.setState({ messages });

    render(<ChatArea />);

    expect(await screen.findByRole('button', { name: /有操作结果待核对/ })).toBeInTheDocument();
    expect(screen.getByText('操作停止后草稿可能已写入；请先去目标应用核对，勿自动重试。')).toBeInTheDocument();
    expect(screen.queryByText('private draft')).not.toBeInTheDocument();
  });

  it('keeps a superseded confirmation out of the reading flow', async () => {
    const messages: Message[] = [{
      id: 'msg-cancelled-confirmation',
      sessionId: 'session-1',
      role: 'assistant',
      content: '这项操作需要确认。',
      createdAt: Date.now(),
      toolCalls: [{
        id: 'call-cancelled-confirmation',
        name: 'open_windows_setting',
        arguments: JSON.stringify({ page: 'sound' }),
      }],
      toolResults: [{
        callId: 'call-cancelled-confirmation',
        toolName: 'open_windows_setting',
        success: false,
        output: 'Confirmation cancelled: a newer user message superseded this action.',
        error: 'Confirmation cancelled: a newer user message superseded this action.',
        confirmationRequired: false,
        confirmationStatus: 'cancelled',
      }],
      timelineBlocks: [{ kind: 'text', index: 0, iterationId: 0, content: '这项操作需要确认。' }],
    }];
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ ok: true, data: messages })),
    );
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: 'session-1',
      activeSession: makeSession('session-1', 'Demo'),
    });
    useMessagesStore.setState({ messages });

    render(<ChatArea />);

    expect(await screen.findByText('这项操作需要确认。')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '允许一次' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '拒绝' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /有步骤失败/ })).not.toBeInTheDocument();
  });

  it('keeps a second confirmation actionable when its sibling was cancelled', async () => {
    const messages: Message[] = [{
      id: 'msg-multiple-confirmations',
      sessionId: 'session-1',
      role: 'assistant',
      content: '两项操作需要确认。',
      createdAt: Date.now(),
      toolCalls: [
        {
          id: 'call-cancelled',
          name: 'open_windows_setting',
          arguments: JSON.stringify({ page: 'sound' }),
        },
        {
          id: 'call-still-pending',
          name: 'write_file',
          arguments: JSON.stringify({ path: 'todo.txt', content: 'later' }),
        },
      ],
      toolResults: [
        {
          callId: 'call-cancelled',
          toolName: 'open_windows_setting',
          success: false,
          output: 'Confirmation cancelled: a newer user message superseded this action.',
          confirmationStatus: 'cancelled',
        },
        // A stale replay must not revive the first confirmation.
        {
          callId: 'call-cancelled',
          toolName: 'open_windows_setting',
          success: false,
          output: 'Confirmation required before executing side-effect tool.',
          confirmationRequired: true,
          confirmationStatus: 'pending',
        },
        {
          callId: 'call-still-pending',
          toolName: 'write_file',
          success: false,
          output: 'Confirmation required before executing side-effect tool.',
          confirmationRequired: true,
          confirmationStatus: 'pending',
        },
      ],
    }];
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ ok: true, data: messages })),
    );
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: 'session-1',
      activeSession: makeSession('session-1', 'Demo'),
    });
    useMessagesStore.setState({ messages });

    render(<ChatArea />);

    expect(await screen.findByText('写入项目文件：todo.txt')).toBeInTheDocument();
    expect(screen.queryByText('打开 Windows 设置：声音')).not.toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: '允许一次' })).toHaveLength(1);
    expect(screen.getAllByRole('button', { name: '拒绝' })).toHaveLength(1);
  });

  it('does not hide an unfinished action because another result was replayed twice', async () => {
    const messages: Message[] = [{
      id: 'msg-duplicate-cancelled-result',
      sessionId: 'session-1',
      role: 'assistant',
      content: '还有一项操作正在处理。',
      createdAt: Date.now(),
      toolCalls: [
        { id: 'call-cancelled', name: 'open_windows_setting', arguments: JSON.stringify({ page: 'sound' }) },
        { id: 'call-unsettled', name: 'read_file', arguments: JSON.stringify({ path: 'todo.txt' }) },
      ],
      toolResults: [
        {
          callId: 'call-cancelled',
          toolName: 'open_windows_setting',
          success: false,
          output: 'Confirmation cancelled: a newer user message superseded this action.',
          confirmationStatus: 'cancelled',
        },
        {
          callId: 'call-cancelled',
          toolName: 'open_windows_setting',
          success: false,
          output: 'Confirmation cancelled: a newer user message superseded this action.',
          confirmationStatus: 'cancelled',
        },
      ],
    }];
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ ok: true, data: messages })),
    );
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: 'session-1',
      activeSession: makeSession('session-1', 'Demo'),
    });
    useMessagesStore.setState({ messages });

    render(<ChatArea />);

    expect(await screen.findByText('0/2 已完成')).toBeInTheDocument();
  });

  it('redacts delegated-network queries and URL paths in every confirmation display', async () => {
    const messages: Message[] = [{
      id: 'msg-network-approval',
      sessionId: 'session-1',
      role: 'assistant',
      content: '需要确认一个受限联网探索。',
      createdAt: Date.now(),
      toolCalls: [{
        id: 'call-network-approval',
        name: 'delegate_network_exploration',
        arguments: JSON.stringify({
          goal: 'private goal must not render',
          explorer_operations: [
            { kind: 'search', provider_host: 'search.example.com', query: 'private medical question' },
            { kind: 'fetch', url: 'https://docs.example.com/private/path?token=secret-value', method: 'GET' },
          ],
        }),
      }],
      toolResults: [{
        callId: 'call-network-approval',
        toolName: 'delegate_network_exploration',
        success: false,
        output: 'Confirmation required before delegated network exploration.',
        error: 'Confirmation required before delegated network exploration.',
        confirmationRequired: true,
        confirmationStatus: 'pending',
      }],
    }];
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ ok: true, data: messages })),
    );
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: 'session-1',
      activeSession: makeSession('session-1', 'Demo'),
    });
    useMessagesStore.setState({ messages });

    render(<ChatArea />);

    const summary = '委派受限联网探索：站点 search.example.com、docs.example.com；操作 搜索、抓取；上限为最多 12 次网络操作、单次响应 512 KiB、最多 3 次重定向';
    expect(await screen.findByText('联网委派')).toBeInTheDocument();
    expect(screen.getByText(summary)).toBeInTheDocument();
    expect(screen.queryByText('private goal must not render')).not.toBeInTheDocument();
    expect(screen.queryByText('private medical question')).not.toBeInTheDocument();
    expect(screen.queryByText('https://docs.example.com/private/path?token=secret-value')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /等待确认/ }));

    expect(screen.queryByRole('button', { name: '查看调用参数' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '查看工具输出' })).not.toBeInTheDocument();
    expect(screen.queryByText('private goal must not render')).not.toBeInTheDocument();
    expect(screen.queryByText('private medical question')).not.toBeInTheDocument();
    expect(screen.queryByText('https://docs.example.com/private/path?token=secret-value')).not.toBeInTheDocument();
  });

  it('keeps task-run approvals actionable without an abstract run summary', async () => {
    const messages: Message[] = [{
      id: 'msg-task-run',
      sessionId: 'session-1',
      role: 'assistant',
      content: 'I need approval before writing.',
      createdAt: Date.now(),
      taskRun: {
        id: 'msg-task-run',
        goal: 'Update my notes',
        status: 'awaiting_confirmation',
        plan: ['write_file'],
        confirmationState: 'pending',
        resumable: true,
        stepCount: 1,
        completedStepCount: 0,
      },
      toolCalls: [{
        id: 'call-approval',
        name: 'write_file',
        arguments: JSON.stringify({ path: 'notes.txt', content: 'hello' }),
      }],
      toolResults: [{
        callId: 'call-approval',
        toolName: 'write_file',
        success: false,
        output: "Confirmation required before executing side-effect tool 'write_file'.",
        error: "Confirmation required before executing side-effect tool 'write_file'.",
        confirmationRequired: true,
        confirmationStatus: 'pending',
      }],
    }];
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ ok: true, data: messages }))
    );
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: 'session-1',
      activeSession: makeSession('session-1', 'Demo'),
    });
    useMessagesStore.setState({ messages });

    render(<ChatArea />);

    expect(await screen.findByText('I need approval before writing.')).toBeInTheDocument();
    expect(screen.getByText('写入文件')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '允许一次' })).toBeEnabled();
    expect(screen.getByRole('button', { name: '拒绝' })).toBeEnabled();
    expect(screen.queryByText('Update my notes')).not.toBeInTheDocument();
    expect(screen.queryByText('运行摘要')).not.toBeInTheDocument();
  });

  it('does not render abstract task cards for persisted task state', async () => {
    const messages: Message[] = [
      {
        id: 'assistant-task-1',
        sessionId: 'session-1',
        role: 'assistant',
        content: 'Waiting for approval.',
        createdAt: Date.now(),
        taskRun: {
          id: 'task-1',
          goal: 'Update my notes',
          status: 'awaiting_confirmation',
          plan: ['write_file'],
          confirmationState: 'pending',
          resumable: true,
          stepCount: 1,
          completedStepCount: 0,
        },
      },
      {
        id: 'assistant-task-2',
        sessionId: 'session-1',
        role: 'assistant',
        content: 'The check needs attention.',
        createdAt: Date.now() + 1,
        taskRun: {
          id: 'task-2',
          goal: 'Verify project health',
          status: 'needs_attention',
          plan: ['verify_result'],
          confirmationState: 'none',
          resumable: true,
          stepCount: 2,
          completedStepCount: 1,
        },
      },
    ];
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ ok: true, data: messages }))
    );
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: 'session-1',
      activeSession: makeSession('session-1', 'Demo'),
    });

    render(<ChatArea />);

    expect(await screen.findByText('Waiting for approval.')).toBeInTheDocument();
    expect(screen.queryByLabelText('运行摘要')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('任务队列')).not.toBeInTheDocument();
  });

  it('approves a pending side-effect step and updates the result', async () => {
    const messages: Message[] = [{
      id: 'msg-approval',
      sessionId: 'session-1',
      role: 'assistant',
      content: 'This action needs approval.',
      createdAt: Date.now(),
      toolCalls: [{
        id: 'call-approval',
        name: 'write_file',
        arguments: JSON.stringify({ path: 'notes.txt', content: 'hello' }),
      }],
      toolResults: [{
        callId: 'call-approval',
        toolName: 'write_file',
        success: false,
        output: "Confirmation required before executing side-effect tool 'write_file'.",
        error: "Confirmation required before executing side-effect tool 'write_file'.",
        confirmationRequired: true,
        confirmationStatus: 'pending',
      }],
    }];
    const resumedMessages: Message[] = [{
      ...messages[0],
      taskRun: {
        id: 'msg-approval', goal: 'Write project note', status: 'continue_suggested',
        plan: [], confirmationState: 'approved', resumable: true,
        stepCount: 1, completedStepCount: 1,
      },
      toolResults: [{
        ...messages[0].toolResults![0],
        success: true,
        output: 'Confirmation approved: file written',
        error: undefined,
        confirmationRequired: false,
        confirmationStatus: 'approved',
      }],
    }];
    let confirmationResolved = false;
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const command = (JSON.parse(init?.body as string) as { cmd: string }).cmd;
      if (command === 'resolve_agent_confirmation') {
        confirmationResolved = true;
        return new Response(JSON.stringify({
          ok: true,
          data: {
            callId: 'call-approval',
            toolName: 'write_file',
            success: true,
            output: 'Confirmation approved: file written',
            confirmationRequired: false,
            confirmationStatus: 'approved',
          },
        }));
      }
      // Approval must immediately start the next bounded foreground slice,
      // then reload the durable timeline instead of waiting for a second click.
      if (command === 'continue_agent_task') {
        return new Response(JSON.stringify({ ok: true, data: resumedMessages[0] }));
      }
      if (command === 'get_messages') {
        return new Response(JSON.stringify({
          ok: true,
          data: confirmationResolved ? resumedMessages : messages,
        }));
      }
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: 'session-1',
      activeSession: makeSession('session-1', 'Demo'),
    });
    useMessagesStore.setState({ messages });

    render(<ChatArea />);
    await userEvent.click(await screen.findByRole('button', { name: '允许一次' }));

    await waitFor(() => {
      const commands = vi.mocked(globalThis.fetch).mock.calls.map(([, init]) =>
        (JSON.parse(init?.body as string) as { cmd: string }).cmd,
      );
      expect(commands).toContain('resolve_agent_confirmation');
      expect(commands).toContain('continue_agent_task');
    });
    // After approval, successful tool audit leaves the main conversation.
    await waitFor(() => expect(screen.queryByRole('button', { name: '允许一次' })).not.toBeInTheDocument());
    expect(screen.queryByText(/动作已完成/)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '技术详情' })).not.toBeInTheDocument();
  });

  it('does not continue the agent after an approved draft has an unknown result', async () => {
    const pending: Message = {
      id: 'msg-draft-confirmation',
      sessionId: 'session-1',
      role: 'assistant',
      content: '请确认填写草稿。',
      createdAt: Date.now(),
      toolCalls: [{
        id: 'draft-call', name: 'prepare_message_draft',
        arguments: JSON.stringify({ app_id: 'forged-app', text: 'model-authored draft' }),
      }],
      toolResults: [{
        callId: 'draft-call', toolName: 'prepare_message_draft', success: false,
        output: "Confirmation required before executing side-effect tool 'prepare_message_draft'.",
        confirmationRequired: true, confirmationStatus: 'pending',
      }],
    };
    const settled: Message = {
      ...pending,
      toolResults: [{
        callId: 'draft-call', toolName: 'prepare_message_draft', success: false,
        output: 'Error: {"code":"RESULT_UNKNOWN","message":"inspect before retrying"}',
        error: 'Error: {"code":"RESULT_UNKNOWN","message":"inspect before retrying"}',
        confirmationRequired: false,
      }],
    };
    let resolved = false;
    let resolvedPreviewId: string | undefined;
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd: command, args } = JSON.parse(init?.body as string) as {
        cmd: string;
        args?: { req?: { previewId?: string } };
      };
      if (command === 'preflight_pending_desktop_action') {
        return new Response(JSON.stringify({ ok: true, data: {
          previewId: 'attested-preview-1', operation: 'draft', appDisplayName: '便笺', executableName: 'notes.exe',
          windowTitle: '当前便笺', controlName: '正文', text: 'private draft',
          expiresAt: Date.now() + 60_000,
        } }));
      }
      if (command === 'resolve_agent_confirmation') {
        resolved = true;
        resolvedPreviewId = args?.req?.previewId;
        return new Response(JSON.stringify({ ok: true, data: settled.toolResults![0] }));
      }
      if (command === 'get_messages') {
        return new Response(JSON.stringify({ ok: true, data: resolved ? [settled] : [pending] }));
      }
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    useSessionsStore.setState({
      sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo'),
    });
    useMessagesStore.setState({ messages: [pending] });

    render(<ChatArea />);
    expect(await screen.findByText('private draft')).toBeInTheDocument();
    expect(screen.getByText(/便笺（notes.exe）/)).toBeInTheDocument();
    expect(screen.queryByText('model-authored draft')).not.toBeInTheDocument();
    expect(screen.queryByText('forged-app')).not.toBeInTheDocument();
    await userEvent.click(await screen.findByRole('button', { name: '允许一次' }));
    expect(await screen.findByRole('button', { name: /有操作结果待核对/ })).toBeInTheDocument();
    expect(resolvedPreviewId).toBe('attested-preview-1');
    const commands = vi.mocked(globalThis.fetch).mock.calls.map(([, init]) =>
      (JSON.parse(init?.body as string) as { cmd: string }).cmd,
    );
    expect(commands).not.toContain('continue_agent_task');
  });

  it('does not allow approval of a persisted desktop draft without backend preflight', async () => {
    const pending: Message = {
      id: 'msg-draft-missing-text',
      sessionId: 'session-1',
      role: 'assistant',
      content: '请确认填写草稿。',
      createdAt: Date.now(),
      toolCalls: [{ id: 'draft-call', name: 'prepare_message_draft', arguments: '{"app_id":"notes"}' }],
      toolResults: [{
        callId: 'draft-call', toolName: 'prepare_message_draft', success: false,
        output: 'Confirmation required', confirmationRequired: true, confirmationStatus: 'pending',
      }],
    };
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const command = (JSON.parse(init?.body as string) as { cmd: string }).cmd;
      if (command === 'get_messages') {
        return new Response(JSON.stringify({ ok: true, data: [pending] }));
      }
      if (command === 'preflight_pending_desktop_action') {
        return new Response(JSON.stringify({ ok: false, error: 'No unique edit target' }));
      }
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    useSessionsStore.setState({
      sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo'),
    });
    useMessagesStore.setState({ messages: [pending] });

    render(<ChatArea />);
    expect(await screen.findByRole('alert')).toHaveTextContent('无法核对目标窗口或草稿内容');
    expect(screen.getByRole('button', { name: '允许一次' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '拒绝' })).toBeEnabled();
  });

  it('shows only attested generic field text in a persisted confirmation', async () => {
    const pending: Message = {
      id: 'msg-field-confirmation', sessionId: 'session-1', role: 'assistant',
      content: '请核对输入框。', createdAt: Date.now(),
      toolCalls: [{ id: 'field-call', name: 'set_trusted_app_text', arguments: JSON.stringify({
        app_id: 'forged-app', field_ref: 'forged-ref', text: 'model-authored text',
      }) }],
      toolResults: [{
        callId: 'field-call', toolName: 'set_trusted_app_text', success: false,
        output: 'Confirmation required', confirmationRequired: true, confirmationStatus: 'pending',
      }],
    };
    let approvedPreviewId: string | undefined;
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd, args } = JSON.parse(init?.body as string) as {
        cmd: string; args?: { req?: { previewId?: string } };
      };
      if (cmd === 'get_messages') return new Response(JSON.stringify({ ok: true, data: [pending] }));
      if (cmd === 'preflight_pending_desktop_action') {
        return new Response(JSON.stringify({ ok: true, data: {
          previewId: 'attested-field', operation: 'field', appDisplayName: '记事本',
          executableName: 'notepad.exe', windowTitle: '当前记录', controlName: '正文',
          text: '后端确认内容', expiresAt: Date.now() + 60_000,
        } }));
      }
      if (cmd === 'resolve_agent_confirmation') {
        approvedPreviewId = args?.req?.previewId;
        return new Response(JSON.stringify({ ok: true, data: {
          callId: 'field-call', toolName: 'set_trusted_app_text', success: true,
          output: '{"status":"verified"}',
        } }));
      }
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    useSessionsStore.setState({
      sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo'),
    });
    useMessagesStore.setState({ messages: [pending] });

    render(<ChatArea />);
    expect(await screen.findByText('后端确认内容')).toBeInTheDocument();
    expect(screen.getByText(/将修改这一个输入框/)).toBeInTheDocument();
    expect(screen.queryByText('model-authored text')).not.toBeInTheDocument();
    expect(screen.queryByText('forged-app')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: '允许一次' }));
    await waitFor(() => expect(approvedPreviewId).toBe('attested-field'));
  });

  it.each([
    { action: 'invoke', label: '调用控件' }, { action: 'select', label: '选中控件' },
    { action: 'expand', label: '展开控件' }, { action: 'collapse', label: '收起控件' },
    { action: 'scrollup', label: '向上小幅滚动' }, { action: 'scrolldown', label: '向下小幅滚动' },
  ])('uses the same attested approval for a persisted $action action', async ({ action, label }) => {
    const targetName = action === 'scrollup' || action === 'scrolldown' ? '正文浏览区' : '发送';
    const pending: Message = {
      id: 'msg-invoke-confirmation', sessionId: 'session-1', role: 'assistant',
      content: '请核对控件。', createdAt: Date.now(),
      toolCalls: [{ id: 'invoke-call', name: 'operate_trusted_app_control', arguments: JSON.stringify({
        app_id: 'forged-app', control_ref: 'forged-ref', action, name: 'Safe button',
      }) }],
      toolResults: [{
        callId: 'invoke-call', toolName: 'operate_trusted_app_control', success: false,
        output: 'Confirmation required', confirmationRequired: true, confirmationStatus: 'pending',
      }],
    };
    let approvedPreviewId: string | undefined;
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd, args } = JSON.parse(init?.body as string) as {
        cmd: string; args?: { req?: { previewId?: string } };
      };
      if (cmd === 'get_messages') return new Response(JSON.stringify({ ok: true, data: [pending] }));
      if (cmd === 'preflight_pending_desktop_action') {
        return new Response(JSON.stringify({ ok: true, data: {
          previewId: 'attested-invoke', operation: action, appDisplayName: '邮件',
          executableName: 'mail.exe', windowTitle: '新邮件', controlName: targetName,
          text: null, expiresAt: Date.now() + 60_000,
        } }));
      }
      if (cmd === 'resolve_agent_confirmation') {
        approvedPreviewId = args?.req?.previewId;
        return new Response(JSON.stringify({ ok: true, data: {
          callId: 'invoke-call', toolName: 'operate_trusted_app_control', success: true,
          output: JSON.stringify({ status: action === 'invoke' ? 'dispatched' : 'verified', action }),
        } }));
      }
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    useSessionsStore.setState({
      sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo'),
    });
    useMessagesStore.setState({ messages: [pending] });

    render(<ChatArea />);
    expect(await screen.findByText(new RegExp(`邮件（mail.exe） · 新邮件 · ${targetName}`))).toBeInTheDocument();
    expect(screen.getByText(/可能触发发送、删除等后果/)).toBeInTheDocument();
    expect(screen.queryByText('将填写的内容')).not.toBeInTheDocument();
    expect(screen.queryByText('forged-app')).not.toBeInTheDocument();
    expect(screen.queryByText('Safe button')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: /等待确认/ }));
    expect(screen.getByText('目标窗口和待操作控件以实时预检卡片为准。')).toBeInTheDocument();
    expect(screen.queryByText('forged-ref')).not.toBeInTheDocument();
    expect(screen.queryByText('Safe button')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: `允许${label}一次` }));
    await waitFor(() => expect(approvedPreviewId).toBe('attested-invoke'));
  });

  it.each([
    { toolName: 'operate_trusted_app_control', arguments: '{}' },
    { toolName: 'operate_trusted_app_control', arguments: '{"action":"unknown"}' },
    { toolName: 'operate_trusted_app_control', arguments: '{"action":"ScrollUp"}' },
    { toolName: 'operate_trusted_app_control', arguments: '{"action":"scroll_up"}' },
    { toolName: 'invoke_trusted_app_control', arguments: '{"action":"invoke"}' },
  ])('fails closed for persisted invalid or legacy $toolName without target inspection', async ({ toolName, arguments: args }) => {
    const pending: Message = {
      id: 'msg-blocked-control', sessionId: 'session-1', role: 'assistant', content: '', createdAt: Date.now(),
      toolCalls: [{ id: 'blocked-call', name: toolName, arguments: args }],
      toolResults: [{ callId: 'blocked-call', toolName, success: false, output: 'Confirmation required',
        confirmationRequired: true, confirmationStatus: 'pending' }],
    };
    const commands: string[] = [];
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd } = JSON.parse(init?.body as string) as { cmd: string };
      commands.push(cmd);
      return new Response(JSON.stringify({ ok: true, data: cmd === 'get_messages' ? [pending] : [] }));
    });
    useSessionsStore.setState({ sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo') });
    useMessagesStore.setState({ messages: [pending] });
    render(<ChatArea />);
    expect(await screen.findByRole('alert')).toHaveTextContent('请求动作缺失、无效或已停用');
    expect(screen.getByRole('button', { name: '允许一次' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: '重新核对' })).not.toBeInTheDocument();
    expect(commands).not.toContain('preflight_pending_desktop_action');
    expect(commands).not.toContain('resolve_agent_confirmation');
  });

  it.each([
    { toolName: 'set_trusted_app_text', operation: 'field', text: '后端确认内容', button: '允许一次',
      explanation: '操作停止后输入框可能已改变；请先去目标应用核对，勿自动重试。' },
    { toolName: 'operate_trusted_app_control', operation: 'invoke', text: null, button: '允许调用控件一次',
      explanation: '操作停止后控件可能已被触发；请重新观察目标确认结果，勿自动重试。' },
    { toolName: 'operate_trusted_app_control', operation: 'select', text: null, button: '允许选中控件一次',
      explanation: '操作停止后控件可能已被触发；请重新观察目标确认结果，勿自动重试。' },
    { toolName: 'operate_trusted_app_control', operation: 'expand', text: null, button: '允许展开控件一次',
      explanation: '操作停止后控件可能已被触发；请重新观察目标确认结果，勿自动重试。' },
    { toolName: 'operate_trusted_app_control', operation: 'collapse', text: null, button: '允许收起控件一次',
      explanation: '操作停止后控件可能已被触发；请重新观察目标确认结果，勿自动重试。' },
    { toolName: 'operate_trusted_app_control', operation: 'scrollup', text: null, button: '允许向上小幅滚动一次',
      explanation: '操作停止后控件可能已被触发；请重新观察目标确认结果，勿自动重试。' },
    { toolName: 'operate_trusted_app_control', operation: 'scrolldown', text: null, button: '允许向下小幅滚动一次',
      explanation: '操作停止后控件可能已被触发；请重新观察目标确认结果，勿自动重试。' },
  ])('does not repeat $operation approval when both confirmation and refresh fail', async ({ toolName, operation, text, button, explanation }) => {
    const pending: Message = {
      id: 'msg-field-uncertain', sessionId: 'session-1', role: 'assistant',
      content: '请核对输入框。', createdAt: Date.now(),
      taskRun: {
        id: 'msg-field-uncertain', goal: 'Fill field', status: 'awaiting_confirmation',
        plan: [], confirmationState: 'pending', resumable: true,
        stepCount: 1, completedStepCount: 0,
      },
      toolCalls: [{ id: 'field-call', name: toolName, arguments: JSON.stringify({ action: operation, text: 'model text' }) }],
      toolResults: [{
        callId: 'field-call', toolName, success: false,
        output: 'Confirmation required', confirmationRequired: true, confirmationStatus: 'pending',
      }],
    };
    let approvalAttempted = false;
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd } = JSON.parse(init?.body as string) as { cmd: string };
      if (cmd === 'get_messages') {
        if (approvalAttempted) throw new Error('durable refresh unavailable');
        return new Response(JSON.stringify({ ok: true, data: [pending] }));
      }
      if (cmd === 'preflight_pending_desktop_action') {
        return new Response(JSON.stringify({ ok: true, data: {
          previewId: 'field-preview', operation, appDisplayName: '记事本',
          executableName: 'notepad.exe', windowTitle: '当前记录', controlName: '正文',
          text, expiresAt: Date.now() + 60_000,
        } }));
      }
      if (cmd === 'resolve_agent_confirmation') {
        approvalAttempted = true;
        throw new Error('IPC result lost');
      }
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    useSessionsStore.setState({
      sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo'),
    });
    useMessagesStore.setState({ messages: [pending] });

    render(<ChatArea />);
    await screen.findByText(/已核对目标：记事本/);
    await userEvent.click(screen.getByRole('button', { name: button }));

    expect(await screen.findByText(explanation)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: button })).not.toBeInTheDocument();
    const local = useMessagesStore.getState().messages.find((message) => message.id === pending.id);
    expect(local?.toolResults?.find((result) => result.callId === 'field-call')?.error).toContain('RESULT_UNKNOWN');
    expect(local?.taskRun?.resumable).toBe(false);
  });

  it('does not continue after another step succeeds while a desktop write remains unknown', async () => {
    const unknown = {
      callId: 'field-call', toolName: 'set_trusted_app_text', success: false,
      output: 'Error: {"code":"RESULT_UNKNOWN","message":"inspect field"}',
      error: 'Error: {"code":"RESULT_UNKNOWN","message":"inspect field"}',
      confirmationRequired: false,
    };
    const pending: Message = {
      id: 'msg-two-steps', sessionId: 'session-1', role: 'assistant',
      content: '还有一步等待确认。', createdAt: Date.now(),
      toolCalls: [
        { id: 'field-call', name: 'set_trusted_app_text', arguments: '{}' },
        { id: 'file-call', name: 'write_file', arguments: '{"path":"notes.txt"}' },
      ],
      toolResults: [unknown, {
        callId: 'file-call', toolName: 'write_file', success: false,
        output: 'Confirmation required', confirmationRequired: true, confirmationStatus: 'pending',
      }],
    };
    const settled: Message = {
      ...pending,
      taskRun: {
        id: pending.id, goal: 'Two steps', status: 'needs_attention',
        plan: [], confirmationState: 'approved', resumable: false,
        stepCount: 2, completedStepCount: 1,
      },
      toolResults: [unknown, {
        callId: 'file-call', toolName: 'write_file', success: true,
        output: 'Confirmation approved: file written', confirmationRequired: false,
        confirmationStatus: 'approved',
      }],
    };
    let approved = false;
    const commands: string[] = [];
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd } = JSON.parse(init?.body as string) as { cmd: string };
      commands.push(cmd);
      if (cmd === 'get_messages') return new Response(JSON.stringify({ ok: true, data: approved ? [settled] : [pending] }));
      if (cmd === 'resolve_agent_confirmation') {
        approved = true;
        return new Response(JSON.stringify({ ok: true, data: settled.toolResults![1] }));
      }
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    useSessionsStore.setState({
      sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo'),
    });
    useMessagesStore.setState({ messages: [pending] });

    render(<ChatArea />);
    await userEvent.click(await screen.findByRole('button', { name: '允许一次' }));
    await waitFor(() => expect(screen.queryByRole('button', { name: '允许一次' })).not.toBeInTheDocument());
    expect(commands).not.toContain('continue_agent_task');
    expect(screen.getByText('操作停止后输入框可能已改变；请先去目标应用核对，勿自动重试。')).toBeInTheDocument();
  });

  it('rejects a pending side-effect step and keeps recovery visible', async () => {
    const messages: Message[] = [{
      id: 'msg-approval',
      sessionId: 'session-1',
      role: 'assistant',
      content: 'This action needs approval.',
      createdAt: Date.now(),
      toolCalls: [{
        id: 'call-approval',
        name: 'write_file',
        arguments: JSON.stringify({ path: 'notes.txt', content: 'hello' }),
      }],
      toolResults: [{
        callId: 'call-approval',
        toolName: 'write_file',
        success: false,
        output: "Confirmation required before executing side-effect tool 'write_file'.",
        error: "Confirmation required before executing side-effect tool 'write_file'.",
        confirmationRequired: true,
        confirmationStatus: 'pending',
      }],
    }];
    const resumedMessages: Message[] = [{
      ...messages[0],
      toolResults: [{
        ...messages[0].toolResults![0],
        output: "Confirmation rejected: user rejected side-effect tool 'write_file'.",
        error: "Confirmation rejected: user rejected side-effect tool 'write_file'.",
        confirmationRequired: false,
        confirmationStatus: 'rejected',
      }],
    }];
    let confirmationResolved = false;
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const command = (JSON.parse(init?.body as string) as { cmd: string }).cmd;
      if (command === 'resolve_agent_confirmation') {
        confirmationResolved = true;
        return new Response(JSON.stringify({
          ok: true,
          data: {
            callId: 'call-approval',
            toolName: 'write_file',
            success: false,
            output: "Confirmation rejected: user rejected side-effect tool 'write_file'.",
            error: "Confirmation rejected: user rejected side-effect tool 'write_file'.",
            confirmationRequired: false,
            confirmationStatus: 'rejected',
          },
        }));
      }
      if (command === 'get_messages') {
        return new Response(JSON.stringify({
          ok: true,
          data: confirmationResolved ? resumedMessages : messages,
        }));
      }
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: 'session-1',
      activeSession: makeSession('session-1', 'Demo'),
    });
    useMessagesStore.setState({ messages });

    render(<ChatArea />);
    await userEvent.click(await screen.findByRole('button', { name: '拒绝' }));

    await waitFor(() => {
      const commands = vi.mocked(globalThis.fetch).mock.calls.map(([, init]) =>
        (JSON.parse(init?.body as string) as { cmd: string }).cmd,
      );
      expect(commands).toContain('resolve_agent_confirmation');
    });
    const commands = vi.mocked(globalThis.fetch).mock.calls.map(([, init]) =>
      (JSON.parse(init?.body as string) as { cmd: string }).cmd,
    );
    expect(commands).not.toContain('continue_agent_task');
    // Rejected step remains discoverable through the failed-action summary.
    expect(await screen.findByText('有步骤失败')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '拒绝' })).not.toBeInTheDocument();
  });
});

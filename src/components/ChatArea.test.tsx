import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ChatArea, restoreRetryableLiveConfirmation, workspaceFilePathFromHref } from '$components/ChatArea';
import { ContentBlockRenderer } from '$components/ContentBlockRenderer';
import { AUTOMATIONS_UPDATED_EVENT } from '$lib/automation-events';
import { useSessionsStore } from '$stores/sessions';
import { useWorkspacesStore } from '$stores/workspaces';
import { useMessagesStore, type LiveBlock } from '$stores/messages';
import { fromRustConfig, useSettingsStore } from '$stores/settings';
import { useThinkingEffortStore } from '$stores/thinkingEffort';
import { usePreferencesStore } from '$stores/preferences';
import type { Message } from '$types';
import type { WorkspaceAutomationStatus } from '$lib/commands/automation';
import type { ApiProviderConfig } from '$lib/commands/settings';
import * as messageCommands from '$lib/commands/message';

const automationFixture = vi.hoisted(() => ({ items: [] as WorkspaceAutomationStatus[] }));
const quotaNotice = '当前模型用量已达限制。任务状态已安全保留，请检查服务商额度，恢复后发送新消息继续；不会自动重做已完成操作或切换计费来源。';

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
  getWorkspaceAutomationOverview: () => Promise.resolve({ items: automationFixture.items, truncated: false }),
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

function desktopApprovalFixture() {
  const pending: Message = {
    id: 'desktop-turn', sessionId: 'session-1', role: 'assistant', content: '请核对输入框。', createdAt: Date.now(),
    taskRun: { id: 'desktop-turn', goal: 'Fill and check', status: 'awaiting_confirmation',
      plan: [], confirmationState: 'pending', resumable: true, stepCount: 1, completedStepCount: 0 },
    toolCalls: [{ id: 'field-call', name: 'set_trusted_app_text', arguments: '{"field_ref":"mock-field-ref","text":"计划内容"}' }],
    toolResults: [{ callId: 'field-call', toolName: 'set_trusted_app_text', success: false,
      output: 'Confirmation required', confirmationRequired: true, confirmationStatus: 'pending' }],
  };
  const approved: Message = { ...pending,
    taskRun: { ...pending.taskRun!, status: 'continue_suggested', confirmationState: 'approved', completedStepCount: 1 },
    toolResults: [{ callId: 'field-call', toolName: 'set_trusted_app_text', success: true,
      output: 'Confirmation approved: {"status":"verified"}', confirmationRequired: false, confirmationStatus: 'approved' }],
  };
  const preview = { previewId: 'field-preview', operation: 'field', appDisplayName: '便笺', executableName: 'notes.exe',
    windowTitle: '当前记录', controlName: '正文', text: '计划内容', expiresAt: Date.now() + 60_000 };
  return { pending, approved, preview };
}

describe('Main Agent file citations', () => {
  it('accepts only project-relative angelbot-file links', () => {
    expect(workspaceFilePathFromHref('angelbot-file:src%2Fmain.ts')).toBe('src/main.ts');
    expect(workspaceFilePathFromHref('angelbot-file:docs%2F%E7%94%9F%E6%B4%BB%20%E5%B0%8F%E7%BB%93%231.md')).toBe('docs/生活 小结#1.md');
    expect(workspaceFilePathFromHref('angelbot-file:C%3A%2Fprivate%2Fnotes.txt')).toBeNull();
    expect(workspaceFilePathFromHref('angelbot-file:notes.txt%3Aprivate')).toBeNull();
    expect(workspaceFilePathFromHref('angelbot-file:notes%0A.txt')).toBeNull();
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
      inputSessionId: null,
      inputToken: {},
      draftInputs: {},
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
    await user.click(screen.getByText('daily.csv'));
    const preview = screen.getByLabelText('附件文字 daily.csv');
    expect(preview).toBeVisible();
    const previewText = preview.textContent;
    expect(submitted).toHaveLength(0);
    await user.click(screen.getByRole('button', { name: '发送' }));
    await screen.findByText('已读完整文本。');
    expect(submitted).toMatchObject([{ sessionId: personalSession.id, content: '', textAttachments: [{ name: 'daily.csv', text: 'Named Project,不要改原文件' }] }]);
    expect(useWorkspacesStore.getState().activeWorkspaceId).toBe('personal');
    expect(screen.queryByLabelText('待发送文本附件')).not.toBeInTheDocument();
    const history = screen.getByLabelText('已发送文本附件');
    expect(within(history).getByText('daily.csv')).toBeInTheDocument();
    await user.click(within(history).getByText('daily.csv'));
    expect(within(history).getByText('Named Project,不要改原文件')).toBeVisible();
    expect(within(history).getByLabelText('附件文字 daily.csv').textContent).toBe(previewText);
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

  it.each([false, true])('rechecks saved model readiness after provider failure without touching the draft or retrying (%s)', async (connected) => {
    const user = userEvent.setup();
    const session = makeSession('model-failure', 'Model failure');
    const plan: ApiProviderConfig = {
      provider: 'openai', model: 'gpt-6-luna', base_url: 'https://api.openai.com/v1',
      api_key: '', has_api_key: false, max_tokens: 4096, temperature: 0.7,
      protocol: 'openai_responses', auth_mode: 'chatgpt_plan', credential_ref: 'opaque-account',
      plan_connected: true, plan_enabled: true,
    };
    const saved = fromRustConfig(plan);
    const draft = { ...saved, model: 'unsaved-draft-model' };
    useSettingsStore.setState({ apiConfig: draft, activeApiConfig: saved, apiConfigLoaded: true, apiConfigLoadError: null });
    const failed: Message = {
      id: 'failed-provider-turn', sessionId: session.id, role: 'assistant',
      content: quotaNotice, createdAt: 2,
      taskRun: { id: 'failed-provider-turn', goal: 'Check automations', status: 'provider_unavailable',
        plan: [], confirmationState: 'none', resumable: false, stepCount: 1, completedStepCount: 1 },
      toolResults: [{ callId: 'known-read', toolName: 'list_automations', success: true, output: '[]' }],
    };
    let durable: Message[] = [];
    const commands: string[] = [];
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd, args } = JSON.parse(String(init?.body ?? '{}'));
      commands.push(cmd);
      if (cmd === 'get_messages') return new Response(JSON.stringify({ ok: true, data: durable }));
      if (cmd === 'send_message') {
        durable = [{ id: args.req.clientMessageId, sessionId: session.id, role: 'user', content: args.req.content, createdAt: 1 }, failed];
        return new Response(JSON.stringify({ ok: true, data: failed }));
      }
      if (cmd === 'load_api_config') return new Response(JSON.stringify({ ok: true,
        data: { ...plan, plan_connected: connected, plan_enabled: connected, plan_account_label: 'observed-account' } }));
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    useSessionsStore.setState({ activeSessionId: session.id, activeSession: session, sessions: [session] });
    render(<ChatArea />);
    expect(screen.queryByText('连接 ChatGPT 套餐后即可开始对话')).not.toBeInTheDocument();
    await user.type(screen.getByRole('textbox'), '查看自动化');
    await user.click(screen.getByRole('button', { name: '发送' }));
    await screen.findByText(failed.content);
    await waitFor(() => expect(useSettingsStore.getState().activeApiConfig.planAccountLabel).toBe('observed-account'));
    expect(useSettingsStore.getState().activeApiConfig).toMatchObject({
      model: plan.model, credentialRef: plan.credential_ref, authMode: 'chatgpt_plan', planEnabled: connected,
    });
    expect(useSettingsStore.getState().apiConfig).toBe(draft);
    if (!connected) expect(screen.getByText('连接 ChatGPT 套餐后即可开始对话')).toBeVisible();
    else expect(screen.queryByText('连接 ChatGPT 套餐后即可开始对话')).not.toBeInTheDocument();
    await act(async () => { useMessagesStore.getState().loadMessages([...durable]); });
    expect(screen.getByText(quotaNotice)).toBeVisible();
    expect(screen.queryByText(/请检查模型连接/)).not.toBeInTheDocument();
    expect(commands.filter((cmd) => cmd === 'load_api_config')).toHaveLength(1);
    expect(commands.filter((cmd) => cmd === 'send_message')).toHaveLength(1);
    expect(commands.some((cmd) => cmd === 'save_api_config' || cmd.startsWith('chatgpt_plan_'))).toBe(false);
  });

  it.each([
    { persisted: false, rejection: 'Error', failure: 'none' },
    { persisted: true, rejection: 'string', failure: 'none' },
    { persisted: true, rejection: 'string', failure: 'current' },
    { persisted: false, rejection: 'Error', failure: 'current-without-user' },
    { persisted: true, rejection: 'string', failure: 'wrong-parent' },
    { persisted: true, rejection: 'Error', failure: 'wrong-session' },
    { persisted: true, rejection: 'Error', failure: 'wrong-task' },
    { persisted: true, rejection: 'string', failure: 'completed' },
    { persisted: true, rejection: 'string', failure: 'running' },
    { persisted: true, rejection: 'Error', failure: 'awaiting_confirmation' },
    { persisted: true, rejection: 'string', failure: 'empty-content' },
    { persisted: true, rejection: 'Error', failure: 'reload-failed' },
    { persisted: true, rejection: 'string', failure: 'load-failed' },
  ])('preserves a quota notice and the accepted snapshot after $rejection rejection ($persisted, $failure)', async ({ persisted, rejection, failure }) => {
    const user = userEvent.setup();
    const session = makeSession('failure-session', 'Failure');
    let durable: Message[] = [];
    let sendCount = 0;
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd } = JSON.parse(String(init?.body ?? '{}'));
      if (cmd === 'get_messages') return new Response(JSON.stringify(
        failure === 'reload-failed' && sendCount > 0
          ? { ok: false, error: 'History reload unavailable' }
          : { ok: true, data: durable }));
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    vi.spyOn(messageCommands, 'sendMessage').mockImplementation(async (args) => {
      sendCount++;
      if (!args.clientMessageId) throw new Error('Expected the durable client message ID');
      if (persisted) durable = [{ id: args.clientMessageId, sessionId: session.id, role: 'user',
        content: args.content, textAttachments: args.textAttachments, createdAt: 1 }];
      if (failure !== 'none') durable.push({
        id: 'durable-failed-reply', sessionId: failure === 'wrong-session' ? 'another-session' : session.id,
        role: 'assistant', parentId: failure === 'wrong-parent' ? 'older-user' : args.clientMessageId,
        content: failure === 'empty-content' ? '' : quotaNotice, createdAt: 2,
        taskRun: { id: failure === 'wrong-task' ? 'another-run' : 'durable-failed-reply', goal: args.content,
          status: ['completed', 'running', 'awaiting_confirmation'].includes(failure) ? failure : 'needs_attention',
          plan: [], confirmationState: 'none', resumable: true, stepCount: 0, completedStepCount: 0 },
      });
      throw rejection === 'string' ? quotaNotice : new Error(quotaNotice);
    });
    const originalLoadMessages = useMessagesStore.getState().loadMessages;
    if (failure === 'load-failed') useMessagesStore.setState({ loadMessages: (messages) => {
      if (messages.some((message) => message.id === 'durable-failed-reply')) throw new Error('Visible history hydration failed');
      originalLoadMessages(messages);
    } });
    try {
      useSessionsStore.setState({ activeSessionId: session.id, activeSession: session, sessions: [session] });
      render(<ChatArea />);
      await user.upload(screen.getByLabelText('选择文本附件'), new File(['important'], 'keep.txt'));
      await screen.findByText('keep.txt');
      await user.type(screen.getByRole('textbox'), '请处理这份文本');
      await user.click(screen.getByRole('button', { name: '发送' }));
      if (failure === 'current') {
        expect(await screen.findByText(quotaNotice)).toBeVisible();
        expect(screen.queryByText(`发送失败：${quotaNotice}`)).not.toBeInTheDocument();
      } else expect(await screen.findByText(`发送失败：${quotaNotice}`)).toBeVisible();
      expect(screen.queryByText(/请检查模型连接/)).not.toBeInTheDocument();
      if (persisted && failure !== 'reload-failed') {
        expect(screen.queryByLabelText('待发送文本附件')).not.toBeInTheDocument();
        expect(screen.getByLabelText('已发送文本附件')).toHaveTextContent('keep.txt');
        expect(screen.getByRole('textbox')).toHaveValue('');
      } else {
        expect(await screen.findByRole('alert')).toHaveTextContent('附件仍保留');
        expect(screen.getByLabelText('待发送文本附件')).toHaveTextContent('keep.txt');
        if (failure === 'reload-failed') expect(screen.getByLabelText('已发送文本附件')).toHaveTextContent('keep.txt');
        else expect(screen.queryByLabelText('已发送文本附件')).not.toBeInTheDocument();
        expect(screen.getByRole('textbox')).toHaveValue('请处理这份文本');
      }
      expect(sendCount).toBe(1);
    } finally {
      useMessagesStore.setState({ loadMessages: originalLoadMessages });
    }
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
      return true;
    });
    useWorkspacesStore.setState({ activeWorkspaceId: 'personal', workspaces, openWorkspace: open });
    useSessionsStore.setState({ activeSessionId: personalSession.id, activeSession: personalSession, sessions: [personalSession, projectSession] });
    useMessagesStore.getState().activateInput(projectSession.id);
    useMessagesStore.getState().setInput('项目中未发送的草稿');
    useMessagesStore.getState().activateInput(personalSession.id);
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
        expect(screen.getByRole('textbox')).toHaveValue('项目中未发送的草稿');
        await waitFor(() => expect(useMessagesStore.getState().draftInputs[personalSession.id]).toBeUndefined());
      }
    } finally { useWorkspacesStore.setState({ openWorkspace: originalOpen }); }
  });

  it('never restores a late failed project-route draft after leaving and returning to its source', async () => {
    const user = userEvent.setup();
    const personalSession = makeSession('route-source', '日常');
    const otherSession = makeSession('route-other', 'Other');
    const originalOpen = useWorkspacesStore.getState().openWorkspace;
    let rejectOpen: (reason: Error) => void = () => {};
    const open = vi.fn(() => new Promise<boolean>((_resolve, reject) => { rejectOpen = reject; }));
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
      expect(screen.getByRole('textbox')).toHaveValue('交给 Named Project 处理');
      await user.clear(screen.getByRole('textbox'));
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
      const reminder: WorkspaceAutomationStatus = {
        id: 'triggered-reminder', title: '回看当前项目进展',
        triggerKind: 'once', triggerValue: '2026-10-03T16:00:00', enabled: true,
        executorKind: 'notification', workspaceId: 'project-current', nextRunAt: now + 3600,
        lastRunAt: null, latestRun: null, lastSkippedAt: null, maaConfig: null,
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
            ? { ...item, enabled: false, nextRunAt: null }
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
        { ...reminder, enabled: false, nextRunAt: null, lastRunAt: now - 1 },
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

  it('keeps text and file-reference drafts in their original session across switches and remounts', async () => {
    const firstSession = makeSession('session-1', 'First');
    const secondSession = makeSession('session-2', 'Second');
    useSessionsStore.setState({
      sessions: [firstSession, secondSession],
      activeSessionId: firstSession.id,
      activeSession: firstSession,
    });
    const { unmount } = render(<ChatArea />);

    act(() => useMessagesStore.getState().setInput('[引用文件: notes.md]'));
    expect(useMessagesStore.getState().input).toBe('[引用文件: notes.md]');
    const submittedToken = useMessagesStore.getState().inputToken;
    act(() => {
      useMessagesStore.getState().setInput('');
      useMessagesStore.getState().setInput('[引用文件: notes.md]');
      useMessagesStore.getState().clearSubmittedInput(firstSession.id, '[引用文件: notes.md]', submittedToken);
    });
    expect(useMessagesStore.getState().input).toBe('[引用文件: notes.md]');

    act(() => {
      useSessionsStore.setState({
        sessions: [firstSession, secondSession],
        activeSessionId: secondSession.id,
        activeSession: secondSession,
      });
    });

    await waitFor(() => expect(useMessagesStore.getState().input).toBe(''));
    act(() => useMessagesStore.getState().setInput('第二个项目的草稿'));
    act(() => useSessionsStore.setState({ activeSessionId: firstSession.id, activeSession: firstSession }));
    await waitFor(() => expect(useMessagesStore.getState().input).toBe('[引用文件: notes.md]'));
    unmount();
    render(<ChatArea />);
    expect(useMessagesStore.getState().input).toBe('[引用文件: notes.md]');
    act(() => useSessionsStore.setState({ activeSessionId: secondSession.id, activeSession: secondSession }));
    await waitFor(() => expect(useMessagesStore.getState().input).toBe('第二个项目的草稿'));
  });

  it('shows failed history reads without an empty-chat claim and retries only the read while retaining text', async () => {
    const session = makeSession('session-1', 'Saved Main');
    useSessionsStore.setState({ activeSessionId: session.id, activeSession: session });
    const getHistory = vi.spyOn(globalThis, 'fetch').mockImplementation(async (_url, init) => {
      const { cmd } = JSON.parse(String(init?.body));
      if (cmd === 'get_messages') return new Response(JSON.stringify({ ok: false, error: 'temporary read failure' }));
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    render(<ChatArea />);
    await screen.findByText('暂时无法读取对话记录。输入草稿已保留，请重新载入后继续。');
    expect(screen.getByRole('textbox')).toBeEnabled();
    await userEvent.type(screen.getByRole('textbox'), '未发送的内容');
    expect(screen.getByRole('button', { name: '发送' })).toBeDisabled();
    expect(screen.queryByText('直接说出想处理的事。需要具体项目文件时，AngelBot 会切换到对应项目再继续。')).not.toBeInTheDocument();
    getHistory.mockImplementation(async (_url, init) => {
      const { cmd } = JSON.parse(String(init?.body));
      return new Response(JSON.stringify({ ok: true, data: cmd === 'get_messages' ? [{
        id: 'saved', sessionId: session.id, role: 'assistant', content: '原有记录', createdAt: 1,
      }] : [] }));
    });
    fireEvent.click(screen.getByRole('button', { name: '重新载入对话' }));
    await screen.findByText('原有记录');
    expect(useMessagesStore.getState().input).toBe('未发送的内容');
    expect(screen.getByRole('button', { name: '发送' })).toBeEnabled();
    expect(getHistory.mock.calls.map(([, init]) => JSON.parse(String(init?.body)).cmd)).not.toContain('send_message');
  });

  it.each(['switch', 'leave'] as const)('does not dispatch a late project route after the user chooses to %s', async (action) => {
    const user = userEvent.setup();
    const source = makeSession('source-main', '日常');
    const target = makeSession('target-main', 'Named Project');
    const other = makeSession('other-main', 'Other');
    const workspaces = [
      { id: 'personal', name: '日常', kind: 'personal' as const, activeSessionId: source.id, createdAt: 1, updatedAt: 1 },
      { id: 'project', name: 'Named Project', kind: 'project' as const, activeSessionId: target.id, createdAt: 1, updatedAt: 1 },
      { id: 'other', name: 'Other', kind: 'project' as const, activeSessionId: other.id, createdAt: 1, updatedAt: 1 },
    ];
    let finishOpen!: (response: Response) => void;
    const pending = new Promise<Response>((resolve) => { finishOpen = resolve; });
    const commands: string[] = [];
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_url, init) => {
      const { cmd, args } = JSON.parse(String(init?.body));
      commands.push(cmd);
      if (cmd === 'open_workspace') {
        if (args.id === 'project') return pending;
        return new Response(JSON.stringify({ ok: true, data: { workspace: workspaces[2], session: other } }));
      }
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    useWorkspacesStore.setState({ workspaces, activeWorkspaceId: 'personal' });
    useSessionsStore.setState({ sessions: [source], activeSessionId: source.id, activeSession: source });
    const frame = render(<ChatArea />);
    await user.type(screen.getByRole('textbox'), '请在 Named Project 检查资料');
    await user.click(screen.getByRole('button', { name: '发送' }));
    await waitFor(() => expect(commands).toContain('open_workspace'));
    if (action === 'switch') await act(async () => { await useWorkspacesStore.getState().openWorkspace('other'); });
    else frame.unmount();
    await act(async () => { finishOpen(new Response(JSON.stringify({ ok: true, data: { workspace: workspaces[1], session: target } }))); });
    if (action === 'switch') {
      expect(useWorkspacesStore.getState().activeWorkspaceId).toBe('other');
      expect(useSessionsStore.getState().activeSessionId).toBe(other.id);
      expect(screen.getByRole('textbox')).toHaveValue('');
      expect(useMessagesStore.getState().draftInputs[source.id]?.text).toBe('请在 Named Project 检查资料');
    } else expect(useMessagesStore.getState().input).toBe('请在 Named Project 检查资料');
    expect(commands).not.toContain('send_message');
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

  it('reloads deleted branches without replaying unknown deletions or crossing workspaces', async () => {
    const session = makeSession('session-1', 'Demo');
    const original: Message[] = [
      { id: 'prefix-user', sessionId: session.id, role: 'user', content: 'Retained question', createdAt: 1 },
      { id: 'prefix-reply', sessionId: session.id, role: 'assistant', content: 'Retained reply', createdAt: 2 },
      { id: 'delete-user', sessionId: session.id, role: 'user', content: 'Delete this branch', createdAt: 3 },
      { id: 'delete-reply', sessionId: session.id, role: 'assistant', content: 'Deleted branch reply', createdAt: 4 },
    ];
    const fetch = vi.spyOn(globalThis, 'fetch').mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: original })));
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
    useSessionsStore.setState({ sessions: [session], activeSessionId: session.id, activeSession: session });
    render(<ChatArea />);
    await screen.findByText('Delete this branch');
    await userEvent.click(screen.getAllByRole('button', { name: '删除' })[1]);
    expect(confirm).toHaveBeenLastCalledWith(expect.stringContaining('后续分支'));
    expect(fetch).toHaveBeenCalledTimes(1);

    let finish!: (response: Response) => void;
    fetch.mockReturnValueOnce(new Promise<Response>((resolve) => { finish = resolve; }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: original.slice(0, 2) })));
    confirm.mockReturnValue(true);
    await userEvent.click(screen.getAllByRole('button', { name: '删除' })[1]);
    expect(screen.getByText('正在更新对话记录…')).toBeVisible();
    expect(screen.getByPlaceholderText('给 AngelBot 发消息…')).toBeDisabled();
    for (const button of screen.getAllByRole('button', { name: '删除' })) expect(button).toBeDisabled();
    for (const button of screen.getAllByRole('button', { name: '复制' })) expect(button).toBeEnabled();
    await userEvent.click(screen.getAllByRole('button', { name: '删除' })[1]);
    expect(confirm).toHaveBeenCalledTimes(2);
    await act(async () => finish(new Response(JSON.stringify({ ok: true, data: null }))));
    await waitFor(() => expect(screen.queryByText('Delete this branch')).not.toBeInTheDocument());
    expect(screen.queryByText('Deleted branch reply')).not.toBeInTheDocument();
    expect(screen.getByText('Retained reply')).toBeVisible();
    expect(screen.getByPlaceholderText('给 AngelBot 发消息…')).toBeEnabled();

    // The delete may have committed even when its subsequent projection fails.
    fetch.mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: null })))
      .mockRejectedValueOnce(new Error('PRIVATE_HISTORY_DIAGNOSTIC'));
    await userEvent.click(screen.getByRole('button', { name: '删除' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('未能确认删除或更新对话');
    expect(screen.queryByText(/PRIVATE_HISTORY_DIAGNOSTIC/)).not.toBeInTheDocument();
    expect(screen.getByText('Retained question')).toBeVisible();
    expect(screen.getByRole('button', { name: '删除' })).toBeDisabled();
    expect(screen.getByPlaceholderText('给 AngelBot 发消息…')).toBeDisabled();
    fetch.mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: [] })));
    await userEvent.click(screen.getByRole('button', { name: '重新载入对话' }));
    await waitFor(() => expect(screen.queryByText('Retained question')).not.toBeInTheDocument());
    const deleteCalls = () => fetch.mock.calls.filter(([, init]) => JSON.parse(String(init?.body)).cmd === 'delete_message');
    expect(deleteCalls()).toHaveLength(2);
    expect(confirm).toHaveBeenCalledTimes(3);

    act(() => useMessagesStore.getState().loadMessages(original));
    fetch.mockReturnValueOnce(new Promise<Response>((resolve) => { finish = resolve; }));
    await userEvent.click(screen.getAllByRole('button', { name: '删除' })[1]);
    const other = makeSession('session-2', 'Other');
    fetch.mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: [
      { id: 'other-user', sessionId: other.id, role: 'user', content: 'Other workspace history', createdAt: 1 },
    ] })));
    act(() => useSessionsStore.setState({ sessions: [session, other], activeSessionId: other.id, activeSession: other }));
    await screen.findByText('Other workspace history');
    await act(async () => finish(new Response(JSON.stringify({ ok: true, data: null }))));
    expect(screen.getByText('Other workspace history')).toBeVisible();
    expect(screen.queryByText('正在更新对话记录…')).not.toBeInTheDocument();
    expect(screen.queryByText('Delete this branch')).not.toBeInTheDocument();
    expect(deleteCalls()).toHaveLength(3);
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
    const replacement: Message = { id: 'assistant-2', sessionId: session.id, role: 'assistant', content: 'Replacement reply', createdAt: Date.now() + 2 };
    const fetch = vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: originalMessages })))
      .mockReturnValueOnce(pendingReply)
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: [{ ...originalMessages[0], content: 'Edited user message' }, replacement] })));
    const confirm = vi.spyOn(window, 'confirm');
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
    expect(screen.getByRole('button', { name: '编辑' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '删除' })).toBeDisabled();
    await userEvent.click(screen.getByRole('button', { name: '编辑' }));
    await userEvent.click(screen.getByRole('button', { name: '删除' }));
    expect(screen.queryByRole('textbox', { name: '编辑后的消息内容' })).not.toBeInTheDocument();
    expect(confirm).not.toHaveBeenCalled();
    expect(fetch).toHaveBeenCalledTimes(2);

    resolveReply(new Response(JSON.stringify({
      ok: true,
      data: replacement,
    })));
    expect(await screen.findByText('Replacement reply')).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole('button', { name: '编辑' })).toBeEnabled());
    expect(screen.getByRole('button', { name: '删除' })).toBeEnabled();
    expect(screen.getByRole('button', { name: '复制' })).toBeEnabled();
  });

  it('reconciles failed edits against saved history without replaying the request', async () => {
    const session = makeSession('session-1', 'Demo');
    const original: Message[] = [
      { id: 'user-1', sessionId: session.id, role: 'user', content: 'Original user message', createdAt: 1 },
      { id: 'assistant-1', sessionId: session.id, role: 'assistant', content: 'Original reply', createdAt: 2 },
    ];
    const saved: Message[] = [
      { ...original[0], content: 'Edited user message', createdAt: 3 },
      { id: 'assistant-2', sessionId: session.id, role: 'assistant', content: 'Saved partial reply', createdAt: 4 },
    ];
    let transcript = original;
    let commitEdit = true;
    let failRead = false;
    let readPending: Promise<Response> | undefined;
    let pendingReads = 0;
    const fetch = vi.spyOn(globalThis, 'fetch').mockImplementation(async (_url, init) => {
      const { cmd } = JSON.parse(String(init?.body));
      if (cmd === 'get_messages') {
        if (failRead) throw new Error('PRIVATE_HISTORY_DIAGNOSTIC');
        if (readPending) { pendingReads += 1; return readPending; }
        return new Response(JSON.stringify({ ok: true, data: transcript }));
      }
      if (cmd === 'edit_and_resend_message') {
        if (commitEdit) transcript = saved;
        return new Response(JSON.stringify({ ok: false, error: 'PRIVATE_RESEND_DIAGNOSTIC' }));
      }
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    vi.spyOn(console, 'error').mockImplementation(() => {});
    useSessionsStore.setState({ sessions: [session], activeSessionId: session.id, activeSession: session });
    render(<ChatArea />);
    await screen.findByText('Original user message');
    await userEvent.click(screen.getByRole('button', { name: '编辑' }));
    await userEvent.clear(screen.getByRole('textbox', { name: '编辑后的消息内容' }));
    await userEvent.type(screen.getByRole('textbox', { name: '编辑后的消息内容' }), 'Edited user message');
    await userEvent.click(screen.getByRole('button', { name: '重新发送' }));
    await waitFor(() => expect(fetch.mock.calls.filter(([, init]) => JSON.parse(String(init?.body)).cmd !== 'get_agent_events').map(([, init]) => JSON.parse(String(init?.body)).cmd))
      .toEqual(['get_messages', 'edit_and_resend_message', 'get_messages']));
    expect(await screen.findByText('Saved partial reply')).toBeVisible();
    expect(screen.getByText('Edited user message')).toBeVisible();
    expect(screen.queryByText('Original reply')).not.toBeInTheDocument();
    expect(screen.queryByText(/PRIVATE_RESEND_DIAGNOSTIC/)).not.toBeInTheDocument();
    const editCalls = () => fetch.mock.calls.filter(([, init]) => JSON.parse(String(init?.body)).cmd === 'edit_and_resend_message');
    expect(editCalls()).toHaveLength(1);

    // A pre-admission rejection keeps the durable old branch and the new draft,
    // without overwriting a separate composer draft or retrying the edit.
    transcript = original;
    commitEdit = false;
    act(() => useMessagesStore.getState().loadMessages(original));
    await userEvent.type(screen.getByPlaceholderText('给 AngelBot 发消息…'), 'Separate draft');
    await userEvent.click(screen.getByRole('button', { name: '编辑' }));
    await userEvent.clear(screen.getByRole('textbox', { name: '编辑后的消息内容' }));
    await userEvent.type(screen.getByRole('textbox', { name: '编辑后的消息内容' }), 'Edited user message');
    await userEvent.click(screen.getByRole('button', { name: '重新发送' }));
    await waitFor(() => expect(screen.getByRole('textbox', { name: '编辑后的消息内容' })).toHaveValue('Edited user message'));
    expect(screen.getByText('Original reply')).toBeVisible();
    expect(screen.getByPlaceholderText('给 AngelBot 发消息…')).toHaveValue('Separate draft');
    expect(screen.getByRole('alert')).toHaveTextContent('已载入保存记录');
    expect(editCalls()).toHaveLength(2);

    // Cancelling a recovered edit must not let an unrelated later deletion
    // resurrect that discarded draft through the shared reload path.
    act(() => useMessagesStore.getState().loadMessages([...original,
      { id: 'later-user', sessionId: session.id, role: 'user', content: 'Unrelated later turn', createdAt: 5 },
    ]));
    await userEvent.click(screen.getByRole('button', { name: '取消' }));
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    await userEvent.click(screen.getAllByRole('button', { name: '删除' })[1]);
    await waitFor(() => expect(screen.queryByText('Unrelated later turn')).not.toBeInTheDocument());
    expect(screen.queryByRole('textbox', { name: '编辑后的消息内容' })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: '编辑' }));
    expect(screen.getByRole('textbox', { name: '编辑后的消息内容' })).toHaveValue('Original user message');
    await userEvent.clear(screen.getByRole('textbox', { name: '编辑后的消息内容' }));
    await userEvent.type(screen.getByRole('textbox', { name: '编辑后的消息内容' }), 'Edited user message');

    // A lost read is not proof that the old branch survived. Only a read-only
    // reload can unblock history/composer; it must not dispatch another edit.
    failRead = true;
    await userEvent.click(screen.getByRole('button', { name: '重新发送' }));
    expect(await screen.findByRole('button', { name: '重新载入对话' })).toBeEnabled();
    expect(screen.getByRole('button', { name: '编辑' })).toBeDisabled();
    expect(screen.getByPlaceholderText('给 AngelBot 发消息…')).toBeDisabled();
    expect(screen.queryByText(/PRIVATE_HISTORY_DIAGNOSTIC/)).not.toBeInTheDocument();
    expect(editCalls()).toHaveLength(3);
    failRead = false;
    await userEvent.click(screen.getByRole('button', { name: '重新载入对话' }));
    await waitFor(() => expect(screen.getByRole('textbox', { name: '编辑后的消息内容' })).toHaveValue('Edited user message'));
    expect(screen.getByRole('button', { name: '重新发送' })).toBeEnabled();
    expect(screen.getByPlaceholderText('给 AngelBot 发消息…')).toHaveValue('Separate draft');
    expect(editCalls()).toHaveLength(3);

    // The same unknown-outcome recovery also discovers a committed replacement,
    // and must not reopen the old draft over that actual branch.
    commitEdit = true;
    failRead = true;
    await userEvent.click(screen.getByRole('button', { name: '重新发送' }));
    await screen.findByRole('button', { name: '重新载入对话' });
    expect(editCalls()).toHaveLength(4);
    failRead = false;
    await userEvent.click(screen.getByRole('button', { name: '重新载入对话' }));
    expect(await screen.findByText('Saved partial reply')).toBeVisible();
    expect(screen.queryByRole('textbox', { name: '编辑后的消息内容' })).not.toBeInTheDocument();
    expect(screen.getByPlaceholderText('给 AngelBot 发消息…')).toBeEnabled();
    expect(editCalls()).toHaveLength(4);

    // A late read from a failed edit cannot replace a newer workspace.
    transcript = original;
    commitEdit = false;
    act(() => useMessagesStore.getState().loadMessages(original));
    let finishRead!: (value: Response) => void;
    readPending = new Promise<Response>((resolve) => { finishRead = resolve; });
    await userEvent.click(screen.getByRole('button', { name: '编辑' }));
    await userEvent.clear(screen.getByRole('textbox', { name: '编辑后的消息内容' }));
    await userEvent.type(screen.getByRole('textbox', { name: '编辑后的消息内容' }), 'Edited user message');
    await userEvent.click(screen.getByRole('button', { name: '重新发送' }));
    await waitFor(() => expect(pendingReads).toBe(1));
    await userEvent.click(within(screen.getByLabelText('运行控制')).getByRole('button', { name: '停止运行' }));
    expect(screen.getByRole('button', { name: '编辑' })).toBeDisabled();
    expect(screen.getByPlaceholderText('给 AngelBot 发消息…')).toBeDisabled();
    const other = makeSession('session-2', 'Other');
    transcript = [{ id: 'other-user', sessionId: other.id, role: 'user', content: 'Other workspace history', createdAt: 1 }];
    readPending = undefined;
    act(() => useSessionsStore.setState({ sessions: [session, other], activeSessionId: other.id, activeSession: other }));
    await screen.findByText('Other workspace history');
    await act(async () => finishRead(new Response(JSON.stringify({ ok: true, data: original }))));
    expect(screen.getByText('Other workspace history')).toBeVisible();
    expect(screen.queryByText('Original reply')).not.toBeInTheDocument();
    expect(screen.queryByRole('textbox', { name: '编辑后的消息内容' })).not.toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(editCalls()).toHaveLength(5);
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
    const fetch = vi.spyOn(globalThis, 'fetch')
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
    expect(screen.getByRole('alert')).toHaveTextContent('未能确认删除或更新对话');
    expect(screen.getByRole('button', { name: '编辑' })).toBeDisabled();
    expect(screen.getByPlaceholderText('给 AngelBot 发消息…')).toBeDisabled();
    fetch.mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, data: [
      { ...originalMessages[0], content: 'Edited user message' }, replacementReply,
    ] })));
    await userEvent.click(screen.getByRole('button', { name: '重新载入对话' }));
    await waitFor(() => expect(screen.getByRole('button', { name: '编辑' })).toBeEnabled());
    expect(screen.getAllByText('Replacement reply')).toHaveLength(1);
    expect(fetch.mock.calls.filter(([, init]) => JSON.parse(String(init?.body)).cmd === 'edit_and_resend_message')).toHaveLength(1);
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

  it.each(['field', 'image'])('renders one $0 approval and preflight when persisted and live views share a pending call', async (operation) => {
    const isImage = operation === 'image';
    const toolName = isImage ? 'observe_trusted_app_window' : 'set_trusted_app_text';
    const toolArguments = isImage ? { mode: 'image', app_id: 'forged-app' } : { text: 'model text' };
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
          previewId: 'one-preview', operation, appDisplayName: '记事本',
          executableName: 'notepad.exe', windowTitle: isImage ? null : '工作记录', controlName: isImage ? null : '正文',
          text: isImage ? null : '后端预检的内容', expiresAt: Date.now() + 60_000,
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
        toolCalls: [{ id: 'field-call', name: toolName, arguments: JSON.stringify(toolArguments) }],
        toolResults: [{
          callId: 'field-call', toolName, success: false,
          output: 'Confirmation required', confirmationRequired: true, confirmationStatus: 'pending',
        }],
      } : message),
      liveBlocks: [{
        kind: 'tool_call', index: 0, iterationId: 0, status: 'needs_approval',
        callId: 'field-call', toolName, arguments: toolArguments,
      }],
      liveMessageId: assistant!.id,
    })));

    expect(await screen.findByText(isImage ? /已核对授权应用：记事本（notepad.exe）/ : '后端预检的内容')).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: isImage ? '允许发送图像一次' : '允许' })).toHaveLength(1);
    expect(preflights).toBe(1);
    expect(screen.queryByText('model text')).not.toBeInTheDocument();
    expect(screen.queryByText('forged-app')).not.toBeInTheDocument();

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
      timelineBlocks: [{
        kind: 'tool_call', index: 0, iterationId: 0,
        callId: 'call-2', toolName: 'write_file', arguments: {}, status: 'failed',
      }],
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
      id: 'msg-compact-steps', sessionId: 'session-1', role: 'assistant', content: '', createdAt: Date.now(),
      toolCalls,
      toolResults: toolCalls.map((tool) => ({ callId: tool.id, toolName: tool.name, success: true, output: 'ok' })),
    }, {
      id: 'msg-final-answer', sessionId: 'session-1', role: 'assistant', content: 'Done', createdAt: Date.now(),
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
    expect(screen.getAllByRole('button', { name: '复制' })).toHaveLength(1);
  });

  it('loads a durable journal only for the assistant owning its following tool rows', async () => {
    const messages: Message[] = [
      { id: 'ordinary-old', sessionId: 'session-1', role: 'assistant', content: '此前的普通回复', createdAt: 1 },
      { id: 'next-user', sessionId: 'session-1', role: 'user', content: '处理这项事务', createdAt: 2 },
      { id: 'tool-owner', sessionId: 'session-1', role: 'assistant', content: '已核对这项事务', createdAt: 3 },
      { id: 'tool-row', sessionId: 'session-1', role: 'tool', content: 'synthetic receipt', createdAt: 3 },
      { id: 'last-user', sessionId: 'session-1', role: 'user', content: '然后继续聊天', createdAt: 4 },
      { id: 'ordinary-last', sessionId: 'session-1', role: 'assistant', content: '结束的普通回复', createdAt: 5 },
    ];
    const loadedRuns: string[] = [];
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd, args } = JSON.parse(init?.body as string) as { cmd: string; args?: { runId?: string } };
      if (cmd === 'get_messages') return new Response(JSON.stringify({ ok: true, data: messages }));
      if (cmd === 'get_agent_run_events') {
        loadedRuns.push(args?.runId ?? 'missing-run');
      }
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    useSessionsStore.setState({ sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo') });
    render(<ChatArea />);
    expect(await screen.findByText('结束的普通回复')).toBeInTheDocument();
    await waitFor(() => expect(loadedRuns).toContain('tool-owner'));
    expect(loadedRuns).toEqual(['tool-owner']);
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
    expect(screen.getByRole('region', { name: '拟写入的正文' })).toHaveTextContent('hello');
    expect(screen.getByRole('button', { name: '允许一次' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '拒绝' })).toBeInTheDocument();
    // No verbose "audit trail" intro text
    expect(screen.queryByText(/以下是 AngelBot/)).not.toBeInTheDocument();
  });

  it('keeps approved desktop failures visible after the final timeline without exposing raw data or retry controls', async () => {
    const messages: Message[] = [{
      id: 'msg-desktop-failure', sessionId: 'session-1', role: 'assistant',
      content: '请检查目标应用。', createdAt: Date.now(),
      toolCalls: [{ id: 'open', name: 'open_trusted_app', arguments: '{"app_id":"PRIVATE_APP"}' },
        { id: 'image', name: 'observe_trusted_app_window', arguments: '{"app_id":"PRIVATE_APP","mode":"image"}' }],
      toolResults: [{ callId: 'open', toolName: 'open_trusted_app', success: false,
        output: 'PRIVATE_OUTPUT', error: 'PRIVATE_ERROR_PATH',
        confirmationRequired: true, confirmationStatus: 'approved' },
        { callId: 'image', toolName: 'observe_trusted_app_window', success: false,
          output: 'Error: FRAME_CHANGED: PRIVATE_FRAME_DETAILS',
          confirmationRequired: true, confirmationStatus: 'approved' }],
      timelineBlocks: [{ kind: 'text', index: 0, iterationId: 0, content: '请检查目标应用。' }],
    }];
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({ ok: true, data: messages })));
    useSessionsStore.setState({ sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo') });
    useMessagesStore.setState({ messages });

    render(<ChatArea />);

    const receipts = await screen.findByRole('list', { name: '操作结果' });
    expect(receipts).toHaveTextContent('操作未完成');
    expect(receipts).toHaveTextContent('重试前请先核对');
    expect(receipts).toHaveTextContent('未取得窗口图像');
    expect(receipts).toHaveTextContent('等待窗口稳定后重新发起观察');
    expect(screen.queryByText(/PRIVATE/)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /重试|允许一次|查看工具输出/ })).not.toBeInTheDocument();
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

    const receipts = await screen.findByRole('list', { name: '操作结果' });
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

    expect(await screen.findByRole('list', { name: '操作结果' })).toHaveTextContent('已发出请求');
    expect(screen.getByText('已向 Windows 发出打开请求；尚未确认目标窗口已就绪。')).toBeInTheDocument();
  });

  it('asks the user to inspect a stopped draft whose outcome is unknown', async () => {
    const messages: Message[] = [{
      id: 'msg-unknown-draft',
      sessionId: 'session-1',
      role: 'assistant',
      content: '请检查目标应用。',
      createdAt: Date.now(),
      taskRun: {
        id: 'msg-unknown-draft', goal: 'Save draft', status: 'needs_attention',
        plan: ['prepare_message_draft'], confirmationState: 'none', resumable: false,
        stepCount: 1, completedStepCount: 0,
      },
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
    act(() => useMessagesStore.getState().addToolResult(messages[0].id, messages[0].toolResults![0]));
    expect(useMessagesStore.getState().messages[0].taskRun?.resumable).toBe(false);
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

  it.each([
    { backendContinued: false, switchSession: false },
    { backendContinued: true, switchSession: false },
    { backendContinued: true, switchSession: true },
  ])('approves a pending step without repeating a backend-composed continuation: $backendContinued, switched: $switchSession', async ({ backendContinued, switchSession }) => {
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
    const continuedMessage: Message = {
      id: 'msg-continued', sessionId: 'session-1', role: 'assistant',
      content: 'The approved task is complete.', createdAt: Date.now() + 1,
    };
    const refreshedMessages = backendContinued ? [...resumedMessages, continuedMessage] : resumedMessages;
    const otherSession = makeSession('session-2', 'Other workspace');
    const otherMessage: Message = {
      id: 'other-message', sessionId: otherSession.id, role: 'assistant',
      content: 'This is the other workspace.', createdAt: Date.now(),
    };
    const originalSubscribe = useMessagesStore.getState().subscribeToAgentEvents;
    const subscribe = vi.fn().mockResolvedValue(undefined);
    useMessagesStore.setState({ subscribeToAgentEvents: subscribe });
    let finishResolution!: () => void;
    const resolutionPending = new Promise<void>((resolve) => { finishResolution = resolve; });
    let confirmationResolved = false;
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd: command, args } = JSON.parse(init?.body as string);
      if (command === 'resolve_agent_confirmation') {
        expect(subscribe).toHaveBeenCalledWith('session-1');
        expect(args.req).toMatchObject({ sessionId: 'session-1', messageId: 'msg-approval', callId: 'call-approval', decision: 'approved' });
        expect(useMessagesStore.getState().messages.filter((message) => message.role === 'assistant' && !message.content))
          .toMatchObject([{ sessionId: 'session-1' }]);
        await resolutionPending;
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
      if (command === 'get_messages') {
        return new Response(JSON.stringify({
          ok: true,
          data: args.sessionId === otherSession.id ? [otherMessage] : confirmationResolved ? refreshedMessages : messages,
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

    try {
      render(<ChatArea />);
      await userEvent.click(await screen.findByRole('button', { name: '允许一次' }));
      expect(await screen.findByTitle('停止运行')).toBeInTheDocument();
      if (switchSession) {
        act(() => useSessionsStore.setState({ activeSessionId: otherSession.id, activeSession: otherSession }));
        expect(await screen.findByText(otherMessage.content)).toBeInTheDocument();
      }
      await act(async () => finishResolution());

      await waitFor(() => expect(screen.queryByTitle('停止运行')).not.toBeInTheDocument());
      const commands = vi.mocked(globalThis.fetch).mock.calls.map(([, init]) =>
        (JSON.parse(init?.body as string) as { cmd: string }).cmd,
      );
      expect(commands).toContain('resolve_agent_confirmation');
      expect(commands.filter((command) => command === 'resolve_agent_confirmation')).toHaveLength(1);
      expect(commands.filter((command) => command === 'continue_agent_task')).toHaveLength(0);
      if (switchSession) {
        expect(screen.getByText(otherMessage.content)).toBeInTheDocument();
        expect(screen.queryByText(continuedMessage.content)).not.toBeInTheDocument();
        expect(useMessagesStore.getState().messages).toEqual([otherMessage]);
        act(() => useSessionsStore.setState({ activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo') }));
        expect(await screen.findByText(continuedMessage.content)).toBeInTheDocument();
      }
      // After approval, successful tool audit leaves the main conversation.
      expect(screen.queryByRole('button', { name: '允许一次' })).not.toBeInTheDocument();
      expect(screen.queryByText(/动作已完成/)).not.toBeInTheDocument();
      expect(screen.queryByRole('button', { name: '技术详情' })).not.toBeInTheDocument();
      if (backendContinued) expect(screen.getByText(continuedMessage.content)).toBeInTheDocument();
      expect(useMessagesStore.getState().messages.filter((message) => message.role === 'assistant' && !message.content)).toHaveLength(0);
    } finally {
      finishResolution();
      useMessagesStore.setState({ subscribeToAgentEvents: originalSubscribe });
    }
  });

  it.each(['refresh', 'refresh-stop-failed', 'refresh-new-run', 'resolution-new-run'])('fences the stopped desktop chain without clearing a newer Main run: %s', async (stage) => {
    const { pending, approved, preview } = desktopApprovalFixture();
    const resolving = stage === 'resolution-new-run';
    const newRun = stage.endsWith('new-run');
    const newReply: Message = { id: 'new-main-reply', sessionId: 'session-1', role: 'assistant',
      content: '新的事项已处理。', createdAt: Date.now() + 1 };
    const originalSubscribe = useMessagesStore.getState().subscribeToAgentEvents;
    useMessagesStore.setState({ subscribeToAgentEvents: vi.fn().mockResolvedValue(undefined) });
    let finishRefresh!: (response: Response) => void;
    let finishInterrupt!: (response: Response) => void;
    let finishResolution!: () => void;
    let finishNewRun!: (response: Response) => void;
    const refreshing = new Promise<Response>((resolve) => { finishRefresh = resolve; });
    const interrupting = new Promise<Response>((resolve) => { finishInterrupt = resolve; });
    const resolutionPending = new Promise<void>((resolve) => { finishResolution = resolve; });
    const newRunning = new Promise<Response>((resolve) => { finishNewRun = resolve; });
    const refreshRequested = vi.fn();
    const resolutionRequested = vi.fn();
    let resolved = false;
    let newStarted = false;
    let newFinished = false;
    let newUserId = '';
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd, args } = JSON.parse(init?.body as string);
      if (cmd === 'preflight_pending_desktop_action') return new Response(JSON.stringify({ ok: true, data: preview }));
      if (cmd === 'resolve_agent_confirmation') {
        resolutionRequested();
        if (resolving) await resolutionPending;
        resolved = true;
        return new Response(JSON.stringify({ ok: true, data: approved.toolResults![0] }));
      }
      if (cmd === 'get_messages') {
        if (newFinished) return new Response(JSON.stringify({ ok: true, data: [approved,
          { id: newUserId, sessionId: 'session-1', role: 'user', content: '处理新的事项', createdAt: Date.now() }, newReply] }));
        if (!resolved) return new Response(JSON.stringify({ ok: true, data: [pending] }));
        refreshRequested();
        return resolving ? new Response(JSON.stringify({ ok: true, data: [approved] })) : refreshing;
      }
      if (cmd === 'interrupt_agent') return interrupting;
      if (cmd === 'send_message') {
        newStarted = true;
        newUserId = args.req.clientMessageId;
        const response = await newRunning;
        newFinished = true;
        return response;
      }
      return new Response(JSON.stringify({ ok: true, data: approved }));
    });
    useSessionsStore.setState({ sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo') });
    try {
      render(<ChatArea />);
      await screen.findByText('计划内容');
      await userEvent.click(screen.getByRole('button', { name: '允许一次' }));
      await waitFor(() => expect(resolving ? resolutionRequested : refreshRequested).toHaveBeenCalledTimes(1));
      // The stop fence must take effect before interrupt IPC returns.
      fireEvent.click(within(screen.getByLabelText('运行控制')).getByRole('button', { name: '停止运行' }));
      if (stage === 'refresh-stop-failed') {
        await act(async () => finishInterrupt(new Response(JSON.stringify({ ok: false, error: 'owned stop rejection' }))));
        expect(await screen.findByText('未能确认停止请求，Agent 可能仍在执行。请先核对运行状态与操作结果，不要重复提交原任务。')).toBeInTheDocument();
      }
      if (newRun) {
        await userEvent.type(screen.getByPlaceholderText('给 AngelBot 发消息…'), '处理新的事项');
        await userEvent.click(screen.getByRole('button', { name: '发送' }));
        await waitFor(() => expect(newStarted).toBe(true));
        act(() => useMessagesStore.setState({ liveMessageId: newReply.id,
          liveBlocks: [{ kind: 'text', index: 0, iterationId: 0, content: '正在处理新的事项。' }] }));
        expect(await screen.findByText('正在处理新的事项。')).toBeInTheDocument();
      }
      await act(async () => {
        finishRefresh(new Response(JSON.stringify({ ok: true, data: [approved] })));
        finishResolution();
      });
      if (!resolving) {
        await waitFor(() => expect(useMessagesStore.getState().messages.find((message) => message.id === pending.id)
          ?.toolResults?.[0].confirmationStatus).toBe('approved'));
        expect(screen.getByRole('list', { name: '操作结果' })).toHaveTextContent('已校验');
      } else {
        // The old IPC cannot write into the new run's projection. Its receipt
        // returns through the new run's authoritative timeline refresh below.
        expect(useMessagesStore.getState().messages.find((message) => message.id === pending.id)
          ?.toolResults?.[0].confirmationStatus).toBe('pending');
      }
      const commands = vi.mocked(globalThis.fetch).mock.calls.map(([, init]) => JSON.parse(init?.body as string).cmd);
      expect(commands.filter((command) => command === 'continue_agent_task')).toHaveLength(0);
      if (newRun) {
        expect(screen.getByText('正在处理新的事项。')).toBeInTheDocument();
        expect(screen.getByLabelText('运行控制')).toBeInTheDocument();
        expect(useMessagesStore.getState().messages.filter((message) => message.role === 'assistant' && !message.content)).toHaveLength(1);
        await act(async () => finishNewRun(new Response(JSON.stringify({ ok: true, data: newReply }))));
        await screen.findByText(newReply.content);
        expect(screen.getByRole('list', { name: '操作结果' })).toHaveTextContent('已校验');
        expect(useMessagesStore.getState().messages.filter((message) => message.role === 'assistant' && !message.content)).toHaveLength(0);
        expect(useSessionsStore.getState().activeSessionId).toBe('session-1');
      }
    } finally {
      await act(async () => {
        finishRefresh(new Response(JSON.stringify({ ok: true, data: [approved] })));
        finishInterrupt(new Response(JSON.stringify({ ok: true, data: null })));
        finishResolution();
        finishNewRun(new Response(JSON.stringify({ ok: true, data: newReply })));
      });
      useMessagesStore.setState({ subscribeToAgentEvents: originalSubscribe });
    }
  });

  it('does not duplicate desktop target reads when the live approval becomes durable', async () => {
    const { pending, preview } = desktopApprovalFixture();
    const originalSubscribe = useMessagesStore.getState().subscribeToAgentEvents;
    useMessagesStore.setState({ subscribeToAgentEvents: vi.fn().mockResolvedValue(undefined) });
    let finishSend!: () => void;
    let finishPreview!: () => void;
    const sending = new Promise<void>((resolve) => { finishSend = resolve; });
    const inspecting = new Promise<void>((resolve) => { finishPreview = resolve; });
    let sent = false;
    let durable = false;
    let previewReady = false;
    let preflights = 0;
    const commands: string[] = [];
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd } = JSON.parse(init?.body as string);
      commands.push(cmd);
      if (cmd === 'send_message') {
        sent = true;
        await sending;
        return new Response(JSON.stringify({ ok: true, data: pending }));
      }
      if (cmd === 'preflight_pending_desktop_action') {
        preflights++;
        // Match the native adapter's one-at-a-time, bounded observation gate.
        if (preflights > 1 && !previewReady) return new Response(JSON.stringify({ ok: false,
          error: 'TARGET_BUSY: Another trusted desktop operation is in progress' }));
        await inspecting;
        return new Response(JSON.stringify({ ok: true, data: preview }));
      }
      return new Response(JSON.stringify({ ok: true, data: cmd === 'get_messages' && durable ? [pending] : [] }));
    });
    useSessionsStore.setState({ sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo') });
    try {
      render(<ChatArea />);
      await userEvent.type(screen.getByPlaceholderText('给 AngelBot 发消息…'), '填写当前记录');
      await userEvent.click(screen.getByRole('button', { name: '发送' }));
      await waitFor(() => expect(sent).toBe(true));
      act(() => useMessagesStore.setState({ liveMessageId: pending.id, liveBlocks: [{
        kind: 'tool_call', index: 0, iterationId: 0, status: 'needs_approval',
        callId: 'field-call', toolName: 'set_trusted_app_text', arguments: { field_ref: 'mock-field-ref', text: '计划内容' },
      }] }));
      await waitFor(() => expect(preflights).toBe(1));
      durable = true;
      await act(async () => finishSend());
      await screen.findByText(pending.content);
      previewReady = true;
      await act(async () => finishPreview());
      expect(preflights).toBe(1);
      await screen.findByText('计划内容');
      expect(screen.getByRole('button', { name: /^允许/ })).toBeEnabled();
      expect(screen.queryByRole('button', { name: '重新核对' })).not.toBeInTheDocument();
      expect(commands).not.toContain('resolve_agent_confirmation');
    } finally {
      await act(async () => { finishSend(); finishPreview(); });
      useMessagesStore.setState({ subscribeToAgentEvents: originalSubscribe });
    }
  });

  it.each(['persisted', 'live'])('renders approved desktop continuation in the same conversation before its next confirmation settles: %s', async (source) => {
    const { pending, approved, preview } = desktopApprovalFixture();
    const next: Message = { ...pending, id: 'next-desktop-turn', content: '正在重新观察目标窗口。',
      taskRun: { ...pending.taskRun!, id: 'next-desktop-turn' },
      toolCalls: [{ id: 'check-call', name: 'operate_trusted_app_control', arguments: '{"action":"check","control_ref":"fresh-toggle-ref"}' }],
      toolResults: [{ callId: 'check-call', toolName: 'operate_trusted_app_control', success: false,
        output: 'Confirmation required', confirmationRequired: true, confirmationStatus: 'pending' }],
    };
    const nextApproved: Message = { ...next,
      taskRun: { ...next.taskRun!, status: 'continue_suggested', confirmationState: 'approved', completedStepCount: 1 },
      toolResults: [{ callId: 'check-call', toolName: 'operate_trusted_app_control', success: true,
        output: 'Confirmation approved: {"status":"verified","action":"check"}', confirmationRequired: false, confirmationStatus: 'approved' }],
    };
    const final: Message = { id: 'desktop-final', sessionId: 'session-1', role: 'assistant', content: '输入框与复选框已核对。', createdAt: Date.now() + 1 };
    const originalSubscribe = useMessagesStore.getState().subscribeToAgentEvents;
    useMessagesStore.setState({ subscribeToAgentEvents: vi.fn().mockResolvedValue(undefined) });
    let finishContinuation!: () => void;
    let finishSend!: () => void;
    const continuing = new Promise<void>((resolve) => { finishContinuation = resolve; });
    const sending = new Promise<void>((resolve) => { finishSend = resolve; });
    let sendStarted = false;
    let phase = 0;
    const preflightCalls: string[] = [];
    const continuationRequested = vi.fn();
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd, args } = JSON.parse(init?.body as string);
      if (cmd === 'send_message') {
        sendStarted = true;
        await sending;
        return new Response(JSON.stringify({ ok: true, data: pending }));
      }
      if (cmd === 'preflight_pending_desktop_action') {
        preflightCalls.push(args.callId);
        return new Response(JSON.stringify({ ok: true, data: args.callId === 'field-call'
          ? preview : { ...preview, previewId: 'check-preview', operation: 'check', controlName: '确认项', text: null } }));
      }
      if (cmd === 'resolve_agent_confirmation') {
        phase = args.req.callId === 'field-call' ? 1 : 3;
        continuationRequested();
        if (phase === 1) { await continuing; phase = 2; }
        else phase = 4;
        return new Response(JSON.stringify({ ok: true, data: phase === 2 ? approved.toolResults![0] : nextApproved.toolResults![0] }));
      }
      const messages = phase === 0 ? source === 'live' ? [] : [pending] : phase === 1 ? [approved] : phase === 2 ? [approved, next]
        : phase === 3 ? [approved, nextApproved] : [approved, nextApproved, final];
      return new Response(JSON.stringify({ ok: true, data: cmd === 'get_messages' ? messages : [] }));
    });
    useSessionsStore.setState({ sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo') });
    try {
      render(<ChatArea />);
      if (source === 'live') {
        await userEvent.type(screen.getByPlaceholderText('给 AngelBot 发消息…'), '处理当前窗口');
        await userEvent.click(screen.getByRole('button', { name: '发送' }));
        await waitFor(() => expect(sendStarted).toBe(true));
      }
      // A finished send retains its last live trace. Approving the durable
      // card must not revive that old approval in the new streaming anchor.
      act(() => useMessagesStore.setState({ liveMessageId: pending.id, liveBlocks: [{
        kind: 'tool_call', index: 0, iterationId: 0, status: 'needs_approval',
        callId: 'field-call', toolName: pending.toolCalls![0].name,
        arguments: JSON.parse(pending.toolCalls![0].arguments),
      }] }));
      await screen.findByText('计划内容');
      await userEvent.click(screen.getByRole('button', { name: /^允许/ }));
      await waitFor(() => expect(continuationRequested).toHaveBeenCalledTimes(1));
      expect(preflightCalls.filter((call) => call === 'field-call')).toHaveLength(1);
      await act(async () => finishSend());
      expect(useMessagesStore.getState().messages.filter((message) => message.role === 'assistant' && !message.content)).toHaveLength(1);
      act(() => useMessagesStore.setState({ liveMessageId: next.id,
        liveBlocks: [{ kind: 'text', index: 0, iterationId: 0, content: next.content }] }));
      expect(await screen.findByText(next.content)).toBeInTheDocument();
      expect(useSessionsStore.getState().activeSessionId).toBe(pending.sessionId);
      await act(async () => finishContinuation());
      await screen.findByText(/已核对目标：便笺（notes.exe） · 当前记录 · 确认项/);
      expect(screen.getAllByText(next.content)).toHaveLength(1);
      expect(screen.getAllByRole('button', { name: '允许勾选复选框一次' })).toHaveLength(1);
      await userEvent.click(screen.getByRole('button', { name: '允许勾选复选框一次' }));
      await screen.findByText(final.content);
      expect(continuationRequested).toHaveBeenCalledTimes(2);
      expect(vi.mocked(globalThis.fetch).mock.calls.map(([, init]) => JSON.parse(init?.body as string).cmd))
        .not.toContain('continue_agent_task');
      const approvals = vi.mocked(globalThis.fetch).mock.calls.map(([, init]) => JSON.parse(init?.body as string))
        .filter((request) => request.cmd === 'resolve_agent_confirmation').map((request) => request.args.req);
      expect(approvals.map((request) => [request.callId, request.previewId])).toEqual([
        ['field-call', 'field-preview'], ['check-call', 'check-preview'],
      ]);
      expect(screen.queryByRole('button', { name: /^允许/ })).not.toBeInTheDocument();
    } finally {
      await act(async () => { finishContinuation(); finishSend(); });
      useMessagesStore.setState({ subscribeToAgentEvents: originalSubscribe });
    }
  });

  it.each([
    { operation: 'field', source: 'persisted' }, { operation: 'image', source: 'persisted' },
    { operation: 'field', source: 'live' }, { operation: 'image', source: 'live' },
  ])('invalidates a consumed $operation preview from $source only when failed approval refresh proves the same call remains pending', async ({ operation, source }) => {
    const { pending, approved, preview } = desktopApprovalFixture();
    const isImage = operation === 'image';
    if (isImage) {
      for (const message of [pending, approved]) {
        message.toolCalls![0] = { id: 'field-call', name: 'observe_trusted_app_window', arguments: '{"mode":"image","app_id":"notes"}' };
        message.toolResults![0].toolName = 'observe_trusted_app_window';
      }
    }
    const readyText = isImage ? /已核对授权应用：便笺/ : '计划内容';
    const button = /^允许/;
    const originalSubscribe = useMessagesStore.getState().subscribeToAgentEvents;
    useMessagesStore.setState({ subscribeToAgentEvents: vi.fn().mockResolvedValue(undefined) });
    let preflights = 0;
    let approvals = 0;
    let sendStarted = false;
    let finishSend!: () => void;
    const sending = new Promise<void>((resolve) => { finishSend = resolve; });
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd, args } = JSON.parse(init?.body as string);
      if (cmd === 'send_message') {
        sendStarted = true;
        await sending;
        return new Response(JSON.stringify({ ok: true, data: pending }));
      }
      if (cmd === 'preflight_pending_desktop_action') {
        preflights += 1;
        return new Response(JSON.stringify({ ok: true, data: { ...preview, operation,
          windowTitle: isImage ? null : preview.windowTitle, controlName: isImage ? null : preview.controlName,
          text: isImage ? null : preview.text, previewId: `preview-${preflights}` } }));
      }
      if (cmd === 'resolve_agent_confirmation') {
        approvals += 1;
        if (approvals === 1) throw new Error('Trusted application changed; inspect the target again');
        expect(args.req.previewId).toBe('preview-2');
        return new Response(JSON.stringify({ ok: true, data: approved.toolResults![0] }));
      }
      return new Response(JSON.stringify({ ok: true, data: cmd === 'get_messages'
        ? source === 'live' && approvals === 0 ? []
          : approvals < 2 ? [pending] : [{ ...approved, taskRun: undefined }] : [] }));
    });
    useSessionsStore.setState({ sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo') });
    try {
      render(<ChatArea />);
      if (source === 'live') {
        await userEvent.type(screen.getByPlaceholderText('给 AngelBot 发消息…'), '处理当前窗口');
        await userEvent.click(screen.getByRole('button', { name: '发送' }));
        await waitFor(() => expect(sendStarted).toBe(true));
        act(() => useMessagesStore.setState({ liveMessageId: pending.id, liveBlocks: [{
          kind: 'tool_call', index: 0, iterationId: 0, status: 'needs_approval',
          callId: 'field-call', toolName: pending.toolCalls![0].name,
          arguments: JSON.parse(pending.toolCalls![0].arguments),
        }] }));
      }
      await screen.findByText(readyText);
      await userEvent.click(screen.getByRole('button', { name: button }));
      await waitFor(() => expect(screen.queryByLabelText('运行控制')).not.toBeInTheDocument());
      expect(approvals).toBe(1);
      expect(preflights).toBe(1);
      expect(screen.getByRole('button', { name: button })).toBeDisabled();
      expect(screen.getByRole('alert')).toHaveTextContent('重新核对');
      expect(useMessagesStore.getState().messages[0].toolResults?.[0].confirmationStatus).toBe('pending');
      await userEvent.click(screen.getByRole('button', { name: '重新核对' }));
      await screen.findByText(readyText);
      await userEvent.click(screen.getByRole('button', { name: button }));
      await waitFor(() => expect(approvals).toBe(2));
      expect(preflights).toBe(2);
    } finally {
      await act(async () => finishSend());
      useMessagesStore.setState({ subscribeToAgentEvents: originalSubscribe });
    }
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

  it('shows explicit clearing only from attested generic field text in a persisted confirmation', async () => {
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
          text: '', expiresAt: Date.now() + 60_000,
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
    expect(await screen.findByText('待确认操作：清空输入框')).toBeInTheDocument();
    expect(screen.getByText(/将删除这个输入框中的全部内容/)).toHaveTextContent('无法通过 AngelBot 自动恢复');
    expect(screen.getByText(/将修改这一个输入框/)).toBeInTheDocument();
    expect(screen.queryByText('model-authored text')).not.toBeInTheDocument();
    expect(screen.queryByText('forged-app')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: '允许清空一次' }));
    await waitFor(() => expect(approvedPreviewId).toBe('attested-field'));
  });

  it.each([
    { action: 'invoke', label: '调用控件' }, { action: 'select', label: '选中控件' },
    { action: 'choose', label: '选择下拉项' },
    { action: 'expand', label: '展开控件' }, { action: 'collapse', label: '收起控件' },
    { action: 'scrollup', label: '向上小幅滚动' }, { action: 'scrolldown', label: '向下小幅滚动' },
    { action: 'check', label: '勾选复选框' }, { action: 'uncheck', label: '取消勾选复选框' },
    { action: 'image', label: '发送图像' },
  ])('uses the same attested approval for a persisted $action request', async ({ action, label }) => {
    const isImage = action === 'image';
    const toolName = isImage ? 'observe_trusted_app_window' : 'operate_trusted_app_control';
    const targetName = action === 'scrollup' || action === 'scrolldown' ? '正文浏览区' : '发送';
    const pending: Message = {
      id: 'msg-invoke-confirmation', sessionId: 'session-1', role: 'assistant',
      content: '请核对控件。', createdAt: Date.now(),
      toolCalls: [{ id: 'invoke-call', name: toolName, arguments: JSON.stringify(isImage
        ? { app_id: 'forged-app', mode: 'image', name: 'FORGED_TARGET', data: 'RAW_IMAGE_CANARY' }
        : { app_id: 'forged-app', control_ref: 'forged-ref', action, name: 'Safe button', option: 'FORGED_OPTION' }) }],
      toolResults: [{
        callId: 'invoke-call', toolName, success: false,
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
          executableName: 'mail.exe', windowTitle: isImage ? null : '新邮件', controlName: isImage ? null : targetName,
          option: action === 'choose' ? '归档' : null,
          text: null, expiresAt: Date.now() + 60_000,
        } }));
      }
      if (cmd === 'resolve_agent_confirmation') {
        approvedPreviewId = args?.req?.previewId;
        return new Response(JSON.stringify({ ok: true, data: {
          callId: 'invoke-call', toolName, success: true,
          output: isImage ? '已观察已授权应用的单个窗口图像。' : JSON.stringify({ status: action === 'invoke' ? 'dispatched' : 'verified', action,
            ...(action === 'choose' ? { choice: { controlName: targetName, option: '归档', expanded: false } } : {}) }),
        } }));
      }
      return new Response(JSON.stringify({ ok: true, data: [] }));
    });
    useSessionsStore.setState({
      sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo'),
    });
    useMessagesStore.setState({ messages: [pending] });

    render(<ChatArea />);
    expect(await screen.findByText(isImage ? /已核对授权应用：邮件（mail.exe）/ : new RegExp(`邮件（mail.exe） · 新邮件 · ${targetName}`))).toBeInTheDocument();
    expect(screen.getByText(isImage ? /窗口可能包含私人内容/ : /可能触发发送、删除等后果/)).toBeInTheDocument();
    if (action === 'check' || action === 'uncheck') {
      expect(screen.getByText(/已达到目标状态则不重复操作；最多切换一次/)).toBeInTheDocument();
    }
    if (action === 'choose') {
      expect(screen.getByText(/将选择选项“归档”/)).toHaveTextContent('本次批准包含展开、精确选择、状态核验及收起');
      expect(screen.getByText(/只接受该下拉框中唯一可见的匹配选项/)).toBeInTheDocument();
    }
    expect(screen.queryByText(/FORGED_OPTION/)).not.toBeInTheDocument();
    expect(screen.queryByText('将填写的内容')).not.toBeInTheDocument();
    expect(screen.queryByText('forged-app')).not.toBeInTheDocument();
    expect(screen.queryByText('Safe button')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: /等待确认/ }));
    expect(screen.getByText(isImage ? '授权应用以实时预检卡片为准；本次核对不读取或截取窗口。' : '目标窗口和待操作控件以实时预检卡片为准。')).toBeInTheDocument();
    expect(screen.queryByText('forged-ref')).not.toBeInTheDocument();
    expect(screen.queryByText('Safe button')).not.toBeInTheDocument();
    expect(screen.queryByText(/FORGED_TARGET|RAW_IMAGE_CANARY|未命名窗口|未命名控件/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: `允许${label}一次` }));
    await waitFor(() => expect(approvedPreviewId).toBe('attested-invoke'));
  });

  it.each([undefined, '', 'x'.repeat(81)])('refuses an incomplete attested dropdown option: %j', async (option) => {
    const pending: Message = {
      id: 'msg-choice-confirmation', sessionId: 'session-1', role: 'assistant', content: '', createdAt: Date.now(),
      toolCalls: [{ id: 'choice-call', name: 'operate_trusted_app_control', arguments: '{"action":"choose","option":"MODEL_OPTION"}' }],
      toolResults: [{ callId: 'choice-call', toolName: 'operate_trusted_app_control', success: false,
        output: 'Confirmation required', confirmationRequired: true, confirmationStatus: 'pending' }],
    };
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd } = JSON.parse(init?.body as string) as { cmd: string };
      return new Response(JSON.stringify({ ok: true, data: cmd === 'get_messages' ? [pending]
        : cmd === 'preflight_pending_desktop_action' ? {
          previewId: 'incomplete-choice', operation: 'choose', appDisplayName: '列表验收', executableName: 'fixture.exe',
          windowTitle: '选项', controlName: '下拉框', text: null, option, expiresAt: Date.now() + 60_000,
        } : [] }));
    });
    useSessionsStore.setState({ sessions: [], activeSessionId: 'session-1', activeSession: makeSession('session-1', 'Demo') });
    useMessagesStore.setState({ messages: [pending] });
    render(<ChatArea />);
    expect(await screen.findByRole('alert')).toHaveTextContent('无法核对');
    expect(screen.getByRole('button', { name: '允许选择下拉项一次' })).toBeDisabled();
    expect(screen.queryByText(/MODEL_OPTION/)).not.toBeInTheDocument();
  });

  it.each([
    { toolName: 'operate_trusted_app_control', arguments: '{}' },
    { toolName: 'operate_trusted_app_control', arguments: '{"action":"unknown"}' },
    { toolName: 'operate_trusted_app_control', arguments: '{"action":"ScrollUp"}' },
    { toolName: 'operate_trusted_app_control', arguments: '{"action":"scroll_up"}' },
    { toolName: 'operate_trusted_app_control', arguments: '{"action":"toggle"}' },
    { toolName: 'operate_trusted_app_control', arguments: '{"action":"Check"}' },
    { toolName: 'invoke_trusted_app_control', arguments: '{"action":"invoke"}' },
    { toolName: 'observe_trusted_app_window', arguments: '{"mode":"Image"}' },
    { toolName: 'observe_trusted_app_window', arguments: '{' },
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
    { toolName: 'set_trusted_app_text', operation: 'field', text: '', button: '允许清空一次',
      explanation: '操作停止后输入框可能已改变；请先去目标应用核对，勿自动重试。' },
    { toolName: 'operate_trusted_app_control', operation: 'invoke', text: null, button: '允许调用控件一次',
      explanation: '操作停止后控件可能已被触发；请重新观察目标确认结果，勿自动重试。' },
    { toolName: 'operate_trusted_app_control', operation: 'select', text: null, button: '允许选中控件一次',
      explanation: '操作停止后控件可能已被触发；请重新观察目标确认结果，勿自动重试。' },
    { toolName: 'operate_trusted_app_control', operation: 'expand', text: null, button: '允许展开控件一次',
      explanation: '操作停止后控件可能已被触发；请重新观察目标确认结果，勿自动重试。' },
    { toolName: 'operate_trusted_app_control', operation: 'collapse', text: null, button: '允许收起控件一次',
      explanation: '操作停止后控件可能已被触发；请重新观察目标确认结果，勿自动重试。' },
    { toolName: 'operate_trusted_app_control', operation: 'choose', text: null, button: '允许选择下拉项一次',
      explanation: '操作停止后控件可能已被触发；请重新观察目标确认结果，勿自动重试。' },
    { toolName: 'operate_trusted_app_control', operation: 'scrollup', text: null, button: '允许向上小幅滚动一次',
      explanation: '操作停止后控件可能已被触发；请重新观察目标确认结果，勿自动重试。' },
    { toolName: 'operate_trusted_app_control', operation: 'scrolldown', text: null, button: '允许向下小幅滚动一次',
      explanation: '操作停止后控件可能已被触发；请重新观察目标确认结果，勿自动重试。' },
    { toolName: 'operate_trusted_app_control', operation: 'check', text: null, button: '允许勾选复选框一次',
      explanation: '操作停止后控件可能已被触发；请重新观察目标确认结果，勿自动重试。' },
    { toolName: 'operate_trusted_app_control', operation: 'uncheck', text: null, button: '允许取消勾选复选框一次',
      explanation: '操作停止后控件可能已被触发；请重新观察目标确认结果，勿自动重试。' },
    { toolName: 'mcp_browser__browser_click_a1b2c3d4', operation: 'mcp', text: null, button: '允许一次',
      explanation: '外部操作可能已部分生效；请先检查目标应用或服务，再决定是否发起新操作。不要重复批准或自动重试。' },
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
          option: operation === 'choose' ? '第二项' : null,
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
    if (operation !== 'mcp') await screen.findByText(/已核对目标：记事本/);
    await userEvent.click(await screen.findByRole('button', { name: button }));

    expect(await screen.findByText(explanation)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: button })).not.toBeInTheDocument();
    const local = useMessagesStore.getState().messages.find((message) => message.id === pending.id);
    expect(local?.toolResults?.find((result) => result.callId === 'field-call')?.error).toContain('RESULT_UNKNOWN');
    expect(local?.taskRun?.resumable).toBe(false);
  });

  it('blocks remaining approvals while an earlier write remains unknown', async () => {
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
        { id: 'file-call', name: 'write_file', arguments: '{"path":"notes.txt","content":"ok"}' },
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
    const allow = await screen.findByRole('button', { name: '允许一次' });
    expect(allow).toBeDisabled();
    await userEvent.click(allow);
    expect(commands).not.toContain('resolve_agent_confirmation');
    expect(commands).not.toContain('continue_agent_task');
    expect(screen.getByText('操作停止后输入框可能已改变；请先去目标应用核对，勿自动重试。')).toBeInTheDocument();
  });

  it('rejects a pending side-effect step without presenting the user decision as a failure', async () => {
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
    // The rejected action remains visible, but rejection is not an execution failure.
    expect(await screen.findByText('已按你的决定取消操作')).toBeInTheDocument();
    expect(screen.queryByText('有步骤失败')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '拒绝' })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: /已按你的决定取消操作/ }));
    expect(screen.getByText('已拒绝')).toBeInTheDocument();
    expect(screen.getByText('你已拒绝这项操作；未执行该请求，不会自动重试。')).toBeInTheDocument();
    expect(screen.queryByText(/此操作未完成：/)).not.toBeInTheDocument();
    const rejection = "Confirmation rejected: user rejected side-effect tool 'write_file'.";
    expect(screen.queryByText(rejection)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: '查看工具输出' }));
    expect(screen.getByText(rejection)).toBeInTheDocument();
    expect(document.querySelector('.agent-step-output.error')).toBeNull();

    // An actual failed action in the same turn must not be hidden by a rejection.
    useMessagesStore.setState({ messages: [{
      ...resumedMessages[0],
      toolCalls: [...resumedMessages[0].toolCalls!, { id: 'failed-read', name: 'read_file', arguments: '{}' }],
      toolResults: [...resumedMessages[0].toolResults!, {
        callId: 'failed-read', toolName: 'read_file', success: false, output: 'File unavailable',
      }],
    }] });
    expect(await screen.findByText('有步骤失败')).toBeInTheDocument();
  });
});

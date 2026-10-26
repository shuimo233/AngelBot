import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const eventMocks = vi.hoisted(() => ({
  listen: vi.fn(),
  unlisten: vi.fn(),
  invoke: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: eventMocks.listen,
}));

vi.mock('$lib/invoke', () => ({
  invoke: eventMocks.invoke,
}));

import { agentEventManager, subscribeAgentSessionEvents, contentBlocksFromDurableEvents, formatAgentEvent, type DurableAgentRunEvent } from './agent-event';
import { useMessagesStore } from '$stores/messages';
import { useWorkspaceActivityStore } from '$stores/workspaceActivity';

describe('formatAgentEvent', () => {
  beforeEach(() => {
    agentEventManager.destroy();
    eventMocks.listen.mockReset();
    eventMocks.unlisten.mockReset();
    eventMocks.invoke.mockReset();
    eventMocks.listen.mockResolvedValue(eventMocks.unlisten);
    eventMocks.invoke.mockResolvedValue({ events: [] });
  });

  afterEach(() => {
    agentEventManager.destroy();
    vi.useRealTimers();
  });

  it('exposes the durable source of a task-fact-aware compression', () => {
    expect(formatAgentEvent({
      type: 'ContextCompressed',
      data: {
        session_id: 'session-1',
        turn_id: 'summary-1',
        before_tokens: 12000,
        after_tokens: 300,
        summary_id: 'summary-1',
        includes_task_facts: true,
      },
    })).toContain('summary-1');
  });

  it('does not deliver an old event to a replacement lease created during dispatch', async () => {
    let listener: ((event: { payload: { type: string; data: unknown } }) => void) | undefined;
    eventMocks.listen.mockImplementation(async (_name, callback) => { listener = callback; return eventMocks.unlisten; });
    const keepWorkbench = subscribeAgentSessionEvents('session-reentrant', () => {});
    const receive = vi.fn();
    let replacement: ReturnType<typeof subscribeAgentSessionEvents> | undefined;
    const oldMain = subscribeAgentSessionEvents('session-reentrant', () => {
      oldMain.unsubscribe();
      replacement = subscribeAgentSessionEvents('session-reentrant', receive);
    });
    try {
      await Promise.all([keepWorkbench.ready, oldMain.ready]);
      listener?.({ payload: { type: 'MessageDelta', data: { delta: '上一轮尾事件' } } });
      expect(replacement).toBeDefined();
      expect(receive).not.toHaveBeenCalled();
      await replacement?.ready;
      listener?.({ payload: { type: 'MessageDelta', data: { delta: '下一轮事件' } } });
      expect(receive).toHaveBeenCalledTimes(1);
      expect(receive).toHaveBeenCalledWith({ type: 'MessageDelta', data: { delta: '下一轮事件' } });
      expect(eventMocks.listen).toHaveBeenCalledTimes(1);
    } finally {
      oldMain.unsubscribe();
      replacement?.unsubscribe();
      keepWorkbench.unsubscribe();
    }
  });

  it('keeps workbench discovery and the actual Main stream connected across Main handler resets', async () => {
    let listener: ((event: { payload: { type: string; data: unknown } }) => void) | undefined;
    eventMocks.listen.mockImplementation(async (_name, callback) => { listener = callback; return eventMocks.unlisten; });
    let cursor = 1;
    eventMocks.invoke.mockImplementation(async (command) => command === 'get_workspace_activity_projection'
      ? { workspaceId: 'workspace-shared', cursor, delegations: [], pending_decisions: [], attention: null }
      : { events: [] });
    const releaseWorkbench = useWorkspaceActivityStore.getState().observe('workspace-shared', 'session-shared');
    try {
      await vi.waitFor(() => expect(useWorkspaceActivityStore.getState().projection?.cursor).toBe(1));
      await useMessagesStore.getState().subscribeToAgentEvents('session-shared');
      expect(eventMocks.listen).toHaveBeenCalledTimes(1);
      cursor = 2;
      listener?.({ payload: { type: 'ToolExecutionEnd', data: { call_id: 'delegated-call', tool_name: 'delegate_work', success: true, output: 'issued' } } });
      await vi.waitFor(() => expect(useWorkspaceActivityStore.getState().projection?.cursor).toBe(2));
      useMessagesStore.getState().unsubscribeFromAgentEvents();
      expect(eventMocks.unlisten).not.toHaveBeenCalled();
      cursor = 3;
      listener?.({ payload: { type: 'AgentEnd', data: { session_id: 'session-shared' } } });
      await vi.waitFor(() => expect(useWorkspaceActivityStore.getState().projection?.cursor).toBe(3));
      releaseWorkbench();
      releaseWorkbench();
      expect(eventMocks.unlisten).toHaveBeenCalledTimes(1);
    } finally {
      releaseWorkbench();
      useWorkspaceActivityStore.getState().clear();
      useMessagesStore.getState().unsubscribeFromAgentEvents();
      useMessagesStore.getState().clearMessages();
    }
  });

  it('subscribes to the desktop event bus and dispatches events immediately', async () => {
    let listener: ((event: { payload: { type: string; data: unknown } }) => void) | undefined;
    eventMocks.listen.mockImplementation(async (_name, callback) => {
      listener = callback;
      return eventMocks.unlisten;
    });
    const handler = vi.fn();
    const removeHandler = agentEventManager.on('MessageDelta', handler);

    await agentEventManager.subscribeToSession('session-live');
    listener?.({
      payload: { type: 'MessageDelta', data: { turn_id: 'turn-1', message_id: 'message-1', delta: '实时' } },
    });

    expect(eventMocks.listen).toHaveBeenCalledWith('agent_event_session-live', expect.any(Function));
    expect(handler).toHaveBeenCalledWith(expect.objectContaining({ delta: '实时' }));
    removeHandler();

    // Stop and a new run can both happen before Tauri finishes registering the
    // old listener. Its queued callbacks and late cleanup must remain obsolete.
    agentEventManager.destroy();
    let oldListener: typeof listener;
    let newListener: typeof listener;
    let resolveOldListen: ((unlisten: () => void) => void) | undefined;
    const oldUnlisten = vi.fn();
    const newUnlisten = vi.fn();
    eventMocks.listen
      .mockImplementationOnce((_name, callback) => {
        oldListener = callback;
        return new Promise<() => void>((resolve) => { resolveOldListen = resolve; });
      })
      .mockImplementationOnce(async (_name, callback) => {
        newListener = callback;
        return newUnlisten;
      });
    const oldHandler = vi.fn();
    agentEventManager.on('MessageDelta', oldHandler);
    const oldSubscription = agentEventManager.subscribeToSession('session-live');
    const delta = { payload: { type: 'MessageDelta', data: { delta: '旧任务' } } };
    oldListener?.(delta);
    expect(oldHandler).toHaveBeenCalledTimes(1);

    agentEventManager.destroy();
    const newHandler = vi.fn();
    agentEventManager.on('MessageDelta', newHandler);
    await agentEventManager.subscribeToSession('session-live');
    oldListener?.(delta);
    expect(newHandler).not.toHaveBeenCalled();
    resolveOldListen?.(oldUnlisten);
    await oldSubscription;
    expect(oldUnlisten).toHaveBeenCalledTimes(1);
    oldListener?.(delta);
    expect(newHandler).not.toHaveBeenCalled();
    newListener?.({ payload: { type: 'MessageDelta', data: { delta: '新任务' } } });
    expect(newHandler).toHaveBeenCalledTimes(1);
    expect(newHandler).toHaveBeenCalledWith(expect.objectContaining({ delta: '新任务' }));
    agentEventManager.destroy();
    expect(oldUnlisten).toHaveBeenCalledTimes(1);
    expect(newUnlisten).toHaveBeenCalledTimes(1);
  });

  it('replaces live approval blocks only when the foreground message changes', async () => {
    let listener: ((event: { payload: { type: string; data: unknown } }) => void) | undefined;
    eventMocks.listen.mockImplementation(async (_name, callback) => {
      listener = callback;
      return eventMocks.unlisten;
    });
    const saved = { id: 'saved-reply', sessionId: 'session-live', role: 'assistant' as const,
      content: '输入框已经写入。', createdAt: 1 };
    useMessagesStore.setState({ messages: [saved], liveMessageId: 'field-reply',
      streamingText: '旧内容', liveBlocks: [{ kind: 'tool_call', index: 0, iterationId: 0,
        callId: 'field-call', toolName: 'set_trusted_app_text', arguments: {}, status: 'needs_approval' }],
      liveToolSteps: {}, toolProgress: {}, toolExecuting: false, executingTool: null });
    try {
      await useMessagesStore.getState().subscribeToAgentEvents('session-live');
      listener?.({ payload: { type: 'MessageStart', data: { turn_id: 'next-turn', message_id: 'check-reply' } } });
      expect(useMessagesStore.getState().liveMessageId).toBe('check-reply');
      expect(useMessagesStore.getState().liveBlocks).toEqual([]);
      expect(useMessagesStore.getState().streamingText).toBe('');
      expect(useMessagesStore.getState().messages).toEqual([saved]);
      listener?.({ payload: { type: 'ContentBlock', data: { turn_id: 'next-turn', message_id: 'check-reply',
        index: 0, iteration_id: 0, block: { kind: 'ToolCallStart', data: {
          call_id: 'check-call', tool_name: 'operate_trusted_app_control', arguments: { action: 'check' },
        } } } } });
      // A new model iteration in the same reply must preserve prior tool facts.
      listener?.({ payload: { type: 'MessageStart', data: { turn_id: 'next-turn', message_id: 'check-reply' } } });
      expect(useMessagesStore.getState().liveBlocks).toEqual([expect.objectContaining({ callId: 'check-call' })]);
    } finally {
      useMessagesStore.getState().unsubscribeFromAgentEvents();
      useMessagesStore.getState().clearMessages();
    }
  });

  it('replaces pending A → B → A subscriptions without joining the obsolete registration', async () => {
    let finishOld: ((stop: () => void) => void) | undefined;
    const oldStop = vi.fn();
    eventMocks.listen.mockImplementationOnce(() => new Promise<() => void>((resolve) => { finishOld = resolve; }));
    const old = agentEventManager.subscribeToSession('pending-A');
    try {
      await agentEventManager.subscribeToSession('ready-B');
      await agentEventManager.subscribeToSession('pending-A');
      expect(eventMocks.listen).toHaveBeenCalledTimes(3);
      finishOld?.(oldStop);
      await old;
      expect(oldStop).toHaveBeenCalledTimes(1);
    } finally {
      finishOld?.(oldStop);
      await old;
      agentEventManager.destroy();
    }
  });

  it('does not start the polling fallback when the desktop event bus is available', async () => {
    vi.useFakeTimers();
    await agentEventManager.subscribeToSession('session-live');
    await vi.advanceTimersByTimeAsync(1_000);

    expect(eventMocks.invoke).not.toHaveBeenCalled();

    // A stopped registration that rejects late must not restart polling over
    // the newer desktop listener.
    agentEventManager.destroy();
    let rejectOldListen: ((reason: Error) => void) | undefined;
    eventMocks.listen.mockImplementationOnce(() => new Promise<() => void>((_resolve, reject) => {
      rejectOldListen = reject;
    }));
    const oldSubscription = agentEventManager.subscribeToSession('session-live');
    agentEventManager.destroy();
    await agentEventManager.subscribeToSession('session-live');
    rejectOldListen?.(new Error('Stopped bridge registration'));
    await oldSubscription;
    await vi.advanceTimersByTimeAsync(1_000);
    expect(eventMocks.invoke).not.toHaveBeenCalled();

    // The browser fallback has the same lifetime: a response already in flight
    // at Stop must neither reach new handlers nor advance their replay cursor.
    agentEventManager.destroy();
    let resolveOldPoll: ((response: { events: { type: string; data: unknown }[] }) => void) | undefined;
    const fresh = { type: 'MessageDelta', data: { delta: '新轮询' } };
    eventMocks.listen.mockRejectedValue(new Error('Browser has no desktop bridge'));
    eventMocks.invoke
      .mockImplementationOnce(() => new Promise((resolve) => { resolveOldPoll = resolve; }))
      .mockResolvedValue({ events: [fresh] });
    await agentEventManager.subscribeToSession('session-live');
    await vi.advanceTimersByTimeAsync(500);
    expect(eventMocks.invoke).toHaveBeenCalledTimes(1);
    agentEventManager.destroy();
    const handler = vi.fn();
    agentEventManager.on('MessageDelta', handler);
    await agentEventManager.subscribeToSession('session-live');
    resolveOldPoll?.({ events: [{ type: 'MessageDelta', data: { delta: '旧轮询' } }] });
    await vi.advanceTimersByTimeAsync(0);
    expect(handler).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(500);
    expect(handler).toHaveBeenCalledTimes(1);
    expect(handler).toHaveBeenCalledWith(fresh.data);
    await vi.advanceTimersByTimeAsync(500);
    expect(handler).toHaveBeenCalledTimes(1);
    agentEventManager.destroy();
    const callsAfterStop = eventMocks.invoke.mock.calls.length;
    await vi.advanceTimersByTimeAsync(1_000);
    expect(eventMocks.invoke).toHaveBeenCalledTimes(callsAfterStop);
  });

  it('replays interleaved text and tool blocks in journal order', () => {
    const event = (sequence: number, block: unknown): DurableAgentRunEvent => ({
      id: `event-${sequence}`,
      run_id: 'run-1',
      session_id: 'session-1',
      sequence,
      event_type: 'content_block',
      created_at: sequence,
      payload: {
        type: 'ContentBlock',
        data: { turn_id: 'turn-1', message_id: 'run-1', index: sequence, iteration_id: sequence, block },
      },
    });

    const blocks = contentBlocksFromDurableEvents([
      { ...event(0, {}), event_type: 'ExternalToolDispatchReserved', payload: {
        type: 'ExternalToolDispatchReserved', data: { call_id: 'call-1', tool_name: 'mcp_owned__echo_1234abcd' },
      } },
      event(1, { kind: 'Text', data: { content: '先检查目录。' } }),
      event(2, { kind: 'ToolCallStart', data: { call_id: 'call-1', tool_name: 'list_directory', arguments: {} } }),
      event(3, { kind: 'ToolCallResult', data: { call_id: 'call-1', tool_name: 'list_directory', success: true, output: '[]' } }),
      event(4, { kind: 'Text', data: { content: '目录为空，开始创建报告。' } }),
    ]);

    expect(blocks.map((block) => block.kind)).toEqual(['text', 'tool_call', 'text']);
    expect(blocks[1]).toMatchObject({ status: 'completed', output: '[]' });
  });

  it('reconciles stale approval blocks with the durable tool settlement', () => {
    const event = (sequence: number, block: unknown): DurableAgentRunEvent => ({
      id: `event-${sequence}`,
      run_id: 'run-1',
      session_id: 'session-1',
      sequence,
      event_type: 'content_block',
      created_at: sequence,
      payload: {
        type: 'ContentBlock',
        data: { turn_id: 'turn-1', message_id: 'run-1', index: sequence, iteration_id: sequence, block },
      },
    });

    const blocks = contentBlocksFromDurableEvents(
      [
        event(1, { kind: 'ToolCallStart', data: { call_id: 'cancelled', tool_name: 'open_windows_setting', arguments: {} } }),
        event(2, { kind: 'ToolCallNeedsApproval', data: { call_id: 'cancelled', tool_name: 'open_windows_setting', arguments: {}, reason: 'approval required' } }),
        event(3, { kind: 'ToolCallFailed', data: { call_id: 'cancelled', tool_name: 'open_windows_setting', error: 'old journal failure' } }),
        event(4, { kind: 'ToolCallStart', data: { call_id: 'rejected', tool_name: 'write_file', arguments: {} } }),
        event(5, { kind: 'ToolCallNeedsApproval', data: { call_id: 'rejected', tool_name: 'write_file', arguments: {}, reason: 'approval required' } }),
      ],
      [
        { callId: 'cancelled', success: false, output: 'Confirmation cancelled', confirmationStatus: 'cancelled' },
        { callId: 'cancelled', success: false, output: 'Confirmation required', confirmationStatus: 'pending' },
        { callId: 'rejected', success: false, output: 'Confirmation rejected', confirmationStatus: 'rejected' },
      ],
    );

    expect(blocks).toHaveLength(1);
    expect(blocks[0]).toMatchObject({
      kind: 'tool_call',
      callId: 'rejected',
      status: 'failed',
      output: 'Confirmation rejected',
    });
  });

  it('settles the newest active occurrence when a provider reuses a call id', () => {
    const event = (sequence: number, block: unknown): DurableAgentRunEvent => ({
      id: `event-${sequence}`,
      run_id: 'run-1',
      session_id: 'session-1',
      sequence,
      event_type: 'content_block',
      created_at: sequence,
      payload: {
        type: 'ContentBlock',
        data: { turn_id: 'turn-1', message_id: 'run-1', index: sequence, iteration_id: sequence, block },
      },
    });

    const blocks = contentBlocksFromDurableEvents([
      event(1, { kind: 'ToolCallStart', data: { call_id: 'reused', tool_name: 'read_file', arguments: {} } }),
      event(2, { kind: 'ToolCallResult', data: { call_id: 'reused', tool_name: 'read_file', success: true, output: 'first' } }),
      event(3, { kind: 'ToolCallStart', data: { call_id: 'reused', tool_name: 'read_file', arguments: {} } }),
      event(4, { kind: 'ToolCallNeedsApproval', data: { call_id: 'reused', tool_name: 'read_file', arguments: {}, reason: 'second' } }),
    ]);

    expect(blocks).toHaveLength(2);
    expect(blocks[0]).toMatchObject({ status: 'completed', output: 'first' });
    expect(blocks[1]).toMatchObject({ status: 'needs_approval', reason: 'second' });
  });
});

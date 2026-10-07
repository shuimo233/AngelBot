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

import { agentEventManager, contentBlocksFromDurableEvents, formatAgentEvent, type DurableAgentRunEvent } from './agent-event';

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
  });

  it('does not start the polling fallback when the desktop event bus is available', async () => {
    vi.useFakeTimers();
    await agentEventManager.subscribeToSession('session-live');
    await vi.advanceTimersByTimeAsync(1_000);

    expect(eventMocks.invoke).not.toHaveBeenCalled();
    vi.useRealTimers();
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

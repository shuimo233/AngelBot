import { create } from 'zustand';
import type { Message, ToolResult } from '$types';
import { useSessionsStore } from './sessions';
import { agentEventManager } from '$lib/commands/agent-event';

export type LiveToolStep = {
  callId: string;
  toolName: string;
  arguments: unknown;
  status: 'running' | 'completed' | 'failed';
  progress?: string;
  percent?: number;
  output?: string;
};

/**
 * A single content block in the live streaming UI. Each block arrives in
 * order from the backend's unified `ContentBlock` event stream and is
 * inserted into the conversation at the position indicated by `index`.
 *
 * `iterationId` groups consecutive blocks that share a single main-loop
 * iteration (i.e. one model response plus the tools it called). The
 * renderer uses this to fold a tool batch into one "execution slice"
 * card rather than rendering each call as a separate inline badge.
 */
export type LiveBlock =
  | { kind: 'text'; index: number; iterationId: number; content: string }
  | {
      kind: 'tool_call';
      index: number;
      iterationId: number;
      status: 'running' | 'completed' | 'failed' | 'needs_approval';
      callId: string;
      toolName: string;
      arguments: unknown;
      output?: string;
      error?: string;
      reason?: string;
    };

function updateTaskRunFromToolResults(message: Message, toolResults: ToolResult[]): Message {
  if (!message.taskRun) return { ...message, toolResults };

  const hasPendingConfirmation = toolResults.some((r) =>
    r.confirmationRequired && r.confirmationStatus === 'pending'
  );
  const hasFailure = toolResults.some((r) => !r.success);
  const hasApprovedConfirmation = toolResults.some((r) => r.confirmationStatus === 'approved');
  const completedStepCount = toolResults.filter((r) => r.success).length;
  const status = hasPendingConfirmation
    ? 'awaiting_confirmation'
    : hasFailure
      ? 'needs_attention'
    : toolResults.length < message.taskRun.stepCount
        ? 'running'
        : hasApprovedConfirmation
          ? 'continue_suggested'
        : 'completed';
  const confirmationState = hasPendingConfirmation
    ? 'pending'
    : toolResults.some((r) => r.confirmationStatus === 'rejected')
      ? 'rejected'
      : toolResults.some((r) => r.confirmationStatus === 'approved')
        ? 'approved'
        : 'none';

  return {
    ...message,
    toolResults,
    taskRun: {
      ...message.taskRun,
      status,
      confirmationState,
      resumable: hasPendingConfirmation || hasFailure || hasApprovedConfirmation,
      completedStepCount,
    },
  };
}

interface MessagesState {
  messages: Message[];
  input: string;
  /** 工具正在执行中（用于显示加载指示器） */
  toolExecuting: boolean;
  /** 当前正在执行的工具名称 */
  executingTool: string | null;
  /** 当前流式文本（用于实时显示） */
  streamingText: string;
  /** Concise, user-visible status for the live agent activity card. */
  liveStatus: string;
  /** 工具执行进度 */
  toolProgress: Record<string, { name: string; progress: string; percent?: number }>;
  /** Safe, user-visible execution trace for the currently running agent. */
  liveToolSteps: Record<string, LiveToolStep>;
  /** Ordered content blocks for the currently streaming turn. */
  liveBlocks: LiveBlock[];
  /** Persisted assistant message ID reported by the active event stream. */
  liveMessageId: string | null;
  loadMessages: (messages: Message[]) => void;
  appendMessage: (message: Message) => void;
  removeMessage: (messageId: string) => void;
  updateMessageContent: (messageId: string, content: string) => void;
  setInput: (input: string) => void;
  clearMessages: () => void;
  setToolExecuting: (executing: boolean, tool?: string) => void;
  addToolResult: (messageId: string, result: ToolResult) => void;
  /** Called after a message is persisted — triggers auto-title if first user message */
  onMessageSent: (message: Message) => void;
  /** Update streaming text from agent events */
  updateStreamingText: (text: string) => void;
  setLiveStatus: (status: string) => void;
  /** Set tool progress from agent events */
  setToolProgress: (callId: string, name: string, progress: string, percent?: number) => void;
  /** Clear tool progress */
  clearToolProgress: (callId: string) => void;
  clearLiveActivity: () => void;
  /** Subscribe to agent events for real-time updates */
  subscribeToAgentEvents: (sessionId: string) => Promise<void>;
  /** Unsubscribe from agent events */
  unsubscribeFromAgentEvents: () => void;
}

export const useMessagesStore = create<MessagesState>((set, get) => ({
  messages: [],
  input: '',
  toolExecuting: false,
  executingTool: null,
  streamingText: '',
  liveStatus: '',
  toolProgress: {},
  liveToolSteps: {},
  liveBlocks: [],
  liveMessageId: null,

  loadMessages: (messages) => set({ messages }),
  appendMessage: (message) => set((state) => ({ messages: [...state.messages, message] })),
  removeMessage: (messageId) => set((state) => ({
    messages: state.messages.filter((m) => m.id !== messageId),
  })),
  updateMessageContent: (messageId, content) => set((state) => ({
    messages: state.messages.map((m) =>
      m.id === messageId ? { ...m, content } : m
    ),
  })),
  setInput: (input) => set({ input }),
  clearMessages: () => set({ messages: [], streamingText: '', liveStatus: '', liveBlocks: [], liveMessageId: null }),

  setToolExecuting: (executing, tool) =>
    set({ toolExecuting: executing, executingTool: tool ?? null }),

  addToolResult: (messageId, result) =>
    set((state) => ({
      messages: state.messages.map((m) => {
        if (m.id !== messageId) return m;
        const existing = m.toolResults ?? [];
        const index = existing.findIndex((item) => item.callId === result.callId);
        const toolResults = index >= 0
          ? existing.map((item, i) => (i === index ? result : item))
          : [...existing, result];
        return updateTaskRunFromToolResults(m, toolResults);
      }),
    })),

  onMessageSent: (message: Message) => {
    if (message.role !== 'user') return;
    const { messages } = get();
    // Only auto-title if this is the first user message in an untitled session
    const sessionId = message.sessionId;
    const sessions = useSessionsStore.getState().sessions;
    const session = sessions.find((s) => s.id === sessionId);
    if (!session || session.title.trim()) return; // already titled

    const userMessages = messages.filter((m) => m.role === 'user');
    if (userMessages.length > 1) return; // not the first

    // Auto-generate title from first message content
    const title = message.content.replace(/\n+/g, ' ').trim().slice(0, 40);
    const displayTitle = title.length < message.content.length ? `${title}...` : title;
    useSessionsStore.getState().updateSessionTitle(sessionId, displayTitle || '新会话');
  },

  updateStreamingText: (text) => set((state) => ({ streamingText: state.streamingText + text })),
  setLiveStatus: (liveStatus) => set({ liveStatus }),

  setToolProgress: (callId, name, progress, percent) =>
    set((state) => ({
      toolProgress: {
        ...state.toolProgress,
        [callId]: { name, progress, percent },
      },
      toolExecuting: true,
      executingTool: name,
    })),

  clearToolProgress: (callId) =>
    set((state) => {
      const newProgress = { ...state.toolProgress };
      delete newProgress[callId];
      const hasMore = Object.keys(newProgress).length > 0;
      return {
        toolProgress: newProgress,
        toolExecuting: hasMore,
        executingTool: hasMore ? Object.values(newProgress)[0]?.name ?? null : null,
      };
    }),

  clearLiveActivity: () => set({
    toolProgress: {},
    liveToolSteps: {},
    streamingText: '',
    liveStatus: '',
    liveBlocks: [],
    liveMessageId: null,
    toolExecuting: false,
    executingTool: null,
  }),

  subscribeToAgentEvents: async (sessionId) => {
    agentEventManager.destroy();
    const { setToolExecuting, setToolProgress, clearToolProgress, updateStreamingText, setLiveStatus } = get();

    agentEventManager.on('AgentStart', () => {
      setLiveStatus('正在理解你的请求');
    });

    // Do not expose hidden model reasoning. This is a concise, user-visible
    // task phase derived from the durable turn lifecycle event instead.
    agentEventManager.on('TurnStart', () => {
      setLiveStatus('正在分析任务并规划下一步');
    });

    agentEventManager.on('MessageStart', (data) => {
      set({ liveMessageId: data.message_id });
      setLiveStatus('正在规划下一步');
    });

    // Subscribe to tool execution events
    agentEventManager.on('ToolExecutionStart', (data) => {
      setToolExecuting(true, data.tool_name);
      setLiveStatus(`正在调用 ${data.tool_name}`);
      set((state) => ({
        liveToolSteps: {
          ...state.liveToolSteps,
          [data.call_id]: {
            callId: data.call_id,
            toolName: data.tool_name,
            arguments: data.arguments,
            status: 'running',
          },
        },
      }));
    });

    agentEventManager.on('ToolExecutionProgress', (data) => {
      setToolProgress(data.call_id, data.tool_name, data.progress, data.percent);
      set((state) => ({
        liveToolSteps: {
          ...state.liveToolSteps,
          [data.call_id]: {
            ...(state.liveToolSteps[data.call_id] ?? {
              callId: data.call_id,
              toolName: data.tool_name,
              arguments: {},
              status: 'running' as const,
            }),
            progress: data.progress,
            percent: data.percent,
          },
        },
      }));
    });

    agentEventManager.on('ToolExecutionEnd', (data) => {
      clearToolProgress(data.call_id);
      setLiveStatus(data.success ? `${data.tool_name} 已完成` : `${data.tool_name} 需要处理`);
      set((state) => ({
        liveToolSteps: {
          ...state.liveToolSteps,
          [data.call_id]: {
            ...(state.liveToolSteps[data.call_id] ?? {
              callId: data.call_id,
              toolName: data.tool_name,
              arguments: {},
              status: 'running' as const,
            }),
            status: data.success ? 'completed' : 'failed',
            output: data.error ?? data.output,
          },
        },
      }));
    });

    agentEventManager.on('MessageDelta', (data) => {
      setLiveStatus('正在生成回复');
      updateStreamingText(data.delta);
    });

    /**
     * Unified ContentBlock stream: each block carries an `index` that defines
     * its arrival order. Text deltas accumulate; tool blocks replace a
     * running block when a matching ToolCallResult/Failed/Approved arrives.
     * This is what powers the inline tool UI.
     */
    agentEventManager.on('ContentBlock', (data) => {
      const idx = data.index;
      const iterationId: number = data.iteration_id ?? 0;
      const block = data.block;
      set((state) => {
        const blocks = state.liveBlocks.slice();
        switch (block.kind) {
          case 'Text': {
            // Coalesce consecutive text blocks to avoid a flood of zero-length frames
            const last = blocks[blocks.length - 1];
            if (
              last &&
              last.kind === 'text' &&
              last.iterationId === iterationId &&
              last.index + 1 === idx
            ) {
              blocks[blocks.length - 1] = {
                ...last,
                index: idx,
                content: last.content + block.data.content,
              };
            } else {
              // Place at correct sorted position by index
              const insertAt = blocks.findIndex((b) => b.index > idx);
              const entry: LiveBlock = {
                kind: 'text',
                index: idx,
                iterationId,
                content: block.data.content,
              };
              if (insertAt < 0) blocks.push(entry);
              else blocks.splice(insertAt, 0, entry);
            }
            return { liveBlocks: blocks, liveStatus: '正在生成回复' };
          }
          case 'ToolCallStart': {
            const insertAt = blocks.findIndex((b) => b.index > idx);
            const entry: LiveBlock = {
              kind: 'tool_call',
              index: idx,
              iterationId,
              status: 'running',
              callId: block.data.call_id,
              toolName: block.data.tool_name,
              arguments: block.data.arguments,
            };
            if (insertAt < 0) blocks.push(entry);
            else blocks.splice(insertAt, 0, entry);
            return { liveBlocks: blocks };
          }
          case 'ToolCallResult': {
            // Find the running tool_call with matching callId and update its status
            const targetIdx = blocks.findIndex(
              (b) => b.kind === 'tool_call' && b.callId === block.data.call_id,
            );
            if (targetIdx >= 0) {
              const updated = {
                ...(blocks[targetIdx] as Extract<LiveBlock, { kind: 'tool_call' }>),
                status: (block.data.success ? 'completed' : 'failed') as 'completed' | 'failed',
                output: block.data.output,
                error: block.data.error,
              };
              blocks[targetIdx] = updated;
            }
            return { liveBlocks: blocks };
          }
          case 'ToolCallNeedsApproval': {
            const targetIdx = blocks.findIndex(
              (b) => b.kind === 'tool_call' && b.callId === block.data.call_id,
            );
            if (targetIdx >= 0) {
              blocks[targetIdx] = {
                ...(blocks[targetIdx] as Extract<LiveBlock, { kind: 'tool_call' }>),
                status: 'needs_approval',
                reason: block.data.reason,
              };
            }
            return { liveBlocks: blocks };
          }
          case 'ToolCallFailed': {
            const targetIdx = blocks.findIndex(
              (b) => b.kind === 'tool_call' && b.callId === block.data.call_id,
            );
            if (targetIdx >= 0) {
              blocks[targetIdx] = {
                ...(blocks[targetIdx] as Extract<LiveBlock, { kind: 'tool_call' }>),
                status: 'failed',
                error: block.data.error,
              };
            }
            return { liveBlocks: blocks };
          }
        }
        return state;
      });
    });

    agentEventManager.on('MessageEnd', (data) => {
      set({ streamingText: data.full_text, liveStatus: '正在整理结果' });
    });

    // Start listening to session events
    await agentEventManager.subscribeToSession(sessionId);
  },

  unsubscribeFromAgentEvents: () => {
    // Note: The event manager handles cleanup of individual handlers
    agentEventManager.destroy();
    set({ toolProgress: {}, liveToolSteps: {}, liveBlocks: [], streamingText: '', liveStatus: '' });
  },
}));

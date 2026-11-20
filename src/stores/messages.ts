import { create } from 'zustand';
import type { Message, ToolResult } from '$types';
import { useSessionsStore } from './sessions';
import { agentEventManager } from '$lib/commands/agent-event';
import { isResultUnknown } from '$lib/tool-action-result';
import { useWorkspaceActivityStore } from './workspaceActivity';
import { saveComposerDraft, type ComposerDraft } from '$lib/commands/composer-draft';
import { isSecureWebSearchSetupDraft } from '$lib/web-search-setup';

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
      resumable: !toolResults.some(isResultUnknown)
        && (hasPendingConfirmation || hasFailure || hasApprovedConfirmation),
      completedStepCount,
    },
  };
}

type DraftInput = { text: string; token: object; localOnly: boolean };

interface MessagesState {
  messages: Message[];
  input: string;
  inputSessionId: string | null;
  inputToken: object;
  inputLocalOnly: boolean;
  draftInputs: Record<string, DraftInput>;
  durableDraftSessions: ReadonlySet<string>;
  draftSaveErrors: ReadonlySet<string>;
  queuedFollowUps: Record<string, QueuedFollowUp[]>;
  /** Stop requests still awaiting a reply, including while Main is closed. */
  stoppingSessions: ReadonlySet<string>;
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
  setInput: (input: string, localOnly?: boolean) => void;
  registerDraft: (sessionId: string, saved: ComposerDraft) => void;
  activateInput: (sessionId: string | null) => void;
  clearSubmittedInput: (sessionId: string, submitted: string, token: object, clearedToken?: object) => void;
  forgetInput: (sessionId: string) => void;
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

export type QueuedFollowUp = { id: string; text: string; unconfirmed?: boolean };

let lastDraftRevision = 0;
const pendingDraftWrites = new Map<string, number>();

function saveInput(sessionId: string, text: string) {
  if (!useMessagesStore.getState().durableDraftSessions.has(sessionId)) return;
  // Use the shared monotonic epoch across page loads, and the host's last
  // observed revision as a floor. Each edit can dispatch immediately.
  const revision = Math.max(lastDraftRevision + 1, Math.floor(1000 * (performance.timeOrigin + performance.now())));
  lastDraftRevision = revision;
  pendingDraftWrites.set(sessionId, revision);
  const settle = (saved: boolean) => {
    if (pendingDraftWrites.get(sessionId) !== revision) return;
    pendingDraftWrites.delete(sessionId);
    useMessagesStore.setState((state) => {
      if (saved === !state.draftSaveErrors.has(sessionId)) return state;
      const draftSaveErrors = new Set(state.draftSaveErrors);
      if (saved) draftSaveErrors.delete(sessionId);
      else draftSaveErrors.add(sessionId);
      return { draftSaveErrors };
    });
  };
  void saveComposerDraft(sessionId, revision, text).then(settle, () => settle(false));
}

export const useMessagesStore = create<MessagesState>((set, get) => ({
  messages: [],
  input: '',
  inputSessionId: null,
  inputToken: {},
  inputLocalOnly: false,
  draftInputs: {},
  durableDraftSessions: new Set(),
  draftSaveErrors: new Set(),
  queuedFollowUps: {},
  stoppingSessions: new Set(),
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
  setInput: (input, localOnly = false) => {
    const state = get();
    const inputLocalOnly = Boolean(input) && (state.inputLocalOnly || localOnly || isSecureWebSearchSetupDraft(input));
    if (state.input === input && state.inputLocalOnly === inputLocalOnly) return;
    set({ input, inputLocalOnly, inputToken: {} });
    if (state.inputSessionId && (!inputLocalOnly || !state.inputLocalOnly)) {
      saveInput(state.inputSessionId, inputLocalOnly || !input.trim() ? '' : input);
    }
  },
  registerDraft: (sessionId, saved) => set((state) => {
    lastDraftRevision = Math.max(lastDraftRevision, saved.revision);
    if (state.durableDraftSessions.has(sessionId)) return state;
    return {
      durableDraftSessions: new Set([...state.durableDraftSessions, sessionId]),
      ...(state.inputSessionId !== sessionId && !Object.prototype.hasOwnProperty.call(state.draftInputs, sessionId) && saved.text ? {
        draftInputs: { ...state.draftInputs, [sessionId]: { text: saved.text, token: {}, localOnly: false } },
      } : {}),
    };
  }),
  activateInput: (sessionId) => set((state) => {
    if (state.inputSessionId === sessionId) return state;
    const draftInputs = { ...state.draftInputs };
    if (state.inputSessionId) {
      if (state.input) draftInputs[state.inputSessionId] = { text: state.input, token: state.inputToken, localOnly: state.inputLocalOnly };
      else delete draftInputs[state.inputSessionId];
    }
    const draft = sessionId ? draftInputs[sessionId] : undefined;
    if (sessionId) delete draftInputs[sessionId];
    return {
      input: draft?.text ?? '',
      inputToken: draft?.token ?? {},
      inputSessionId: sessionId,
      inputLocalOnly: draft?.localOnly ?? false,
      draftInputs,
    };
  }),
  clearSubmittedInput: (sessionId, submitted, token, clearedToken = {}) => {
    const state = get();
    if (state.inputSessionId === sessionId) {
      if (state.input.trim() !== submitted.trim() || token !== state.inputToken) return;
      set({ input: '', inputToken: clearedToken, inputLocalOnly: false });
    } else {
      const draft = state.draftInputs[sessionId];
      if (!draft || draft.text.trim() !== submitted.trim() || token !== draft.token) return;
      const draftInputs = { ...state.draftInputs };
      delete draftInputs[sessionId];
      set({ draftInputs });
    }
    saveInput(sessionId, '');
  },
  forgetInput: (sessionId) => set((state) => {
    pendingDraftWrites.delete(sessionId);
    const durableDraftSessions = new Set(state.durableDraftSessions);
    durableDraftSessions.delete(sessionId);
    const draftSaveErrors = new Set(state.draftSaveErrors);
    draftSaveErrors.delete(sessionId);
    const draftInputs = { ...state.draftInputs };
    delete draftInputs[sessionId];
    return {
      ...(state.inputSessionId === sessionId ? { input: '', inputSessionId: null, inputToken: {}, inputLocalOnly: false } : {}),
      draftInputs,
      durableDraftSessions,
      draftSaveErrors,
    };
  }),
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
      useWorkspaceActivityStore.getState().invalidate(sessionId);
    });

    // Do not expose hidden model reasoning. This is a concise, user-visible
    // task phase derived from the durable turn lifecycle event instead.
    agentEventManager.on('TurnStart', () => {
      setLiveStatus('正在分析任务并规划下一步');
    });

    agentEventManager.on('MessageStart', (data) => {
      // A confirmation can start another durable reply before its IPC returns.
      // Its block indexes restart at zero; never carry the prior approval into
      // that new message. Repeated starts within one reply are model iterations.
      set((state) => state.liveMessageId === data.message_id ? state : {
        liveMessageId: data.message_id,
        liveBlocks: [],
        streamingText: '',
        liveToolSteps: {},
        toolProgress: {},
        toolExecuting: false,
        executingTool: null,
      });
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
      useWorkspaceActivityStore.getState().invalidate(sessionId);
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

    const refreshActivity = () => useWorkspaceActivityStore.getState().invalidate(sessionId);
    agentEventManager.on('AgentEnd', refreshActivity);
    agentEventManager.on('Error', refreshActivity);

    // Start listening to session events
    await agentEventManager.subscribeToSession(sessionId);
  },

  unsubscribeFromAgentEvents: () => {
    // Note: The event manager handles cleanup of individual handlers
    agentEventManager.destroy();
    set({ toolProgress: {}, liveToolSteps: {}, liveBlocks: [], streamingText: '', liveStatus: '' });
  },
}));

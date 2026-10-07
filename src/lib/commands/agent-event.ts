/**
 * Agent Event System (Issue #40)
 * 
 * Provides real-time UI updates by subscribing to agent events.
 * Events are emitted via Tauri event system during agent execution.
 */

import { useEffect } from 'react';
import { invoke } from '$lib/invoke';
import { listen } from '@tauri-apps/api/event';
import type { LiveBlock } from '$stores/messages';

export interface AgentEvent {
  type: string;
  data: unknown;
}

export interface DurableAgentRunEvent {
  id: string;
  run_id: string;
  session_id: string;
  sequence: number;
  event_type: string;
  payload: AgentEvent;
  created_at: number;
}

/** Read persisted lifecycle events for audit/replay after an app restart. */
export async function getDurableAgentRunEvents(
  sessionId: string,
  runId: string,
  afterSequence?: number,
): Promise<DurableAgentRunEvent[]> {
  return invoke<DurableAgentRunEvent[]>('get_agent_run_events', {
    sessionId,
    runId,
    afterSequence,
  });
}

// Event types for type-safe handling
export type AgentEventType =
  | 'AgentStart'
  | 'TurnStart'
  | 'MessageStart'
  | 'MessageDelta'
  | 'MessageUpdate'  // Issue #094
  | 'MessageEnd'
  | 'ContentBlock'   // Unified text+tool stream
  | 'ToolExecutionStart'
  | 'ToolExecutionProgress'
  | 'ToolExecutionEnd'
  | 'TurnEnd'
  | 'AgentEnd'
  | 'ContextCompressed'
  | 'Error';

export interface EventData {
  AgentStart: { session_id: string; timestamp: number };
  TurnStart: { turn_id: string; session_id: string; message: string };
  MessageStart: { turn_id: string; message_id: string };
  MessageDelta: { turn_id: string; message_id: string; delta: string };
  MessageUpdate: { turn_id: string; message_id: string; content: string; is_final: boolean };  // Issue #094
  MessageEnd: { turn_id: string; message_id: string; full_text: string };
  /** Unified content block stream — sequential index carries arrival order. */
  ContentBlock: {
    turn_id: string;
    message_id: string;
    index: number;
    /**
     * Main-loop iteration that produced this block. Consecutive tool
     * blocks sharing the same `iteration_id` belong to one tool batch
     * and the frontend folds them into a single "execution slice".
     */
    iteration_id: number;
    block: ContentBlockData;
  };
  ToolExecutionStart: { call_id: string; tool_name: string; arguments: unknown };
  ToolExecutionProgress: { call_id: string; tool_name: string; progress: string; percent?: number };
  ToolExecutionEnd: { call_id: string; tool_name: string; success: boolean; output: string; error?: string };
  TurnEnd: { turn_id: string; success: boolean; tool_calls_count: number };
  AgentEnd: { session_id: string; summary?: string; total_turns: number; total_tool_calls: number };
  ContextCompressed: {
    session_id: string;
    turn_id: string;
    before_tokens: number;
    after_tokens: number;
    summary_id?: string;
    includes_task_facts: boolean;
  };
  Error: { code: string; message: string; recoverable: boolean; turn_id?: string };
}

/** Unified block types emitted in arrival order during streaming. */
export type ContentBlockData =
  | { kind: 'Text'; data: { content: string } }
  | { kind: 'ToolCallStart'; data: { call_id: string; tool_name: string; arguments: unknown } }
  | { kind: 'ToolCallResult'; data: { call_id: string; tool_name: string; success: boolean; output: string; error?: string } }
  | { kind: 'ToolCallNeedsApproval'; data: { call_id: string; tool_name: string; arguments: unknown; reason: string } }
  | { kind: 'ToolCallFailed'; data: { call_id: string; tool_name: string; error: string } };

/** The durable tool protocol is authoritative over an older append-only block. */
export type DurableToolResult = {
  callId: string;
  success: boolean;
  output: string;
  error?: string;
  confirmationStatus?: string;
};

/**
 * Collapse replayed tool rows without allowing an old pending projection to
 * resurrect an action that has already reached a terminal confirmation state.
 * Equal-precedence rows retain durable input order, which is the only order
 * exposed by the persisted message protocol.
 */
export function authoritativeToolResultsByCallId<T extends DurableToolResult>(
  results: readonly T[],
): Map<string, T> {
  const precedence = (result: DurableToolResult): number => {
    if (result.confirmationStatus === 'pending') return 0;
    if (result.confirmationStatus) return 2;
    return 1;
  };

  const indexed = new Map<string, T>();
  for (const result of results) {
    const prior = indexed.get(result.callId);
    if (!prior || precedence(result) >= precedence(prior)) {
      indexed.set(result.callId, result);
    }
  }
  return indexed;
}

function latestUnsettledToolBlockIndex(blocks: readonly LiveBlock[], callId: string): number {
  for (let index = blocks.length - 1; index >= 0; index -= 1) {
    const block = blocks[index];
    if (
      block.kind === 'tool_call'
      && block.callId === callId
      && (block.status === 'running' || block.status === 'needs_approval')
    ) {
      return index;
    }
  }
  return -1;
}

/** Rebuild the visible text/tool stream from the append-only run journal. */
export function contentBlocksFromDurableEvents(
  events: DurableAgentRunEvent[],
  durableToolResults: readonly DurableToolResult[] = [],
): LiveBlock[] {
  const blocks: LiveBlock[] = [];
  for (const event of events) {
    if (event.payload.type !== 'ContentBlock') continue;
    const data = event.payload.data as EventData['ContentBlock'];
    const { index, iteration_id: iterationId, block } = data;
    if (block.kind === 'Text') {
      blocks.push({ kind: 'text', index, iterationId, content: block.data.content });
    } else if (block.kind === 'ToolCallStart') {
      blocks.push({
        kind: 'tool_call', index, iterationId, status: 'running',
        callId: block.data.call_id, toolName: block.data.tool_name, arguments: block.data.arguments,
      });
    } else {
      const callId = block.data.call_id;
      const position = latestUnsettledToolBlockIndex(blocks, callId);
      if (position < 0 || blocks[position].kind !== 'tool_call') continue;
      const prior = blocks[position];
      if (block.kind === 'ToolCallResult') {
        blocks[position] = { ...prior, status: block.data.success ? 'completed' : 'failed', output: block.data.output, error: block.data.error };
      } else if (block.kind === 'ToolCallNeedsApproval') {
        blocks[position] = { ...prior, status: 'needs_approval', reason: block.data.reason };
      } else if (block.kind === 'ToolCallFailed') {
        blocks[position] = { ...prior, status: 'failed', error: block.data.error };
      }
    }
  }
  const resultsByCallId = authoritativeToolResultsByCallId(durableToolResults);
  return blocks.flatMap<LiveBlock>((block): LiveBlock[] => {
    if (block.kind !== 'tool_call') return [block];
    const result = resultsByCallId.get(block.callId);
    if (!result || result.confirmationStatus === 'pending') return [block];

    // A new user turn can cancel a once-pending action. The event log remains
    // append-only, but the cancelled action must not reappear as a live card
    // after rehydration.
    if (result.confirmationStatus === 'cancelled') return [];

    return [{
      ...block,
      status: result.success ? 'completed' : 'failed',
      output: result.output,
      error: result.error,
    }];
  });
}

type EventHandler<T extends AgentEventType> = (data: EventData[T]) => void;

type UnlistenFn = () => void;

/**
 * Agent Event Manager
 * Manages event subscriptions and dispatches events to registered handlers.
 */
class AgentEventManager {
  private handlers: Map<AgentEventType, Set<EventHandler<AgentEventType>>> = new Map();
  private unlistenFns: Map<string, UnlistenFn> = new Map();
  private isListening = false;

  /**
   * Subscribe to a specific agent event type
   */
  on<T extends AgentEventType>(eventType: T, handler: EventHandler<T>): () => void {
    if (!this.handlers.has(eventType)) {
      this.handlers.set(eventType, new Set());
    }
    this.handlers.get(eventType)!.add(handler as EventHandler<AgentEventType>);

    // Return unsubscribe function
    return () => {
      const handlers = this.handlers.get(eventType);
      if (handlers) {
        handlers.delete(handler as EventHandler<AgentEventType>);
      }
    };
  }

  /**
   * Subscribe to all agent events for a session
   */
  async subscribeToSession(sessionId: string): Promise<void> {
    if (this.isListening && this.unlistenFns.has(sessionId)) return;
    if (this.isListening) this.destroy();

    // Register the desktop event listener before sending the request so early
    // TurnStart/MessageDelta events cannot be lost while it is still running.
    let subscribedToDesktopEvents = false;
    try {
      const unlisten = await listen<AgentEvent>(`agent_event_${sessionId}`, (event) => {
        this.dispatch(event.payload);
      });
      this.unlistenFns.set(sessionId, unlisten);
      subscribedToDesktopEvents = true;
    } catch (error) {
      // Browser/test and Dev HTTP environments have no Tauri event bridge.
      // Keep the polling path alive there; desktop still receives events via
      // the listener above.
      console.debug('[AgentEventManager] Tauri event bridge unavailable; using polling fallback.', error);
    }

    // Polling is only for a browser-only Dev HTTP session.  A desktop runner
    // already pushes the same events through Tauri's event bus; polling there
    // duplicates deltas and can make streamed text appear twice.
    if (!subscribedToDesktopEvents) this.startPolling(sessionId);
    this.isListening = true;
  }

  /**
   * Unsubscribe from session events
   */
  unsubscribe(sessionId: string): void {
    const unlisten = this.unlistenFns.get(sessionId);
    if (unlisten) {
      unlisten();
      this.unlistenFns.delete(sessionId);
    }
  }

  /**
   * Dispatch an event to registered handlers
   */
  private dispatch(event: AgentEvent): void {
    const handlers = this.handlers.get(event.type as AgentEventType);
    if (handlers) {
      handlers.forEach((handler) => {
        try {
          handler(event.data as EventData[AgentEventType]);
        } catch (e) {
          console.error('[AgentEvent] Handler error:', e);
        }
      });
    }
  }

  /**
   * Fallback polling mechanism
   */
  private pollingInterval: number | null = null;
  private lastEventCount = 0;

  private startPolling(sessionId: string): void {
    if (this.pollingInterval) return;
    this.lastEventCount = 0;

    this.pollingInterval = window.setInterval(async () => {
      try {
        const response = await invoke<{ events: AgentEvent[] }>('get_agent_events', {
          channelId: `session-${sessionId}`,
        });
        // Handle polled events
        if (response.events.length > this.lastEventCount) {
          const newEvents = response.events.slice(this.lastEventCount);
          newEvents.forEach((event) => this.dispatch(event));
          this.lastEventCount = response.events.length;
        }
      } catch (e) {
        // Silently fail polling
      }
    }, 500);
  }

  private stopPolling(): void {
    if (this.pollingInterval) {
      clearInterval(this.pollingInterval);
    this.pollingInterval = null;
    this.lastEventCount = 0;
    }
  }

  /**
   * Cleanup all subscriptions
   */
  destroy(): void {
    this.unlistenFns.forEach((unlisten) => unlisten());
    this.unlistenFns.clear();
    this.handlers.clear();
    this.stopPolling();
    this.isListening = false;
  }
}

// Singleton instance
export const agentEventManager = new AgentEventManager();

/**
 * React hook for subscribing to agent events
 */
export function useAgentEvents<T extends AgentEventType>(
  eventType: T,
  handler: EventHandler<T>
): void {
  useEffect(() => {
    const unsubscribe = agentEventManager.on(eventType, handler);
    return unsubscribe;
  }, [eventType, handler]);
}

/**
 * Create a typed event handler helper
 */
export function createTypedHandler<T extends AgentEventType>(
  eventType: T,
  handler: (data: EventData[T]) => void
): { type: T; handler: (data: EventData[T]) => void } {
  return { type: eventType, handler };
}

/**
 * Utility to format agent event for display
 */
export function formatAgentEvent(event: AgentEvent): string {
  switch (event.type) {
    case 'AgentStart':
      return `Agent started (session: ${(event.data as EventData['AgentStart']).session_id})`;
    case 'TurnStart':
      return `Turn started: ${(event.data as EventData['TurnStart']).message.slice(0, 50)}...`;
    case 'MessageDelta':
      return 'Receiving text...';
    case 'MessageEnd':
      return 'Message complete';
    case 'ContextCompressed': {
      const data = event.data as EventData['ContextCompressed'];
      const source = data.summary_id ? ` (summary: ${data.summary_id})` : '';
      return `Context compressed${data.includes_task_facts ? ' with task facts' : ''}${source}`;
    }
    case 'ToolExecutionStart':
      return `Running tool: ${(event.data as EventData['ToolExecutionStart']).tool_name}`;
    case 'ToolExecutionProgress':
      return `${(event.data as EventData['ToolExecutionProgress']).tool_name}: ${(event.data as EventData['ToolExecutionProgress']).progress}`;
    case 'ToolExecutionEnd': {
      const data = event.data as EventData['ToolExecutionEnd'];
      return data.success ? `Tool complete: ${data.tool_name}` : `Tool failed: ${data.tool_name}`;
    }
    case 'TurnEnd': {
      const data = event.data as EventData['TurnEnd'];
      return data.success ? `Turn complete (${data.tool_calls_count} tools)` : 'Turn failed';
    }
    case 'AgentEnd': {
      const data = event.data as EventData['AgentEnd'];
      return `Agent finished (${data.total_turns} turns, ${data.total_tool_calls} tools)`;
    }
    case 'Error': {
      const data = event.data as EventData['Error'];
      return `Error [${data.code}]: ${data.message}`;
    }
    default:
      return `Unknown event: ${event.type}`;
  }
}

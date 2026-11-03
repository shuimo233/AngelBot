import { create } from 'zustand';
import { getWorkspaceActivity, type WorkspaceActivityProjection } from '$lib/commands/workspace-activity';
import { subscribeAgentSessionEvents } from '$lib/commands/agent-event';

interface WorkspaceActivityState {
  projection: WorkspaceActivityProjection | null;
  workspaceId: string | null;
  sessionId: string | null;
  isLoading: boolean;
  error: string | null;
  refresh: (workspaceId: string) => Promise<void>;
  observe: (workspaceId: string, sessionId: string) => () => void;
  /** Existing Main polling fallback hint; native hints are already delivered. */
  invalidate: (sessionId: string) => void;
  clear: () => void;
}

function messageFrom(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const pollingStatuses = new Set(['queued', 'running', 'awaitingSummary', 'awaitingReview', 'materializationPending', 'unknown']);
const refreshEvents = new Set(['AgentStart', 'ToolExecutionEnd', 'AgentEnd', 'Error']);

/** Owns read synchronization only. Hints never execute, retry or approve work. */
export const useWorkspaceActivityStore = create<WorkspaceActivityState>((set, get) => {
  let read: { workspaceId: string; started: boolean; dirty: boolean; promise: Promise<void> } | null = null;
  let observation: {
    workspaceId: string; sessionId: string; consumers: number;
    nativeConnected: boolean; timer?: number; dispose: () => void;
  } | null = null;

  const stopPolling = () => {
    if (observation?.timer !== undefined) window.clearTimeout(observation.timer);
    if (observation) observation.timer = undefined;
  };
  const schedule = () => {
    stopPolling();
    const owner = observation;
    if (!owner || read || document.visibilityState === 'hidden'
      || !get().projection?.work.some((item) => pollingStatuses.has(item.status))) return;
    owner.timer = window.setTimeout(() => {
      owner.timer = undefined;
      if (observation === owner) void get().refresh(owner.workspaceId);
    }, 3000);
  };
  const detach = () => {
    stopPolling();
    observation?.dispose();
    observation = null;
    read = null; // Late reads cannot commit into a replacement lease.
    set({ isLoading: false });
  };

  return {
    projection: null, workspaceId: null, sessionId: null, isLoading: false, error: null,
    refresh: (workspaceId) => {
      if (!workspaceId || (observation && observation.workspaceId !== workspaceId)) return Promise.resolve();
      if (read?.workspaceId === workspaceId) {
        if (read.started) read.dirty = true; // At most one trailing read, not N parallel requests.
        return read.promise;
      }
      stopPolling();
      const request: NonNullable<typeof read> = {
        workspaceId, dirty: false, started: false,
        promise: Promise.resolve().then(async () => {
          request.started = true;
          do {
            if (read !== request) return;
            request.dirty = false;
            try {
              const projection = await getWorkspaceActivity(workspaceId);
              if (projection.workspaceId !== workspaceId) throw new Error('收到不属于当前工作区的任务状态，请重新检查。');
              if (read === request) set({ projection, error: null });
            } catch (error) {
              if (read === request) set({ error: messageFrom(error) });
            }
          } while (read === request && request.dirty);
        }).finally(() => {
          if (read !== request) return;
          read = null;
          set({ isLoading: false });
          schedule();
        }),
      };
      read = request;
      set(get().workspaceId === workspaceId ? { isLoading: true } : {
        workspaceId, sessionId: observation?.sessionId ?? null,
        projection: null, isLoading: true, error: null,
      });
      return request.promise;
    },
    observe: (workspaceId, sessionId) => {
      if (!workspaceId || !sessionId) return () => {};
      if (observation?.workspaceId !== workspaceId || observation.sessionId !== sessionId) {
        detach();
        const sameScope = get().workspaceId === workspaceId && get().sessionId === sessionId;
        set({ workspaceId, sessionId, isLoading: !sameScope || !get().projection,
          ...(sameScope ? {} : { projection: null, error: null }) });
        let stopNative: (() => void) | undefined;
        const owner: NonNullable<typeof observation> = {
          workspaceId, sessionId, consumers: 0, nativeConnected: false,
          dispose: () => {
            stopNative?.();
            window.removeEventListener('focus', changed);
            document.removeEventListener('visibilitychange', changed);
          },
        };
        const changed = () => {
          if (observation !== owner) return;
          if (document.visibilityState === 'hidden') stopPolling();
          else void get().refresh(workspaceId);
        };
        observation = owner;
        window.addEventListener('focus', changed);
        document.addEventListener('visibilitychange', changed);
        // Register before the first read, so an initially empty view can see
        // newly dispatched work. Main and the panel share this native stream.
        const subscription = subscribeAgentSessionEvents(sessionId, (event) => {
          if (refreshEvents.has(event.type)) changed();
        });
        stopNative = subscription.unsubscribe;
        void subscription.ready.then(() => {
          if (observation !== owner) return;
          owner.nativeConnected = true;
          changed();
        }).catch(() => {
          // Dev HTTP has no native bridge. Existing Main event polling calls
          // invalidate; focus/manual reads and known-active polling remain.
          changed();
        });
      }
      const owner = observation;
      owner.consumers += 1;
      let released = false;
      return () => {
        if (released) return;
        released = true;
        owner.consumers -= 1;
        if (observation === owner && owner.consumers === 0) detach();
      };
    },
    invalidate: (sessionId) => {
      if (observation?.sessionId === sessionId && !observation.nativeConnected
        && document.visibilityState !== 'hidden') void get().refresh(observation.workspaceId);
    },
    clear: () => {
      detach();
      set({ projection: null, workspaceId: null, sessionId: null, isLoading: false, error: null });
    },
  };
});

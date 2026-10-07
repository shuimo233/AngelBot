import { create } from 'zustand';
import type { Session } from '$types';
import { getSessions, createSession as apiCreate, deleteSession as apiDelete } from '$lib/commands';
import { updateSessionTitle as apiUpdateTitle, updateSessionModel as apiUpdateModel } from '$lib/commands/session';
import { useSettingsStore } from '$stores/settings';

interface SessionsState {
  sessions: Session[];
  activeSessionId: string | null;
  activeSession: Session | null;
  isLoading: boolean;
  /** Most recent store-level error, formatted for UI display. Subscribers
   * surface this as a toast. Cleared on the next successful operation. */
  lastError: string | null;
  clearError: () => void;
  loadSessions: () => Promise<void>;
  createSession: (title: string, workDir?: string, agentProvider?: string, agentModel?: string) => Promise<Session>;
  selectSession: (id: string) => void;
  deleteSession: (id: string) => Promise<void>;
  updateSessionTitle: (id: string, title: string) => void;
  updateSessionWorkDir: (id: string, workDir: string) => void;
  updateSessionModel: (id: string, agentProvider: string, agentModel: string) => Promise<void>;
}

function resolveActiveSession(sessions: Session[], activeSessionId: string | null) {
  if (sessions.length === 0) {
    return {
      activeSessionId: null,
      activeSession: null,
    };
  }

  const activeSession = activeSessionId
    ? sessions.find((session) => session.id === activeSessionId) ?? null
    : null;

  if (activeSession) {
    return {
      activeSessionId: activeSession.id,
      activeSession,
    };
  }

  return {
    activeSessionId: sessions[0].id,
    activeSession: sessions[0],
  };
}

const SESSION_LOAD_ATTEMPTS = 4;

function formatError(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

async function loadSessionsWithRetry(): Promise<Session[]> {
  let lastError: unknown;
  for (let attempt = 0; attempt < SESSION_LOAD_ATTEMPTS; attempt += 1) {
    try {
      return await getSessions();
    } catch (error) {
      lastError = error;
      if (attempt < SESSION_LOAD_ATTEMPTS - 1) {
        await new Promise((resolve) => setTimeout(resolve, 250 * (attempt + 1)));
      }
    }
  }
  throw lastError;
}

export const useSessionsStore = create<SessionsState>((set, get) => ({
  sessions: [],
  activeSessionId: null,
  activeSession: null,
  isLoading: false,
  lastError: null,

  clearError: () => set({ lastError: null }),

  loadSessions: async () => {
    set({ isLoading: true });
    try {
      const sessions = await loadSessionsWithRetry();
      set((state) => ({
        sessions,
        isLoading: false,
        lastError: null,
        ...resolveActiveSession(sessions, state.activeSessionId),
      }));
    } catch (err) {
      const message = formatError(err);
      console.error('Failed to load sessions:', err);
      set({ isLoading: false, lastError: message });
    }
  },

  createSession: async (title: string, workDir?: string, agentProvider?: string, agentModel?: string) => {
    const apiConfig = useSettingsStore.getState().activeApiConfig;
    const provider = agentProvider ?? apiConfig.provider;
    const model = agentModel ?? apiConfig.model;
    const session = await apiCreate(title, workDir, provider, model);
    set((state) => ({
      sessions: [session, ...state.sessions],
      activeSessionId: session.id,
      activeSession: session,
    }));
    return session;
  },

  selectSession: (id: string) => {
    const { sessions } = get();
    const session = sessions.find((s) => s.id === id) ?? null;
    set({ activeSessionId: id, activeSession: session });
  },

  deleteSession: async (id: string) => {
    await apiDelete(id);
    set((state) => {
      const sessions = state.sessions.filter((s) => s.id !== id);
      return {
        sessions,
        ...resolveActiveSession(
          sessions,
          state.activeSessionId === id ? null : state.activeSessionId
        ),
      };
    });
  },

  /** Update session title. The local state is updated immediately so the
   * rename feels instant; the backend write is fire-and-forget and a
   * failure is surfaced as a toast via `lastError`. We deliberately do
   * not roll back the optimistic update — the rename is recoverable on
   * the next reload, and reverting would create the very "system
   * undoing what I just did" friction this app is built to avoid. */
  updateSessionTitle: (id: string, title: string) => {
    set((state) => ({
      sessions: state.sessions.map((s) => (s.id === id ? { ...s, title } : s)),
      activeSession: state.activeSession?.id === id ? { ...state.activeSession, title } : state.activeSession,
    }));
    apiUpdateTitle(id, title).catch((err: unknown) => {
      const message = formatError(err);
      console.error('Failed to persist session title:', err);
      set({ lastError: message });
    });
  },

  /** Update session work directory. Local state only; backend persistence
   * happens through the dedicated work-dir command path. */
  updateSessionWorkDir: (id: string, workDir: string) => {
    set((state) => ({
      sessions: state.sessions.map((s) => (s.id === id ? { ...s, workDir } : s)),
      activeSession: state.activeSession?.id === id ? { ...state.activeSession, workDir } : state.activeSession,
    }));
  },

  updateSessionModel: async (id: string, agentProvider: string, agentModel: string) => {
    await apiUpdateModel(id, agentProvider, agentModel);
    set((state) => ({
      sessions: state.sessions.map((s) =>
        s.id === id ? { ...s, agentProvider, agentModel } : s
      ),
      activeSession: state.activeSession?.id === id
        ? { ...state.activeSession, agentProvider, agentModel }
        : state.activeSession,
    }));
  },
}));

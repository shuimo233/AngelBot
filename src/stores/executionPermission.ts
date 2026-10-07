import { create } from 'zustand';
import {
  getAgentExecutionPermission,
  setAgentExecutionPermission,
  type AgentExecutionPermission,
} from '$lib/commands/settings';

interface ExecutionPermissionState {
  permission: AgentExecutionPermission | null;
  loading: boolean;
  saving: boolean;
  error: string | null;
  load: () => Promise<boolean>;
  save: (permission: AgentExecutionPermission) => Promise<boolean>;
}

let pendingLoad: Promise<boolean> | null = null;

export const useExecutionPermissionStore = create<ExecutionPermissionState>((set, get) => ({
  permission: null,
  loading: true,
  saving: false,
  error: null,
  load: () => {
    if (get().permission !== null) return Promise.resolve(true);
    if (pendingLoad) return pendingLoad;
    set({ loading: true, error: null });
    pendingLoad = (async () => {
      try {
        set({ permission: await getAgentExecutionPermission(), error: null });
        return true;
      } catch (reason) {
        set({ error: reason instanceof Error ? reason.message : String(reason) });
        return false;
      } finally {
        set({ loading: false });
        pendingLoad = null;
      }
    })();
    return pendingLoad;
  },
  save: async (next) => {
    if (get().permission === null || get().loading || get().saving) return false;
    if (next === get().permission) return true;
    set({ saving: true, error: null });
    try {
      set({ permission: await setAgentExecutionPermission(next), error: null });
      return true;
    } catch (reason) {
      set({ error: reason instanceof Error ? reason.message : String(reason) });
      return false;
    } finally {
      set({ saving: false });
    }
  },
}));

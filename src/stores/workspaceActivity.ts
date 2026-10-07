import { create } from 'zustand';
import { getWorkspaceActivity, type WorkspaceActivityProjection } from '$lib/commands/workspace-activity';

interface WorkspaceActivityState {
  projection: WorkspaceActivityProjection | null;
  workspaceId: string | null;
  isLoading: boolean;
  error: string | null;
  refresh: (workspaceId: string) => Promise<void>;
  clear: () => void;
}

function messageFrom(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export const useWorkspaceActivityStore = create<WorkspaceActivityState>((set, get) => ({
  projection: null,
  workspaceId: null,
  isLoading: false,
  error: null,

  refresh: async (workspaceId) => {
    if (!workspaceId) return;
    set({ workspaceId, isLoading: true, error: null });
    try {
      const projection = await getWorkspaceActivity(workspaceId);
      if (get().workspaceId === workspaceId) {
        set({ projection, isLoading: false, error: null });
      }
    } catch (error) {
      if (get().workspaceId === workspaceId) {
        set({ isLoading: false, error: messageFrom(error) });
      }
    }
  },

  clear: () => set({ projection: null, workspaceId: null, isLoading: false, error: null }),
}));

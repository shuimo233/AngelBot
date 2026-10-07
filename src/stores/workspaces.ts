import { create } from 'zustand';
import { createProjectWorkspace, getWorkspaces, openWorkspace, type Workspace } from '$lib/commands/workspace';
import { useSessionsStore } from './sessions';

/**
 * Resolve an unambiguous project mention before a Personal-space turn starts.
 *
 * This deliberately uses only the user-maintained project names already shown
 * in the sidebar. It never asks the model to infer a filesystem target, and it
 * leaves generic Personal-space conversation where it is.
 */
export function projectWorkspaceForMessage(
  content: string,
  workspaces: Workspace[],
  activeWorkspaceId: string | null,
): Workspace | null {
  if (workspaces.find((workspace) => workspace.id === activeWorkspaceId)?.kind !== 'personal') {
    return null;
  }
  const normalizedContent = content.trim().toLocaleLowerCase();
  if (!normalizedContent) return null;
  return workspaces
    .filter((workspace) => workspace.kind === 'project' && workspace.name.trim().length >= 2)
    .filter((workspace) => normalizedContent.includes(workspace.name.trim().toLocaleLowerCase()))
    .sort((left, right) => right.name.length - left.name.length)[0] ?? null;
}

interface WorkspaceState {
  workspaces: Workspace[];
  activeWorkspaceId: string | null;
  loading: boolean;
  lastError: string | null;
  loadWorkspaces: () => Promise<void>;
  openWorkspace: (id: string) => Promise<void>;
  createProject: (path: string, name?: string) => Promise<void>;
}

export const useWorkspacesStore = create<WorkspaceState>((set, get) => ({
  workspaces: [], activeWorkspaceId: null, loading: false, lastError: null,
  loadWorkspaces: async () => {
    set({ loading: true, lastError: null });
    try {
      const workspaces = await getWorkspaces();
      const current = get().activeWorkspaceId;
      const activeWorkspaceId = workspaces.some((item) => item.id === current)
        ? current : workspaces[0]?.id ?? null;
      set({ workspaces, activeWorkspaceId, loading: false });
      if (activeWorkspaceId) await get().openWorkspace(activeWorkspaceId);
    } catch (error) {
      set({ loading: false, lastError: error instanceof Error ? error.message : String(error) });
    }
  },
  openWorkspace: async (id) => {
    const opened = await openWorkspace(id);
    useSessionsStore.setState({
      sessions: [opened.session], activeSessionId: opened.session.id, activeSession: opened.session,
    });
    set((state) => ({
      activeWorkspaceId: id,
      workspaces: state.workspaces.map((item) => item.id === id ? opened.workspace : item),
    }));
  },
  createProject: async (path, name) => {
    const opened = await createProjectWorkspace(path, name);
    useSessionsStore.setState({
      sessions: [opened.session], activeSessionId: opened.session.id, activeSession: opened.session,
    });
    set((state) => ({
      activeWorkspaceId: opened.workspace.id,
      workspaces: [opened.workspace, ...state.workspaces.filter((item) => item.id !== opened.workspace.id)],
    }));
  },
}));

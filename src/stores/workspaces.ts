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
  /** False means the operation completed but a newer selection owns the UI. */
  openWorkspace: (id: string) => Promise<boolean>;
  createProject: (path: string, name?: string) => Promise<boolean>;
}

// UI selection ownership only; it neither cancels IPC nor retries creation.
let activationRequest = 0;
let catalogueRequest = 0;
let activationInFlight = false;

export const useWorkspacesStore = create<WorkspaceState>((set, get) => ({
  workspaces: [], activeWorkspaceId: null, loading: false, lastError: null,
  loadWorkspaces: async () => {
    const request = ++catalogueRequest;
    let activation = activationRequest;
    const wasActivating = activationInFlight;
    set({ loading: true, lastError: null });
    try {
      const workspaces = await getWorkspaces();
      if (request !== catalogueRequest) return;
      if (activation !== activationRequest || wasActivating || activationInFlight) {
        set({ loading: false });
        return;
      }
      const current = get().activeWorkspaceId;
      const activeWorkspaceId = workspaces.some((item) => item.id === current)
        ? current : workspaces[0]?.id ?? null;
      set({ workspaces, loading: false });
      if (activeWorkspaceId) {
        const opening = get().openWorkspace(activeWorkspaceId);
        activation = activationRequest;
        await opening;
      } else {
        useSessionsStore.setState({ sessions: [], activeSessionId: null, activeSession: null });
        set({ activeWorkspaceId: null });
      }
    } catch (error) {
      if (request === catalogueRequest) set({ loading: false,
        ...(activation === activationRequest ? { lastError: error instanceof Error ? error.message : String(error) } : {}),
      });
    }
  },
  openWorkspace: async (id) => {
    const request = ++activationRequest;
    activationInFlight = true;
    try {
      const opened = await openWorkspace(id);
      if (request !== activationRequest) return false;
      useSessionsStore.setState({
        sessions: [opened.session], activeSessionId: opened.session.id, activeSession: opened.session,
      });
      set((state) => ({
        activeWorkspaceId: id,
        workspaces: state.workspaces.some((item) => item.id === id)
          ? state.workspaces.map((item) => item.id === id ? opened.workspace : item)
          : [opened.workspace, ...state.workspaces],
      }));
      return request === activationRequest;
    } finally {
      if (request === activationRequest) activationInFlight = false;
    }
  },
  createProject: async (path, name) => {
    const request = ++activationRequest;
    activationInFlight = true;
    try {
      const opened = await createProjectWorkspace(path, name);
      if (request === activationRequest) useSessionsStore.setState({
        sessions: [opened.session], activeSessionId: opened.session.id, activeSession: opened.session,
      });
      // A created project is real even when a newer selection superseded its UI.
      set((state) => ({
        ...(request === activationRequest ? { activeWorkspaceId: opened.workspace.id } : {}),
        workspaces: request !== activationRequest && state.workspaces.some((item) => item.id === opened.workspace.id)
          ? state.workspaces
          : [opened.workspace, ...state.workspaces.filter((item) => item.id !== opened.workspace.id)],
      }));
      return request === activationRequest;
    } finally {
      if (request === activationRequest) activationInFlight = false;
    }
  },
}));

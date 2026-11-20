import { create } from 'zustand';
import { createProjectWorkspace, getWorkspaces, openWorkspace, type OpenWorkspace, type Workspace } from '$lib/commands/workspace';
import { useMessagesStore } from './messages';
import { useSessionsStore } from './sessions';

/**
 * Cheap admission only: names identify possible contexts, never user intent.
 *
 * This deliberately uses only the user-maintained project names already shown
 * in the sidebar. It never asks the model to infer a filesystem target, and it
 * leaves actual routing to a bounded semantic decision or explicit UI choice.
 */
export function projectCandidatesForMessage(
  content: string,
  workspaces: Workspace[],
  activeWorkspaceId: string | null,
): Workspace[] {
  if (workspaces.find((workspace) => workspace.id === activeWorkspaceId)?.kind !== 'personal') {
    return [];
  }
  const normalizedContent = content.trim().toLocaleLowerCase();
  if (!normalizedContent) return [];
  return workspaces
    .filter((workspace) => workspace.kind === 'project' && workspace.name.trim().length >= 2)
    .filter((workspace) => normalizedContent.includes(workspace.name.trim().toLocaleLowerCase()));
}

interface WorkspaceState {
  workspaces: Workspace[];
  activeWorkspaceId: string | null;
  recentProjectIds: string[];
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
const RECENT_PROJECTS_KEY = 'recent-project-ids';
const RECENT_PROJECTS_LIMIT = 16;

function loadRecentProjects(): string[] {
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(RECENT_PROJECTS_KEY) ?? '[]');
    if (Array.isArray(saved)) return [...new Set(saved.filter((id): id is string => typeof id === 'string'))].slice(0, RECENT_PROJECTS_LIMIT);
  } catch { /* The catalogue still provides a default order. */ }
  return [];
}

function recordRecentProject(ids: string[], workspace: Workspace): string[] {
  if (workspace.kind !== 'project' || ids[0] === workspace.id) return ids;
  const recent = [workspace.id, ...ids.filter((id) => id !== workspace.id)].slice(0, RECENT_PROJECTS_LIMIT);
  try { localStorage.setItem(RECENT_PROJECTS_KEY, JSON.stringify(recent)); }
  catch { /* Keep the recent order in this window if storage is unavailable. */ }
  return recent;
}

function activateOpenedSession({ session, composerDraft }: OpenWorkspace) {
  if (composerDraft) useMessagesStore.getState().registerDraft(session.id, composerDraft);
  useSessionsStore.setState({ sessions: [session], activeSessionId: session.id, activeSession: session });
}

export const useWorkspacesStore = create<WorkspaceState>((set, get) => ({
  workspaces: [], activeWorkspaceId: null, recentProjectIds: loadRecentProjects(), loading: false, lastError: null,
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
      activateOpenedSession(opened);
      set((state) => ({
        activeWorkspaceId: id,
        recentProjectIds: recordRecentProject(state.recentProjectIds, opened.workspace),
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
      if (request === activationRequest) activateOpenedSession(opened);
      // A created project is real even when a newer selection superseded its UI.
      set((state) => ({
        ...(request === activationRequest ? {
          activeWorkspaceId: opened.workspace.id,
          recentProjectIds: recordRecentProject(state.recentProjectIds, opened.workspace),
        } : {}),
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

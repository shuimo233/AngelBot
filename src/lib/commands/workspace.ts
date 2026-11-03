import { invoke } from '../invoke';
import type { Session } from '$types';

export type WorkspaceKind = 'personal' | 'project';

export interface Workspace {
  id: string;
  name: string;
  kind: WorkspaceKind;
  rootPath?: string;
  createdAt: number;
  updatedAt: number;
  activeSessionId: string;
  directoryConfirmed?: boolean;
}

export interface OpenWorkspace {
  workspace: Workspace;
  session: Session;
}

export function getWorkspaces(): Promise<Workspace[]> {
  return invoke<Workspace[]>('get_workspaces');
}

export function openWorkspace(id: string): Promise<OpenWorkspace> {
  return invoke<OpenWorkspace>('open_workspace', { id });
}

export type WorkspaceIntent = { kind: 'daily' } | { kind: 'project'; projectId: string } | { kind: 'clarify' };
/** Read-only, conditional use of the currently configured model, not a send. */
export async function resolveWorkspaceIntent(sessionId: string, content: string): Promise<WorkspaceIntent> {
  const result = await invoke<WorkspaceIntent>('resolve_workspace_intent', { sessionId, content });
  if (!result || !['daily', 'project', 'clarify'].includes(result.kind)
    || (result.kind === 'project' && (typeof result.projectId !== 'string' || !result.projectId))) {
    throw new Error('未能确认项目归属，尚未发送任务。');
  }
  return result;
}

export interface DirectoryConfirmation { id: string; expectedPath: string; path: string }

export function confirmRestoredProjectDirectories(directories: DirectoryConfirmation[]): Promise<void> {
  return invoke<void>('confirm_restored_project_directories', { directories });
}

export function createProjectWorkspace(path: string, name?: string): Promise<OpenWorkspace> {
  return invoke<OpenWorkspace>('create_project_workspace', {
    path,
    name: name?.trim() || null,
    agent_provider: null,
    agent_model: null,
  });
}

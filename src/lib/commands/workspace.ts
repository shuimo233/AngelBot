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

export function createProjectWorkspace(path: string, name?: string): Promise<OpenWorkspace> {
  return invoke<OpenWorkspace>('create_project_workspace', {
    path,
    name: name?.trim() || null,
    agent_provider: null,
    agent_model: null,
  });
}

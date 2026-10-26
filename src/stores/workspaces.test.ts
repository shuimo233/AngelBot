import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  createProjectWorkspace,
  getWorkspaces,
  openWorkspace,
  type OpenWorkspace,
  type Workspace,
} from '$lib/commands/workspace';
import { useSessionsStore } from './sessions';
import { projectWorkspaceForMessage, useWorkspacesStore } from './workspaces';

vi.mock('$lib/commands/workspace', () => ({
  createProjectWorkspace: vi.fn(),
  getWorkspaces: vi.fn(),
  openWorkspace: vi.fn(),
}));

const project: Workspace = {
  id: 'project-a',
  name: 'Project A',
  kind: 'project',
  rootPath: 'D:\\Projects\\project-a',
  createdAt: 1,
  updatedAt: 2,
  activeSessionId: 'project-a-main',
};

const personal: Workspace = {
  id: 'personal',
  name: 'AngelBot 日常',
  kind: 'personal',
  createdAt: 1,
  updatedAt: 3,
  activeSessionId: 'personal-main',
};

const opened = (workspace: Workspace): OpenWorkspace => ({
  workspace,
  session: {
    id: workspace.activeSessionId,
    title: workspace.name,
    createdAt: 1,
    updatedAt: workspace.updatedAt,
    contextVersion: 0,
    workDir: workspace.rootPath,
  },
});

const createProjectWorkspaceMock = vi.mocked(createProjectWorkspace);
const getWorkspacesMock = vi.mocked(getWorkspaces);
const openWorkspaceMock = vi.mocked(openWorkspace);

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

describe('workspace store', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useWorkspacesStore.setState({
      workspaces: [],
      activeWorkspaceId: null,
      loading: false,
      lastError: null,
    });
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: null,
      activeSession: null,
      isLoading: false,
      lastError: null,
    });
  });

  it('projects exactly one main session when switching workspaces', async () => {
    useSessionsStore.setState({
      sessions: [{ id: 'stale-branch', title: 'Internal branch', createdAt: 0, updatedAt: 0, contextVersion: 0 }],
      activeSessionId: 'stale-branch',
      activeSession: { id: 'stale-branch', title: 'Internal branch', createdAt: 0, updatedAt: 0, contextVersion: 0 },
    });
    useWorkspacesStore.setState({ workspaces: [personal, project] });
    openWorkspaceMock.mockResolvedValue(opened(project));

    await useWorkspacesStore.getState().openWorkspace(project.id);

    expect(openWorkspaceMock).toHaveBeenCalledWith(project.id);
    expect(useSessionsStore.getState()).toMatchObject({
      sessions: [opened(project).session],
      activeSessionId: project.activeSessionId,
      activeSession: opened(project).session,
    });
    expect(useWorkspacesStore.getState().activeWorkspaceId).toBe(project.id);
  });

  it('loads the retained workspace and opens only its main session', async () => {
    useWorkspacesStore.setState({ activeWorkspaceId: project.id });
    getWorkspacesMock.mockResolvedValue([personal, project]);
    openWorkspaceMock.mockResolvedValue(opened(project));

    await useWorkspacesStore.getState().loadWorkspaces();

    expect(openWorkspaceMock).toHaveBeenCalledTimes(1);
    expect(openWorkspaceMock).toHaveBeenCalledWith(project.id);
    expect(useSessionsStore.getState().sessions).toEqual([opened(project).session]);
  });

  it('creates a project and replaces the personal main-session projection', async () => {
    const created = { ...project, id: 'project-new', activeSessionId: 'project-new-main' };
    useWorkspacesStore.setState({ workspaces: [personal], activeWorkspaceId: personal.id });
    useSessionsStore.setState({ sessions: [opened(personal).session], activeSessionId: personal.activeSessionId, activeSession: opened(personal).session });
    createProjectWorkspaceMock.mockResolvedValue(opened(created));

    await useWorkspacesStore.getState().createProject('D:\\Projects\\project-new', 'Project New');

    expect(createProjectWorkspaceMock).toHaveBeenCalledWith('D:\\Projects\\project-new', 'Project New');
    expect(useWorkspacesStore.getState().activeWorkspaceId).toBe(created.id);
    expect(useSessionsStore.getState().sessions).toEqual([opened(created).session]);
    expect(useSessionsStore.getState().activeSessionId).toBe(created.activeSessionId);
  });

  it.each([true, false])('keeps the newer workspace selection when requests finish out of order (older first: %s)', async (olderFirst) => {
    const older = deferred<OpenWorkspace>();
    const newer = deferred<OpenWorkspace>();
    useWorkspacesStore.setState({ workspaces: [personal, project] });
    openWorkspaceMock.mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);
    const openingProject = useWorkspacesStore.getState().openWorkspace(project.id);
    const openingPersonal = useWorkspacesStore.getState().openWorkspace(personal.id);
    if (olderFirst) {
      older.resolve(opened(project));
      expect(await openingProject).toBe(false);
      expect(useSessionsStore.getState().activeSessionId).not.toBe(project.activeSessionId);
    }
    newer.resolve(opened(personal));
    expect(await openingPersonal).toBe(true);
    if (!olderFirst) { older.resolve(opened(project)); expect(await openingProject).toBe(false); }
    expect(useWorkspacesStore.getState().activeWorkspaceId).toBe(personal.id);
    expect(useSessionsStore.getState().activeSessionId).toBe(personal.activeSessionId);
    expect(openWorkspaceMock).toHaveBeenCalledTimes(2);
  });

  it('does not let an older catalogue refresh reselect a workspace after the user switched', async () => {
    const catalog = deferred<Workspace[]>();
    useWorkspacesStore.setState({ workspaces: [personal, project] });
    getWorkspacesMock.mockReturnValueOnce(catalog.promise);
    const loading = useWorkspacesStore.getState().loadWorkspaces();
    openWorkspaceMock.mockResolvedValueOnce(opened(project));
    await useWorkspacesStore.getState().openWorkspace(project.id);
    catalog.resolve([personal]);
    await loading;
    expect(useWorkspacesStore.getState().activeWorkspaceId).toBe(project.id);
    expect(useWorkspacesStore.getState().workspaces).toContainEqual(project);
    expect(useSessionsStore.getState().activeSessionId).toBe(project.activeSessionId);
    expect(openWorkspaceMock).toHaveBeenCalledTimes(1);
  });

  it.each([false, true])('retains a created project without hijacking a newer selection or metadata (known: %s)', async (known) => {
    const creating = deferred<OpenWorkspace>();
    const knownProject = { ...project, name: 'Project A renamed' };
    useWorkspacesStore.setState({ workspaces: [personal, ...(known ? [knownProject] : [])] });
    createProjectWorkspaceMock.mockReturnValueOnce(creating.promise);
    const created = useWorkspacesStore.getState().createProject(project.rootPath!);
    openWorkspaceMock.mockResolvedValueOnce(opened(personal));
    await useWorkspacesStore.getState().openWorkspace(personal.id);
    creating.resolve(opened(project));
    expect(await created).toBe(false);
    expect(useWorkspacesStore.getState().workspaces).toContainEqual(known ? knownProject : project);
    expect(useWorkspacesStore.getState().activeWorkspaceId).toBe(personal.id);
    expect(useSessionsStore.getState().activeSessionId).toBe(personal.activeSessionId);
    expect(createProjectWorkspaceMock).toHaveBeenCalledTimes(1);
  });

  it('routes only an explicit Personal-space project mention to one known project', () => {
    const nested = { ...project, id: 'project-angelbot', name: 'AngelBot' };

    expect(projectWorkspaceForMessage('继续推进 AngelBot 的工作', [personal, project, nested], personal.id))
      .toEqual(nested);
    expect(projectWorkspaceForMessage('继续推进日常事项', [personal, project, nested], personal.id))
      .toBeNull();
    expect(projectWorkspaceForMessage('继续推进 AngelBot', [personal, project, nested], nested.id))
      .toBeNull();
  });
});

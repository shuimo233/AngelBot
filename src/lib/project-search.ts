import type { Workspace } from './commands/workspace';

export function normalizeProjectSearch(value: string): string {
  return value.trim().toLocaleLowerCase().replace(/\\/g, '/');
}

export function projectsByRecentUse(workspaces: Workspace[], recentProjectIds: readonly string[] = []) {
  const projects = new Map(workspaces.filter(({ kind }) => kind === 'project').map((project) => [project.id, project]));
  const recent: Workspace[] = [];
  for (const id of recentProjectIds) {
    const project = projects.get(id);
    if (project) {
      recent.push(project);
      projects.delete(id);
    }
  }
  return [...recent, ...projects.values()];
}

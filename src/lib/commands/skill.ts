import { invoke } from '$lib/invoke';

export interface SkillManifest {
  id: string;
  name: string;
  description: string;
  version: string;
  author?: string | null;
  actions: unknown[];
  dependencies: string[];
  permissions: string[];
}

export function getSkills(): Promise<SkillManifest[]> {
  return invoke<SkillManifest[]>('get_skills');
}

export function loadSkills(directory: string): Promise<number> {
  return invoke<number>('load_skills', { directory });
}

export function importGithubSkills(repositoryUrl: string): Promise<number> {
  return invoke<number>('import_github_skills', { repositoryUrl });
}

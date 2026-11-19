import { invoke } from '$lib/invoke';

export interface ComposerDraft {
  text: string;
  revision: number;
}

export function saveComposerDraft(sessionId: string, revision: number, text: string): Promise<boolean> {
  return invoke<boolean>('save_composer_draft', { sessionId, revision, text });
}

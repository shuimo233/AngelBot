import { invoke } from '../invoke';

export interface UsageStat { prompt_tokens: number; completion_tokens: number; }

export function getSessionUsage(sessionId: string): Promise<UsageStat[]> {
  return invoke<UsageStat[]>('get_session_usage', { session_id: sessionId });
}

import { invoke } from '../invoke';

export interface ProviderUsage {
  inputTokens: number;
  outputTokens: number;
  cachedInputTokens: number | null;
  cacheWriteTokens: number | null;
  uncachedInputTokens: number | null;
  cacheWrite30mTokens: number | null;
  cacheWrite12hTokens: number | null;
  reasoningTokens: number | null;
  detailsInvalid: boolean;
}

export interface UsageStat {
  prompt_tokens: number;
  completion_tokens: number;
  provider_usage?: ProviderUsage | null;
  cost_basis?: 'legacy_static_estimate_not_bill';
}

export function getSessionUsage(sessionId: string): Promise<UsageStat[]> {
  return invoke<UsageStat[]>('get_session_usage', { session_id: sessionId });
}

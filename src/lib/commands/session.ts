/**
 * Session commands - typed wrappers around Tauri IPC
 */
import { invoke } from '../invoke';
import type { Session } from '$types';

export interface HistorySession {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  messageCount: number;
}

export interface ContextUsage {
  activeMessageCount: number;
  totalMessageCount: number;
  estimatedTokens: number;
  contextLimit: number;
  isCompressed: boolean;
  /** Provider usage anchor plus any locally estimated newer content. */
  measurementSource: 'provider_reported' | 'hybrid_estimate' | 'local_estimate';
  reportedPromptTokens?: number;
  estimatedTrailingTokens?: number;
  measuredAt?: number;
}

export async function getSessions(): Promise<Session[]> {
  return invoke<Session[]>('get_sessions');
}

export async function getHistorySessions(limit = 20): Promise<HistorySession[]> {
  return invoke<HistorySession[]>('get_history_sessions', { limit });
}

export async function createSession(
  title = '',
  workDir?: string,
  agentProvider?: string,
  agentModel?: string,
): Promise<Session> {
  return invoke<Session>('create_session', {
    title,
    work_dir: workDir ?? null,
    agent_provider: agentProvider ?? null,
    agent_model: agentModel ?? null,
  });
}

export async function deleteSession(id: string): Promise<void> {
  return invoke<void>('delete_session', { id });
}

export async function updateSessionTitle(id: string, title: string): Promise<void> {
  return invoke<void>('update_session_title', { id, title });
}

export async function updateSessionModel(sessionId: string, agentProvider: string, agentModel: string): Promise<void> {
  return invoke<void>('update_session_model', {
    session_id: sessionId,
    agent_provider: agentProvider,
    agent_model: agentModel,
  });
}

export async function getContextUsage(sessionId: string): Promise<ContextUsage> {
  return invoke<ContextUsage>('get_context_usage', { session_id: sessionId });
}

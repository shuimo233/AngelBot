/**
 * Search commands — FTS5 full-text search via Tauri IPC
 */
import { invoke } from '../invoke';

export interface MessageSearchResult {
  id: string;
  sessionId: string;
  role: string;
  snippet: string;
  content: string;
  createdAt: number;
  rank: number;
}

export interface MemoryKeywordResult {
  id: string;
  content: string;
  category: string;
  snippet: string;
  rank: number;
}

export async function searchMessages(
  query: string,
  sessionId?: string,
  limit?: number,
): Promise<MessageSearchResult[]> {
  return invoke<MessageSearchResult[]>('search_messages', {
    query,
    sessionId: sessionId ?? null,
    limit: limit ?? 50,
  });
}

export async function searchMemoriesByKeyword(
  query: string,
  limit?: number,
): Promise<MemoryKeywordResult[]> {
  return invoke<MemoryKeywordResult[]>('search_memories_by_keyword', {
    query,
    limit: limit ?? 50,
  });
}

export async function rebuildFtsIndex(): Promise<void> {
  return invoke<void>('rebuild_fts_index');
}

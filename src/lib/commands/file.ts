/**
 * File commands - typed wrappers around Tauri IPC (sandboxed)
 */
import { invoke } from '../invoke';

export interface FileReadResult {
  success: boolean;
  content?: string;
  error?: string;
}

export interface WorkDirEntry {
  path: string;
  name: string;
  isDirectory: boolean;
  size: number | null;
}

/** Read-only workbench access. File mutation remains a Main-Agent tool action. */
export async function readFile(path: string): Promise<FileReadResult> {
  try {
    return { success: true, content: await invoke<string>('read_file', { path }) };
  } catch (error) {
    return { success: false, error: error instanceof Error ? error.message : String(error) };
  }
}

export async function readSessionFile(sessionId: string, path: string): Promise<FileReadResult> {
  try {
    return { success: true, content: await invoke<string>('read_session_file', { sessionId, path }) };
  } catch (error) {
    return { success: false, error: error instanceof Error ? error.message : String(error) };
  }
}

export function listWorkDir(sessionId: string, path = ''): Promise<WorkDirEntry[]> {
  return invoke<WorkDirEntry[]>('list_work_dir', { sessionId, path });
}

/** Session-owned context roots. These are configuration, not a user-facing
 * file browser: the Main Agent is the only component that consumes them. */
export interface SessionFileAccess {
  workDir: string;
  additionalReadDirs: string[];
}

export async function getWorkDir(sessionId?: string): Promise<string> {
  return invoke<string>('get_work_dir', { sessionId: sessionId ?? null });
}

export async function setWorkDir(sessionId: string, workDir: string): Promise<void> {
  return invoke<void>('set_work_dir', { sessionId, workDir });
}

export function getSessionFileAccess(sessionId: string): Promise<SessionFileAccess> {
  return invoke<SessionFileAccess>('get_session_file_access', { sessionId });
}

export function setSessionReadDirs(sessionId: string, directories: string[]): Promise<void> {
  return invoke<void>('set_session_read_dirs', { sessionId, directories });
}

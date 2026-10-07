import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '../invoke';

vi.mock('../invoke', () => ({
  invoke: vi.fn(),
}));

import {
  getSessionFileAccess,
  getWorkDir,
  setSessionReadDirs,
  setWorkDir,
} from './file';

describe('file command IPC contract', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(invoke).mockResolvedValue(undefined);
  });

  it('uses Tauri camelCase argument names for session-scoped file commands', async () => {
    await getWorkDir('session-1');
    await setWorkDir('session-1', 'D:/Projects/test');
    await getSessionFileAccess('session-1');
    await setSessionReadDirs('session-1', ['D:/Projects/read-only']);

    expect(invoke).toHaveBeenNthCalledWith(1, 'get_work_dir', { sessionId: 'session-1' });
    expect(invoke).toHaveBeenNthCalledWith(2, 'set_work_dir', {
      sessionId: 'session-1',
      workDir: 'D:/Projects/test',
    });
    expect(invoke).toHaveBeenNthCalledWith(3, 'get_session_file_access', { sessionId: 'session-1' });
    expect(invoke).toHaveBeenNthCalledWith(4, 'set_session_read_dirs', {
      sessionId: 'session-1',
      directories: ['D:/Projects/read-only'],
    });
  });
});

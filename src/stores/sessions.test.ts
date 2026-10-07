import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createSession, getSessions } from '$lib/commands';
import { useSettingsStore } from './settings';
import { useSessionsStore } from './sessions';

vi.mock('$lib/commands', () => ({
  getSessions: vi.fn(),
  createSession: vi.fn(),
}));

const getSessionsMock = vi.mocked(getSessions);

describe('sessions store', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useSessionsStore.setState({
      sessions: [],
      activeSessionId: null,
      activeSession: null,
      isLoading: false,
    });
  });

  it('can set and get state', () => {
    useSessionsStore.setState({ activeSessionId: 'x' });
    expect(useSessionsStore.getState().activeSessionId).toBe('x');
  });

  it('binds new conversations to the saved model rather than an unsaved plan draft', async () => {
    useSettingsStore.setState((state) => ({
      activeApiConfig: { ...state.activeApiConfig, provider: 'openai', model: 'saved-api-model', authMode: 'api_key' },
      apiConfig: { ...state.apiConfig, provider: 'openai', model: 'unsaved-plan-model', authMode: 'chatgpt_plan' },
    }));
    vi.mocked(createSession).mockResolvedValue({ id: 'session-fixture', title: 'New conversation', createdAt: 1, updatedAt: 1, contextVersion: 0 });
    await useSessionsStore.getState().createSession('New conversation');
    expect(createSession).toHaveBeenCalledWith('New conversation', undefined, 'openai', 'saved-api-model');
  });

  it('recovers when the development backend is unavailable during initial load', async () => {
    const session = {
      id: 'session-1',
      title: 'Recovered session',
      createdAt: 1,
      updatedAt: 2,
      contextVersion: 0,
    };
    getSessionsMock
      .mockRejectedValueOnce(new Error('Dev HTTP is still starting'))
      .mockResolvedValue([session]);

    await useSessionsStore.getState().loadSessions();

    expect(getSessionsMock).toHaveBeenCalledTimes(2);
    expect(useSessionsStore.getState()).toMatchObject({
      sessions: [session],
      activeSessionId: session.id,
      activeSession: session,
      isLoading: false,
    });
  });
});

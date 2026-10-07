import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '$lib/invoke';
import {
  clearWebSearchConfig,
  configureWebSearch,
  getWebSearchConfig,
  setWebSearchEnabled,
} from './web-search';

vi.mock('$lib/invoke', () => ({ invoke: vi.fn() }));

const config = {
  provider: 'tavily' as const,
  endpoint: 'https://api.tavily.com/search',
  apiKeyConfigured: true,
  enabled: true,
};

describe('Web Search command boundary', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it('uses the registered read command without an API key payload', async () => {
    vi.mocked(invoke).mockResolvedValue(config);

    await expect(getWebSearchConfig()).resolves.toEqual(config);
    expect(invoke).toHaveBeenCalledWith('get_web_search_config');
  });

  it('nests configure and enabled changes in the backend request contract', async () => {
    vi.mocked(invoke).mockResolvedValue(config);

    await configureWebSearch({ provider: 'searxng', apiKey: '', endpoint: 'https://search.example.test' });
    await setWebSearchEnabled(false);

    expect(invoke).toHaveBeenNthCalledWith(1, 'configure_web_search', {
      request: { provider: 'searxng', apiKey: '', endpoint: 'https://search.example.test' },
    });
    expect(invoke).toHaveBeenNthCalledWith(2, 'set_web_search_enabled', {
      request: { enabled: false },
    });
  });

  it('uses the dedicated keychain-and-config cleanup command', async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);

    await clearWebSearchConfig();

    expect(invoke).toHaveBeenCalledWith('clear_web_search_config');
  });
});

import { describe, expect, it } from 'vitest';
import { extractSecureWebSearchSetup } from './web-search-setup';

describe('secure Web Search setup parsing', () => {
  it('moves a credential out of the visible message', () => {
    const result = extractSecureWebSearchSetup('配置联网搜索：tavily tvly-secret-value');
    expect(result.displayContent).not.toContain('tvly-secret-value');
    expect(result.setup).toEqual({ provider: 'tavily', apiKey: 'tvly-secret-value' });
  });

  it('leaves ordinary conversations untouched', () => {
    const text = '请搜索 AngelBot 的最新资料';
    expect(extractSecureWebSearchSetup(text)).toEqual({ displayContent: text, setup: null });
  });

  it('keeps a self-hosted SearXNG address out of the transcript too', () => {
    const result = extractSecureWebSearchSetup('配置联网搜索 searxng http://127.0.0.1:8080');
    expect(result.displayContent).not.toContain('127.0.0.1');
    expect(result.setup).toEqual({
      provider: 'searxng',
      apiKey: '',
      endpoint: 'http://127.0.0.1:8080',
    });
  });
});

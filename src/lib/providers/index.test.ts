import { describe, expect, it } from 'vitest';
import { getProviderMeta, providerProtocolLabel } from './index';

describe('provider registry', () => {
  it('describes the actual protocol and endpoint contract used at runtime', () => {
    expect(providerProtocolLabel('anthropic')).toBe('Anthropic Messages');
    expect(providerProtocolLabel('google')).toBe('OpenAI 兼容');
    expect(providerProtocolLabel('openai', 'openai_responses')).toBe('OpenAI Responses');
    expect(providerProtocolLabel('custom', 'anthropic_messages')).toBe('Anthropic Messages');
    expect(getProviderMeta('google').defaultBaseUrl)
      .toBe('https://generativelanguage.googleapis.com/v1beta/openai');
    expect(getProviderMeta('ollama')).toMatchObject({
      isLocal: true,
      endpointMode: 'required',
      protocol: 'openai-chat-completions',
    });
  });
});

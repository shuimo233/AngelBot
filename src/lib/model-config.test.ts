import { describe, expect, it } from 'vitest';
import { formatModelSendError, getModelConfigReadiness } from './model-config';

const base = {
  provider: 'anthropic' as const,
  model: 'claude-sonnet-4-20250514',
  baseUrl: 'https://api.anthropic.com',
  apiKey: '',
  hasApiKey: false,
  credentialSource: 'none' as const,
  maxTokens: 4096,
  temperature: 0.7,
};

describe('model configuration readiness', () => {
  it('requires provider-matched credentials without blocking application loading', () => {
    expect(getModelConfigReadiness(base, true)).toMatchObject({ kind: 'needs-credentials' });
    expect(getModelConfigReadiness({ ...base, hasApiKey: true }, true)).toEqual({ kind: 'ready' });
  });

  it('requires an endpoint only for providers whose endpoint is user-managed', () => {
    expect(getModelConfigReadiness({
      ...base,
      provider: 'ollama',
      model: 'llama3',
      baseUrl: '',
    }, true)).toMatchObject({ kind: 'invalid' });

    expect(getModelConfigReadiness({
      ...base,
      provider: 'google',
      model: 'gemini-2.5-pro',
      baseUrl: '',
      hasApiKey: true,
    }, true)).toEqual({ kind: 'ready' });
  });

  it('turns backend credential errors into an actionable conversation message', () => {
    expect(formatModelSendError(new Error('LLM provider credentials are not configured')))
      .toContain('模型设置');
  });

  it('requires plan permission and an opaque account reference instead of an API key', () => {
    const plan = { ...base, provider: 'openai' as const, baseUrl: 'https://api.openai.com/v1', protocol: 'openai_responses' as const, authMode: 'chatgpt_plan' as const, planEnabled: true, credentialRef: 'opaque-ref' };
    expect(getModelConfigReadiness(plan, true)).toEqual({ kind: 'ready' });
    expect(getModelConfigReadiness({ ...plan, planEnabled: false }, true)).toMatchObject({ kind: 'needs-credentials' });
    expect(getModelConfigReadiness({ ...plan, credentialRef: null, hasApiKey: true }, true)).toMatchObject({ kind: 'needs-credentials' });
    expect(getModelConfigReadiness({ ...plan, baseUrl: 'https://gateway.example/v1' }, true)).toMatchObject({ kind: 'invalid' });
  });

  it('does not require credentials for explicitly unauthenticated custom endpoints', () => {
    expect(getModelConfigReadiness({ ...base, provider: 'custom', baseUrl: 'http://localhost:1234', authMode: 'none' }, true)).toEqual({ kind: 'ready' });
  });
});

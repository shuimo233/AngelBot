import { act, fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useSettingsStore } from '$stores/settings';
import { loadApiConfig, type ApiProviderConfig } from '$lib/commands/settings';
import { ModelConfigurationNotice } from './ModelConfigurationNotice';

vi.mock('$lib/commands/settings', () => ({ loadApiConfig: vi.fn(), saveApiConfig: vi.fn() }));

describe('ModelConfigurationNotice', () => {
  beforeEach(() => {
    vi.mocked(loadApiConfig).mockReset();
    useSettingsStore.setState({
      apiConfigLoaded: true,
      apiConfigLoadError: null,
      activeApiConfig: {
        provider: 'anthropic', model: 'claude-sonnet-4-20250514',
        baseUrl: 'https://api.anthropic.com', apiKey: '', hasApiKey: false,
        credentialSource: 'none', maxTokens: 4096, temperature: 0.7,
      },
    });
  });

  it('guides without blocking the workspace and opens model settings', () => {
    const listener = vi.fn();
    window.addEventListener('open-settings', listener);
    render(<ModelConfigurationNotice />);
    expect(screen.getByText('连接模型后即可开始对话')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '模型设置' }));
    expect(listener).toHaveBeenCalledTimes(1);
    window.removeEventListener('open-settings', listener);
  });

  it.each([
    {
      name: 'API credentials',
      config: { provider: 'anthropic', model: 'claude-sonnet-4-20250514', base_url: 'https://api.anthropic.com', api_key: '', has_api_key: true, credential_source: 'keychain', max_tokens: 4096, temperature: 0.7 },
    },
    {
      name: 'a saved Luna plan without an API key',
      config: { provider: 'openai', model: 'gpt-5.6-luna', base_url: 'https://api.openai.com/v1', api_key: '', has_api_key: false, credential_source: 'chatgpt_plan', protocol: 'openai_responses', auth_mode: 'chatgpt_plan', credential_ref: 'fixture-account', plan_connected: true, plan_enabled: true, max_tokens: 4096, temperature: 0.7 },
    },
  ] satisfies { name: string; config: ApiProviderConfig }[])(
    'hides the connection prompt after loading $name from the backend', async ({ config }) => {
      vi.mocked(loadApiConfig).mockResolvedValue(config);
      const unconfigured = useSettingsStore.getState().activeApiConfig;
      for (const loaded of [false, true]) {
        useSettingsStore.setState({ activeApiConfig: unconfigured, apiConfigLoaded: loaded });
        const { container, unmount } = render(<ModelConfigurationNotice />);
        if (loaded) expect(screen.getByText('连接模型后即可开始对话')).toBeInTheDocument();
        else expect(container).toBeEmptyDOMElement();
        await act(() => useSettingsStore.getState().loadApiConfig());
        expect(useSettingsStore.getState().activeApiConfig.model).toBe(config.model);
        expect(useSettingsStore.getState().activeApiConfig.credentialSource).toBe(config.credential_source);
        expect(container).toBeEmptyDOMElement();
        unmount();
      }
    },
  );

  it('does not declare chat ready because an unsaved plan draft is authorized', () => {
    useSettingsStore.getState().updateApiConfig({ provider: 'openai', model: 'draft-model', baseUrl: 'https://api.openai.com/v1', protocol: 'openai_responses', authMode: 'chatgpt_plan', credentialRef: 'draft-ref', planEnabled: true });
    render(<ModelConfigurationNotice />);
    expect(screen.getByText('连接模型后即可开始对话')).toBeInTheDocument();
  });
});

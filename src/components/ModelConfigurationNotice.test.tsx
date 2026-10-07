import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useSettingsStore } from '$stores/settings';
import { ModelConfigurationNotice } from './ModelConfigurationNotice';

describe('ModelConfigurationNotice', () => {
  beforeEach(() => {
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

  it('stays hidden when a provider-matched credential exists', () => {
    useSettingsStore.setState((state) => ({
      activeApiConfig: { ...state.activeApiConfig, hasApiKey: true, credentialSource: 'keychain' },
    }));
    const { container } = render(<ModelConfigurationNotice />);
    expect(container).toBeEmptyDOMElement();
  });

  it('does not declare chat ready because an unsaved plan draft is authorized', () => {
    useSettingsStore.getState().updateApiConfig({ provider: 'openai', model: 'draft-model', baseUrl: 'https://api.openai.com/v1', protocol: 'openai_responses', authMode: 'chatgpt_plan', credentialRef: 'draft-ref', planEnabled: true });
    render(<ModelConfigurationNotice />);
    expect(screen.getByText('连接模型后即可开始对话')).toBeInTheDocument();
  });
});

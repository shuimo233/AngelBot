import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  clearWebSearchConfig,
  configureWebSearch,
  getWebSearchConfig,
  setWebSearchEnabled,
  type WebSearchConfig,
} from '$lib/commands/web-search';
import { WebSearchSettings } from './WebSearchSettings';

vi.mock('$lib/commands/web-search', () => ({
  configureWebSearch: vi.fn(),
  clearWebSearchConfig: vi.fn(),
  getWebSearchConfig: vi.fn(),
  setWebSearchEnabled: vi.fn(),
}));

const tavilyConfig: WebSearchConfig = {
  provider: 'tavily',
  endpoint: 'https://api.tavily.com/search',
  apiKeyConfigured: true,
  enabled: false,
};

describe('WebSearchSettings', () => {
  beforeEach(() => {
    vi.mocked(getWebSearchConfig).mockReset();
    vi.mocked(configureWebSearch).mockReset();
    vi.mocked(clearWebSearchConfig).mockReset();
    vi.mocked(setWebSearchEnabled).mockReset();
    vi.mocked(getWebSearchConfig).mockResolvedValue(null);
  });

  it('loads a configured hosted provider without exposing its stored API key', async () => {
    vi.mocked(getWebSearchConfig).mockResolvedValue(tavilyConfig);

    render(<WebSearchSettings />);

    expect(await screen.findByDisplayValue('https://api.tavily.com/search')).toHaveAttribute('readonly');
    expect(screen.getByLabelText('API Key')).toHaveValue('');
    expect(screen.getByPlaceholderText('已安全保存；重新保存时请输入 API Key')).toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: '允许使用联网搜索' })).not.toBeChecked();
  });

  it('requires a fresh hosted-provider key and never sends a custom endpoint', async () => {
    const user = userEvent.setup();
    vi.mocked(configureWebSearch).mockResolvedValue({ ...tavilyConfig, enabled: true });
    render(<WebSearchSettings />);

    const saveButton = await screen.findByRole('button', { name: '保存搜索配置' });
    expect(saveButton).toBeDisabled();

    await user.type(screen.getByLabelText('API Key'), 'tvly-test-key');
    await user.click(saveButton);

    await waitFor(() => expect(configureWebSearch).toHaveBeenCalledWith({
      provider: 'tavily',
      apiKey: 'tvly-test-key',
    }));
  });

  it('uses a self-hosted SearXNG endpoint without requesting an API key', async () => {
    const user = userEvent.setup();
    vi.mocked(configureWebSearch).mockResolvedValue({
      provider: 'searxng',
      endpoint: 'https://search.example.test/',
      apiKeyConfigured: false,
      enabled: true,
    });
    render(<WebSearchSettings />);

    await screen.findByRole('button', { name: '保存搜索配置' });
    await user.selectOptions(screen.getByLabelText('搜索服务商'), 'searxng');

    expect(screen.queryByLabelText('API Key')).not.toBeInTheDocument();
    expect(screen.getByText(/不需要 API Key/)).toBeInTheDocument();

    await user.type(screen.getByLabelText('SearXNG 服务地址'), 'https://search.example.test');
    await user.click(screen.getByRole('button', { name: '保存搜索配置' }));

    await waitFor(() => expect(configureWebSearch).toHaveBeenCalledWith({
      provider: 'searxng',
      apiKey: '',
      endpoint: 'https://search.example.test',
    }));
  });

  it('changes enabled state through its dedicated command', async () => {
    const user = userEvent.setup();
    vi.mocked(getWebSearchConfig).mockResolvedValue(tavilyConfig);
    vi.mocked(setWebSearchEnabled).mockResolvedValue({ ...tavilyConfig, enabled: true });
    render(<WebSearchSettings />);

    const toggle = await screen.findByRole('checkbox', { name: '允许使用联网搜索' });
    await user.click(toggle);

    await waitFor(() => expect(setWebSearchEnabled).toHaveBeenCalledWith(true));
    expect(toggle).toBeChecked();
  });

  it('keeps the enabled toggle disabled until a configuration save completes', async () => {
    const user = userEvent.setup();
    let resolveSave: (config: WebSearchConfig) => void = () => {};
    vi.mocked(getWebSearchConfig).mockResolvedValue(tavilyConfig);
    vi.mocked(configureWebSearch).mockImplementation(
      () => new Promise<WebSearchConfig>((resolve) => { resolveSave = resolve; }),
    );
    render(<WebSearchSettings />);

    const toggle = await screen.findByRole('checkbox', { name: '允许使用联网搜索' });
    await user.type(screen.getByLabelText('API Key'), 'tvly-test-key');
    await user.click(screen.getByRole('button', { name: '保存搜索配置' }));

    await waitFor(() => expect(configureWebSearch).toHaveBeenCalledTimes(1));
    expect(toggle).toBeDisabled();
    await user.click(toggle);
    expect(setWebSearchEnabled).not.toHaveBeenCalled();

    resolveSave({ ...tavilyConfig, enabled: true });
    await waitFor(() => expect(toggle).not.toBeDisabled());
  });

  it('requires a second decision before removing the active configuration', async () => {
    const user = userEvent.setup();
    vi.mocked(getWebSearchConfig).mockResolvedValue({ ...tavilyConfig, enabled: true });
    vi.mocked(clearWebSearchConfig).mockResolvedValue();
    render(<WebSearchSettings />);

    await user.click(await screen.findByRole('button', { name: '移除配置' }));
    expect(clearWebSearchConfig).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: '确认移除配置' }));

    await waitFor(() => expect(clearWebSearchConfig).toHaveBeenCalledTimes(1));
    expect(screen.getByText('已移除当前联网搜索配置。')).toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: '允许使用联网搜索' })).toBeDisabled();
  });
});

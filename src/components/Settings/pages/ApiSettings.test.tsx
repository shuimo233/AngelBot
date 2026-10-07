import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { chatgptPlanStatus, chatgptPlanSignIn, chatgptPlanChangeAccount, chatgptPlanModels, chatgptPlanCancelSignIn, chatgptPlanDisconnect, testModelConnection, saveApiConfig } from '$lib/commands/settings';
import { useSettingsStore } from '$stores/settings';
import { ApiSettings } from './ApiSettings';

vi.mock('$lib/commands/settings', () => ({
  testModelConnection: vi.fn(), loadApiConfig: vi.fn(), saveApiConfig: vi.fn(),
  chatgptPlanStatus: vi.fn(), chatgptPlanSignIn: vi.fn(), chatgptPlanChangeAccount: vi.fn(), chatgptPlanModels: vi.fn(),
  chatgptPlanCancelSignIn: vi.fn(), chatgptPlanDisconnect: vi.fn(),
}));

describe('ApiSettings', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    vi.mocked(chatgptPlanChangeAccount).mockReset();
    vi.mocked(testModelConnection).mockResolvedValue(true);
    vi.mocked(chatgptPlanStatus).mockResolvedValue({ connected: false, planEnabled: false, needsSignIn: true, credentialRef: null, accountLabel: null });
    vi.mocked(chatgptPlanModels).mockResolvedValue([{ id: 'account-model', name: 'Account model' }]);
    useSettingsStore.setState({
      apiConfigLoaded: true,
      apiConfigLoadError: null,
      apiConfig: {
        provider: 'anthropic',
        model: 'claude-sonnet-4-20250514',
        baseUrl: 'https://api.anthropic.com',
        apiKey: '',
        hasApiKey: false,
        credentialSource: 'none',
        maxTokens: 4096,
        temperature: 0.7,
      },
      activeApiConfig: { provider: 'anthropic', model: 'claude-sonnet-4-20250514', baseUrl: 'https://api.anthropic.com', apiKey: '', hasApiKey: true, credentialSource: 'keychain', maxTokens: 4096, temperature: 0.7 },
    });
  });

  it('keeps the connection test unavailable until required credentials exist', async () => {
    const user = userEvent.setup();
    render(<ApiSettings />);

    expect(screen.getAllByText('需要 API Key')).not.toHaveLength(0);
    expect(screen.getByRole('button', { name: '测试连接' })).toBeDisabled();

    await user.selectOptions(screen.getByLabelText('服务商'), 'ollama');

    expect(screen.getByText('此服务商不需要 API Key。请先确保本地服务已经启动。')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '测试连接' })).toBeEnabled();
  });

  it('shows the runtime protocol and applies the provider default endpoint', async () => {
    const user = userEvent.setup();
    render(<ApiSettings />);

    expect(screen.getByText('Anthropic Messages')).toBeInTheDocument();
    expect(screen.getByText('外部模型服务')).toBeInTheDocument();

    await user.selectOptions(screen.getByLabelText('服务商'), 'google');

    expect(screen.getByText('OpenAI 兼容')).toBeInTheDocument();
    expect(screen.getByDisplayValue(
      'https://generativelanguage.googleapis.com/v1beta/openai',
    )).toBeInTheDocument();
  });

  it('explains environment-managed credentials instead of offering a misleading edit', () => {
    useSettingsStore.setState((state) => ({
      apiConfig: {
        ...state.apiConfig,
        hasApiKey: true,
        credentialSource: 'environment',
      },
    }));

    render(<ApiSettings />);

    expect(screen.getByLabelText('API Key')).toBeDisabled();
    expect(screen.getByText(/当前由环境变量 ANTHROPIC_API_KEY 管理/)).toBeInTheDocument();
  });

  it('tests the current unsaved form without exposing a stored key', async () => {
    const user = userEvent.setup();
    useSettingsStore.setState((state) => ({
      apiConfig: {
        ...state.apiConfig,
        hasApiKey: true,
        credentialSource: 'keychain',
      },
    }));

    render(<ApiSettings />);
    await user.click(screen.getByRole('button', { name: '测试连接' }));

    await waitFor(() => expect(testModelConnection).toHaveBeenCalledWith(expect.objectContaining({
      base_url: 'https://api.anthropic.com', api_key: '', provider: 'anthropic', model: 'claude-sonnet-4-20250514',
    })));
    expect(within(screen.getByRole('region', { name: '连接测试' })).getByText('连接测试通过')).toBeInTheDocument();
  });

  it('allows custom protocols and explicit no-auth without an API key', async () => {
    const user = userEvent.setup(); render(<ApiSettings />);
    await user.selectOptions(screen.getByLabelText('服务商'), 'custom');
    await user.selectOptions(screen.getByLabelText('协议'), 'anthropic_messages');
    await user.selectOptions(screen.getByLabelText('认证方式'), 'none');
    await user.type(screen.getByLabelText('模型标识'), 'local-model');
    await user.type(screen.getByLabelText('API 地址'), 'http://localhost:1234');
    expect(screen.queryByLabelText('API Key')).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '测试连接' }));
    expect(testModelConnection).toHaveBeenCalledWith(expect.objectContaining({ protocol: 'anthropic_messages', auth_mode: 'none', model: 'local-model' }));
  });

  it('does not start OAuth on selection and fixes the plan route without showing keys', async () => {
    const user = userEvent.setup(); render(<ApiSettings />);
    await user.selectOptions(screen.getByLabelText('服务商'), 'openai');
    await user.selectOptions(screen.getByLabelText('认证方式'), 'chatgpt_plan');
    await waitFor(() => expect(chatgptPlanStatus).toHaveBeenCalled());
    expect(chatgptPlanSignIn).not.toHaveBeenCalled();
    expect(screen.queryByLabelText('API Key')).not.toBeInTheDocument();
    expect(screen.getByLabelText('API 地址')).toHaveAttribute('readonly');
    expect(screen.getByLabelText('API 地址')).toHaveValue('https://api.openai.com/v1');
    expect(screen.getByLabelText('协议')).toBeDisabled();
    expect(screen.getByRole('button', { name: '测试连接' })).toBeDisabled();
    expect(screen.getByText(/不等于无限用量/)).toBeInTheDocument();
  });

  it('requires explicit sign-in, loads the authorized account catalog and does not save or auto-switch models', async () => {
    const user = userEvent.setup();
    vi.mocked(chatgptPlanSignIn).mockResolvedValue({ connected: true, planEnabled: true, needsSignIn: false, credentialRef: 'opaque-account-ref', accountLabel: 'offline-account' });
    render(<ApiSettings />);
    await user.selectOptions(screen.getByLabelText('服务商'), 'openai');
    await user.selectOptions(screen.getByLabelText('认证方式'), 'chatgpt_plan');
    const button = await screen.findByRole('button', { name: 'Continue with ChatGPT' });
    await waitFor(() => expect(button).toBeEnabled());
    const chosenModel = useSettingsStore.getState().apiConfig.model;
    await user.click(button);
    await waitFor(() => expect(chatgptPlanModels).toHaveBeenCalled());
    expect(screen.getByRole('dialog', { name: 'ChatGPT 套餐授权说明' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '知道了' }));
    expect(useSettingsStore.getState().apiConfig.model).toBe(chosenModel);
    expect(saveApiConfig).not.toHaveBeenCalled();
    expect(useSettingsStore.getState().apiConfig.credentialRef).toBe('opaque-account-ref');
    expect(useSettingsStore.getState().activeApiConfig).toMatchObject({ provider: 'anthropic', model: 'claude-sonnet-4-20250514', hasApiKey: true });
    expect(useSettingsStore.getState().activeApiConfig.authMode).not.toBe('chatgpt_plan');
    expect(screen.getByRole('button', { name: '测试连接' })).toBeEnabled();
    await user.clear(screen.getByLabelText('模型标识'));
    await user.type(screen.getByLabelText('模型标识'), 'manual-authorized-model');
    await user.click(screen.getByRole('button', { name: '测试连接' }));
    expect(testModelConnection).toHaveBeenCalledWith(expect.objectContaining({ provider: 'openai', protocol: 'openai_responses', auth_mode: 'chatgpt_plan', credential_ref: 'opaque-account-ref', api_key: '', model: 'manual-authorized-model' }));
  });

  it('offers explicit cancellation without applying a late login result', async () => {
    const user = userEvent.setup();
    let resolve: ((status: Awaited<ReturnType<typeof chatgptPlanSignIn>>) => void) | undefined;
    vi.mocked(chatgptPlanSignIn).mockImplementation(() => new Promise((done) => { resolve = done; }));
    render(<ApiSettings />);
    await user.selectOptions(screen.getByLabelText('服务商'), 'openai');
    await user.selectOptions(screen.getByLabelText('认证方式'), 'chatgpt_plan');
    await waitFor(() => expect(screen.getByRole('button', { name: 'Continue with ChatGPT' })).toBeEnabled());
    await user.click(screen.getByRole('button', { name: 'Continue with ChatGPT' }));
    await user.click(screen.getByRole('button', { name: '取消登录' }));
    expect(chatgptPlanCancelSignIn).toHaveBeenCalledOnce();
    await act(async () => { resolve?.({ connected: true, planEnabled: true, needsSignIn: false, accountLabel: 'late', credentialRef: 'late-ref' }); });
    await waitFor(() => expect(screen.queryByText(/late ·/)).not.toBeInTheDocument());
    expect(useSettingsStore.getState().apiConfig.credentialRef).toBeNull();
  });

  it('refreshes local status when revocation reports an error after disconnect', async () => {
    const user = userEvent.setup();
    vi.mocked(chatgptPlanStatus).mockResolvedValueOnce({ connected: true, planEnabled: true, needsSignIn: false, accountLabel: 'connected', credentialRef: 'old-ref' });
    vi.mocked(chatgptPlanDisconnect).mockRejectedValue(new Error('remote revocation unavailable'));
    render(<ApiSettings />);
    await user.selectOptions(screen.getByLabelText('服务商'), 'openai');
    await user.selectOptions(screen.getByLabelText('认证方式'), 'chatgpt_plan');
    await waitFor(() => expect(screen.getByRole('button', { name: '断开 ChatGPT 连接' })).toBeEnabled());
    await user.click(screen.getByRole('button', { name: '断开 ChatGPT 连接' }));
    await waitFor(() => expect(chatgptPlanStatus).toHaveBeenCalledTimes(2));
    expect(useSettingsStore.getState().apiConfig.credentialRef).toBeNull();
    expect(screen.getByRole('button', { name: '测试连接' })).toBeDisabled();
  });

  it('does not mark an edited model as tested when an old test finishes late', async () => {
    const user = userEvent.setup();
    let resolve: ((success: boolean) => void) | undefined;
    vi.mocked(testModelConnection).mockImplementation(() => new Promise((done) => { resolve = done; }));
    useSettingsStore.setState((state) => ({ apiConfig: { ...state.apiConfig, hasApiKey: true } }));
    render(<ApiSettings />);
    await user.click(screen.getByRole('button', { name: '测试连接' }));
    await user.clear(screen.getByLabelText('模型标识'));
    await user.type(screen.getByLabelText('模型标识'), 'edited-model');
    await act(async () => { resolve?.(true); });
    expect(within(screen.getByRole('region', { name: '连接测试' })).getByText('尚未测试')).toBeInTheDocument();
    expect(screen.queryByText('连接测试通过')).not.toBeInTheDocument();
  });

  it('does not apply an old disconnect result after a new plan lifecycle starts', async () => {
    const user = userEvent.setup();
    const connected = { connected: true, planEnabled: true, needsSignIn: false, accountLabel: 'new-account', credentialRef: 'new-ref' };
    vi.mocked(chatgptPlanStatus).mockResolvedValue(connected);
    let resolve: ((status: Awaited<ReturnType<typeof chatgptPlanDisconnect>>) => void) | undefined;
    vi.mocked(chatgptPlanDisconnect).mockImplementation(() => new Promise((done) => { resolve = done; }));
    render(<ApiSettings />);
    await user.selectOptions(screen.getByLabelText('服务商'), 'openai');
    await user.selectOptions(screen.getByLabelText('认证方式'), 'chatgpt_plan');
    await waitFor(() => expect(screen.getByRole('button', { name: '断开 ChatGPT 连接' })).toBeEnabled());
    await user.click(screen.getByRole('button', { name: '断开 ChatGPT 连接' }));
    await user.selectOptions(screen.getByLabelText('认证方式'), 'api_key');
    await user.selectOptions(screen.getByLabelText('认证方式'), 'chatgpt_plan');
    await waitFor(() => expect(chatgptPlanStatus).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(useSettingsStore.getState().apiConfig.credentialRef).toBe('new-ref'));
    await act(async () => { resolve?.({ connected: false, planEnabled: false, needsSignIn: true, accountLabel: null, credentialRef: null }); });
    expect(useSettingsStore.getState().apiConfig.credentialRef).toBe('new-ref');
    expect(useSettingsStore.getState().apiConfig.planEnabled).toBe(true);
  });

  it('preserves the old account when an explicit account change is cancelled', async () => {
    const user = userEvent.setup();
    const oldAccount = { connected: true, planEnabled: true, needsSignIn: false, accountLabel: 'original-account', credentialRef: 'original-ref' };
    useSettingsStore.setState((state) => ({ activeApiConfig: { ...state.activeApiConfig, provider: 'openai', model: 'saved-model', baseUrl: 'https://api.openai.com/v1', protocol: 'openai_responses', authMode: 'chatgpt_plan', credentialRef: 'original-ref', planEnabled: true, planConnected: true } }));
    vi.mocked(chatgptPlanStatus).mockResolvedValue(oldAccount);
    let resolve: ((status: Awaited<ReturnType<typeof chatgptPlanChangeAccount>>) => void) | undefined;
    vi.mocked(chatgptPlanChangeAccount).mockImplementation(() => new Promise((done) => { resolve = done; }));
    render(<ApiSettings />);
    await user.selectOptions(screen.getByLabelText('服务商'), 'openai');
    await user.selectOptions(screen.getByLabelText('认证方式'), 'chatgpt_plan');
    await waitFor(() => expect(screen.getByRole('button', { name: '更换账号' })).toBeEnabled());
    expect(chatgptPlanChangeAccount).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: '更换账号' }));
    expect(chatgptPlanChangeAccount).toHaveBeenCalledOnce();
    await user.click(screen.getByRole('button', { name: '取消登录' }));
    expect(chatgptPlanCancelSignIn).toHaveBeenCalledOnce();
    await act(async () => { resolve?.({ ...oldAccount, accountLabel: 'late-replacement', credentialRef: 'late-ref' }); });
    expect(useSettingsStore.getState().apiConfig.credentialRef).toBe('original-ref');
    expect(useSettingsStore.getState().apiConfig.planEnabled).toBe(true);
    expect(useSettingsStore.getState().activeApiConfig).toMatchObject({ credentialRef: 'original-ref', model: 'saved-model', planEnabled: true });
    expect(screen.getByText(/original-account · 已授权套餐用量/)).toBeInTheDocument();
    expect(screen.queryByText(/late-replacement/)).not.toBeInTheDocument();
    expect(saveApiConfig).not.toHaveBeenCalled();
  });

  it('updates the account draft and catalog after an explicit change without saving or selecting a model', async () => {
    const user = userEvent.setup();
    useSettingsStore.setState((state) => ({ activeApiConfig: { ...state.activeApiConfig, provider: 'openai', model: 'saved-model', baseUrl: 'https://api.openai.com/v1', protocol: 'openai_responses', authMode: 'chatgpt_plan', credentialRef: 'old-ref', planEnabled: true, planConnected: true } }));
    vi.mocked(chatgptPlanStatus).mockResolvedValue({ connected: true, planEnabled: true, needsSignIn: false, accountLabel: 'old-account', credentialRef: 'old-ref' });
    vi.mocked(chatgptPlanChangeAccount).mockResolvedValue({ connected: true, planEnabled: true, needsSignIn: false, accountLabel: 'new-account', credentialRef: 'new-ref' });
    vi.mocked(chatgptPlanModels).mockResolvedValueOnce([{ id: 'old-model', name: 'Old model' }]).mockResolvedValueOnce([{ id: 'new-model', name: 'New model' }]);
    render(<ApiSettings />);
    await user.selectOptions(screen.getByLabelText('服务商'), 'openai');
    await user.selectOptions(screen.getByLabelText('认证方式'), 'chatgpt_plan');
    await waitFor(() => expect(screen.getByRole('button', { name: '更换账号' })).toBeEnabled());
    await waitFor(() => expect(chatgptPlanModels).toHaveBeenCalledOnce());
    const chosenModel = useSettingsStore.getState().apiConfig.model;
    await user.click(screen.getByRole('button', { name: '更换账号' }));
    await waitFor(() => expect(useSettingsStore.getState().apiConfig.credentialRef).toBe('new-ref'));
    await waitFor(() => expect(chatgptPlanModels).toHaveBeenCalledTimes(2));
    expect(screen.getByRole('dialog', { name: 'ChatGPT 套餐授权说明' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '知道了' }));
    expect(screen.getByText(/new-account · 已授权套餐用量/)).toBeInTheDocument();
    expect(useSettingsStore.getState().apiConfig.model).toBe(chosenModel);
    expect(useSettingsStore.getState().activeApiConfig).toMatchObject({ authMode: 'chatgpt_plan', credentialRef: 'old-ref', model: 'saved-model', planEnabled: false, planConnected: false });
    expect(chatgptPlanSignIn).not.toHaveBeenCalled();
    expect(saveApiConfig).not.toHaveBeenCalled();
  });
});

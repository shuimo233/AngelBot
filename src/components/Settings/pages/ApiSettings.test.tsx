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
    vi.mocked(testModelConnection).mockResolvedValue({ toolRoundTrip: 'verified' });
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

    expect(screen.getByLabelText('API 地址')).toHaveAccessibleDescription('请先启动本地模型服务。');
    expect(screen.queryByLabelText('API Key')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '测试连接' })).toBeEnabled();
  });

  it('opens with just the necessary fields for first connection and allows opening details', async () => {
    const user = userEvent.setup();
    useSettingsStore.setState(state => ({ activeApiConfig: state.apiConfig }));
    render(<ApiSettings saveAction={<button type="button">保存</button>} />);

    expect(screen.getByLabelText('服务商')).toBeVisible();
    expect(screen.getByLabelText('API Key')).toHaveAccessibleDescription('密钥保存在系统凭据库。');
    expect(screen.queryByLabelText('API 地址')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('模型标识')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '测试连接' })).toBeDisabled();
    expect(screen.getByRole('region', { name: '连接测试' })).toContainElement(screen.getByRole('button', { name: '保存' }));
    await user.click(screen.getByRole('button', { name: '高级选项' }));
    expect(screen.getByLabelText('API 地址')).toBeVisible();
    expect(screen.getByLabelText('模型标识')).toBeVisible();
    expect(saveApiConfig).not.toHaveBeenCalled();
  });

  it('shows configured connection and generation fields without disclosure and keeps changes as drafts', async () => {
    const user = userEvent.setup();
    useSettingsStore.setState(state => ({ apiConfig: { ...state.apiConfig, hasApiKey: true, credentialSource: 'keychain' } }));
    render(<ApiSettings />);
    expect(screen.getByLabelText('模型标识')).toBeVisible();
    expect(screen.getByRole('region', { name: '生成参数' })).toBeVisible();
    const endpoint = screen.getByLabelText('API 地址');
    expect(endpoint).toBeVisible();
    await user.type(endpoint, '/gateway');
    expect(endpoint).toHaveValue('https://api.anthropic.com/gateway');
    expect(saveApiConfig).not.toHaveBeenCalled();
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

  it('offers visible provider model choices while retaining manual model identifiers', async () => {
    const user = userEvent.setup();
    render(<ApiSettings />);
    await user.selectOptions(screen.getByLabelText('服务商'), 'deepseek');
    const choices = screen.getByRole('combobox', { name: '选择模型' });
    const available = within(choices).getAllByRole('option').filter((option) => !option.hasAttribute('disabled'));
    expect(available.length).toBeGreaterThan(1);
    const selected = available[1] as HTMLOptionElement;
    await user.selectOptions(choices, selected.value);
    expect(screen.getByLabelText('模型标识')).toHaveValue(selected.value);
    await user.clear(screen.getByLabelText('模型标识'));
    await user.type(screen.getByLabelText('模型标识'), 'custom-new-model');
    expect(choices).toHaveValue('');
    expect(screen.getByLabelText('模型标识')).toHaveValue('custom-new-model');
    expect(saveApiConfig).not.toHaveBeenCalled();
  });

  it('keeps a manually chosen model when the account catalog arrives late', async () => {
    const user = userEvent.setup();
    vi.mocked(chatgptPlanStatus).mockResolvedValue({ connected: true, planEnabled: true, needsSignIn: false, credentialRef: 'offline-ref', accountLabel: 'offline-account' });
    let resolve!: (models: Awaited<ReturnType<typeof chatgptPlanModels>>) => void;
    vi.mocked(chatgptPlanModels).mockImplementation(() => new Promise((done) => { resolve = done; }));
    render(<ApiSettings />);
    await user.selectOptions(screen.getByLabelText('服务商'), 'openai');
    await user.selectOptions(screen.getByLabelText('认证方式'), 'chatgpt_plan');
    await waitFor(() => expect(chatgptPlanModels).toHaveBeenCalledOnce());
    expect(screen.getByRole('combobox', { name: '选择模型' })).toBeDisabled();
    expect(screen.getByRole('option', { name: '正在读取账号模型…' })).toBeInTheDocument();
    await user.clear(screen.getByLabelText('模型标识'));
    await user.type(screen.getByLabelText('模型标识'), 'manual-authorized-model');
    await act(async () => { resolve([{ id: 'account-model', name: 'Account model' }]); });
    const choices = screen.getByRole('combobox', { name: '选择模型' });
    expect(choices).toBeEnabled();
    expect(screen.getByLabelText('模型标识')).toHaveValue('manual-authorized-model');
    await user.selectOptions(choices, 'account-model');
    expect(screen.getByLabelText('模型标识')).toHaveValue('account-model');
    expect(saveApiConfig).not.toHaveBeenCalled();
  });

  it.each(['failed', 'empty'] as const)('distinguishes a %s account catalog from a missing model capability', async (outcome) => {
    const user = userEvent.setup();
    vi.mocked(chatgptPlanStatus).mockResolvedValue({ connected: true, planEnabled: true, needsSignIn: false, credentialRef: 'offline-ref', accountLabel: 'offline-account' });
    if (outcome === 'failed') vi.mocked(chatgptPlanModels).mockRejectedValueOnce(new Error('offline catalog unavailable'));
    else vi.mocked(chatgptPlanModels).mockResolvedValueOnce([]);
    render(<ApiSettings />);
    await user.selectOptions(screen.getByLabelText('服务商'), 'openai');
    await user.selectOptions(screen.getByLabelText('认证方式'), 'chatgpt_plan');
    await screen.findByRole('option', { name: outcome === 'failed' ? '列表读取失败，可手动填写' : '账号未提供候选，可手动填写' });
    expect(screen.getByRole('combobox', { name: '选择模型' })).toBeDisabled();
    expect(screen.getByLabelText('模型标识')).toBeEnabled();
    expect(screen.getByRole('button', { name: '测试连接' })).toBeEnabled();
    if (outcome === 'failed') expect(screen.getByRole('alert')).toHaveTextContent('offline catalog unavailable');
    expect(saveApiConfig).not.toHaveBeenCalled();
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
    expect(screen.getByLabelText('API Key')).toHaveAccessibleDescription(/当前由环境变量 ANTHROPIC_API_KEY 管理/);
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
    const testPanel = within(screen.getByRole('region', { name: '连接测试' }));
    const testButton = testPanel.getByRole('button', { name: '测试连接' });
    expect(testButton).toHaveAccessibleDescription(/最多 3 次请求.*可能消耗额度.*测试不会保存/);
    await user.click(testButton);

    await waitFor(() => expect(testModelConnection).toHaveBeenCalledWith(expect.objectContaining({
      base_url: 'https://api.anthropic.com', api_key: '', provider: 'anthropic', model: 'claude-sonnet-4-20250514',
    })));
    expect(within(screen.getByRole('region', { name: '连接测试' })).getByText('连接测试通过')).toBeInTheDocument();
    expect(testPanel.getByText('已验证对话和工具往返。')).toBeInTheDocument();

    for (const [status, detail] of [
      ['notObserved', '本次未收到工具调用。可核对模型或协议后重试。'],
      ['invalidCall', '工具调用格式或参数不合要求。请核对模型和协议。'],
      ['invalidResult', '模型未能根据工具结果完成回复。'],
      ['requestFailed', '工具验证请求失败。可检查服务状态后重试。'],
      ['timedOut', '工具验证在 60 秒内未完成。可稍后重试。'],
    ] as const) {
      vi.mocked(testModelConnection).mockResolvedValue({ toolRoundTrip: status });
      await user.click(screen.getByRole('button', { name: '测试连接' }));
      await waitFor(() => expect(screen.getByText(detail)).toBeInTheDocument());
      expect(within(screen.getByRole('region', { name: '连接测试' })).getByText('对话连接正常，工具往返未验证')).toBeInTheDocument();
      expect(screen.getByText('对话连接正常，工具往返未验证')).toHaveAttribute('data-state', 'partial');
      expect(screen.queryByText('连接测试通过')).not.toBeInTheDocument();
      expect(saveApiConfig).not.toHaveBeenCalled();
    }
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
    expect(within(screen.getByRole('region', { name: 'ChatGPT 套餐' })).getByText(/不会切换到 API 计费/)).toBeVisible();
  });

  it('shows protected status failures and restores the saved plan through a status-only retry', async () => {
    const user = userEvent.setup();
    const plan = {
      ...useSettingsStore.getState().apiConfig,
      provider: 'openai' as const, model: 'gpt-6-luna',
      baseUrl: 'https://api.openai.com/v1', protocol: 'openai_responses' as const,
      authMode: 'chatgpt_plan' as const, credentialRef: 'saved-ref',
      planConnected: true, planEnabled: true,
    };
    useSettingsStore.setState({ apiConfig: plan, activeApiConfig: plan });
    vi.mocked(chatgptPlanStatus).mockRejectedValueOnce('资料正在迁移或需要恢复。');
    vi.mocked(chatgptPlanStatus).mockResolvedValueOnce({
      connected: true, planEnabled: true, needsSignIn: false,
      credentialRef: 'saved-ref', accountLabel: 'saved-account',
    });
    render(<ApiSettings />);

    const overview = within(screen.getByRole('region', { name: 'OpenAI (GPT)' }));
    expect(await overview.findByText('无法读取 ChatGPT 连接状态')).toBeInTheDocument();
    expect(screen.getByText('ChatGPT 授权状态未知')).toBeInTheDocument();
    expect(useSettingsStore.getState().activeApiConfig).toMatchObject({
      credentialRef: 'saved-ref', planEnabled: false, planStatusError: '资料正在迁移或需要恢复。',
    });
    expect(screen.queryByText('尚未连接 ChatGPT 账号')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Continue with ChatGPT' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '重新检查连接状态' }));
    expect(await screen.findByText('saved-account · 已授权套餐用量')).toBeInTheDocument();
    expect(useSettingsStore.getState().activeApiConfig).toMatchObject({
      credentialRef: 'saved-ref', planEnabled: true, planStatusError: null,
    });
    expect(chatgptPlanSignIn).not.toHaveBeenCalled();
    expect(saveApiConfig).not.toHaveBeenCalled();
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
    let resolve: ((report: Awaited<ReturnType<typeof testModelConnection>>) => void) | undefined;
    vi.mocked(testModelConnection).mockImplementation(() => new Promise((done) => { resolve = done; }));
    useSettingsStore.setState((state) => ({ apiConfig: { ...state.apiConfig, hasApiKey: true } }));
    render(<ApiSettings />);
    await user.click(screen.getByRole('button', { name: '测试连接' }));
    await user.clear(screen.getByLabelText('模型标识'));
    await user.type(screen.getByLabelText('模型标识'), 'edited-model');
    await act(async () => { resolve?.({ toolRoundTrip: 'verified' }); });
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

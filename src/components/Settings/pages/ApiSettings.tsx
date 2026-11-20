import { useEffect, useRef, useState } from 'react';
import { chatgptPlanStatus, chatgptPlanSignIn, chatgptPlanChangeAccount, chatgptPlanCancelSignIn, chatgptPlanDisconnect, chatgptPlanModels, testModelConnection, type ChatgptPlanStatus, type ChatgptPlanModel, type ToolRoundTrip } from '$lib/commands/settings';
import {
  ALL_PROVIDERS,
  getModelsForProvider,
  getProviderMeta,
  providerProtocolLabel,
} from '$lib/providers';
import { toRustConfig, useSettingsStore } from '$stores/settings';
import { getModelConfigReadiness } from '$lib/model-config';
import type { ApiConfig, ApiProvider } from '$types';
import { Button, Dialog, Input, Select, SelectField } from '../../../ui';
import { SettingsSection } from '../components/SettingsSection';
import './ApiSettings.css';

type TestState = 'idle' | 'testing' | 'success' | 'error';

const TOOL_CHECK_DETAIL: Record<ToolRoundTrip, string> = {
  verified: '已验证对话和工具往返。',
  notObserved: '本次未收到工具调用。可核对模型或协议后重试。',
  invalidCall: '工具调用格式或参数不合要求。请核对模型和协议。',
  invalidResult: '模型未能根据工具结果完成回复。',
  requestFailed: '工具验证请求失败。可检查服务状态后重试。',
  timedOut: '工具验证在 60 秒内未完成。可稍后重试。',
};

function credentialSummary(config: ApiConfig, requiresKey: boolean): string {
  if (config.authMode === 'chatgpt_plan') {
    if (config.planStatusError) return 'ChatGPT 授权状态未知';
    return config.planEnabled ? 'ChatGPT 套餐授权（无 API Key）' : '尚未授权 ChatGPT 套餐用量';
  }
  if (config.authMode === 'none') return '显式免认证，无需 API Key';
  if (!requiresKey) return '本地服务，无需 API Key';
  if (config.credentialSource === 'environment') return '由环境变量提供';
  if (config.hasApiKey) return '已保存在系统凭据库';
  if (config.apiKey.trim()) return '已填写，保存后写入系统凭据库';
  return '尚未配置';
}

export function ApiSettings({ saveAction }: { saveAction?: React.ReactNode } = {}) {
  const { apiConfig, apiConfigLoaded, updateApiConfig, loadApiConfig } = useSettingsStore();
  const [guidedSetup] = useState(() => {
    const { activeApiConfig, apiConfigLoaded, apiConfigLoadError } = useSettingsStore.getState();
    const initialConnection = getModelConfigReadiness(activeApiConfig, apiConfigLoaded, apiConfigLoadError);
    return initialConnection.kind === 'needs-credentials' || initialConnection.kind === 'invalid';
  });
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [showApiKey, setShowApiKey] = useState(false);
  const [testState, setTestState] = useState<TestState>('idle');
  const [testError, setTestError] = useState('');
  const [toolRoundTrip, setToolRoundTrip] = useState<ToolRoundTrip | null>(null);
  const [planStatus, setPlanStatus] = useState<ChatgptPlanStatus | null>(null);
  const [planModels, setPlanModels] = useState<ChatgptPlanModel[]>([]);
  const [planModelsLoading, setPlanModelsLoading] = useState(false);
  const [planModelsError, setPlanModelsError] = useState('');
  const [planBusy, setPlanBusy] = useState<'idle' | 'checking' | 'signing-in' | 'disconnecting'>('idle');
  const [planError, setPlanError] = useState('');
  const [planCheck, setPlanCheck] = useState(0);
  const [planNotice, setPlanNotice] = useState(false);
  const planOperation = useRef(0);
  const testOperation = useRef(0);

  useEffect(() => {
    if (!apiConfigLoaded) void loadApiConfig();
  }, [apiConfigLoaded, loadApiConfig]);

  useEffect(() => () => { testOperation.current += 1; }, []);

  const provider = apiConfig.provider as ApiProvider;
  const meta = getProviderMeta(provider);
  const authMode = apiConfig.authMode ?? (meta.requiresKey ? 'api_key' : 'none');
  const isPlan = authMode === 'chatgpt_plan';
  const models = isPlan ? planModels.map((model) => ({ value: model.id, label: model.name })) : getModelsForProvider(provider);
  const listedModel = models.some((model) => model.value === apiConfig.model);
  const hasCredentials = Boolean(apiConfig.apiKey.trim() || apiConfig.hasApiKey);
  const environmentManaged = apiConfig.credentialSource === 'environment';
  const endpointHint = isPlan ? '固定为 OpenAI 官方地址。'
    : provider === 'azure' ? '使用 Azure 资源的 /openai/v1 端点。'
      : meta.isLocal ? '请先启动本地模型服务。' : undefined;
  const keyHint = environmentManaged ? `当前由环境变量 ${meta.envKey} 管理。`
    : apiConfig.hasApiKey && !apiConfig.apiKey ? '留空保留现有密钥。' : undefined;
  const endpointRequired = meta.endpointMode === 'required';
  const showConnectionDetails = !guidedSetup || showAdvanced;
  const showEndpoint = showConnectionDetails || endpointRequired || meta.isLocal
    || (!isPlan && Boolean(apiConfig.baseUrl && apiConfig.baseUrl !== meta.defaultBaseUrl));
  const missingModel = !apiConfig.model.trim();
  const missingEndpoint = endpointRequired && !apiConfig.baseUrl.trim();
  const planReady = Boolean(!apiConfig.planStatusError && planBusy !== 'checking'
    && planStatus?.connected && planStatus.planEnabled && !planStatus.needsSignIn && planStatus.credentialRef);
  const missingCredentials = isPlan ? !planReady : authMode === 'api_key' && !hasCredentials;
  const canTest = !missingModel && !missingEndpoint && !missingCredentials;

  const applyPlanStatus = (status: ChatgptPlanStatus) => {
    setPlanStatus(status);
    const fields = { credentialRef: status.credentialRef, planConnected: status.connected, planEnabled: status.connected && status.planEnabled && !status.needsSignIn, planAccountLabel: status.accountLabel, planStatusError: null };
    useSettingsStore.getState().refreshPlanBinding(fields);
    if (useSettingsStore.getState().apiConfig.authMode === 'chatgpt_plan') updateApiConfig(fields);
    resetTest();
  };

  const applyPlanReadError = (error: unknown) => {
    const fields = { planConnected: false, planEnabled: false, planAccountLabel: null, planStatusError: String(error) || '暂时无法读取授权记录。' };
    setPlanStatus(null);
    useSettingsStore.getState().refreshPlanBinding(fields);
    if (useSettingsStore.getState().apiConfig.authMode === 'chatgpt_plan') updateApiConfig(fields);
    resetTest();
  };

  useEffect(() => {
    if (!isPlan) return;
    let cancelled = false;
    setPlanStatus(null);
    setPlanBusy('checking');
    setPlanError('');
    void chatgptPlanStatus().then((status) => {
      if (!cancelled) applyPlanStatus(status);
    }).catch((error: unknown) => {
      if (!cancelled) applyPlanReadError(error);
    }).finally(() => { if (!cancelled) setPlanBusy('idle'); });
    return () => { cancelled = true; planOperation.current += 1; };
    // A status read never starts OAuth or saves the model configuration.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isPlan, planCheck]);

  useEffect(() => {
    setPlanModels([]);
    setPlanModelsLoading(false);
    setPlanModelsError('');
    if (!isPlan || !planStatus?.connected || !planStatus.planEnabled || planStatus.needsSignIn) return;
    let cancelled = false;
    setPlanModelsLoading(true);
    void chatgptPlanModels().then((models) => { if (!cancelled) setPlanModels(models); })
      .catch((error: unknown) => { if (!cancelled) setPlanModelsError(`无法读取账号模型列表，可手动填写已授权模型：${String(error)}`); })
      .finally(() => { if (!cancelled) setPlanModelsLoading(false); });
    return () => { cancelled = true; };
  }, [isPlan, planStatus?.connected, planStatus?.planEnabled, planStatus?.needsSignIn, planStatus?.credentialRef]);

  const handlePlanSignIn = async (changeAccount = false) => {
    const operation = ++planOperation.current;
    setPlanBusy('signing-in'); setPlanError('');
    try {
      const status = await (changeAccount ? chatgptPlanChangeAccount() : chatgptPlanSignIn());
      if (operation !== planOperation.current) return;
      applyPlanStatus(status);
      if (status.planEnabled) {
        try { if (!localStorage.getItem('angelbot_chatgpt_plan_notice_ack')) setPlanNotice(true); }
        catch { setPlanNotice(true); }
      }
    } catch (error) {
      if (operation === planOperation.current) setPlanError(`ChatGPT ${changeAccount ? '更换账号' : '登录'}未完成：${String(error)}`);
    } finally { if (operation === planOperation.current) setPlanBusy('idle'); }
  };

  const dismissPlanNotice = () => {
    try { localStorage.setItem('angelbot_chatgpt_plan_notice_ack', 'true'); } catch { /* Optional, non-secret UI preference. */ }
    setPlanNotice(false);
  };

  const handlePlanCancel = async () => {
    const operation = ++planOperation.current;
    try { await chatgptPlanCancelSignIn(); } catch (error) { if (operation === planOperation.current) setPlanError(`取消登录失败：${String(error)}`); }
    if (operation === planOperation.current) setPlanBusy('idle');
  };

  const handlePlanDisconnect = async () => {
    const operation = ++planOperation.current; setPlanBusy('disconnecting'); setPlanError('');
    useSettingsStore.getState().invalidatePlanBinding(null);
    try {
      const status = await chatgptPlanDisconnect();
      if (operation === planOperation.current) applyPlanStatus(status);
    }
    catch (error) {
      if (operation !== planOperation.current) return;
      setPlanError(`断开连接提示：${String(error)}`);
      // Remote revocation can fail after local credentials are removed.
      try {
        const status = await chatgptPlanStatus();
        if (operation === planOperation.current) applyPlanStatus(status);
      }
      catch (statusError) { if (operation === planOperation.current) applyPlanReadError(statusError); }
    } finally { if (operation === planOperation.current) setPlanBusy('idle'); }
  };

  const resetTest = () => {
    testOperation.current += 1;
    setTestState('idle');
    setTestError('');
    setToolRoundTrip(null);
  };

  const updateConnection = (partial: Partial<ApiConfig>) => {
    updateApiConfig(partial);
    resetTest();
  };

  const handleProviderChange = (nextProvider: ApiProvider) => {
    const nextMeta = getProviderMeta(nextProvider);
    updateConnection({
      provider: nextProvider,
      model: nextMeta.models[0]?.value ?? '',
      baseUrl: nextMeta.defaultBaseUrl,
      apiKey: '',
      hasApiKey: false,
      credentialSource: 'none',
      protocol: undefined,
      authMode: nextMeta.requiresKey ? 'api_key' : 'none',
      credentialRef: null, planConnected: false, planEnabled: false, planAccountLabel: null, planStatusError: null,
    });
    setShowApiKey(false);
  };

  const handleTestConnection = async () => {
    if (!canTest) return;
    const operation = ++testOperation.current;
    const testedConfig = apiConfig;
    const isCurrent = () => operation === testOperation.current && useSettingsStore.getState().apiConfig === testedConfig;
    setTestState('testing');
    setTestError('');
    setToolRoundTrip(null);
    try {
      const report = await testModelConnection(toRustConfig({ ...apiConfig, baseUrl: apiConfig.baseUrl || meta.defaultBaseUrl }));
      if (!isCurrent()) { if (operation === testOperation.current) resetTest(); return; }
      setTestState('success');
      setToolRoundTrip(report.toolRoundTrip);
    } catch (error) {
      if (!isCurrent()) { if (operation === testOperation.current) resetTest(); return; }
      setTestState('error');
      const detail = error instanceof Error ? error.message : String(error);
      setTestError(detail || '服务未提供错误详情。');
    }
  };

  const statusText = isPlan && apiConfig.planStatusError
    ? '无法读取 ChatGPT 连接状态'
    : isPlan && (planBusy === 'checking' || planStatus === null)
      ? '等待读取 ChatGPT 连接状态'
      : testState === 'testing'
    ? '正在测试连接'
    : testState === 'success'
      ? toolRoundTrip === 'verified' ? '连接测试通过' : '对话连接正常，工具往返未验证'
      : testState === 'error'
        ? '连接测试失败'
        : missingCredentials
          ? isPlan ? '需要 ChatGPT 套餐授权' : '需要 API Key'
          : missingModel
            ? '需要模型标识'
            : missingEndpoint
              ? '需要 API 地址'
              : '尚未测试';

  const connectionActions = (
    <section className="model-test-panel" aria-label="连接测试">
      <div className="model-test-copy" aria-live="polite">
        <strong data-state={testState === 'success' && toolRoundTrip !== 'verified' ? 'partial' : testState}>{statusText}</strong>
        {testState === 'success' && toolRoundTrip && <small>{TOOL_CHECK_DETAIL[toolRoundTrip]}</small>}
        {testState === 'idle' && canTest && <small id="model-test-help">最多 3 次请求，可能消耗额度。测试不会保存。</small>}
      </div>
      <div className="model-test-commands">
        <Button
          className="model-test-action"
          aria-describedby={testState === 'idle' && canTest ? 'model-test-help' : undefined}
          onClick={() => { void handleTestConnection(); }}
          disabled={testState === 'testing' || !canTest}
          busy={testState === 'testing'}
        >
          {testState === 'testing' ? '测试中…' : '测试连接'}
        </Button>
        {saveAction}
      </div>
      {testState === 'error' && <p className="model-error" role="alert">{testError}</p>}
    </section>
  );

  const connectionOverview = (
      <section className="model-panel model-connection-overview" aria-labelledby="model-connection-overview-title">
        <div className="model-connection-identity">
          <h3 id="model-connection-overview-title">{meta.name}</h3>
          <p className="model-connection-id">{apiConfig.model || '尚未选择模型'}</p>
          <ul className="model-connection-facts">
            <li>{credentialSummary(apiConfig, meta.requiresKey)}</li>
            <li>{providerProtocolLabel(provider, apiConfig.protocol)}</li>
            <li>{meta.isLocal ? '本机' : '外部模型服务'}</li>
          </ul>
        </div>
        {connectionActions}
      </section>
  );

  return (
    <div className={`model-connection-settings${guidedSetup && !showAdvanced && !isPlan ? ' model-simple-setup' : ''}`}>
      {!guidedSetup && connectionOverview}
      <SettingsSection title="连接方式" className="model-basic-fields">
        <div className="model-panel model-field-grid model-provider-grid">
          <SelectField label="服务商" wrapperClassName="model-field" value={provider}
            onChange={(event) => handleProviderChange(event.target.value as ApiProvider)}>
            {ALL_PROVIDERS.map((item) => (
              <option key={item.id} value={item.id}>{item.name}</option>
            ))}
          </SelectField>
          {(provider === 'openai' || provider === 'custom') && (
            <SelectField label="认证方式" wrapperClassName="model-field" value={authMode}
              onChange={(event) => {
                const nextMode = event.target.value as ApiConfig['authMode'];
                updateConnection(nextMode === 'chatgpt_plan'
                  ? { authMode: nextMode, protocol: 'openai_responses', baseUrl: 'https://api.openai.com/v1', apiKey: '', hasApiKey: false, credentialSource: 'none', credentialRef: planStatus?.credentialRef ?? null, planEnabled: Boolean(planStatus?.planEnabled), planConnected: Boolean(planStatus?.connected) }
                  : { authMode: nextMode, ...(nextMode === 'none' ? { apiKey: '', hasApiKey: false, credentialSource: 'none' as const } : {}), credentialRef: null, planEnabled: false, planConnected: false, planAccountLabel: null });
                setShowApiKey(false);
              }}>
              <option value="api_key">API Key（独立 API 计费）</option>
              {provider === 'openai' ? <option value="chatgpt_plan">ChatGPT 套餐</option> : <option value="none">免认证（显式）</option>}
            </SelectField>
          )}
          {(provider === 'custom' || (provider === 'openai' && showConnectionDetails)) && (
            <SelectField label="协议" wrapperClassName="model-field" value={apiConfig.protocol ?? 'openai_chat_completions'} disabled={isPlan}
              onChange={(event) => updateConnection({ protocol: event.target.value as ApiConfig['protocol'] })}>
              <option value="openai_chat_completions">OpenAI Chat Completions</option>
              <option value="openai_responses">OpenAI Responses</option>
              {provider === 'custom' && <option value="anthropic_messages">Anthropic Messages</option>}
            </SelectField>
          )}
        </div>
      </SettingsSection>

      {isPlan && (
        <SettingsSection title="ChatGPT 套餐">
          <div className="model-panel model-account-panel">
            <div className="model-account-copy">
              <strong role="status">{planBusy === 'checking' ? '正在读取账号状态…' : apiConfig.planStatusError ? '无法读取账号状态' : planStatus?.connected ? `${planStatus.accountLabel || 'ChatGPT 账号'} · ${planStatus.planEnabled && !planStatus.needsSignIn ? '已授权套餐用量' : '需要重新授权套餐用量'}` : planStatus ? '尚未连接 ChatGPT 账号' : '等待读取账号状态'}</strong>
              <small>符合条件的请求使用套餐额度或 credits，不会切换到 API 计费。{planStatus?.connected && '更换后请保存。'}</small>
            </div>
            <div className="model-account-actions">
              {apiConfig.planStatusError ? (
                <Button variant="secondary" disabled={planBusy !== 'idle'} onClick={() => setPlanCheck((count) => count + 1)}>重新检查连接状态</Button>
              ) : planStatus && (
                <Button variant={planStatus.connected && planStatus.planEnabled && !planStatus.needsSignIn ? 'secondary' : 'primary'}
                  busy={planBusy === 'signing-in'} disabled={planBusy !== 'idle'} onClick={() => { void handlePlanSignIn(); }}>Continue with ChatGPT</Button>
              )}
              {planStatus?.connected && <Button variant="secondary" disabled={planBusy !== 'idle'} onClick={() => { void handlePlanSignIn(true); }}>更换账号</Button>}
              {planBusy === 'signing-in' && <Button onClick={() => { void handlePlanCancel(); }}>取消登录</Button>}
              {planStatus?.connected && <Button variant="ghost" disabled={planBusy !== 'idle'} onClick={() => { void handlePlanDisconnect(); }}>断开 ChatGPT 连接</Button>}
            </div>
            {apiConfig.planStatusError && <p className="model-error" role="alert">{apiConfig.planStatusError}</p>}
            {planError && <p className="model-error" role="alert">{planError}</p>}
          </div>
          <Dialog open={planNotice} title="ChatGPT 套餐授权说明" onClose={dismissPlanNotice}>
            <p>保存后即可使用套餐额度或 credits。可在 ChatGPT 设置 → Usage 查看用量。</p>
            <Button onClick={dismissPlanNotice}>知道了</Button>
          </Dialog>
        </SettingsSection>
      )}

      {(showEndpoint || authMode === 'api_key') && <SettingsSection title="连接信息" className="model-basic-fields">
        <div className="model-panel model-credentials-panel">
          {showEndpoint && <div className="model-field model-credential-row">
            <div className="model-field-copy">
              <label htmlFor="model-api-endpoint">API 地址</label>
              {endpointHint && <small id="model-endpoint-help">{endpointHint}</small>}
            </div>
            <div className="model-field-inline">
              <Input
                id="model-api-endpoint"
                type="url"
                value={apiConfig.baseUrl}
                onChange={(event) => updateConnection({ baseUrl: event.target.value })}
                placeholder={meta.defaultBaseUrl || 'https://example.com/v1'}
                aria-describedby={endpointHint ? 'model-endpoint-help' : undefined}
                readOnly={isPlan}
              />
              {!isPlan && meta.defaultBaseUrl && apiConfig.baseUrl !== meta.defaultBaseUrl && (
                <Button variant="ghost" className="model-secondary-action" onClick={() => updateConnection({ baseUrl: meta.defaultBaseUrl })}>
                  恢复默认
                </Button>
              )}
            </div>
          </div>}

          {authMode === 'api_key' ? (
            <div className="model-field model-credential-row">
              <div className="model-field-copy">
                <label htmlFor="model-api-key">API Key</label>
                {(keyHint || guidedSetup) && <small id="model-key-help">{keyHint ?? '密钥保存在系统凭据库。'}</small>}
              </div>
              <div className="model-field-inline">
                <Input
                  id="model-api-key"
                  type={showApiKey ? 'text' : 'password'}
                  value={apiConfig.apiKey}
                  onChange={(event) => updateConnection({ apiKey: event.target.value })}
                  placeholder={apiConfig.hasApiKey ? '已设置' : '输入 API Key'}
                  disabled={environmentManaged}
                  autoComplete="off"
                  aria-describedby={keyHint || guidedSetup ? 'model-key-help' : undefined}
                />
                <Button
                  type="button"
                  variant="ghost"
                  className="model-secondary-action model-key-visibility"
                  onClick={() => setShowApiKey((visible) => !visible)}
                  disabled={environmentManaged}
                  aria-label={showApiKey ? '隐藏 API Key' : '显示 API Key'}
                >
                  {showApiKey ? '隐藏' : '显示'}
                </Button>
              </div>
            </div>
          ) : null}
        </div>
      </SettingsSection>}

      {(showConnectionDetails || missingModel || (isPlan && planReady)) && <SettingsSection title="默认模型">
        <div className="model-panel model-field-grid">
          {(isPlan || models.length > 0) && (
            <label className="model-field">
              <span>选择模型</span>
              <Select
                aria-label="选择模型"
                aria-describedby={planModelsError ? 'model-catalog-error' : undefined}
                value={listedModel ? apiConfig.model : ''}
                disabled={models.length === 0}
                onChange={(event) => updateConnection({ model: event.target.value })}
              >
                <option value="" disabled>
                  {planModelsLoading ? '正在读取账号模型…'
                    : planModelsError ? '列表读取失败，可手动填写'
                      : !models.length ? planReady ? '账号未提供候选，可手动填写' : '授权后读取账号模型'
                        : apiConfig.model ? '自定义模型（见模型标识）' : '请选择或手动填写'}
                </option>
                {models.map((model) => <option key={model.value} value={model.value}>{model.label}</option>)}
              </Select>
              {planModelsError && <small id="model-catalog-error" className="model-inline-error" role="alert">{planModelsError}</small>}
            </label>
          )}
          <label className="model-field">
            <span>{provider === 'azure' ? '部署名称' : '模型标识'}</span>
            <Input
              type="text"
              aria-label={provider === 'azure' ? '部署名称' : '模型标识'}
              value={apiConfig.model}
              onChange={(event) => updateConnection({ model: event.target.value })}
              placeholder={provider === 'azure' ? '例如 production-gpt' : '输入或选择模型'}
              autoComplete="off"
            />
          </label>
        </div>
      </SettingsSection>}

      {guidedSetup && <div className="model-panel model-guided-actions">{connectionActions}</div>}
      {guidedSetup && (
        <Button className="model-setup-disclosure" variant="ghost" aria-expanded={showAdvanced}
          onClick={() => setShowAdvanced((value) => !value)}>
          {showAdvanced ? '收起高级选项' : '高级选项'}
        </Button>
      )}

      {!isPlan && showConnectionDetails && <SettingsSection title="生成参数">
        <div className="model-panel model-field-grid">
          <label className="model-field">
            <span>最大输出 Token</span>
            <Input
              type="number"
              min={256}
              max={200000}
              step={256}
              value={apiConfig.maxTokens}
              onChange={(event) => updateConnection({ maxTokens: Number(event.target.value) || 4096 })}
            />
          </label>
          <label className="model-field">
            <span>随机性（Temperature）</span>
            <Input
              type="number"
              min={0}
              max={2}
              step={0.1}
              value={apiConfig.temperature}
              onChange={(event) => updateConnection({ temperature: Number(event.target.value) })}
            />
          </label>
        </div>
      </SettingsSection>}
    </div>
  );
}

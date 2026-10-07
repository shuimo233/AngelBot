import { useEffect, useId, useRef, useState } from 'react';
import { chatgptPlanStatus, chatgptPlanSignIn, chatgptPlanChangeAccount, chatgptPlanCancelSignIn, chatgptPlanDisconnect, chatgptPlanModels, testModelConnection, type ChatgptPlanStatus, type ChatgptPlanModel } from '$lib/commands/settings';
import {
  ALL_PROVIDERS,
  getModelsForProvider,
  getProviderMeta,
  providerProtocolLabel,
} from '$lib/providers';
import { toRustConfig, useSettingsStore } from '$stores/settings';
import type { ApiConfig, ApiProvider } from '$types';
import { Button, Dialog, Input, Select } from '../../../ui';

type TestState = 'idle' | 'testing' | 'success' | 'error';

function credentialSummary(config: ApiConfig, requiresKey: boolean): string {
  if (config.authMode === 'chatgpt_plan') return config.planEnabled ? 'ChatGPT 套餐授权（无 API Key）' : '尚未授权 ChatGPT 套餐用量';
  if (config.authMode === 'none') return '显式免认证，无需 API Key';
  if (!requiresKey) return '本地服务，无需 API Key';
  if (config.credentialSource === 'environment') return '由环境变量提供';
  if (config.hasApiKey) return '已保存在系统凭据库';
  if (config.apiKey.trim()) return '已填写，保存后写入系统凭据库';
  return '尚未配置';
}

export function ApiSettings() {
  const { apiConfig, apiConfigLoaded, updateApiConfig, loadApiConfig } = useSettingsStore();
  const [showApiKey, setShowApiKey] = useState(false);
  const [testState, setTestState] = useState<TestState>('idle');
  const [testError, setTestError] = useState('');
  const [planStatus, setPlanStatus] = useState<ChatgptPlanStatus | null>(null);
  const [planModels, setPlanModels] = useState<ChatgptPlanModel[]>([]);
  const [planBusy, setPlanBusy] = useState<'idle' | 'checking' | 'signing-in' | 'disconnecting'>('idle');
  const [planError, setPlanError] = useState('');
  const [planNotice, setPlanNotice] = useState(false);
  const planOperation = useRef(0);
  const testOperation = useRef(0);
  const modelListId = useId();

  useEffect(() => {
    if (!apiConfigLoaded) void loadApiConfig();
  }, [apiConfigLoaded, loadApiConfig]);

  useEffect(() => () => { testOperation.current += 1; }, []);

  const provider = apiConfig.provider as ApiProvider;
  const meta = getProviderMeta(provider);
  const authMode = apiConfig.authMode ?? (meta.requiresKey ? 'api_key' : 'none');
  const isPlan = authMode === 'chatgpt_plan';
  const models = isPlan ? planModels.map((model) => ({ value: model.id, label: model.name })) : getModelsForProvider(provider);
  const hasCredentials = Boolean(apiConfig.apiKey.trim() || apiConfig.hasApiKey);
  const environmentManaged = apiConfig.credentialSource === 'environment';
  const endpointRequired = meta.endpointMode === 'required';
  const missingModel = !apiConfig.model.trim();
  const missingEndpoint = endpointRequired && !apiConfig.baseUrl.trim();
  const planReady = Boolean(planStatus?.connected && planStatus.planEnabled && !planStatus.needsSignIn && planStatus.credentialRef);
  const missingCredentials = isPlan ? !planReady : authMode === 'api_key' && !hasCredentials;
  const canTest = !missingModel && !missingEndpoint && !missingCredentials;

  const applyPlanStatus = (status: ChatgptPlanStatus) => {
    setPlanStatus(status);
    useSettingsStore.getState().invalidatePlanBinding(status.connected && status.planEnabled && !status.needsSignIn ? status.credentialRef : null);
    if (useSettingsStore.getState().apiConfig.authMode === 'chatgpt_plan') {
      updateApiConfig({ credentialRef: status.credentialRef, planConnected: status.connected, planEnabled: status.planEnabled && !status.needsSignIn, planAccountLabel: status.accountLabel });
    }
    resetTest();
  };

  useEffect(() => {
    if (!isPlan) return;
    let cancelled = false;
    setPlanBusy('checking');
    setPlanError('');
    void chatgptPlanStatus().then((status) => {
      if (!cancelled) applyPlanStatus(status);
    }).catch((error: unknown) => {
      if (!cancelled) {
        useSettingsStore.getState().invalidatePlanBinding(null);
        setPlanError(`无法读取 ChatGPT 连接状态：${String(error)}`);
      }
    }).finally(() => { if (!cancelled) setPlanBusy('idle'); });
    return () => { cancelled = true; planOperation.current += 1; };
    // A status read never starts OAuth or saves the model configuration.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isPlan]);

  useEffect(() => {
    setPlanModels([]);
    if (!isPlan || !planStatus?.connected || !planStatus.planEnabled || planStatus.needsSignIn) return;
    let cancelled = false;
    void chatgptPlanModels().then((models) => { if (!cancelled) setPlanModels(models); })
      .catch((error: unknown) => { if (!cancelled) setPlanError(`无法读取账号模型列表，可手动填写已授权模型：${String(error)}`); });
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
      catch { if (operation === planOperation.current) { setPlanStatus(null); useSettingsStore.getState().invalidatePlanBinding(null); updateApiConfig({ credentialRef: null, planConnected: false, planEnabled: false }); } }
    } finally { if (operation === planOperation.current) setPlanBusy('idle'); }
  };

  const resetTest = () => {
    testOperation.current += 1;
    setTestState('idle');
    setTestError('');
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
      credentialRef: null, planConnected: false, planEnabled: false, planAccountLabel: null,
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
    try {
      const success = await testModelConnection(toRustConfig({ ...apiConfig, baseUrl: apiConfig.baseUrl || meta.defaultBaseUrl }));
      if (!isCurrent()) { if (operation === testOperation.current) resetTest(); return; }
      setTestState(success ? 'success' : 'error');
      if (!success) setTestError('服务拒绝了连接，请核对模型、API 地址和凭据。');
    } catch (error) {
      if (!isCurrent()) { if (operation === testOperation.current) resetTest(); return; }
      setTestState('error');
      const detail = error instanceof Error ? error.message : String(error);
      setTestError(`连接测试失败：${detail}`);
    }
  };

  const statusText = testState === 'testing'
    ? '正在测试连接'
    : testState === 'success'
      ? '连接测试通过'
      : testState === 'error'
        ? '连接测试失败'
        : missingCredentials
          ? isPlan ? '需要 ChatGPT 套餐授权' : '需要 API Key'
          : missingModel
            ? '需要模型标识'
            : missingEndpoint
              ? '需要 API 地址'
              : '尚未测试';

  return (
    <div className="model-connection-settings">
      <section className="model-connection-overview" aria-labelledby="model-connection-overview-title">
        <div>
          <p className="model-connection-kicker">当前配置</p>
          <h3 id="model-connection-overview-title">{meta.name}</h3>
          <p>{apiConfig.model || '尚未选择模型'}</p>
        </div>
        <dl className="model-connection-facts">
          <div>
            <dt>凭据</dt>
            <dd>{credentialSummary(apiConfig, meta.requiresKey)}</dd>
          </div>
          <div>
            <dt>连接状态</dt>
            <dd data-state={testState}>{statusText}</dd>
          </div>
          <div>
            <dt>接入方式</dt>
            <dd>{providerProtocolLabel(provider, apiConfig.protocol)}</dd>
          </div>
          <div>
            <dt>运行位置</dt>
            <dd>{meta.isLocal ? '本机' : '外部模型服务'}</dd>
          </div>
        </dl>
      </section>

      <section className="model-config-section" aria-labelledby="model-provider-heading">
        <header>
          <h3 id="model-provider-heading">服务商与模型</h3>
          <p>选择当前默认模型；服务商新增的模型标识可直接填写。</p>
        </header>
        <div className="model-field-grid">
          <label className="model-field">
            <span>服务商</span>
            <Select value={provider} onChange={(event) => handleProviderChange(event.target.value as ApiProvider)}>
              {ALL_PROVIDERS.map((item) => (
                <option key={item.id} value={item.id}>{item.name}</option>
              ))}
            </Select>
          </label>
          {(provider === 'openai' || provider === 'custom') && (
            <label className="model-field">
              <span>认证方式</span>
              <Select value={authMode} onChange={(event) => {
                const nextMode = event.target.value as ApiConfig['authMode'];
                updateConnection(nextMode === 'chatgpt_plan'
                  ? { authMode: nextMode, protocol: 'openai_responses', baseUrl: 'https://api.openai.com/v1', apiKey: '', hasApiKey: false, credentialSource: 'none', credentialRef: planStatus?.credentialRef ?? null, planEnabled: Boolean(planStatus?.planEnabled), planConnected: Boolean(planStatus?.connected) }
                  : { authMode: nextMode, ...(nextMode === 'none' ? { apiKey: '', hasApiKey: false, credentialSource: 'none' as const } : {}), credentialRef: null, planEnabled: false, planConnected: false, planAccountLabel: null });
                setShowApiKey(false);
              }}>
                <option value="api_key">API Key（独立 API 计费）</option>
                {provider === 'openai' ? <option value="chatgpt_plan">ChatGPT 套餐</option> : <option value="none">免认证（显式）</option>}
              </Select>
            </label>
          )}
          {(provider === 'custom' || provider === 'openai') && (
            <label className="model-field">
              <span>协议</span>
              <Select value={apiConfig.protocol ?? 'openai_chat_completions'} disabled={isPlan}
                onChange={(event) => updateConnection({ protocol: event.target.value as ApiConfig['protocol'] })}>
                <option value="openai_chat_completions">OpenAI Chat Completions</option>
                <option value="openai_responses">OpenAI Responses</option>
                {provider === 'custom' && <option value="anthropic_messages">Anthropic Messages</option>}
              </Select>
            </label>
          )}
          <label className="model-field">
            <span>{provider === 'azure' ? '部署名称' : '模型标识'}</span>
            <Input
              type="text"
              list={models.length ? modelListId : undefined}
              aria-label={provider === 'azure' ? '部署名称' : '模型标识'}
              value={apiConfig.model}
              onChange={(event) => updateConnection({ model: event.target.value })}
              placeholder={provider === 'azure' ? '例如 production-gpt' : '输入或选择模型'}
              autoComplete="off"
            />
            {models.length > 0 && (
              <datalist id={modelListId}>
                {models.map((model) => <option key={model.value} value={model.value}>{model.label}</option>)}
              </datalist>
            )}
            <small>{isPlan ? '列表来自当前授权账号，也可手动填写该账号允许的模型标识。' : '支持直接填写服务商新增的模型标识。'}</small>
          </label>
        </div>
      </section>

      {isPlan && (
        <section className="model-config-section" aria-labelledby="chatgpt-plan-heading">
          <header><h3 id="chatgpt-plan-heading">使用 ChatGPT 套餐</h3><p>符合条件的请求使用共享套餐额度或可用 credits；不等于无限用量，也不会自动切换付费 API。</p></header>
          <p role="status">{planBusy === 'checking' ? '正在读取账号状态…' : planStatus?.connected ? `${planStatus.accountLabel || 'ChatGPT 账号'} · ${planStatus.planEnabled && !planStatus.needsSignIn ? '已授权套餐用量' : '需要重新授权套餐用量'}` : '尚未连接 ChatGPT 账号'}</p>
          <div className="model-field-inline">
            <Button variant="primary" busy={planBusy === 'signing-in'} disabled={planBusy !== 'idle'} onClick={() => { void handlePlanSignIn(); }}>Continue with ChatGPT</Button>
            {planStatus?.connected && <Button variant="secondary" disabled={planBusy !== 'idle'} onClick={() => { void handlePlanSignIn(true); }}>更换账号</Button>}
            {planBusy === 'signing-in' && <Button onClick={() => { void handlePlanCancel(); }}>取消登录</Button>}
            {planStatus?.connected && <Button variant="secondary" disabled={planBusy !== 'idle'} onClick={() => { void handlePlanDisconnect(); }}>断开 ChatGPT 连接</Button>}
          </div>
          <small>仅点击登录或更换账号才会打开授权页面。当前仅连接一个账号；更换成功后仍需“保存”才会使用新配置，取消不会断开原账号。可在 ChatGPT 设置 → Usage 管理共享额度。</small>
          {planError && <p className="model-test-error" role="alert">{planError}</p>}
          <Dialog open={planNotice} title="ChatGPT 套餐授权说明" onClose={dismissPlanNotice}>
            <p>ChatGPT 套餐用量已授权。保存此配置后，符合条件的 AI 请求将使用套餐额度或可用 credits；额度可在 ChatGPT 设置 → Usage 管理。</p>
            <Button onClick={dismissPlanNotice}>知道了</Button>
          </Dialog>
        </section>
      )}

      <section className="model-config-section" aria-labelledby="model-connection-heading">
        <header>
          <h3 id="model-connection-heading">连接信息</h3>
          <p>测试使用当前表单内容；确认无误后仍需点击右上角“保存”。</p>
        </header>

        <div className="model-field">
          <label htmlFor="model-api-endpoint">API 地址</label>
          <div className="model-field-inline">
            <Input
              id="model-api-endpoint"
              type="url"
              value={apiConfig.baseUrl}
              onChange={(event) => updateConnection({ baseUrl: event.target.value })}
              placeholder={meta.defaultBaseUrl || 'https://example.com/v1'}
              aria-describedby="model-endpoint-help"
              readOnly={isPlan}
            />
            {!isPlan && meta.defaultBaseUrl && apiConfig.baseUrl !== meta.defaultBaseUrl && (
              <Button variant="secondary" className="model-secondary-action" onClick={() => updateConnection({ baseUrl: meta.defaultBaseUrl })}>
                恢复默认
              </Button>
            )}
          </div>
          <small id="model-endpoint-help">
            {isPlan ? '套餐凭据仅发送至官方 https://api.openai.com/v1/responses，不支持自定义网关。' : provider === 'azure'
              ? '填写资源的 OpenAI v1 端点，例如 https://resource.openai.azure.com/openai/v1。'
              : meta.isLocal
                ? '填写本地服务的 OpenAI 兼容地址。'
                : '通常无需修改；只在使用代理网关或兼容端点时覆盖。'}
          </small>
        </div>

        {authMode === 'api_key' ? (
          <div className="model-field">
            <label htmlFor="model-api-key">API Key</label>
            <div className="model-field-inline">
              <Input
                id="model-api-key"
                type={showApiKey ? 'text' : 'password'}
                value={apiConfig.apiKey}
                onChange={(event) => updateConnection({ apiKey: event.target.value })}
                placeholder={environmentManaged
                  ? `${meta.envKey} 已生效`
                  : apiConfig.hasApiKey ? '已安全保存；留空则保留原值' : meta.envKey || '输入 API Key'}
                disabled={environmentManaged}
                autoComplete="off"
                aria-describedby="model-key-help"
              />
              <Button
                type="button"
                variant="secondary"
                className="model-secondary-action model-key-visibility"
                onClick={() => setShowApiKey((visible) => !visible)}
                disabled={environmentManaged}
                aria-label={showApiKey ? '隐藏 API Key' : '显示 API Key'}
              >
                {showApiKey ? '隐藏' : '显示'}
              </Button>
            </div>
            <small id="model-key-help">
              {environmentManaged
                ? `当前由环境变量 ${meta.envKey} 管理；请在系统环境中修改。`
                : apiConfig.hasApiKey && !apiConfig.apiKey
                  ? '现有密钥不会显示；只有填写新值并保存时才会替换。'
                  : '保存后仅写入系统凭据库，不写入项目或普通配置文件。'}
            </small>
          </div>
        ) : isPlan ? <p className="model-local-note">OAuth 凭据仅在后端系统凭据库管理，界面不展示或接收 API Key。</p> : (
          <p className="model-local-note">{meta.isLocal ? '此服务商不需要 API Key。请先确保本地服务已经启动。' : '此连接已显式设为免认证，不会发送 API Key。'}</p>
        )}
      </section>

      {!isPlan && <details className="model-advanced-settings">
        <summary>生成参数</summary>
        <p>这些参数会影响回复长度和随机性。不了解时保留默认值即可。</p>
        <div className="model-field-grid">
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
            <span>Temperature</span>
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
      </details>}

      <section className="model-connection-test" aria-label="连接测试">
        <div aria-live="polite">
          <strong>{statusText}</strong>
          <span>{testState === 'success' ? '当前表单可以连接到模型服务。' : '测试会发送一条最小验证请求，可能消耗少量额度；不发送历史对话，也不保存配置。'}</span>
        </div>
        <Button
          type="button"
          variant="primary"
          className="model-test-action"
          onClick={() => { void handleTestConnection(); }}
          disabled={testState === 'testing' || !canTest}
          busy={testState === 'testing'}
        >
          {testState === 'testing' ? '测试中…' : '测试连接'}
        </Button>
      </section>
      {testState === 'error' && <p className="model-test-error" role="alert">{testError}</p>}
    </div>
  );
}

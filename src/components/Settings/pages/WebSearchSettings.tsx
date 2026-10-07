import { useEffect, useState } from 'react';
import {
  clearWebSearchConfig,
  configureWebSearch,
  getWebSearchConfig,
  setWebSearchEnabled,
  type WebSearchConfig,
  type WebSearchProvider,
} from '$lib/commands/web-search';
import { ToggleField } from '../components/ToggleField';

type SaveState = 'idle' | 'saving' | 'saved' | 'error';

interface ProviderDefinition {
  id: WebSearchProvider;
  label: string;
  trustedEndpoint: string | null;
}

const PROVIDERS: ProviderDefinition[] = [
  { id: 'tavily', label: 'Tavily', trustedEndpoint: 'https://api.tavily.com/search' },
  { id: 'brave', label: 'Brave Search', trustedEndpoint: 'https://api.search.brave.com/res/v1/web/search' },
  { id: 'exa', label: 'Exa', trustedEndpoint: 'https://api.exa.ai/search' },
  { id: 'searxng', label: 'SearXNG（自托管）', trustedEndpoint: null },
];

function providerDefinition(provider: WebSearchProvider): ProviderDefinition {
  return PROVIDERS.find((item) => item.id === provider) ?? PROVIDERS[0];
}

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}

export function WebSearchSettings() {
  const [config, setConfig] = useState<WebSearchConfig | null>(null);
  const [provider, setProvider] = useState<WebSearchProvider>('tavily');
  const [apiKey, setApiKey] = useState('');
  const [apiKeyConfigured, setApiKeyConfigured] = useState(false);
  const [endpoint, setEndpoint] = useState('');
  const [loading, setLoading] = useState(true);
  const [saveState, setSaveState] = useState<SaveState>('idle');
  const [toggling, setToggling] = useState(false);
  const [removeConfirmation, setRemoveConfirmation] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [removed, setRemoved] = useState(false);
  const [showApiKey, setShowApiKey] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const selectedProvider = providerDefinition(provider);
  const isSelfHosted = provider === 'searxng';
  const savedProvider = config ? providerDefinition(config.provider) : null;
  const toggleMatchesSavedConfig = Boolean(
    config
      && config.provider === provider
      && (provider !== 'searxng' || config.endpoint === endpoint.trim()),
  );
  const canSave = !loading
    && !toggling
    && (isSelfHosted ? Boolean(endpoint.trim()) : Boolean(apiKey.trim()));

  useEffect(() => {
    let current = true;

    const load = async () => {
      try {
        const nextConfig = await getWebSearchConfig();
        if (!current) return;

        setConfig(nextConfig);
        if (nextConfig) {
          setProvider(nextConfig.provider);
          setEndpoint(nextConfig.endpoint);
          setApiKeyConfigured(nextConfig.apiKeyConfigured);
        }
      } catch (reason) {
        if (current) setError(`无法读取联网搜索配置：${errorMessage(reason)}`);
      } finally {
        if (current) setLoading(false);
      }
    };

    void load();
    return () => { current = false; };
  }, []);

  const handleProviderChange = (nextProvider: WebSearchProvider) => {
    const nextDefinition = providerDefinition(nextProvider);
    const usesSavedProvider = config?.provider === nextProvider;
    const nextEndpoint = nextProvider === 'searxng'
      ? (usesSavedProvider ? config?.endpoint ?? '' : '')
      : nextDefinition.trustedEndpoint ?? '';

    setProvider(nextProvider);
    setEndpoint(nextEndpoint);
    setApiKey('');
    setApiKeyConfigured(Boolean(usesSavedProvider && config?.apiKeyConfigured));
    setShowApiKey(false);
    setSaveState('idle');
    setRemoved(false);
    setError(null);

  };

  const handleSave = async () => {
    if (!canSave) return;

    setSaveState('saving');
    setError(null);
    try {
      const nextConfig = await configureWebSearch(
        isSelfHosted
          ? { provider, apiKey: '', endpoint: endpoint.trim() }
          : { provider, apiKey: apiKey.trim() },
      );
      setConfig(nextConfig);
      setProvider(nextConfig.provider);
      setEndpoint(nextConfig.endpoint);
      setApiKey('');
      setApiKeyConfigured(nextConfig.apiKeyConfigured);
      setSaveState('saved');
      setRemoved(false);
    } catch (reason) {
      setSaveState('error');
      setError(`未能保存联网搜索配置：${errorMessage(reason)}`);
    }
  };

  const handleEnabledChange = async (enabled: boolean) => {
    if (!config || !toggleMatchesSavedConfig) return;

    setToggling(true);
    setError(null);
    try {
      const nextConfig = await setWebSearchEnabled(enabled);
      setConfig(nextConfig);
      setProvider(nextConfig.provider);
      setEndpoint(nextConfig.endpoint);
      setApiKeyConfigured(nextConfig.apiKeyConfigured);
    } catch (reason) {
      setError(`未能更新联网搜索状态：${errorMessage(reason)}`);
    } finally {
      setToggling(false);
    }
  };

  const handleRemove = async () => {
    if (!config) return;

    setRemoving(true);
    setError(null);
    try {
      await clearWebSearchConfig();
      setConfig(null);
      setProvider('tavily');
      setEndpoint('');
      setApiKey('');
      setApiKeyConfigured(false);
      setShowApiKey(false);
      setSaveState('idle');
      setRemoveConfirmation(false);
      setRemoved(true);
    } catch (reason) {
      setError(`未能移除联网搜索配置：${errorMessage(reason)}`);
    } finally {
      setRemoving(false);
    }
  };

  return (
    <div className="settings-page-content web-search-settings">
      <section className="web-search-section web-search-intro" aria-labelledby="web-search-overview-title">
        <p className="web-search-eyebrow">WEB SEARCH</p>
        <h3 id="web-search-overview-title">按需联网搜索</h3>
        <p>为 AngelBot 单独配置搜索服务。它不会继承模型 API 的地址或凭据。</p>
        <dl className="web-search-status">
          <div>
            <dt>当前服务</dt>
            <dd>{savedProvider?.label ?? '尚未配置'}</dd>
          </div>
          <div>
            <dt>可用状态</dt>
            <dd data-enabled={config?.enabled ? 'true' : 'false'}>
              {config?.enabled ? '已启用' : config ? '已关闭' : '尚未配置'}
            </dd>
          </div>
        </dl>
      </section>

      <section className="web-search-section" aria-labelledby="web-search-provider-heading">
        <header className="web-search-section-header">
          <h3 id="web-search-provider-heading">服务商与连接</h3>
          <p>保存配置会启用联网搜索；之后可随时在下方关闭。</p>
        </header>

        <div className="web-search-field">
          <label htmlFor="web-search-provider">搜索服务商</label>
          <select
            id="web-search-provider"
            value={provider}
            disabled={loading || toggling || saveState === 'saving'}
            onChange={(event) => handleProviderChange(event.target.value as WebSearchProvider)}
          >
            {PROVIDERS.map((item) => <option key={item.id} value={item.id}>{item.label}</option>)}
          </select>
        </div>

        {isSelfHosted ? (
          <div className="web-search-field">
            <label htmlFor="web-search-searxng-endpoint">SearXNG 服务地址</label>
            <input
              id="web-search-searxng-endpoint"
              type="url"
              value={endpoint}
              placeholder="https://search.example.com"
              autoComplete="url"
              disabled={loading || toggling || saveState === 'saving'}
              onChange={(event) => {
                setEndpoint(event.target.value);
                setSaveState('idle');
              }}
              aria-describedby="web-search-searxng-help"
            />
            <small id="web-search-searxng-help">使用你的自托管 SearXNG 的完整 http(s) 地址；不需要 API Key。</small>
          </div>
        ) : (
          <>
            <div className="web-search-field">
              <label htmlFor="web-search-trusted-endpoint">受信任的服务地址</label>
              <input id="web-search-trusted-endpoint" value={selectedProvider.trustedEndpoint ?? ''} readOnly />
              <small>此地址由 AngelBot 固定，避免 API Key 被发送到自定义或未知服务。</small>
            </div>
            <div className="web-search-field">
              <label htmlFor="web-search-api-key">API Key</label>
              <div className="web-search-inline-field">
                <input
                  id="web-search-api-key"
                  type={showApiKey ? 'text' : 'password'}
                  value={apiKey}
                  placeholder={apiKeyConfigured ? '已安全保存；重新保存时请输入 API Key' : '输入 API Key'}
                  autoComplete="new-password"
                  disabled={loading || toggling || saveState === 'saving'}
                  onChange={(event) => {
                    setApiKey(event.target.value);
                    setSaveState('idle');
                  }}
                  aria-describedby="web-search-key-help"
                />
                <button
                  type="button"
                  className="web-search-secondary-action"
                  disabled={loading || toggling || saveState === 'saving'}
                  onClick={() => setShowApiKey((visible) => !visible)}
                  aria-label={showApiKey ? '隐藏 API Key' : '显示 API Key'}
                >
                  {showApiKey ? '隐藏' : '显示'}
                </button>
              </div>
              <small id="web-search-key-help">
                {apiKeyConfigured
                  ? '已保存的密钥不会显示。出于安全考虑，保存这个服务商时需要再次输入 API Key。'
                  : '保存后密钥仅写入系统凭据库，不会写入项目、对话或普通配置文件。'}
              </small>
            </div>
          </>
        )}

        <div className="web-search-actions">
          <button
            type="button"
            className="web-search-save-action"
            disabled={!canSave || saveState === 'saving'}
            onClick={() => { void handleSave(); }}
          >
            {saveState === 'saving' ? '保存中…' : saveState === 'saved' ? '已保存配置' : '保存搜索配置'}
          </button>
          {saveState === 'error' && <span className="web-search-action-error" role="alert">保存失败</span>}
        </div>
      </section>

      <section className="web-search-section" aria-labelledby="web-search-access-heading">
        <header className="web-search-section-header">
          <h3 id="web-search-access-heading">访问控制</h3>
          <p>这个开关只影响联网搜索，不会改变模型 API 或其他工具的权限。</p>
        </header>
        <ToggleField
          label="允许使用联网搜索"
          description={loading
            ? '正在读取当前配置…'
            : !config
              ? '请先保存一个搜索服务商。'
              : !toggleMatchesSavedConfig
                ? '请先保存服务商或 SearXNG 地址的更改。'
                : config.enabled
                  ? '关闭后，主智能体不能使用这个搜索服务。'
                  : '开启后，主智能体可以在获准时使用这个搜索服务。'}
          checked={config?.enabled ?? false}
          disabled={loading || toggling || saveState === 'saving' || !toggleMatchesSavedConfig}
          onChange={(enabled) => { void handleEnabledChange(enabled); }}
        />
        <div className="web-search-remove" aria-live="polite">
          <div>
            <h4>移除当前配置</h4>
            <p>会删除当前服务的密钥和本地配置；之后需要重新配置才能联网搜索。</p>
          </div>
          {removeConfirmation ? (
            <div className="web-search-remove-actions">
              <button
                type="button"
                className="web-search-secondary-action"
                disabled={removing}
                onClick={() => setRemoveConfirmation(false)}
              >
                取消
              </button>
              <button
                type="button"
                className="web-search-danger-action"
                disabled={removing}
                onClick={() => { void handleRemove(); }}
              >
                {removing ? '正在移除…' : '确认移除配置'}
              </button>
            </div>
          ) : (
            <button
              type="button"
              className="web-search-danger-action"
              disabled={!config || loading || toggling || saveState === 'saving'}
              onClick={() => {
                setRemoveConfirmation(true);
                setRemoved(false);
                setError(null);
              }}
            >
              移除配置
            </button>
          )}
          {removed && <span className="web-search-remove-success">已移除当前联网搜索配置。</span>}
        </div>
      </section>

      <section className="web-search-section web-search-privacy" aria-labelledby="web-search-privacy-heading">
        <h3 id="web-search-privacy-heading">隐私与批准</h3>
        <ul>
          <li>密钥只保存在操作系统凭据库，设置页面不会回显它。</li>
          <li>Tavily、Brave Search 和 Exa 只能使用上方显示的受信任地址；只有无密钥的 SearXNG 可以指定自托管地址。</li>
          <li>关闭开关会撤销主智能体的搜索能力。开启后，实际联网请求仍遵循当前任务的批准和权限范围。</li>
        </ul>
      </section>

      {error && <p className="web-search-error" role="alert">{error}</p>}
    </div>
  );
}

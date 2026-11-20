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
import { SettingsRow, SettingsSection } from '../components/SettingsSection';
import { Button, Input, Select } from '../../../ui';

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
      <SettingsSection title="服务商与连接" status={loading ? '读取中…' : config && savedProvider
        ? `${savedProvider.label} · ${config.enabled ? '已启用' : '已关闭'}` : '尚未配置'}>
        <div className="settings-rows web-search-form">
          <div className="web-search-field">
            <label htmlFor="web-search-provider">搜索服务商</label>
            <Select
              id="web-search-provider"
              value={provider}
              disabled={loading || toggling || saveState === 'saving'}
              onChange={(event) => handleProviderChange(event.target.value as WebSearchProvider)}
            >
              {PROVIDERS.map((item) => <option key={item.id} value={item.id}>{item.label}</option>)}
            </Select>
          </div>

          {isSelfHosted ? (
            <div className="web-search-field">
              <label htmlFor="web-search-searxng-endpoint">SearXNG 服务地址</label>
              <Input
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
              <small id="web-search-searxng-help">填写完整的 HTTP(S) 地址，不需要 API Key。</small>
            </div>
          ) : (
            <>
              <div className="web-search-field">
                <label htmlFor="web-search-trusted-endpoint">受信任的服务地址</label>
                <Input id="web-search-trusted-endpoint" value={selectedProvider.trustedEndpoint ?? ''} readOnly />
              </div>
              <div className="web-search-field">
                <label htmlFor="web-search-api-key">API Key</label>
                <div className="web-search-inline-field">
                  <Input
                    id="web-search-api-key"
                    type={showApiKey ? 'text' : 'password'}
                    value={apiKey}
                    placeholder={apiKeyConfigured ? '已保存 · 输入新密钥' : '输入 API Key'}
                    autoComplete="new-password"
                    disabled={loading || toggling || saveState === 'saving'}
                    onChange={(event) => {
                      setApiKey(event.target.value);
                      setSaveState('idle');
                    }}
                    aria-describedby="web-search-key-help"
                  />
                  <Button
                    disabled={loading || toggling || saveState === 'saving'}
                    onClick={() => setShowApiKey((visible) => !visible)}
                    aria-label={showApiKey ? '隐藏 API Key' : '显示 API Key'}
                  >
                    {showApiKey ? '隐藏' : '显示'}
                  </Button>
                </div>
                <small id="web-search-key-help">
                  {apiKeyConfigured
                    ? '重新保存时需输入密钥。'
                    : '密钥仅保存在系统凭据库。'}
                </small>
              </div>
            </>
          )}

          <div className="settings-action-line web-search-actions" aria-live="polite">
            <p className="settings-muted">保存时启用联网搜索。</p>
            <Button
              variant="primary"
              busy={saveState === 'saving'}
              disabled={!canSave}
              onClick={() => { void handleSave(); }}
            >
              {saveState === 'saving' ? '保存中…' : saveState === 'saved' ? '已保存配置' : '保存搜索配置'}
            </Button>
          </div>
        </div>
        {error && saveState === 'error' && <p className="web-search-error" role="alert">{error}</p>}
      </SettingsSection>

      <SettingsSection title="访问控制">
        <div className="settings-rows">
          <ToggleField
            label="允许使用联网搜索"
            description={loading
              ? '正在读取当前配置…'
              : !config
                ? '请先保存一个搜索服务商。'
                : !toggleMatchesSavedConfig
                  ? '请先保存服务商或 SearXNG 地址的更改。'
                  : '搜索时按任务权限执行。'}
            checked={config?.enabled ?? false}
            disabled={loading || toggling || saveState === 'saving' || !toggleMatchesSavedConfig}
            onChange={(enabled) => { void handleEnabledChange(enabled); }}
          />
          <SettingsRow title="移除当前配置" description={removed
            ? <p className="settings-row-description" role="status">已移除当前联网搜索配置。</p>
            : '删除服务配置和密钥。'}>
            {removeConfirmation ? (
              <>
                <Button
                  size="small"
                  disabled={removing}
                  onClick={() => setRemoveConfirmation(false)}
                >
                  取消
                </Button>
                <Button
                  size="small"
                  variant="danger"
                  busy={removing}
                  onClick={() => { void handleRemove(); }}
                >
                  {removing ? '正在移除…' : '确认移除配置'}
                </Button>
              </>
            ) : (
              <Button
                size="small"
                disabled={!config || loading || toggling || saveState === 'saving'}
                onClick={() => {
                  setRemoveConfirmation(true);
                  setRemoved(false);
                  setError(null);
                }}
              >
                移除配置
              </Button>
            )}
          </SettingsRow>
        </div>
        {error && saveState !== 'error' && <p className="web-search-error" role="alert">{error}</p>}
      </SettingsSection>
    </div>
  );
}

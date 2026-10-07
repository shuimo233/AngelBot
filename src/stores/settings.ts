import { create } from 'zustand';
import type { Persona, ApiConfig } from '$types';
import { loadApiConfig as loadApiConfigCmd, saveApiConfig as saveApiConfigCmd, type ApiProviderConfig } from '$lib/commands/settings';
import { getProviderMeta } from '$lib/providers';

interface SettingsState {
  profile: Persona;
  /** Settings form draft; never use this to label or bind running conversations. */
  apiConfig: ApiConfig;
  activeApiConfig: ApiConfig;
  apiConfigLoaded: boolean;
  apiConfigLoadError: string | null;
  loadProfile: () => void;
  updateProfile: (profile: Partial<Persona>) => void;
  persistProfile: () => void;
  loadApiConfig: () => Promise<void>;
  updateApiConfig: (config: Partial<ApiConfig>) => void;
  invalidatePlanBinding: (credentialRef: string | null) => void;
  persistApiConfig: () => Promise<void>;
}

const STORAGE_KEY_PROFILE = 'angelbot_profile';

const defaultProfile: Persona = {
  name: '',
  avatar: '',
  bio: '',
  languageStyle: 'casual',
  tone: 'friendly',
  responseFormats: ['text'],
  keywords: [],
  greeting: '',
  personality: 'balanced',
  speechBubble: 'default',
  traits: {
    tone: 0,
    verbosity: 0,
    formality: 0,
    humor: 0,
    dependence: 0,
    intimacy: 0,
    patience: 5,
  },
};

const defaultApiConfig: ApiConfig = {
  provider: 'anthropic',
  model: 'claude-sonnet-4-20250514',
  baseUrl: 'https://api.anthropic.com',
  apiKey: '',
  hasApiKey: false,
  credentialSource: 'none',
  maxTokens: 4096,
  temperature: 0.7,
};

function loadFromStorage<T>(key: string, fallback: T): T {
  try {
    const stored = localStorage.getItem(key);
    if (stored) {
      return { ...fallback, ...JSON.parse(stored) };
    }
  } catch {
    // ignore
  }
  return fallback;
}

// Map Rust snake_case config → TS camelCase
export function fromRustConfig(rust: ApiProviderConfig): ApiConfig {
  return {
    provider: rust.provider as ApiConfig['provider'],
    model: rust.model,
    baseUrl: rust.base_url,
    apiKey: rust.api_key,
    hasApiKey: Boolean(rust.has_api_key),
    credentialSource: rust.credential_source ?? 'none',
    protocol: rust.protocol,
    authMode: rust.auth_mode,
    credentialRef: rust.credential_ref,
    planConnected: rust.plan_connected,
    planEnabled: rust.plan_enabled,
    planAccountLabel: rust.plan_account_label,
    maxTokens: rust.max_tokens,
    temperature: rust.temperature,
  };
}

// Map TS camelCase config → Rust snake_case
export function toRustConfig(ts: ApiConfig): ApiProviderConfig {
  return {
    provider: ts.provider,
    model: ts.model,
    base_url: ts.baseUrl,
    api_key: ts.authMode === 'chatgpt_plan' || ts.authMode === 'none' ? '' : ts.apiKey,
    has_api_key: ts.authMode === 'chatgpt_plan' || ts.authMode === 'none' ? false : Boolean(ts.hasApiKey),
    credential_source: ts.authMode === 'chatgpt_plan' || ts.authMode === 'none' ? 'none' : ts.credentialSource ?? 'none',
    protocol: ts.protocol,
    auth_mode: ts.authMode,
    credential_ref: ts.credentialRef,
    plan_connected: ts.planConnected,
    plan_enabled: ts.planEnabled,
    plan_account_label: ts.planAccountLabel,
    max_tokens: ts.maxTokens,
    temperature: ts.temperature,
  };
}

/** Compare public connection identity, never key values or backend-managed key metadata. */
function matchesSavedConnection(saved: ApiConfig, effective: ApiConfig): boolean {
  const protocol = (config: ApiConfig) => config.protocol
    ?? (getProviderMeta(config.provider)?.protocol === 'anthropic-messages'
      ? 'anthropic_messages' : 'openai_chat_completions');
  const authMode = saved.authMode ?? 'api_key';
  return saved.provider.trim() === effective.provider.trim()
    && saved.model.trim() === effective.model.trim()
    && saved.baseUrl.trim() === effective.baseUrl.trim()
    && protocol(saved) === protocol(effective)
    && authMode === (effective.authMode ?? 'api_key')
    && saved.maxTokens === effective.maxTokens
    && saved.temperature === effective.temperature
    && (authMode !== 'chatgpt_plan'
      || (saved.credentialRef === effective.credentialRef && Boolean(effective.planConnected && effective.planEnabled)));
}

let apiConfigRequest = 0;

export const useSettingsStore = create<SettingsState>((set, get) => ({
  profile: loadFromStorage(STORAGE_KEY_PROFILE, defaultProfile),
  apiConfig: defaultApiConfig,
  activeApiConfig: defaultApiConfig,
  apiConfigLoaded: false,
  apiConfigLoadError: null,

  loadProfile: () => {
    set({ profile: loadFromStorage(STORAGE_KEY_PROFILE, defaultProfile) });
  },

  updateProfile: (partial) => {
    set((state) => ({ profile: { ...state.profile, ...partial } }));
  },

  persistProfile: () => {
    const { profile } = get();
    localStorage.setItem(STORAGE_KEY_PROFILE, JSON.stringify(profile));
  },

  loadApiConfig: async () => {
    const request = ++apiConfigRequest;
    const draft = get().apiConfig;
    try {
      const rustConfig = await loadApiConfigCmd();
      if (request !== apiConfigRequest) return;
      const apiConfig = fromRustConfig(rustConfig);
      set({
        ...(get().apiConfig === draft ? { apiConfig } : {}),
        activeApiConfig: apiConfig,
        apiConfigLoaded: true,
        apiConfigLoadError: null,
      });
    } catch (error) {
      if (request !== apiConfigRequest) return;
      set({
        apiConfigLoaded: true,
        apiConfigLoadError: error instanceof Error ? error.message : '无法读取模型配置',
      });
    }
  },

  updateApiConfig: (partial) => {
    set((state) => ({ apiConfig: { ...state.apiConfig, ...partial } }));
  },

  invalidatePlanBinding: (credentialRef) => {
    const active = get().activeApiConfig;
    if (active.authMode === 'chatgpt_plan' && (!credentialRef || active.credentialRef !== credentialRef)) {
      set({ activeApiConfig: { ...active, planConnected: false, planEnabled: false } });
    }
  },

  persistApiConfig: async () => {
    const { apiConfig } = get();
    const request = ++apiConfigRequest;
    set({ apiConfigLoadError: null });
    await saveApiConfigCmd(toRustConfig(apiConfig));
    if (request !== apiConfigRequest) throw new Error('保存确认已被较新的配置读取取代，请重试。');
    // Persistence succeeded; this provisional identity is not confirmed effective
    // until readback succeeds (environment overrides may select another profile).
    const usesApiKey = apiConfig.authMode !== 'chatgpt_plan' && apiConfig.authMode !== 'none';
    set({ activeApiConfig: { ...apiConfig, apiKey: '', hasApiKey: usesApiKey && Boolean(apiConfig.hasApiKey || apiConfig.apiKey.trim()) } });
    let effective: ApiConfig;
    try {
      effective = fromRustConfig(await loadApiConfigCmd());
    } catch {
      if (request !== apiConfigRequest) throw new Error('保存确认已被较新的配置读取取代，请重试。');
      const message = '配置已写入，但无法读取生效配置；请重新检查后重试。';
      set({ apiConfigLoaded: true, apiConfigLoadError: message });
      throw new Error(message);
    }
    if (request !== apiConfigRequest) throw new Error('保存确认已被较新的配置读取取代，请重试。');
    if (!matchesSavedConnection(apiConfig, effective)) {
      const message = '配置已写入，但生效配置与本次保存不一致；请检查环境变量覆盖及账号授权后重试。';
      set({ activeApiConfig: effective, apiConfigLoaded: true, apiConfigLoadError: message });
      throw new Error(message);
    }
    set({
      ...(get().apiConfig === apiConfig ? { apiConfig: effective } : {}),
      activeApiConfig: effective,
      apiConfigLoaded: true,
      apiConfigLoadError: null,
    });
  },
}));

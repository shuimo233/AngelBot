import { beforeEach, describe, expect, it, vi } from 'vitest';
import { loadApiConfig, saveApiConfig } from '$lib/commands/settings';
import { fromRustConfig, toRustConfig, useSettingsStore } from './settings';

vi.mock('$lib/commands/settings', () => ({ loadApiConfig: vi.fn(), saveApiConfig: vi.fn() }));

const legacy = { provider: 'openai', model: 'gpt-4.1', base_url: 'https://api.openai.com/v1', api_key: '', has_api_key: true, max_tokens: 4096, temperature: 0.7 };
const plan = { ...legacy, model: 'authorized-model', protocol: 'openai_responses' as const, auth_mode: 'chatgpt_plan' as const, credential_ref: 'opaque-account', plan_connected: true, plan_enabled: true, has_api_key: false };

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

describe('model configuration IPC mapping', () => {
  it('preserves implicit legacy protocol and authentication behavior', () => {
    const config = fromRustConfig(legacy);
    expect(config.protocol).toBeUndefined();
    expect(config.authMode).toBeUndefined();
    expect(toRustConfig(config)).toMatchObject(legacy);
  });

  it('round-trips plan metadata without exposing API keys or tokens', () => {
    const rust = { ...legacy, protocol: 'openai_responses' as const, auth_mode: 'chatgpt_plan' as const, credential_ref: 'opaque-account', plan_connected: true, plan_enabled: true, plan_account_label: 'test-account', has_api_key: false };
    const config = fromRustConfig(rust);
    expect(config).toMatchObject({ protocol: 'openai_responses', authMode: 'chatgpt_plan', credentialRef: 'opaque-account', planEnabled: true });
    expect(toRustConfig(config)).toMatchObject(rust);
    expect(toRustConfig({ ...config, apiKey: 'must-not-forward', hasApiKey: true })).toMatchObject({ api_key: '', has_api_key: false, credential_source: 'none' });
  });

  it('omits credentials for explicitly unauthenticated connections', () => {
    expect(toRustConfig({ ...fromRustConfig(legacy), authMode: 'none', apiKey: 'must-not-forward' })).toMatchObject({ api_key: '', has_api_key: false });
  });
});

describe('saved runtime configuration versus settings draft', () => {
  beforeEach(() => {
    vi.mocked(loadApiConfig).mockReset();
    vi.mocked(saveApiConfig).mockReset().mockResolvedValue(undefined);
    const saved = fromRustConfig(legacy);
    useSettingsStore.setState({ apiConfig: saved, activeApiConfig: saved, apiConfigLoaded: true, apiConfigLoadError: null });
  });

  it('keeps an authorized plan draft from changing the running API mode or model', () => {
    const saved = useSettingsStore.getState().activeApiConfig;
    useSettingsStore.getState().updateApiConfig(fromRustConfig(plan));
    expect(useSettingsStore.getState().apiConfig.authMode).toBe('chatgpt_plan');
    expect(useSettingsStore.getState().activeApiConfig).toBe(saved);
    expect(useSettingsStore.getState().activeApiConfig.model).toBe('gpt-4.1');
    expect(saveApiConfig).not.toHaveBeenCalled();
  });

  it('commits both configurations only after a successful backend load', async () => {
    vi.mocked(loadApiConfig).mockResolvedValue(plan);
    await useSettingsStore.getState().loadApiConfig();
    expect(useSettingsStore.getState().activeApiConfig).toEqual(fromRustConfig(plan));
    expect(useSettingsStore.getState().apiConfig).toBe(useSettingsStore.getState().activeApiConfig);
  });

  it('preserves the saved billing mode when saving the draft fails', async () => {
    const saved = useSettingsStore.getState().activeApiConfig;
    useSettingsStore.getState().updateApiConfig(fromRustConfig(plan));
    vi.mocked(saveApiConfig).mockRejectedValue(new Error('offline fixture save failed'));
    await expect(useSettingsStore.getState().persistApiConfig()).rejects.toThrow('save failed');
    expect(useSettingsStore.getState().activeApiConfig).toBe(saved);
    expect(loadApiConfig).not.toHaveBeenCalled();
  });

  it('commits the saved plan after successful persistence and normalization', async () => {
    useSettingsStore.getState().updateApiConfig(fromRustConfig(plan));
    vi.mocked(loadApiConfig).mockResolvedValue(plan);
    await useSettingsStore.getState().persistApiConfig();
    expect(saveApiConfig).toHaveBeenCalledWith(expect.objectContaining({ auth_mode: 'chatgpt_plan', credential_ref: 'opaque-account' }));
    expect(useSettingsStore.getState().activeApiConfig).toEqual(fromRustConfig(plan));
  });

  it('does not display the old API mode if save succeeds but readback fails', async () => {
    useSettingsStore.getState().updateApiConfig(fromRustConfig(plan));
    vi.mocked(loadApiConfig).mockRejectedValue(new Error('offline fixture read failed'));
    await expect(useSettingsStore.getState().persistApiConfig()).rejects.toThrow('读取');
    expect(useSettingsStore.getState().activeApiConfig).toMatchObject({ authMode: 'chatgpt_plan', credentialRef: 'opaque-account', model: 'authorized-model', apiKey: '' });
    expect(useSettingsStore.getState().apiConfig).toEqual(fromRustConfig(plan));
    expect(useSettingsStore.getState().apiConfigLoadError).toContain('读取');
  });

  it('rejects a save whose effective readback has a different connection', async () => {
    const draft = fromRustConfig(plan);
    useSettingsStore.getState().updateApiConfig(draft);
    vi.mocked(loadApiConfig).mockResolvedValue(legacy);
    await expect(useSettingsStore.getState().persistApiConfig()).rejects.toThrow('不一致');
    expect(useSettingsStore.getState().activeApiConfig).toEqual(fromRustConfig(legacy));
    expect(useSettingsStore.getState().apiConfig).toEqual(draft);
    expect(useSettingsStore.getState().apiConfigLoadError).toContain('不一致');
  });

  it('accepts secure API-key readback and backend-generated credential metadata', async () => {
    useSettingsStore.getState().updateApiConfig({ apiKey: 'offline-fixture-key', hasApiKey: false, credentialSource: 'none', authMode: 'api_key' });
    const normalized = { ...legacy, auth_mode: 'api_key' as const, credential_source: 'keychain' as const, credential_ref: 'new-keychain-reference' };
    vi.mocked(loadApiConfig).mockResolvedValue(normalized);
    await expect(useSettingsStore.getState().persistApiConfig()).resolves.toBeUndefined();
    expect(useSettingsStore.getState().apiConfig).toEqual(fromRustConfig(normalized));
    expect(useSettingsStore.getState().activeApiConfig.apiKey).toBe('');
  });

  it('keeps edits made while an ordinary load is pending', async () => {
    const read = deferred<typeof legacy>();
    vi.mocked(loadApiConfig).mockReturnValue(read.promise);
    const pending = useSettingsStore.getState().loadApiConfig();
    useSettingsStore.getState().updateApiConfig({ model: 'newer-draft-model' });
    read.resolve(legacy);
    await pending;
    expect(useSettingsStore.getState().apiConfig.model).toBe('newer-draft-model');
    expect(useSettingsStore.getState().activeApiConfig.model).toBe(legacy.model);
  });

  it('ignores an old load that completes after the new plan was saved', async () => {
    const oldRead = deferred<typeof legacy>();
    vi.mocked(loadApiConfig).mockReturnValueOnce(oldRead.promise).mockResolvedValueOnce(plan);
    const pending = useSettingsStore.getState().loadApiConfig();
    useSettingsStore.getState().updateApiConfig(fromRustConfig(plan));
    await useSettingsStore.getState().persistApiConfig();
    oldRead.resolve(legacy);
    await pending;
    expect(useSettingsStore.getState().activeApiConfig).toEqual(fromRustConfig(plan));
    expect(useSettingsStore.getState().apiConfig).toEqual(fromRustConfig(plan));
  });

  it('preserves a newer model selection made during save and readback', async () => {
    const save = deferred<void>();
    const read = deferred<typeof plan>();
    vi.mocked(saveApiConfig).mockReturnValue(save.promise);
    vi.mocked(loadApiConfig).mockReturnValue(read.promise);
    useSettingsStore.getState().updateApiConfig(fromRustConfig(plan));
    const pending = useSettingsStore.getState().persistApiConfig();
    useSettingsStore.getState().updateApiConfig({ model: 'next-draft-model' });
    save.resolve(undefined);
    await Promise.resolve();
    read.resolve(plan);
    await pending;
    expect(useSettingsStore.getState().activeApiConfig.model).toBe(plan.model);
    expect(useSettingsStore.getState().apiConfig.model).toBe('next-draft-model');
  });

  it('keeps an existing active configuration when a later load fails', async () => {
    const saved = useSettingsStore.getState().activeApiConfig;
    vi.mocked(loadApiConfig).mockRejectedValue(new Error('offline fixture read failed'));
    await useSettingsStore.getState().loadApiConfig();
    expect(useSettingsStore.getState().activeApiConfig).toBe(saved);
  });

  it.each(['replacement-account', null])('invalidates a stale plan binding without activating a replacement: %s', (ref) => {
    useSettingsStore.setState({ activeApiConfig: fromRustConfig(plan) });
    useSettingsStore.getState().updateApiConfig({ ...fromRustConfig(plan), credentialRef: 'replacement-account', model: 'replacement-model' });
    useSettingsStore.getState().invalidatePlanBinding(ref);
    expect(useSettingsStore.getState().activeApiConfig).toMatchObject({ authMode: 'chatgpt_plan', credentialRef: 'opaque-account', model: 'authorized-model', planConnected: false, planEnabled: false });
    expect(useSettingsStore.getState().apiConfig).toMatchObject({ credentialRef: 'replacement-account', model: 'replacement-model' });
  });

  it('keeps the matching old plan binding ready when replacement is cancelled', () => {
    const saved = fromRustConfig(plan);
    useSettingsStore.setState({ activeApiConfig: saved });
    useSettingsStore.getState().invalidatePlanBinding(plan.credential_ref);
    expect(useSettingsStore.getState().activeApiConfig).toBe(saved);
    expect(saved.planEnabled).toBe(true);
  });

  it('never changes a saved API connection due to plan account status', () => {
    const saved = useSettingsStore.getState().activeApiConfig;
    useSettingsStore.getState().invalidatePlanBinding(null);
    expect(useSettingsStore.getState().activeApiConfig).toBe(saved);
  });
});

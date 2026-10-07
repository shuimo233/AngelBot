import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ApiProviderConfig } from '$lib/commands/settings';
import { fromRustConfig, useSettingsStore } from '$stores/settings';
import { SettingsModal } from './index';

const nativeInvoke = vi.hoisted(() => vi.fn());
vi.mock('$lib/invoke', () => ({ invoke: nativeInvoke }));

const savedPlan: ApiProviderConfig = {
  provider: 'openai', model: 'saved-plan-model', base_url: 'https://api.openai.com/v1',
  api_key: '', has_api_key: false, protocol: 'openai_responses', auth_mode: 'chatgpt_plan',
  credential_ref: 'offline-account-ref', plan_connected: true, plan_enabled: true,
  max_tokens: 4096, temperature: 0.7,
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

describe('SettingsModal model save confirmation through native commands', () => {
  let readback: () => Promise<ApiProviderConfig>;
  let save: () => Promise<void>;
  let lastSaved: ApiProviderConfig;
  let loads: number;

  beforeEach(() => {
    localStorage.clear();
    loads = 0;
    lastSaved = savedPlan;
    save = async () => {};
    readback = async () => lastSaved;
    const config = fromRustConfig(savedPlan);
    useSettingsStore.setState({ apiConfig: config, activeApiConfig: config, apiConfigLoaded: true, apiConfigLoadError: null });
    nativeInvoke.mockReset().mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      switch (command) {
        case 'load_api_config': return ++loads === 1 ? savedPlan : await readback();
        case 'save_api_config':
          lastSaved = (args as { config: ApiProviderConfig }).config;
          return await save();
        case 'chatgpt_plan_status': return { connected: true, planEnabled: true, needsSignIn: false, accountLabel: 'offline-account', credentialRef: savedPlan.credential_ref };
        case 'chatgpt_plan_models': return [{ id: savedPlan.model, name: 'Saved plan model' }];
        default: throw new Error(`Unexpected native command: ${command}`);
      }
    });
  });

  async function openAndEdit() {
    const user = userEvent.setup();
    render(<SettingsModal isOpen initialPage="api" onClose={() => {}} />);
    await screen.findByText('offline-account · 已授权套餐用量');
    await waitFor(() => expect(loads).toBe(1));
    await user.clear(screen.getByLabelText('模型标识'));
    await user.type(screen.getByLabelText('模型标识'), 'new-plan-model');
    return user;
  }

  it.each(['failed', 'mismatched'] as const)('never reports saved when effective configuration readback is %s', async (outcome) => {
    readback = outcome === 'failed'
      ? async () => { throw new Error('offline readback unavailable'); }
      : async () => savedPlan;
    const user = await openAndEdit();
    await user.click(screen.getByRole('button', { name: /^保存$/ }));
    await waitFor(() => expect(screen.getByRole('button', { name: '重试保存' })).toBeEnabled());
    expect(screen.queryByRole('button', { name: '已保存' })).not.toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('无法确认模型配置已生效');
    expect(screen.getByLabelText('模型标识')).toHaveValue('new-plan-model');
  });

  it('keeps a newer selection unsaved when it was entered during save', async () => {
    const pendingSave = deferred<void>();
    save = () => pendingSave.promise;
    const user = await openAndEdit();
    await user.click(screen.getByRole('button', { name: /^保存$/ }));
    expect(screen.getByRole('button', { name: '保存中…' })).toBeDisabled();
    await user.clear(screen.getByLabelText('模型标识'));
    await user.type(screen.getByLabelText('模型标识'), 'newer-plan-model');
    await act(async () => { pendingSave.resolve(undefined); });
    await waitFor(() => expect(loads).toBe(2));
    expect(screen.getByLabelText('模型标识')).toHaveValue('newer-plan-model');
    expect(screen.getByRole('button', { name: /^保存$/ })).toBeEnabled();
    expect(useSettingsStore.getState().activeApiConfig.model).toBe('new-plan-model');
  });

  it('clears saved confirmation after a subsequent model edit', async () => {
    const user = await openAndEdit();
    await user.click(screen.getByRole('button', { name: /^保存$/ }));
    await screen.findByRole('button', { name: '已保存' });
    await user.type(screen.getByLabelText('模型标识'), '-next');
    expect(screen.queryByRole('button', { name: '已保存' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: /^保存$/ })).toBeEnabled();
  });
});

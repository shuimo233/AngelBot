import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '../invoke';
import { chatgptPlanStatus, chatgptPlanSignIn, chatgptPlanChangeAccount, chatgptPlanCancelSignIn, chatgptPlanDisconnect, chatgptPlanModels, testModelConnection } from './settings';

vi.mock('../invoke', () => ({ invoke: vi.fn() }));

describe('ChatGPT plan command contracts', () => {
  beforeEach(() => vi.mocked(invoke).mockReset());
  it('keeps all credential operations in no-argument backend commands', async () => {
    for (const [call, command] of [
      [chatgptPlanStatus, 'chatgpt_plan_status'], [chatgptPlanSignIn, 'chatgpt_plan_sign_in'],
      [chatgptPlanChangeAccount, 'chatgpt_plan_change_account'],
      [chatgptPlanCancelSignIn, 'chatgpt_plan_cancel_sign_in'], [chatgptPlanDisconnect, 'chatgpt_plan_disconnect'],
      [chatgptPlanModels, 'chatgpt_plan_models'],
    ] as const) {
      await call();
      expect(invoke).toHaveBeenLastCalledWith(command);
    }
  });
  it('tests the complete unsaved model configuration through the model factory', async () => {
    const config = { provider: 'openai', model: 'selected-model', base_url: 'https://api.openai.com/v1', api_key: '', max_tokens: 4096, temperature: 0.7, protocol: 'openai_responses' as const, auth_mode: 'chatgpt_plan' as const, credential_ref: 'opaque-ref' };
    await testModelConnection(config);
    expect(invoke).toHaveBeenCalledWith('test_model_connection', { config });
  });
});

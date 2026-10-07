import type { ApiConfig } from '$types';
import type { ThinkingEffort } from '$stores/thinkingEffort';

export type ReasoningCapability = {
  supported: boolean;
  options: ThinkingEffort[];
  unavailableReason?: string;
};

/** Whether the active transport will accept the app's temperature parameter. */
export function supportsTemperature(config: ApiConfig | null): boolean {
  if (!config) return false;
  const model = config.model.trim().toLowerCase();
  if (config.provider === 'deepseek' && (model.startsWith('deepseek-v4-') || model === 'deepseek-reasoner')) return false;
  if (config.provider === 'anthropic' && model.startsWith('claude-')) {
    return /(opus-4-[678]|sonnet-4-6|sonnet-5|fable-5|mythos-5)/.test(model);
  }
  return true;
}

/**
 * Keep the UI conservative. The backend repeats this decision before it sends
 * any request, so an unsupported OpenAI-compatible endpoint never receives a
 * made-up reasoning parameter.
 */
export function getReasoningCapability(config: ApiConfig | null): ReasoningCapability {
  if (!config) {
    return { supported: false, options: [], unavailableReason: '请先配置模型' };
  }
  const model = config.model.trim().toLowerCase();
  if (config.provider === 'deepseek' && (model.startsWith('deepseek-v4-') || model === 'deepseek-reasoner')) {
    return { supported: true, options: ['low', 'medium', 'high'] };
  }
  if (config.provider === 'anthropic' && model.startsWith('claude-')) {
    const manual = !/(opus-4-[678]|sonnet-4-6|sonnet-5|fable-5|mythos-5)/.test(model);
    const options: ThinkingEffort[] = manual
      ? (config.maxTokens > 4096 ? ['low', 'medium', 'high'] : ['low', 'medium'])
      : ['low', 'medium', 'high'];
    return { supported: true, options };
  }
  return {
    supported: false,
    options: [],
    unavailableReason: '当前模型的接入方式未提供可控推理',
  };
}

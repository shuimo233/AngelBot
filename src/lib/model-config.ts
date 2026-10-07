import type { ApiConfig } from '$types';
import { getProviderMeta } from '$lib/providers';

export type ModelConfigReadiness =
  | { kind: 'loading' }
  | { kind: 'ready' }
  | { kind: 'needs-credentials'; title: string; detail: string }
  | { kind: 'invalid'; title: string; detail: string };

/** Single readiness interface shared by chat guidance and settings. */
export function getModelConfigReadiness(
  config: ApiConfig,
  loaded: boolean,
  loadError: string | null = null,
): ModelConfigReadiness {
  if (!loaded) return { kind: 'loading' };
  if (loadError) {
    return {
      kind: 'invalid',
      title: '无法读取模型配置',
      detail: '可以继续浏览 AngelBot；重新检查或在设置中修复配置后即可对话。',
    };
  }
  if (!config.provider || !config.model.trim()) {
    return {
      kind: 'invalid',
      title: '模型信息不完整',
      detail: '选择服务商和模型后即可开始对话。',
    };
  }
  const meta = getProviderMeta(config.provider);
  if (config.authMode === 'chatgpt_plan') {
    if (config.provider !== 'openai' || config.protocol !== 'openai_responses'
      || config.baseUrl !== 'https://api.openai.com/v1') {
      return { kind: 'invalid', title: 'ChatGPT 套餐接入配置无效', detail: '套餐仅可通过 OpenAI 官方 Responses 接口使用。' };
    }
    if (!config.planEnabled || !config.credentialRef?.trim()) {
      return { kind: 'needs-credentials', title: '连接 ChatGPT 套餐后即可开始对话', detail: '在模型设置中 Continue with ChatGPT，并授权套餐用量；不会自动使用付费 API。' };
    }
    return { kind: 'ready' };
  }
  if (meta.endpointMode === 'required' && !config.baseUrl.trim()) {
    return {
      kind: 'invalid',
      title: '还需要填写 API 地址',
      detail: '当前服务商需要一个可访问的 API 地址。',
    };
  }
  if (config.authMode !== 'none' && (config.authMode === 'api_key' || meta.requiresKey)
    && !config.hasApiKey && !config.apiKey.trim()) {
    return {
      kind: 'needs-credentials',
      title: '连接模型后即可开始对话',
      detail: '主界面和项目仍可浏览；API Key 只会保存在系统凭据库中。',
    };
  }
  return { kind: 'ready' };
}

export function formatModelSendError(error: unknown): string {
  const detail = error instanceof Error ? error.message : String(error);
  if (/credentials? (?:are )?not configured|api key is required|missing credentials/i.test(detail)) {
    return '还没有连接可用的模型。请打开“模型设置”，保存对应服务商的 API Key 后重试。';
  }
  if (/credential store|keychain/i.test(detail)) {
    return '无法读取系统凭据库中的模型密钥。请在“模型设置”中重新保存后重试。';
  }
  return `发送失败：${detail}`;
}

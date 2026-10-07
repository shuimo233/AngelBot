/**
 * Provider Registry - AI 模型提供商注册表
 *
 * 设计原则：
 * 1. 环境变量优先：ENV > UI 配置
 * 2. 单一活跃 Provider：同一时间只接入一家厂商
 * 3. 自动检测：支持 Ollama 等本地无 Key 的 Provider
 */

import type { ApiConfig, ApiProvider } from '$types';

// ============================================
// Provider 元数据定义
// ============================================

export interface ProviderMeta {
  /** 唯一标识符 */
  id: ApiProvider;
  /** 显示名称 */
  name: string;
  /** 环境变量名 */
  envKey: string;
  /** 默认 Base URL */
  defaultBaseUrl: string;
  /** 是否需要 API Key */
  requiresKey: boolean;
  /** 是否为本地服务 */
  isLocal: boolean;
  /** 运行时实际使用的协议 */
  protocol: 'anthropic-messages' | 'openai-chat-completions';
  /** 端点是否必须由用户提供 */
  endpointMode: 'editable' | 'required';
  /** 模型列表 */
  models: { value: string; label: string }[];
  /** Logo SVG */
  logo: string;
  /** 费用等级描述 */
  costTier: '$' | '$$' | '$$$';
  /** 官方文档链接 */
  docsUrl: string;
}

const providerLogos: Record<string, string> = {
  // Anthropic - Official stylized starburst (#C15F3C)
  anthropic: `<svg viewBox="0 0 100 100" xmlns="http://www.w3.org/2000/svg">
    <circle cx="50" cy="50" r="48" fill="#C15F3C"/>
    <path d="M50 12 L56 40 L85 40 L62 58 L72 88 L50 70 L28 88 L38 58 L15 40 L44 40 Z" fill="#fff"/>
  </svg>`,

  // OpenAI - Official petal/flower symbol
  openai: `<svg viewBox="0 0 100 100" xmlns="http://www.w3.org/2000/svg">
    <rect width="100" height="100" rx="12" fill="#000"/>
    <g transform="translate(50,50)" fill="#fff">
      <ellipse cx="0" cy="-16" rx="6" ry="16"/>
      <ellipse cx="15" cy="-8" rx="6" ry="16" transform="rotate(72)"/>
      <ellipse cx="15" cy="8" rx="6" ry="16" transform="rotate(144)"/>
      <ellipse cx="-15" cy="8" rx="6" ry="16" transform="rotate(-144)"/>
      <ellipse cx="-15" cy="-8" rx="6" ry="16" transform="rotate(-72)"/>
      <circle cx="0" cy="0" r="5"/>
    </g>
  </svg>`,

  // Google Gemini - no verified logo, use text badge
  google: '',

  // DeepSeek - no verified SVG, use text badge
  deepseek: '',

  // Groq - stylized lowercase g letterform
  groq: `<svg viewBox="0 0 100 100" xmlns="http://www.w3.org/2000/svg">
    <rect width="100" height="100" rx="12" fill="#4A1D96"/>
    <path d="M65 22 C38 22, 20 42, 20 60 C20 80, 40 92, 60 92 C72 92, 82 86, 88 78 L76 72 C72 78, 64 82, 58 82 C44 82, 36 72, 36 60 C36 48, 44 38, 58 38 C66 38, 74 42, 78 50 L86 42 C78 32, 68 22, 65 22" fill="#fff"/>
    <path d="M68 22 L68 35" stroke="#fff" stroke-width="5"/>
  </svg>`,

  // Azure - no verified SVG, use text badge
  azure: '',

  // Ollama - stylized circle with dot
  ollama: `<svg viewBox="0 0 100 100" xmlns="http://www.w3.org/2000/svg">
    <rect width="100" height="100" rx="12" fill="#1a1a1a"/>
    <circle cx="50" cy="50" r="28" fill="none" stroke="#fff" stroke-width="5"/>
    <circle cx="50" cy="50" r="10" fill="#fff"/>
  </svg>`,

  // Custom - simple plus icon
  custom: `<svg viewBox="0 0 100 100" xmlns="http://www.w3.org/2000/svg">
    <rect width="100" height="100" rx="12" fill="#6366f1"/>
    <path d="M50 28v44M28 50h44" stroke="#fff" stroke-width="8" stroke-linecap="round"/>
  </svg>`,
};

export const PROVIDER_REGISTRY: Record<ApiProvider, ProviderMeta> = {
  anthropic: {
    id: 'anthropic',
    name: 'Anthropic (Claude)',
    envKey: 'ANTHROPIC_API_KEY',
    defaultBaseUrl: 'https://api.anthropic.com',
    requiresKey: true,
    isLocal: false,
    protocol: 'anthropic-messages',
    endpointMode: 'editable',
    models: [
      { value: 'claude-opus-4-1-20250805', label: 'Claude Opus 4.1' },
      { value: 'claude-sonnet-4-20250514', label: 'Claude Sonnet 4' },
      { value: 'claude-haiku-3-5-20241022', label: 'Claude Haiku 3.5' },
    ],
    logo: providerLogos.anthropic,
    costTier: '$$$',
    docsUrl: 'https://docs.anthropic.com/',
  },
  openai: {
    id: 'openai',
    name: 'OpenAI (GPT)',
    envKey: 'OPENAI_API_KEY',
    defaultBaseUrl: 'https://api.openai.com/v1',
    requiresKey: true,
    isLocal: false,
    protocol: 'openai-chat-completions',
    endpointMode: 'editable',
    models: [
      { value: 'gpt-5', label: 'GPT-5' },
      { value: 'gpt-5-mini', label: 'GPT-5 mini' },
      { value: 'gpt-5-nano', label: 'GPT-5 nano' },
      { value: 'gpt-4.1', label: 'GPT-4.1' },
      { value: 'gpt-4.1-mini', label: 'GPT-4.1 mini' },
      { value: 'o3', label: 'o3' },
      { value: 'o4-mini', label: 'o4-mini' },
    ],
    logo: providerLogos.openai,
    costTier: '$$$',
    docsUrl: 'https://platform.openai.com/docs',
  },
  google: {
    id: 'google',
    name: 'Google (Gemini)',
    envKey: 'GEMINI_API_KEY',
    defaultBaseUrl: 'https://generativelanguage.googleapis.com/v1beta/openai',
    requiresKey: true,
    isLocal: false,
    protocol: 'openai-chat-completions',
    endpointMode: 'editable',
    models: [
      { value: 'gemini-3.5-flash', label: 'Gemini 3.5 Flash' },
      { value: 'gemini-2.5-pro', label: 'Gemini 2.5 Pro' },
      { value: 'gemini-2.5-flash', label: 'Gemini 2.5 Flash' },
    ],
    logo: providerLogos.google,
    costTier: '$$',
    docsUrl: 'https://ai.google.dev/',
  },
  deepseek: {
    id: 'deepseek',
    name: 'DeepSeek',
    envKey: 'DEEPSEEK_API_KEY',
    defaultBaseUrl: 'https://api.deepseek.com',
    requiresKey: true,
    isLocal: false,
    protocol: 'openai-chat-completions',
    endpointMode: 'editable',
    models: [
      { value: 'deepseek-v4-pro', label: 'DeepSeek V4 Pro' },
      { value: 'deepseek-v4-flash', label: 'DeepSeek V4 Flash' },
      { value: 'deepseek-chat', label: 'DeepSeek Chat（兼容 · 已废弃）' },
    ],
    logo: providerLogos.deepseek,
    costTier: '$',
    docsUrl: 'https://platform.deepseek.com/',
  },
  groq: {
    id: 'groq',
    name: 'Groq',
    envKey: 'GROQ_API_KEY',
    defaultBaseUrl: 'https://api.groq.com/openai/v1',
    requiresKey: true,
    isLocal: false,
    protocol: 'openai-chat-completions',
    endpointMode: 'editable',
    models: [
      { value: 'openai/gpt-oss-120b', label: 'GPT-OSS 120B' },
      { value: 'openai/gpt-oss-20b', label: 'GPT-OSS 20B' },
      { value: 'llama-3.3-70b-versatile', label: 'Llama 3.3 70B' },
      { value: 'llama-3.1-8b-instant', label: 'Llama 3.1 8B Instant' },
      { value: 'groq/compound', label: 'Groq Compound' },
    ],
    logo: providerLogos.groq,
    costTier: '$',
    docsUrl: 'https://console.groq.com/docs',
  },
  azure: {
    id: 'azure',
    name: 'Azure OpenAI',
    envKey: 'AZURE_OPENAI_KEY',
    defaultBaseUrl: '',
    requiresKey: true,
    isLocal: false,
    protocol: 'openai-chat-completions',
    endpointMode: 'required',
    // Azure OpenAI routes requests by deployment name, which is account-specific.
    models: [],
    logo: providerLogos.azure,
    costTier: '$$$',
    docsUrl: 'https://learn.microsoft.com/azure/ai-services/openai/',
  },
  ollama: {
    id: 'ollama',
    name: 'Ollama (本地)',
    envKey: '',
    defaultBaseUrl: 'http://localhost:11434/v1',
    requiresKey: false,
    isLocal: true,
    protocol: 'openai-chat-completions',
    endpointMode: 'required',
    models: [
      { value: 'llama3', label: 'Llama 3' },
      { value: 'llama3.1', label: 'Llama 3.1' },
      { value: 'codellama', label: 'Code Llama' },
      { value: 'mistral', label: 'Mistral' },
      { value: 'qwen2', label: 'Qwen 2' },
      { value: 'phi3', label: 'Phi-3' },
    ],
    logo: providerLogos.ollama,
    costTier: '$',
    docsUrl: 'https://ollama.com/',
  },
  custom: {
    id: 'custom',
    name: '自定义 API',
    envKey: '',
    defaultBaseUrl: '',
    requiresKey: true,
    isLocal: false,
    protocol: 'openai-chat-completions',
    endpointMode: 'required',
    models: [],
    logo: providerLogos.custom,
    costTier: '$$',
    docsUrl: '',
  },
};

// ============================================
// 导出列表
// ============================================

export const ALL_PROVIDERS: ProviderMeta[] = Object.values(PROVIDER_REGISTRY);

export const PROVIDER_OPTIONS = ALL_PROVIDERS.map((p) => ({
  value: p.id,
  label: p.name,
}));

export function getProviderMeta(provider: ApiProvider): ProviderMeta {
  return PROVIDER_REGISTRY[provider];
}

export function getModelsForProvider(provider: ApiProvider): { value: string; label: string }[] {
  return PROVIDER_REGISTRY[provider]?.models || [];
}

export function providerProtocolLabel(provider: ApiProvider, protocol?: ApiConfig['protocol']): string {
  if (protocol === 'openai_responses') return 'OpenAI Responses';
  if (protocol === 'anthropic_messages') return 'Anthropic Messages';
  if (protocol === 'openai_chat_completions') return 'OpenAI Chat Completions';
  return getProviderMeta(provider).protocol === 'anthropic-messages'
    ? 'Anthropic Messages'
    : 'OpenAI 兼容';
}

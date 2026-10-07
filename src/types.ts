export type Session = {
  id: string;
  title: string;
  /** Existing session-tree parent for an edited/branched conversation. */
  parentId?: string;
  createdAt: number;
  updatedAt: number;
  /** 上下文压缩版本，每次压缩后 +1 */
  contextVersion: number;
  /** 上次压缩时间戳 */
  lastCompressedAt?: number;
  /** 当前工作目录（沙盒根目录） */
  workDir?: string;
  /** 消息数量（可选） */
  messageCount?: number;
  /** 会话绑定的模型 provider */
  agentProvider?: string;
  /** 会话绑定的模型 */
  agentModel?: string;
};

/** Explicit user-supplied text snapshot; never a filesystem permission or path. */
export type TextAttachment = { name: string; text: string };

export type Message = {
  id: string;
  sessionId: string;
  /** Tool rows are legacy persistence details and are hidden when a durable
   *  assistant timeline replays the same execution. */
  role: 'user' | 'assistant' | 'tool';
  content: string;
  textAttachments?: TextAttachment[];
  createdAt: number;
  /** 元数据：压缩信息、子任务标记 */
  metadata?: MessageMetadata;
  /** 工具调用（Agent 场景） */
  toolCalls?: ToolCall[];
  /** 工具调用结果（嵌入在 assistant 消息或独立展示） */
  toolResults?: ToolResult[];
  /** Agent task-level run summary for visible, resumable workflows. */
  taskRun?: AgentTaskRun;
  /** Durable task facts from the agent run: modified files, plan
   *  steps, verification evidence, terminal reason. Optional — older
   *  agent turns before #100 wrote only the ladder summary above. */
  taskFacts?: TaskFacts;
  /** Ordered, durable text/tool timeline for an agent response. */
  timelineBlocks?: import('$stores/messages').LiveBlock[];
};

/** 工具调用 */
export type ToolCall = {
  id: string;
  name: string;
  arguments: string;
};

/** 工具调用结果 */
export type ToolResult = {
  callId: string;
  toolName: string;
  success: boolean;
  output: string;
  error?: string;
  confirmationRequired?: boolean;
  confirmationStatus?: 'pending' | 'approved' | 'rejected' | string;
  // Issue #102: post-call disk-truth evidence. `matched === false` means
  // the verifier said the tool's claim did not agree with the filesystem;
  // the UI should highlight the step in red.
  verification?: {
    kind: 'file_exists' | 'file_absent' | string;
    target: string;
    declaredExistence: boolean;
    actualExistence: boolean;
    source: 'work_dir' | 'supplemental' | 'unresolved' | string;
    matched: boolean;
    note?: string;
  };
};

export type AgentTaskRun = {
  id: string;
  goal: string;
  status: 'awaiting_confirmation' | 'needs_attention' | 'running' | 'paused' | 'stopped' | 'failed' | 'completed' | string;
  plan: string[];
  confirmationState: 'none' | 'pending' | 'approved' | 'rejected' | string;
  resumable: boolean;
  stepCount: number;
  completedStepCount: number;
};

/** Structured task facts for an agent turn. Rehydrated by the chat
 *  fetch path so the timeline can render modified files, verification
 *  evidence, plan steps, and the terminal reason without a separate
 *  fetch. Mirrors the Rust `TaskFacts` struct in `src-tauri/src/agent/task_facts.rs`. */
export type TaskFacts = {
  goal: string;
  plan?: PlanStepFact[];
  completedSteps?: string[];
  failedSteps?: string[];
  modifiedFiles?: string[];
  verificationEvidence?: VerificationEvidence[];
  pendingConfirmation?: PendingConfirmationFact;
  repairAttempts?: number;
  noProgressCount?: number;
  contextSummary?: string;
  providerAttempts?: number;
  providerBinding?: ProviderBinding;
  terminalReason?: TaskTerminalReason;
};

export type PlanStepFact = {
  id: string;
  description: string;
  dependsOn?: string[];
  status: 'pending' | 'in_progress' | 'completed' | 'skipped' | 'failed' | string;
};

export type VerificationEvidence = {
  command: string;
  exitCode: number;
  summary: string;
  verifiedAt: number;
};

export type PendingConfirmationFact = {
  callId: string;
  toolName: string;
  arguments: unknown;
  requestedAt: number;
};

export type ProviderBinding = {
  providerId: string;
  modelId: string;
};

export type TaskTerminalReason =
  | 'completed'
  | 'awaiting_confirmation'
  | 'needs_attention'
  | 'no_progress'
  | 'side_effect_pause'
  | 'wall_clock_budget'
  | 'tool_failure_limit'
  | 'user_stopped'
  | 'provider_unavailable'
  | string;

export type MessageMetadata = {
  /** 消息是否来自压缩后的摘要 */
  isCompressed?: boolean;
  /** 如果被压缩，指向原始消息 ID */
  summaryOf?: string;
  /** 工具执行中（等待结果） */
  isToolExecuting?: boolean;
  /** 工具调用正在进行 */
  executingTool?: string;
};

export type LanguageStyle = 'formal' | 'casual' | 'concise' | 'detailed';
export type Tone = 'friendly' | 'neutral' | 'professional' | 'tsundere' | 'gentle' | 'energetic';
export type ResponseFormat = 'text' | 'code' | 'list' | 'mixed';

export type ApiProvider = 'anthropic' | 'openai' | 'google' | 'deepseek' | 'groq' | 'azure' | 'ollama' | 'custom';

export type CostTier = '$' | '$$' | '$$$';

export type ConfigSource = 'env' | 'ui';

export type ProviderStatus = 'configured' | 'env_only' | 'invalid' | 'unknown' | 'connected' | 'disconnected' | 'connecting';

export type Persona = {
  name: string;
  avatar: string;
  bio: string;
  languageStyle: LanguageStyle;
  tone: Tone;
  responseFormats: ResponseFormat[];
  keywords: string[];
  greeting: string;
  personality: 'balanced' | 'cheerful' | 'serious' | 'cute' | 'cool';
  speechBubble: 'default' | 'thoughtful' | 'excited' | 'serene';
  /** Fine-grained personality controls chosen in Settings → Personality. */
  traits?: TraitConfig;
  /** Imported SillyTavern/TavernCard metadata retained for editing and export. */
  characterCard?: TavernCharacterCard;
};

export type TavernCharacterCard = {
  specVersion: string;
  personality: string;
  scenario: string;
  mesExample: string;
  systemPrompt: string;
  postHistoryInstructions: string;
  alternateGreetings: string[];
  creatorNotes: string;
  creator: string;
  tags: string[];
  /** Keeps extensions from a card intact when it is exported again. */
  rawData?: Record<string, unknown>;
};

export type ApiConfig = {
  provider: ApiProvider;
  model: string;
  baseUrl: string;
  apiKey: string;
  /** Whether a provider-matched key already exists outside the renderer. */
  hasApiKey?: boolean;
  /** The stored key is never exposed; this only describes where it is managed. */
  credentialSource?: 'none' | 'keychain' | 'environment';
  protocol?: 'openai_chat_completions' | 'openai_responses' | 'anthropic_messages';
  authMode?: 'api_key' | 'none' | 'chatgpt_plan';
  /** Non-secret account handle; OAuth tokens never enter the renderer. */
  credentialRef?: string | null;
  planConnected?: boolean;
  planEnabled?: boolean;
  planAccountLabel?: string | null;
  deployment?: string;
  maxTokens: number;
  temperature: number;
  /** 配置来源（只读，由系统设置） */
  source?: ConfigSource;
};

export type ReadFileError = {
  code: 'missing_work_dir' | 'escapes_work_dir' | 'io';
  message: string;
};

export type McpServer = {
  id: string;
  name: string;
  command: string;
  args: string;
  env: string;
  /** Names only; MCP environment values are never returned to the frontend. */
  envKeys: string[];
  /** The local credential store could not be read; no value is exposed. */
  envUnavailable: boolean;
  enabled: boolean;
  enabledWorkspaceIds: string[];
  status?: 'connected' | 'disconnected' | 'starting';
};

// ============================================
// Memory Types
// ============================================

export type MemoryScope = 'global' | 'session';
export type MemoryCategory = 'fact' | 'preference' | 'event' | 'knowledge' | 'personality';

export interface Memory {
  id: string;
  scope: MemoryScope;
  category: MemoryCategory;
  content: string;
  importance: number;
  source: string;
  // 频率遗忘系统字段
  frequency: number;
  lastMentioned: number;
  isPermanent: boolean;
  embedding: number[] | null;
  decayFactor: number;
  createdAt: number;
  updatedAt: number;
}

export interface MemoryConfig {
  similarityThreshold: number;
  baseDecayRate: number;
  forgetThreshold: number;
  maxActiveMemories: number;
  enableVector: boolean;
}

export const DEFAULT_MEMORY_CONFIG: MemoryConfig = {
  similarityThreshold: 0.7,
  baseDecayRate: 0.9,
  forgetThreshold: 0.5,
  maxActiveMemories: 100,
  enableVector: false,
};

export type ForgetStage = 'active' | 'summarized' | 'archived' | 'deleted';

export interface MemoryWithForgetStage extends Memory {
  forgetStage: ForgetStage;
}

// ============================================
// Personality Templates & Evolution Types
// ============================================

export type TraitName = 'tone' | 'verbosity' | 'formality' | 'humor' | 'dependence' | 'intimacy' | 'patience';

export interface TraitConfig {
  tone: number;
  verbosity: number;
  formality: number;
  humor: number;
  dependence: number;
  intimacy: number;
  patience: number;
}

export interface PersonalityTemplate {
  id: string;
  name: string;
  avatar: string;
  description: string;
  traits: TraitConfig;
  greeting: string;
  knowledge?: string[];
}

export interface UserPreferences {
  communication: {
    preferredTone: string[];
    dislikedWords: string[];
    petPeeves: string[];
  };
  habits: {
    greetingStyle: string;
    responseLength: 'short' | 'medium' | 'long';
    responseLanguage: 'auto' | 'zh-CN' | 'en-US';
    useLongTermMemory?: boolean;
  };
  topics: {
    interests: string[];
    avoidTopics: string[];
  };
  learnedAt: number;
  evolutionEnabled: boolean;
}

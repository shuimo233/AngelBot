/**
 * Message commands - typed wrappers around Tauri IPC
 */
import { invoke } from '../invoke';
import type { DesktopActionOperation } from './desktop';
import type {
  Message,
  PendingConfirmationFact,
  PersonalityTemplate,
  TaskFacts,
  ToolCall,
  ToolResult,
  TextAttachment,
} from '$types';

type UnknownRecord = Record<string, unknown>;

function asRecord(value: unknown): UnknownRecord | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as UnknownRecord
    : null;
}

function field(record: UnknownRecord, camelCase: string, snakeCase: string) {
  return record[camelCase] ?? record[snakeCase];
}

function stringArray(value: unknown) {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : undefined;
}

function normalizePendingConfirmation(value: unknown): PendingConfirmationFact | undefined {
  const raw = asRecord(value);
  if (!raw) return undefined;
  const callId = field(raw, 'callId', 'call_id');
  const toolName = field(raw, 'toolName', 'tool_name');
  const requestedAt = field(raw, 'requestedAt', 'requested_at');
  if (typeof callId !== 'string' || typeof toolName !== 'string') return undefined;
  return {
    callId,
    toolName,
    arguments: raw.arguments ?? {},
    requestedAt: typeof requestedAt === 'number' ? requestedAt : 0,
  };
}

/**
 * Normalize the durable Rust task-facts shape at the IPC seam. The database
 * intentionally keeps snake_case JSON, while React callers use camelCase.
 * Pending confirmations are projected into the same ToolResult interface as
 * completed calls so the conversation has one rendering and resolution path.
 */
export function normalizeMessageContract(message: Message): Message {
  const rawFacts = asRecord(message.taskFacts);
  if (!rawFacts) return message;

  // The compact task-run projection is updated atomically with confirmation
  // resolution. Prefer it over a stale pre-fix facts row so old local data
  // cannot leave actionable controls on screen after a rejection.
  const pendingConfirmation = message.taskRun?.confirmationState === 'rejected'
    || message.taskRun?.confirmationState === 'approved'
    ? undefined
    : normalizePendingConfirmation(field(rawFacts, 'pendingConfirmation', 'pending_confirmation'));
  const plan = Array.isArray(rawFacts.plan)
    ? rawFacts.plan.flatMap((value) => {
      const raw = asRecord(value);
      if (!raw || typeof raw.id !== 'string' || typeof raw.description !== 'string' || typeof raw.status !== 'string') {
        return [];
      }
      return [{
        id: raw.id,
        description: raw.description,
        dependsOn: stringArray(field(raw, 'dependsOn', 'depends_on')),
        status: raw.status,
      }];
    })
    : undefined;
  const verificationEvidence = Array.isArray(field(rawFacts, 'verificationEvidence', 'verification_evidence'))
    ? (field(rawFacts, 'verificationEvidence', 'verification_evidence') as unknown[]).flatMap((value) => {
      const raw = asRecord(value);
      const exitCode = raw ? field(raw, 'exitCode', 'exit_code') : undefined;
      const verifiedAt = raw ? field(raw, 'verifiedAt', 'verified_at') : undefined;
      if (!raw || typeof raw.command !== 'string' || typeof exitCode !== 'number'
        || typeof raw.summary !== 'string' || typeof verifiedAt !== 'number') return [];
      return [{ command: raw.command, exitCode, summary: raw.summary, verifiedAt }];
    })
    : undefined;
  const rawBinding = asRecord(field(rawFacts, 'providerBinding', 'provider_binding'));
  const providerId = rawBinding ? field(rawBinding, 'providerId', 'provider_id') : undefined;
  const modelId = rawBinding ? field(rawBinding, 'modelId', 'model_id') : undefined;
  const taskFacts: TaskFacts = {
    goal: typeof rawFacts.goal === 'string' ? rawFacts.goal : '',
    plan,
    completedSteps: stringArray(field(rawFacts, 'completedSteps', 'completed_steps')),
    failedSteps: stringArray(field(rawFacts, 'failedSteps', 'failed_steps')),
    modifiedFiles: stringArray(field(rawFacts, 'modifiedFiles', 'modified_files')),
    verificationEvidence,
    pendingConfirmation,
    repairAttempts: field(rawFacts, 'repairAttempts', 'repair_attempts') as number | undefined,
    noProgressCount: field(rawFacts, 'noProgressCount', 'no_progress_count') as number | undefined,
    contextSummary: field(rawFacts, 'contextSummary', 'context_summary') as string | undefined,
    providerAttempts: field(rawFacts, 'providerAttempts', 'provider_attempts') as number | undefined,
    providerBinding: typeof providerId === 'string' && typeof modelId === 'string'
      ? { providerId, modelId }
      : undefined,
    terminalReason: field(rawFacts, 'terminalReason', 'terminal_reason') as TaskFacts['terminalReason'],
  };

  const existingCalls = message.toolCalls ?? [];
  const toolCalls: ToolCall[] = !pendingConfirmation || existingCalls.some((call) => call.id === pendingConfirmation.callId)
    ? existingCalls
    : [...existingCalls, {
      id: pendingConfirmation.callId,
      name: pendingConfirmation.toolName,
      arguments: JSON.stringify(pendingConfirmation.arguments),
    }];
  const existingResults = message.toolResults ?? [];
  let toolResults: ToolResult[] = existingResults;
  if (pendingConfirmation && !existingResults.some((result) => result.callId === pendingConfirmation.callId)) {
    toolResults = [...existingResults, {
      callId: pendingConfirmation.callId,
      toolName: pendingConfirmation.toolName,
      success: false,
      output: '等待用户确认',
      confirmationRequired: true,
      confirmationStatus: 'pending',
    }];
  } else if (!pendingConfirmation && message.taskRun?.confirmationState === 'rejected'
    && existingCalls.length > 0 && existingResults.length === 0) {
    const rejectedCall = existingCalls[existingCalls.length - 1];
    toolResults = [{
      callId: rejectedCall.id,
      toolName: rejectedCall.name,
      success: false,
      output: '用户已拒绝此操作',
      confirmationRequired: false,
      confirmationStatus: 'rejected',
    }];
  }

  return { ...message, taskFacts, toolCalls, toolResults };
}

/** Flat structure matching backend PreferencesPayload */
export interface FlatPreferences {
  responseLength?: string;
  useLongTermMemory?: boolean;
  interests?: string[];
  avoidTopics?: string[];
  dislikedWords?: string[];
  petPeeves?: string[];
  evolutionEnabled?: boolean;
}

export interface SendMessageArgs {
  sessionId: string;
  /** Stable client-generated ID used for the persisted user turn. */
  clientMessageId?: string;
  role: 'user' | 'assistant';
  content: string;
  textAttachments?: TextAttachment[];
  /** Optional personality template to inject into system prompt */
  personality?: PersonalityTemplate;
  /** Optional user preferences to inject into system prompt */
  preferences?: FlatPreferences | null;
  /** Optional provider-native reasoning preference */
  thinkingEffort?: 'low' | 'medium' | 'high';
  /** Sensitive credential envelope. It is never included in `content`. */
  webSearchSetup?: {
    provider: string;
    apiKey: string;
    endpoint?: string;
  } | null;
}

export async function getMessages(sessionId: string): Promise<Message[]> {
  return (await invoke<Message[]>('get_messages', { sessionId })).map(normalizeMessageContract);
}

export async function sendMessage(args: SendMessageArgs): Promise<Message> {
  return normalizeMessageContract(await invoke<Message>('send_message', {
    req: {
      sessionId: args.sessionId,
      clientMessageId: args.clientMessageId ?? null,
      role: args.role,
      content: args.content,
      textAttachments: args.textAttachments ?? [],
      personality: args.personality ?? null,
      preferences: args.preferences ?? null,
      thinkingEffort: args.thinkingEffort ?? null,
      webSearchSetup: args.webSearchSetup ?? null,
    },
  }));
}

export async function resolveAgentConfirmation(args: {
  sessionId: string;
  messageId: string;
  callId: string;
  decision: 'approved' | 'rejected';
  /** Required for approving a backend-preflighted desktop action. */
  previewId?: string;
}): Promise<ToolResult> {
  // The Tauri command receives a single `req` payload. Keep the production
  // IPC shape aligned with the dev HTTP command gateway.
  return invoke<ToolResult>('resolve_agent_confirmation', { req: args });
}

/** One-use, short-lived server-attested target for a pending desktop action. */
export interface DesktopActionPreview {
  previewId: string;
  operation: DesktopActionOperation;
  appDisplayName: string;
  executableName: string;
  windowTitle: string | null;
  controlName: string | null;
  text: string | null;
  /** Unix milliseconds; approvals must be made before this instant. */
  expiresAt: number;
}

export async function preflightPendingDesktopAction(args: {
  sessionId: string;
  messageId: string;
  callId: string;
}): Promise<DesktopActionPreview> {
  return invoke<DesktopActionPreview>('preflight_pending_desktop_action', args);
}

/** Start a fresh, bounded follow-up slice from an auditable task run. */
export async function continueAgentTask(args: {
  sessionId: string;
  messageId: string;
}): Promise<Message> {
  return normalizeMessageContract(await invoke<Message>('continue_agent_task', { req: args }));
}

export async function cancelAgentTask(args: {
  sessionId: string;
  messageId: string;
}): Promise<void> {
  return invoke<void>('cancel_agent_task', { req: args });
}

export async function deleteMessage(messageId: string): Promise<void> {
  return invoke<void>('delete_message', { messageId });
}

export async function updateMessageContent(messageId: string, content: string): Promise<void> {
  return invoke<void>('update_message_content', { messageId, content });
}

/** Edit a user turn and replace its later transcript with a newly generated reply. */
export async function editAndResendMessage(args: {
  sessionId: string;
  messageId: string;
  content: string;
  personality?: PersonalityTemplate;
  preferences?: FlatPreferences | null;
  thinkingEffort?: 'low' | 'medium' | 'high';
}): Promise<Message> {
  return normalizeMessageContract(await invoke<Message>('edit_and_resend_message', {
    req: {
      sessionId: args.sessionId,
      messageId: args.messageId,
      content: args.content,
      personality: args.personality ?? null,
      preferences: args.preferences ?? null,
      thinkingEffort: args.thinkingEffort ?? null,
    },
  }));
}

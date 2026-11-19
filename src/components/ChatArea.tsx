import { Component, Fragment, useEffect, useLayoutEffect, useMemo, useRef, useState, useCallback, type ErrorInfo, type ReactNode } from 'react';
import { useMessagesStore, type LiveBlock, type QueuedFollowUp } from '$stores/messages';
import { useSessionsStore } from '$stores/sessions';
import { projectCandidatesForMessage, useWorkspacesStore } from '$stores/workspaces';
import { resolveWorkspaceIntent, type Workspace } from '$lib/commands/workspace';
import { usePreferencesStore } from '$stores/preferences';
import { useSettingsStore } from '$stores/settings';
import { useAppearanceStore } from '$stores/appearance';
import { ComposerInput, type ComposerInputHandle, type ComposerSendResult } from './Composer/ComposerInput';
import { ChatEmptyState, ChatStarterPrompts } from './ChatEmptyState';
import { AUTOMATIONS_UPDATED_EVENT } from '$lib/automation-events';
import { ChatHeader } from './ChatHeader';
import { ConversationNavigation } from './ConversationNavigation';
import { messagePageBounds, TRANSCRIPT_PAGE_SIZE, useTranscriptWindow } from './ChatArea/useTranscriptWindow';
import { latestResultToReview, unknownResultCalls } from './ChatArea/pendingResultReview';
import type { WorkspaceView } from './RightPanel';
import { useThinkingEffortStore } from '$stores/thinkingEffort';
import { sendMessage, resolveAgentConfirmation, interruptAgent, pauseAgent, resumeAgent, submitSteeringMessage } from '$lib/commands';
import { continueAgentTask, deleteMessage, editAndResendMessage, getMessagePage, searchConversationPage, type MessagePage, type ForegroundInteractionPayload } from '$lib/commands/message';
import { getWorkspaceTaskUnderstanding, type TaskDecisionProjection } from '$lib/commands/task-understanding';
import { MessageActions } from './MessageActions';
import { ContentBlockRenderer, groupBlocksIntoSegments } from './ContentBlockRenderer';
import { locateWorkspaceFile, MessageMarkdown, WorkspaceFileLink, workspaceFilePath } from './MessageMarkdown';
import { completedReplyDeliveries, ReplyFileDelivery } from './CompletedFileTask';
import {
  authoritativeToolResultsByCallId,
  contentBlocksFromDurableEvents,
  getDurableAgentRunEvents,
  subscribeAgentSessionEvents,
} from '$lib/commands/agent-event';
import type { Message, ToolCall, ToolResult, TextAttachment, TraitConfig } from '$types';
import { TextAttachmentDetails } from './TextAttachmentDetails';
import { cardDescription } from '$lib/tavern-card';
import { extractSecureWebSearchSetup } from '$lib/web-search-setup';
import { WorkspaceActivityCapsule } from './WorkspaceActivityCapsule';
import { TaskDecisionCard } from './TaskDecisionCard';
import { RuntimeHealthNotice } from './RuntimeHealthNotice';
import { UpdateNotice } from './UpdateNotice';
import { ModelConfigurationNotice } from './ModelConfigurationNotice';
import { formatModelSendError, getModelConfigReadiness, openModelSettings } from '$lib/model-config';
import { getToolDisplayName, parseMcpToolIdentity } from '$lib/tool-display';
import { desktopActionOperation, desktopConfirmationOperation, getToolConfirmationSummary, requiresDesktopActionPreview, requiresDesktopConfirmationPreview } from '$lib/tool-confirmation';
import { isResultUnknown, toolActionReceipt } from '$lib/tool-action-result';
import { DesktopActionApproval, type DesktopApprovalRecheck, type DesktopApprovalResolution } from './DesktopActionApproval';
import { ToolActionApproval } from './ToolActionApproval';

const TIMELINE_REPLAY_CONCURRENCY = 4;
const RECENT_TIMELINE_LIMIT = 6;
const EMPTY_FOLLOW_UPS: QueuedFollowUp[] = [];

const isStopPending = (sessionId: string) => useMessagesStore.getState().stoppingSessions.has(sessionId);

function updateFollowUps(sessionId: string, update: (items: QueuedFollowUp[]) => QueuedFollowUp[]) {
  useMessagesStore.setState((state) => ({
    queuedFollowUps: {
      ...state.queuedFollowUps,
      [sessionId]: update(state.queuedFollowUps[sessionId] ?? EMPTY_FOLLOW_UPS),
    },
  }));
}

function followUpsCanContinue(reply: Message) {
  return reply.role === 'assistant' && Boolean(reply.content.trim())
    && (!reply.taskRun || reply.taskRun.status === 'completed')
    && ![...authoritativeToolResultsByCallId(reply.toolResults ?? []).values()]
      .some((result) => result.confirmationRequired && result.confirmationStatus === 'pending');
}

// ─── Tool Display Helpers ──────────────────────────────────────────────────────

function formatArgValue(v: unknown): string {
  if (typeof v === 'string') return v;
  try { return JSON.stringify(v); } catch { return String(v); }
}

function timestampToDate(timestamp: number): Date {
  // Optimistic client messages use Date.now() milliseconds, while persisted
  // backend messages use Unix seconds.
  return new Date(timestamp < 10_000_000_000 ? timestamp * 1000 : timestamp);
}

// ─── Tool Executing Indicator ─────────────────────────────────────────────────

const INTERNAL_TOOL_NAMES = new Set(['record_task_understanding', 'pending_user_message']);
const isInternalTool = (toolName: string) => INTERNAL_TOOL_NAMES.has(toolName);
const isRestrictedNetworkExploration = (toolName: string) => toolName === 'delegate_network_exploration';
const LIVE_RUN_PHASES = new Map([
  ['正在理解你的请求', '正在理解请求'],
  ['正在分析任务并规划下一步', '正在规划下一步'],
  ['正在规划下一步', '正在规划下一步'],
  ['正在生成回复', '正在生成回复'],
  ['正在整理结果', '正在整理结果'],
  ['正在继续任务', '正在继续任务'],
  ['正在基于编辑后的消息重新生成', '正在重新生成'],
]);

/**
 * A failed confirmation IPC request has not changed the durable decision.
 * Restore its local control instead of presenting a false terminal result.
 */
export function restoreRetryableLiveConfirmation(
  blocks: LiveBlock[],
  callId: string,
): LiveBlock[] {
  return blocks.map((block) => (
    block.kind === 'tool_call' && block.callId === callId
      ? {
          ...block,
          status: 'needs_approval',
          error: '确认处理失败，请重新检查后重试',
        }
      : block
  ));
}

const UNKNOWN_ACTION_CONFIRMATION = `Error: ${JSON.stringify({
  code: 'RESULT_UNKNOWN',
  message: '确认请求状态未知；目标应用可能已改变。请先在应用中核对，勿直接重试。',
})}`;

function markUnknownLiveConfirmation(blocks: LiveBlock[], callId: string): LiveBlock[] {
  return blocks.map((block) => block.kind === 'tool_call' && block.callId === callId
    ? { ...block, status: 'failed', error: UNKNOWN_ACTION_CONFIRMATION }
    : block);
}

function markUnknownPersistedConfirmation(messageId: string, callId: string): void {
  useMessagesStore.setState((state) => ({
    messages: state.messages.map((message) => {
      if (message.id !== messageId) return message;
      const toolName = message.toolCalls?.find((tool) => tool.id === callId)?.name;
      if (!toolName || (toolName !== 'run_project_command'
        && !requiresDesktopActionPreview(toolName) && !toolName.startsWith('mcp_'))) return message;
      const result: ToolResult = {
        callId,
        toolName,
        success: false,
        output: UNKNOWN_ACTION_CONFIRMATION,
        error: UNKNOWN_ACTION_CONFIRMATION,
        confirmationRequired: false,
        confirmationStatus: 'unknown',
      };
      return {
        ...message,
        toolResults: [...(message.toolResults ?? []).filter((item) => item.callId !== callId), result],
        taskRun: message.taskRun ? {
          ...message.taskRun,
          status: 'needs_attention',
          confirmationState: 'unknown',
          resumable: false,
        } : undefined,
      };
    }),
  }));
}

const ToolExecutingIndicator = ({ toolName }: { toolName: string }) => {
  if (isInternalTool(toolName)) return null;
  return (
    <div className="tool-executing-indicator">
      <span className="spinner spinner-sm"></span>
      <span>正在执行 <strong>{getToolDisplayName(toolName)}</strong>…</span>
    </div>
  );
};

/** Small inline SVG icon for badge status */
function InlineStatusIcon({ status }: { status: string }) {
  if (status === 'completed') {
    return (
      <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
        <path d="M2 5.5L4 7.5L8 3" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
      </svg>
    );
  }
  if (status === 'failed') {
    return (
      <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
        <path d="M3 3L7 7M7 3L3 7" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
      </svg>
    );
  }
  if (status === 'needs_approval') {
    return (
      <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
        <circle cx="5" cy="5" r="4" stroke="currentColor" strokeWidth="1.2" />
        <rect x="4" y="4" width="2" height="2" rx="0.5" fill="currentColor" />
      </svg>
    );
  }
  return (
    <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
      <circle cx="5" cy="5" r="3.5" stroke="currentColor" strokeWidth="1" opacity="0.4" />
      <circle cx="5" cy="5" r="2" fill="currentColor" />
    </svg>
  );
}


function parseToolArguments(tool: ToolCall): Record<string, unknown> {
  try {
    if (typeof tool.arguments === 'string') {
      return JSON.parse(tool.arguments);
    }
    if (typeof tool.arguments === 'object' && tool.arguments !== null) {
      return tool.arguments as Record<string, unknown>;
    }
  } catch {
    return { raw: tool.arguments };
  }
  return {};
}

class AgentRunDetailsBoundary extends Component<
  { children: ReactNode },
  { failed: boolean }
> {
  state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error('Agent run details failed to render', error, info);
  }

  render() {
    if (this.state.failed) {
      return <div className="agent-run-details-fallback" role="status">技术详情暂时无法显示；对话和运行记录仍可继续使用。</div>;
    }
    return this.props.children;
  }
}

function truncateTechnicalOutput(output: unknown, limit = 4_000): string {
  const text = typeof output === 'string' ? output : formatArgValue(output);
  return text.length > limit ? `${text.slice(0, limit)}\n… 输出已截断` : text;
}

function isFailedToolResult(toolName: string, result?: ToolResult): boolean {
  return result?.success === false
    && result.confirmationStatus !== 'pending'
    && result.confirmationStatus !== 'cancelled'
    && result.confirmationStatus !== 'rejected'
    && toolActionReceipt(toolName, result)?.status !== 'result_unknown';
}

const AgentStepTechnicalDetails = ({
  tool,
  result,
  recalled,
}: {
  tool: ToolCall;
  result?: ToolResult;
  recalled: string[];
}) => {
  const [argumentsOpen, setArgumentsOpen] = useState(false);
  const [outputOpen, setOutputOpen] = useState(false);
  const pendingDesktopOperation = desktopConfirmationOperation(tool.name, tool.arguments);
  const redactPendingDesktopAction = requiresDesktopConfirmationPreview(tool.name, tool.arguments)
    && result?.confirmationRequired && result.confirmationStatus === 'pending';
  const redactNetworkScope = isRestrictedNetworkExploration(tool.name) || redactPendingDesktopAction;
  const args = redactNetworkScope ? {} : parseToolArguments(tool);
  const output = result?.success ? result.output : result?.error ?? result?.output;
  const failed = result?.success === false && result.confirmationStatus !== 'rejected';

  return (
    <div className="agent-step-technical-details">
      {redactNetworkScope ? (
        <div className="agent-step-arguments">
          <code>{tool.name}</code>
          <div className="agent-step-args">
            <span>{redactPendingDesktopAction
              ? pendingDesktopOperation === 'image'
                ? '授权应用以实时预检卡片为准；本次核对不读取或截取窗口。'
              : pendingDesktopOperation !== 'draft' && pendingDesktopOperation !== 'field'
                ? '目标窗口和待操作控件以实时预检卡片为准。'
                : '目标窗口和待填写内容以实时预检卡片为准。'
              : getToolConfirmationSummary(tool.name, tool.arguments)}</span>
          </div>
        </div>
      ) : (
        <>
          <button
            type="button"
            className="agent-step-detail-toggle"
            aria-expanded={argumentsOpen}
            onClick={() => setArgumentsOpen((open) => !open)}
          >
            {argumentsOpen ? '收起调用参数' : '查看调用参数'}
          </button>
          {argumentsOpen && (
            <div className="agent-step-arguments">
              <code>{tool.name}</code>
              {Object.keys(args).length > 0 && (
                <div className="agent-step-args">
                  {Object.entries(args).map(([key, value]) => (
                    <span key={key}>
                      <strong>{key}</strong>: {formatArgValue(value)}
                    </span>
                  ))}
                </div>
              )}
            </div>
          )}
        </>
      )}

      {recalled.length > 0 && (
        <div className="agent-recalled-memories">
          <span className="agent-recalled-label">已检索的记忆</span>
          {recalled.map((memory, memoryIndex) => (
            <div className="agent-recalled-memory" key={`${tool.id}-${memoryIndex}`}>
              {memory}
            </div>
          ))}
          <div>
            <button type="button" className="agent-step-detail-toggle"
              onClick={() => window.dispatchEvent(new CustomEvent('open-settings', { detail: 'memory' }))}>
              查看与更正已保存的记忆
            </button>
          </div>
        </div>
      )}

      {!redactNetworkScope && result && recalled.length === 0 && output != null && (
        <div className={`agent-step-output ${failed ? 'error' : ''}`}>
          {failed && result.error && (
            <div className="agent-step-output-summary">{truncateTechnicalOutput(result.error, 600)}</div>
          )}
          {/* Issue #102: post-call verifier mismatch — render a red note
              that calls out the disk-truth gap. Soft warning, not a hard
              block. */}
          {result.success && result.verification && result.verification.matched === false && (
            <div className="agent-step-verification-mismatch">
              [验证失败] {result.verification.target} —{' '}
              {result.verification.note || '工具声称成功，但磁盘上未找到产物'}
            </div>
          )}
          <button
            type="button"
            className="agent-step-detail-toggle"
            aria-expanded={outputOpen}
            onClick={() => setOutputOpen((open) => !open)}
          >
            {outputOpen ? '收起工具输出' : '查看工具输出'}
          </button>
          {outputOpen && <pre>{truncateTechnicalOutput(output)}</pre>}
        </div>
      )}
    </div>
  );
};

function getStepVerb(toolName: string) {
  return getToolDisplayName(toolName);
}

function ToolActionReceipts({
  toolCalls = [],
  toolResults = [],
  showUnknown = true,
}: {
  toolCalls?: ToolCall[];
  toolResults?: ToolResult[];
  showUnknown?: boolean;
}) {
  const results = authoritativeToolResultsByCallId(toolResults);
  const receipts = toolCalls.flatMap((tool) => {
    const receipt = toolActionReceipt(tool.name, results.get(tool.id));
    return receipt && (showUnknown || receipt.status !== 'result_unknown') ? [{ tool, receipt }] : [];
  });
  if (receipts.length === 0) return null;

  return (
    <div className="desktop-action-receipts" role="list" aria-label="操作结果">
      {receipts.map(({ tool, receipt }) => (
        <div className={`desktop-action-receipt desktop-action-receipt--${receipt.status}`} role="listitem" key={tool.id}>
          <div className="desktop-action-receipt-heading">
            <strong>{getToolDisplayName(tool.name)}</strong>
            <span>{receipt.label}</span>
          </div>
          <p>{receipt.explanation}</p>
        </div>
      ))}
    </div>
  );
}

function ResultReviewCard({ message, onOpenFiles }: { message: Message; onOpenFiles?: () => void }) {
  const calls = unknownResultCalls(message);
  const call = calls.length === 1 ? calls[0] : undefined;
  const file = call && ['write_file', 'edit_file'].includes(call.name)
    ? workspaceFilePath(parseToolArguments(call).path)
    : null;
  const allWorkspaceActions = calls.every(({ name }) => ['run_project_command', 'write_file', 'edit_file'].includes(name));
  const detail = call?.name === 'prepare_message_draft' ? '草稿可能已写入。请先到目标应用核对。'
    : call?.name === 'set_trusted_app_text' ? '输入框可能已改动。请先到目标应用核对。'
    : allWorkspaceActions ? '工作区文件可能已改动。请先核对实际结果，再决定下一步。'
    : '操作可能已经生效。请先核对目标应用或服务，再决定下一步。';
  return (
    <section className="task-recovery-card task-result-review" aria-label="核对操作结果">
      <div className="task-recovery-header">
        <strong>结果待核对</strong>
        {onOpenFiles && allWorkspaceActions && (
          <button type="button" onClick={file ? () => locateWorkspaceFile(file) : onOpenFiles}>
            {file ? '核对目标文件' : '查看工作区文件'}
          </button>
        )}
      </div>
      <p>{detail}</p>
      {file && <p className="task-result-review__path">{file}</p>}
    </section>
  );
}

const TASK_RECOVERY_LABELS: Readonly<Partial<Record<string, string>>> = {
  needs_attention: '任务未完成',
  continue_suggested: '任务可继续',
  stopped: '任务已停止',
  paused: '任务已暂停',
  failed: '任务失败',
  provider_unavailable: '模型连接不可用',
};

function latestConversationMessage(messages: readonly Message[], sessionId: string | undefined) {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message.sessionId === sessionId && (message.role === 'assistant' || message.role === 'user')) return message;
  }
}

type TaskRecoveryAction = 'continue' | 'resultsOnly';
// Keep the affordance aligned with foreground_results_reply.rs; the backend
// still decides eligibility from the authoritative journal.
const RESULTS_REPLY_SOURCES = new Set(['read_file', 'list_workspace_items', 'recall_conversation', 'read_conversation', 'read_tool_result']);

function hasSavedWebSearchSource(output: string | undefined): boolean {
  try {
    const data = JSON.parse(output ?? '');
    return data?.kind === 'web_search_evidence' && data.trust === 'untrusted_external'
      && typeof data.provider === 'string' && typeof data.truncated === 'boolean'
      && Array.isArray(data.results) && data.results.some((item: unknown) => {
        if (typeof item !== 'object' || !item) return false;
        const { title, url, snippet } = item as Record<string, unknown>;
        if (typeof title !== 'string' || typeof snippet !== 'string' || typeof url !== 'string' || url.length > 2048) return false;
        try {
          const source = new URL(url);
          return ['https:', 'http:'].includes(source.protocol) && !!source.hostname && !source.username && !source.password;
        } catch { return false; }
      });
  } catch { return false; }
}

function taskRecoveryState(message: Message) {
  const results = authoritativeToolResultsByCallId(message.toolResults ?? []);
  const unknown = message.toolResults?.some(isResultUnknown) ?? false;
  const awaitingApproval = [...results.values()].some((result) =>
    result.confirmationStatus === 'pending' || (!result.success && /^(?:Error: )?Confirmation required/.test(result.output ?? '')));
  const successful = [...results.values()].filter((result) => result.success
    && !isInternalTool(result.toolName) && result.toolName !== 'agent_loop'
    && result.confirmationStatus !== 'rejected' && result.confirmationStatus !== 'pending' && result.confirmationStatus !== 'cancelled'
    && result.verification?.matched !== false);
  const run = message.taskRun;
  let action: TaskRecoveryAction | undefined;
  if (run?.id === message.id && run.confirmationState !== 'pending' && !unknown && !awaitingApproval) {
    if (run.resumable) action = 'continue';
    else if (run.status === 'provider_unavailable' && run.resumable === false
      && successful.some(result => RESULTS_REPLY_SOURCES.has(result.toolName)
        || (result.toolName === 'web_search' && hasSavedWebSearchSource(result.output)))) action = 'resultsOnly';
  }
  return { lastSuccess: successful.at(-1), unknown, action };
}

function TaskRecoveryCard({ message, disabled, notice, onRecover, canLocateFiles }: {
  message: Message;
  disabled: boolean;
  notice?: string;
  onRecover: (action: TaskRecoveryAction) => void;
  canLocateFiles: boolean;
}) {
  const { lastSuccess, unknown, action } = taskRecoveryState(message);
  const fileOperation = lastSuccess && ['read_file', 'write_file', 'edit_file'].includes(lastSuccess.toolName)
    ? message.toolCalls?.find((tool) => tool.id === lastSuccess.callId && tool.name === lastSuccess.toolName)
    : undefined;
  const path = fileOperation && parseToolArguments(fileOperation)?.path;
  const lastFile = typeof path === 'string' && path.trim() ? path.trim().replace(/\s+/g, ' ') : '';
  const files = message.taskFacts?.modifiedFiles ?? [];
  const lastFilePath = canLocateFiles ? workspaceFilePath(path) : null;
  const lastFileLabel = lastFile.length > 120 ? `${lastFile.slice(0, 120)}…` : lastFile;
  const showLastFileLink = lastFilePath && !files.some(file => workspaceFilePath(file) === lastFilePath);

  return (
    <section className="task-recovery-card" aria-label="任务恢复">
      <div className="task-recovery-header">
        <strong>{unknown ? '结果待核对' : TASK_RECOVERY_LABELS[message.taskRun?.status ?? '']}</strong>
        {action && <button type="button" disabled={disabled} onClick={() => onRecover(action)}>
          {action === 'resultsOnly' ? '用已有结果回答' : '继续任务'}
        </button>}
        {(message.taskRun?.status === 'provider_unavailable' || message.modelRequestFailed) && (
          <button type="button" onClick={openModelSettings}>检查模型连接</button>
        )}
      </div>
      {lastSuccess && <p>
        最近成功 · {getToolDisplayName(lastSuccess.toolName)}
        {lastFile && <> · {showLastFileLink ? <WorkspaceFileLink path={lastFilePath}>{lastFileLabel}</WorkspaceFileLink> : lastFileLabel}</>}
      </p>}
      {files.length > 0 && <ul aria-label="本轮已修改的文件">
        {files.map((path) => {
          const relativePath = workspaceFilePath(path);
          return <li key={path}>
            {relativePath && canLocateFiles ? <button type="button" className="message-file-link"
              title={`在工作台中定位 ${relativePath}`}
              onClick={() => locateWorkspaceFile(relativePath)}>
              {path}
            </button> : path}
          </li>;
        })}
      </ul>}
      {unknown && <p>请先核对实际结果。</p>}
      {notice && <p role="alert">{notice}</p>}
    </section>
  );
}

/** A user-visible explanation of an audited action, never hidden chain-of-thought. */
function getStepContext(tool: ToolCall, result?: ToolResult) {
  if (result?.confirmationRequired && result.confirmationStatus === 'pending') {
    if (desktopConfirmationOperation(tool.name, tool.arguments) === 'image') return '等待核对授权应用与窗口图像观察范围。';
    const desktopOperation = desktopActionOperation(tool.name, tool.arguments);
    if (requiresDesktopActionPreview(tool.name)) return desktopOperation !== 'draft' && desktopOperation !== 'field'
      ? '等待核对目标窗口和待操作控件。'
      : '等待核对目标窗口和待填写内容。';
    return getToolConfirmationSummary(tool.name, tool.arguments);
  }
  if (result?.confirmationStatus === 'rejected') return '你已拒绝这项操作；未执行该请求，不会自动重试。';
  if (isRestrictedNetworkExploration(tool.name)) {
    if (result?.success === false) return '受限联网探索委派未能建立。';
    if (result?.success) return '已创建受限联网探索委派。';
    return '正在准备受限联网探索委派。';
  }
  const desktopReceipt = toolActionReceipt(tool.name, result);
  if (desktopReceipt) return desktopReceipt.explanation;
  if (result?.success === false) return result.error ?? result.output ?? '该操作未能完成。';
  try {
    const args = JSON.parse(tool.arguments) as Record<string, unknown>;
    const path = typeof args.path === 'string' ? args.path : undefined;
    const query = typeof args.query === 'string' ? args.query : undefined;
    const command = typeof args.command === 'string' ? args.command : undefined;
    if (path) return path;
    if (query) return query.length > 96 ? `${query.slice(0, 96)}…` : query;
    if (command) return command;
  } catch {
    // Raw arguments stay available in technical details; the timeline remains safe.
  }
  return result?.success ? '已完成并记录结果。' : '正在执行此操作。';
}

function getRecoveryHint(toolName: string, result?: ToolResult) {
  const detail = `${result?.error ?? ''}\n${result?.output ?? ''}`.toLowerCase();
  if (toolName === 'write_file' && /missing (field|required argument)|arguments are invalid/.test(detail)) {
    return '检查操作参数是否包含目标路径和内容，然后重试。';
  }
  if (toolName === 'write_file') return '检查工作目录，并确认目标路径位于允许的工作区内。';
  if (toolName === 'recall_memories') return '换用更具体的记忆查询，或在设置中检查记忆治理。';
  const mcp = parseMcpToolIdentity(toolName);
  if (mcp) return `检查服务“${mcp.server}”是否仍在运行并提供“${mcp.tool}”，然后重试。`;
  if (toolName.startsWith('mcp_')) return '检查外部服务是否仍在运行并向当前工作区启用，然后重试。';
  return '检查本次操作的参数，调整请求后重新执行。';
}

function parseRecalledMemories(output: string) {
  const trimmed = output.trim();
  if (!trimmed || /^(?:no .*memor|本次没有找到相关记忆|本次沒有找到相關記憶)/i.test(trimmed)) return [];
  return trimmed.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
}

function getStepStatusLabel(needsApproval: boolean, success: boolean | undefined, resultLabel?: string) {
  if (needsApproval) return '等待确认';
  if (success === undefined) return '执行中';
  return resultLabel ?? (success ? '已完成' : '需要处理');
}

/**
 * AgentRunSteps — integrated tool call display for desktop UX.
 *
 * Design goals:
 * - Tools are NOT shown as a separate "execution record" panel.
 * - Instead, completed tools appear as compact inline badges in the message flow.
 * - Confirmation-required tools show an inline card with approve/reject buttons.
 * - Failed tools show a simple error hint (no "continue from this step" button).
 * - Technical details (arguments, output) are hidden by default — accessible via
 *   "查看技术详情" button that expands the full step list.
 */
const AgentRunSteps = ({
  toolCalls = [],
  toolResults = [],
  taskCompleted,
  reviewShown = false,
  sessionId,
  messageId,
  desktopRecheck,
  onResolveConfirmation,
}: {
  toolCalls?: ToolCall[];
  toolResults?: ToolResult[];
  taskCompleted: boolean;
  reviewShown?: boolean;
  sessionId?: string;
  messageId: string;
  desktopRecheck?: DesktopApprovalRecheck;
  onResolveConfirmation?: (
    messageId: string,
    callId: string,
    decision: 'approved' | 'rejected',
    previewId?: string,
  ) => DesktopApprovalResolution | Promise<DesktopApprovalResolution>;
}) => {
  const [detailsOpen, setDetailsOpen] = useState(false);
  const safeToolCalls = Array.isArray(toolCalls)
    ? toolCalls.filter((tool): tool is ToolCall => Boolean(tool && tool.id && tool.name) && !isInternalTool(tool.name))
    : [];
  const visibleCallIds = new Set(safeToolCalls.map((tool) => tool.id));
  const safeToolResults = Array.isArray(toolResults)
    ? toolResults.filter((result) => visibleCallIds.has(result.callId))
    : [];

  if (safeToolCalls.length === 0) return null;

  // Every aggregate is derived from the same settled map used by the cards.
  // Otherwise duplicate persisted rows for A could make unfinished B look
  // complete, or let a stale pending row revive a terminal confirmation.
  const resultMap = authoritativeToolResultsByCallId(safeToolResults);
  const effectiveToolResults = safeToolCalls.flatMap((tool) => {
    const result = resultMap.get(tool.id);
    return result ? [result] : [];
  });

  // Check for pending confirmations and failures
  const hasPendingConfirmation = safeToolCalls.some((tool) => {
    const result = resultMap.get(tool.id);
    return result?.confirmationRequired && result.confirmationStatus === 'pending';
  });
  const hasUnknownActionResult = safeToolCalls.some((tool) =>
    toolActionReceipt(tool.name, resultMap.get(tool.id))?.status === 'result_unknown'
  );
  const hasFailure = safeToolCalls.some((tool) => isFailedToolResult(tool.name, resultMap.get(tool.id)));
  const hasRejection = effectiveToolResults.some((result) => result.confirmationStatus === 'rejected');
  const completedCount = effectiveToolResults.filter((r) => r.success).length;
  const terminalCount = completedCount
    + effectiveToolResults.filter((result) => result.confirmationStatus === 'cancelled').length;
  const isPassive = !hasPendingConfirmation && !hasFailure && !hasRejection && terminalCount === safeToolCalls.length;

  // Successful implementation details are not part of the user's main
  // conversation. The final assistant response and project worktree carry the
  // result; only confirmations, rejections, failures, or genuinely incomplete work need
  // to interrupt the reading flow.
  if (isPassive) return null;

  // Unknown results and outstanding approvals still take precedence over
  // a completed task. Known failed receipts stay available as history.
  const [summaryStatus, summaryText] = hasUnknownActionResult && reviewShown ? ['details', '查看执行记录']
    : hasUnknownActionResult ? ['unknown', '有操作结果待核对']
    : hasPendingConfirmation ? ['needs_approval', '等待确认']
    : hasFailure ? (taskCompleted ? ['completed', '任务已完成 · 查看执行记录'] : ['failed', '有步骤失败'])
    : hasRejection ? ['cancelled', '已按你的决定取消操作']
    : ['running', `${completedCount}/${safeToolCalls.length} 已完成`];

  return (
    <div className="agent-run-integrated">
      {/* Compact badge row — primary display */}
      {hasPendingConfirmation && (
      <div className="agent-run-badges">
        {safeToolCalls.filter((tool) => {
          const result = resultMap.get(tool.id);
          return result?.confirmationRequired && result.confirmationStatus === 'pending';
        }).map((tool) => {
          const result = resultMap.get(tool.id);
          const needsApproval = result?.confirmationRequired && result.confirmationStatus === 'pending';
          const status: 'running' | 'completed' | 'failed' | 'needs_approval' =
            needsApproval
              ? 'needs_approval'
              : result?.success === false
                ? 'failed'
                : result?.success === true
                  ? 'completed'
                  : 'running';
          const desktopOperation = desktopConfirmationOperation(tool.name, tool.arguments);
          const needsDesktopPreview = requiresDesktopConfirmationPreview(tool.name, tool.arguments);

          return (
            <div key={tool.id} className={`agent-run-badge-row agent-run-badge-row--${status}`}>
              <span className={`agent-inline-badge agent-inline-badge--${status}`}>
                <InlineStatusIcon status={status} />
                <span>{getStepVerb(tool.name)}</span>
              </span>
              {needsApproval && needsDesktopPreview && onResolveConfirmation && (
                <DesktopActionApproval
                  sessionId={sessionId}
                  messageId={messageId}
                  callId={tool.id}
                  operation={desktopOperation}
                  layout="compact"
                  recheck={desktopRecheck}
                  approvalBlocked={hasUnknownActionResult}
                  onResolve={(decision, previewId) => onResolveConfirmation(messageId, tool.id, decision, previewId)}
                />
              )}
              {needsApproval && !needsDesktopPreview && onResolveConfirmation && (
                <ToolActionApproval
                  toolName={tool.name}
                  arguments={tool.arguments}
                  layout="compact"
                  approvalBlocked={hasUnknownActionResult}
                  onResolve={(decision) => onResolveConfirmation(messageId, tool.id, decision)}
                />
              )}
            </div>
          );
        })}
      </div>
      )}

      {/* Show summary + toggle only when there's something to report */}
      {(
        <button
          type="button"
          className={`agent-run-disclosure agent-run-disclosure--${summaryStatus}`}
          aria-expanded={detailsOpen}
          onClick={() => setDetailsOpen((v) => !v)}
        >
          <span className="agent-run-disclosure-chevron" aria-hidden="true">
            {detailsOpen ? '▾' : '▸'}
          </span>
          <span className="agent-run-disclosure-title">{summaryText}</span>
          <span className="agent-run-disclosure-count">{safeToolCalls.length} 项调用</span>
        </button>
      )}

      {/* Expanded technical details */}
      {detailsOpen && (
        <ol className="agent-step-list">
          {safeToolCalls.map((tool) => {
            const result = resultMap.get(tool.id);
            const success = result?.success;
            const needsApproval = result?.confirmationRequired && result.confirmationStatus === 'pending';
            const rejected = result?.confirmationStatus === 'rejected';
            const recalled = tool.name === 'recall_memories' && result?.success
              ? parseRecalledMemories(result.output)
              : [];

            return (
              <li
                key={tool.id}
                className={`agent-step ${needsApproval ? 'approval' : rejected ? 'cancelled' : success === false ? 'error' : success === true ? 'success' : 'pending'}`}
              >
                <div className="agent-step-index">{safeToolCalls.indexOf(tool) + 1}</div>
                <div className="agent-step-body">
                  <div className="agent-step-topline">
                    <span className="agent-step-title">{getStepVerb(tool.name)}</span>
                    <span className="agent-step-status">
                      {rejected ? '已拒绝' : getStepStatusLabel(Boolean(needsApproval), success, toolActionReceipt(tool.name, result)?.label)}
                    </span>
                  </div>
                  <p className="agent-step-context">{getStepContext(tool, result)}</p>

                  <AgentRunDetailsBoundary>
                    <AgentStepTechnicalDetails tool={tool} result={result} recalled={recalled} />
                  </AgentRunDetailsBoundary>

                  {!taskCompleted && result?.success === false && !needsApproval && !rejected && !toolActionReceipt(tool.name, result) && (
                    <div className="agent-step-recovery">
                      <span>此操作未完成：{getRecoveryHint(tool.name, result)}</span>
                    </div>
                  )}
                </div>
              </li>
            );
          })}
        </ol>
      )}
    </div>
  );
};

// ─── Message Bubble ───────────────────────────────────────────────────────────

function completedTimelineText(blocks: LiveBlock[]): string {
  return groupBlocksIntoSegments(blocks)
    .flatMap((segment) => segment.kind === 'text' ? [segment.content.trim()] : [])
    .filter(Boolean)
    .join('\n\n');
}

function timelineCandidates(messages: readonly Message[], sessionId: string) {
  const latest = latestConversationMessage(messages, sessionId);
  const candidates = new Set<string>();
  let hasFollowingToolRow = false;
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message.sessionId !== sessionId) continue;
    if (message.role === 'tool') {
      hasFollowingToolRow = true;
    } else if (message.role === 'user' || message.role === 'assistant') {
      if (message.role === 'assistant' && (message.toolCalls?.length || message.toolResults?.length
        || message.timelineBlocks?.length || hasFollowingToolRow
        || (message === latest && message.taskRun?.status === 'needs_attention'))) {
        candidates.add(message.id);
      }
      hasFollowingToolRow = false;
    }
  }
  return candidates;
}

function withoutReplayedToolRows(messages: Message[]) {
  let replayingAssistantTurn = false;
  return messages.filter((message) => {
    if (message.role === 'user') replayingAssistantTurn = false;
    if (message.role === 'assistant') replayingAssistantTurn = Boolean(message.timelineBlocks?.length);
    return message.role !== 'tool' || !replayingAssistantTurn;
  });
}

function retainUnchangedTimelines(source: Message[], previous: Message[], sessionId: string) {
  const latestId = latestConversationMessage(source, sessionId)?.id;
  const hydrated = new Map(previous.filter((message) => message.timelineBlocks?.length || message.modelRequestFailed)
    .map((message) => [message.id, message]));
  return source.map((message) => {
    const prior = hydrated.get(message.id);
    if (!prior || prior.sessionId !== sessionId || message.timelineBlocks?.length) return message;
    const { timelineBlocks, modelRequestFailed, ...saved } = prior;
    const { timelineBlocks: _timeline, modelRequestFailed: _failure, ...fresh } = message;
    // A refreshed durable approval or receipt always outranks cached event
    // decoration. Preserve an existing timeline only for the same reply data.
    const keys = Object.keys(fresh) as (keyof typeof fresh)[];
    if (keys.length !== Object.keys(saved).length || keys.some((key) =>
      JSON.stringify(fresh[key]) !== JSON.stringify(saved[key]))) return message;
    if (!modelRequestFailed || message.id === latestId) return prior;
    return timelineBlocks ? { ...fresh, timelineBlocks } : message;
  });
}

async function readMessageTimeline(sessionId: string, message: Message, latest: boolean): Promise<Message> {
  const events = (await getDurableAgentRunEvents(sessionId, message.id))
    .filter((event) => event.run_id === message.id && event.session_id === sessionId);
  const modelRequestFailed = latest && message.taskRun?.status === 'needs_attention'
    && events.some(({ payload }) => {
      if (payload.type !== 'Error') return false;
      const code = (payload.data as { code?: string } | null)?.code;
      return typeof code === 'string' && code.startsWith('Llm');
    });
  const timelineBlocks = contentBlocksFromDurableEvents(events, message.toolResults);
  if (!timelineBlocks.length) return modelRequestFailed ? { ...message, modelRequestFailed } : message;

  // Append terminal text saved on the reply when no text delta carried it.
  const finalText = message.content.trim();
  const emittedText = timelineBlocks.filter((block) => block.kind === 'text').map((block) => block.content).join('');
  if (finalText && !emittedText.includes(finalText)) {
    const lastBlock = timelineBlocks[timelineBlocks.length - 1];
    timelineBlocks.push({
      kind: 'text',
      index: lastBlock.index + 1,
      iterationId: lastBlock.iterationId + 1,
      content: message.content,
    });
  }
  return modelRequestFailed ? { ...message, timelineBlocks, modelRequestFailed } : { ...message, timelineBlocks };
}

function HistoricalRunDetails({ message, onLoad }: {
  message: Message;
  onLoad: (message: Message) => Promise<Message | undefined>;
}) {
  const [open, setOpen] = useState(false);
  const [loaded, setLoaded] = useState<{ source: Message; result: Message }>();
  const [loading, setLoading] = useState(false);
  const load = async () => {
    setLoading(true);
    try {
      const result = await onLoad(message);
      if (result) setLoaded({ source: message, result });
    } finally {
      setLoading(false);
    }
  };
  const loadedCurrent = loaded?.source === message;
  const blocks = message.timelineBlocks ?? (loadedCurrent ? loaded.result.timelineBlocks : undefined);
  const steps = blocks?.filter((block): block is Extract<LiveBlock, { kind: 'tool_call' }> =>
    block.kind === 'tool_call' && !isInternalTool(block.toolName) && block.toolName !== 'agent_loop');
  return <details className="agent-run-integrated" onToggle={(event) => {
    setOpen(event.currentTarget.open);
    if (event.currentTarget.open && !blocks && !loadedCurrent && !loading) void load();
  }}>
    <summary className="agent-run-summary">查看运行过程</summary>
    {open && (loading ? <p role="status">正在读取运行记录…</p>
      : !blocks && !loadedCurrent ? <p><button type="button" onClick={() => void load()}>重新读取运行记录</button></p>
        : steps?.length ? <ol aria-label="已记录的运行步骤">
          {steps.map((step) => <li key={step.index}>{getToolDisplayName(step.toolName)} · {{
            running: '结束状态未记录', completed: '完成', failed: '失败', needs_approval: '曾等待确认',
          }[step.status]}</li>)}
        </ol> : <p>没有可显示的运行步骤。</p>)}
  </details>;
}

/** Hide only the empty shell of tool details that are already intentionally hidden. */
function isEmptySuccessfulToolTurn(message: Message): boolean {
  if (message.role !== 'assistant' || message.content.trim()
    || completedTimelineText(message.timelineBlocks ?? [])
    || message.metadata?.isToolExecuting || !message.toolCalls?.length) return false;
  const results = authoritativeToolResultsByCallId(message.toolResults ?? []);
  return message.toolCalls.every((tool) => {
    const result = results.get(tool.id);
    return result?.success
      && (!result.confirmationStatus || result.confirmationStatus === 'approved')
      && !toolActionReceipt(tool.name, result);
  });
}

// ─── Personality Mapping ──────────────────────────────────────────────────────

function mapPersonalityTraits(tone: string, personality: string): TraitConfig {
  const archetypeDefaults: Record<string, TraitConfig> = {
    balanced:   { tone: 0, verbosity: 0, formality: 0, humor: 0, dependence: 0, intimacy: 0, patience: 0 },
    cheerful:   { tone: -3, verbosity: 3, formality: 1, humor: 3, dependence: 2, intimacy: 2, patience: 2 },
    serious:    { tone: 2, verbosity: -1, formality: -2, humor: -3, dependence: -2, intimacy: -2, patience: 1 },
    cute:       { tone: -4, verbosity: 2, formality: 2, humor: 2, dependence: 3, intimacy: 3, patience: 3 },
    cool:       { tone: 3, verbosity: -2, formality: -1, humor: 1, dependence: -2, intimacy: -2, patience: -1 },
  };
  const base = archetypeDefaults[personality] ?? archetypeDefaults.balanced;
  const toneMap: Record<string, number> = { friendly: -2, neutral: 0, professional: 2, tsundere: 3, gentle: -4, energetic: -3 };
  const toneOffset = toneMap[tone] ?? 0;
  return { ...base, tone: toneOffset };
}

function mapPreferences(prefs: {
  communication: { preferredTone: string[]; dislikedWords: string[]; petPeeves: string[] };
  habits: { greetingStyle: string; responseLength: string; responseLanguage?: string; useLongTermMemory?: boolean };
  topics: { interests: string[]; avoidTopics: string[] };
  learnedAt: number; evolutionEnabled: boolean;
}) {
  return {
    responseLength: prefs.habits.responseLength || 'medium',
    responseLanguage: prefs.habits.responseLanguage || 'auto',
    useLongTermMemory: prefs.habits.useLongTermMemory ?? true,
    interests: prefs.topics.interests,
    avoidTopics: prefs.topics.avoidTopics,
    dislikedWords: prefs.communication.dislikedWords,
    petPeeves: prefs.communication.petPeeves,
    evolutionEnabled: prefs.evolutionEnabled,
  };
}

function currentForegroundInteraction(): ForegroundInteractionPayload {
  const profile = useSettingsStore.getState().activeProfile;
  return {
    personality: {
      name: profile.name,
      description: profile.characterCard
        ? cardDescription({ name: profile.name, description: profile.bio, firstMes: profile.greeting, card: profile.characterCard })
        : profile.bio,
      greeting: profile.greeting,
      traits: profile.traits ?? mapPersonalityTraits(profile.tone, profile.personality),
      supplement: profile.personaNotes ?? '',
    },
    preferences: mapPreferences(usePreferencesStore.getState().preferences),
    thinkingEffort: useThinkingEffortStore.getState().effort,
  };
}

// ─── Main ChatArea Component ──────────────────────────────────────────────────

function titleFromFirstUserMessage(content: string) {
  const normalized = content.replace(/\s+/g, ' ').trim();
  const characters = Array.from(normalized);
  return characters.length > 40 ? `${characters.slice(0, 40).join('')}…` : normalized;
}

type PauseAction = {
  sessionId: string;
  generation: number;
  stopEpoch: number;
  pending: boolean;
  paused: boolean;
  error?: string;
};

export function ChatArea({ onToggleWorkbench, onOpenWorkbenchView, workbenchOpen = false }: {
  onToggleWorkbench?: () => void;
  onOpenWorkbenchView?: (view: WorkspaceView) => void;
  workbenchOpen?: boolean;
}) {
  const session = useSessionsStore((state) => state.activeSession);
  const activeWorkspaceId = useWorkspacesStore((state) => state.activeWorkspaceId);
  const activeWorkspaceKind = useWorkspacesStore((state) => (
    state.workspaces.find((workspace) => workspace.id === state.activeWorkspaceId)?.kind ?? null
  ));
  const messages = useMessagesStore((state) => state.messages);
  const olderTimelineIds = useMemo(() => {
    if (!session?.id) return new Set<string>();
    return new Set([...timelineCandidates(messages, session.id)].slice(RECENT_TIMELINE_LIMIT));
  }, [messages, session?.id]);
  const latestMessage = latestConversationMessage(messages, session?.id);
  const resultToReview = useMemo(() => latestResultToReview(messages, session?.id), [messages, session?.id]);
  const replyDeliveries = useMemo(() => session?.id ? completedReplyDeliveries(messages, session.id) : null, [messages, session?.id]);
  const recoveryMessage = latestMessage?.role === 'assistant'
    && TASK_RECOVERY_LABELS[latestMessage.taskRun?.status ?? ''] ? latestMessage : undefined;
  const [recoveryNotice, setRecoveryNotice] = useState<{ messageId: string; text: string } | null>(null);
  const checkedModelFailure = useRef<string | null>(null);
  useEffect(() => {
    if (!recoveryMessage || checkedModelFailure.current === recoveryMessage.id
      || (recoveryMessage.taskRun?.status !== 'provider_unavailable' && !recoveryMessage.modelRequestFailed)) return;
    checkedModelFailure.current = recoveryMessage.id;
    // A revoked/expired plan may have been invalidated during this turn.
    // Recheck local metadata once; never sign in or retry the failed task.
    void useSettingsStore.getState().refreshApiConfigReadiness();
  }, [recoveryMessage]);
  const input = useMessagesStore((state) => state.inputSessionId === session?.id ? state.input : '');
  const backgroundImage = useAppearanceStore((state) => state.bgImage);
  const setInput = useMessagesStore((state) => state.setInput);
  const activateInput = useMessagesStore((state) => state.activateInput);
  const clearSubmittedInput = useMessagesStore((state) => state.clearSubmittedInput);
  const draftSaveFailed = useMessagesStore((state) => session ? state.draftSaveErrors.has(session.id) : false);
  const loadMessages = useMessagesStore((state) => state.loadMessages);
  const appendMessage = useMessagesStore((state) => state.appendMessage);
  const addToolResult = useMessagesStore((state) => state.addToolResult);
  const onMessageSent = useMessagesStore((state) => state.onMessageSent);
  const clearMessages = useMessagesStore((state) => state.clearMessages);
  const subscribeToAgentEvents = useMessagesStore((state) => state.subscribeToAgentEvents);
  const unsubscribeFromAgentEvents = useMessagesStore((state) => state.unsubscribeFromAgentEvents);
  const clearLiveActivity = useMessagesStore((state) => state.clearLiveActivity);
  const streamingText = useMessagesStore((state) => state.streamingText);
  const liveStatus = useMessagesStore((state) => state.liveStatus);
  const liveToolSteps = useMessagesStore((state) => state.liveToolSteps);
  const runningTool = Object.values(liveToolSteps)
    .filter((step) => step.status === 'running' && !isInternalTool(step.toolName)).at(-1);
  const runningToolLabel = runningTool && getToolDisplayName(runningTool.toolName);
  const liveRunPhase = runningToolLabel && runningToolLabel !== runningTool?.toolName
    ? `正在${runningToolLabel}`
    : LIVE_RUN_PHASES.get(liveStatus) ?? '正在执行';
  const liveBlocks = useMessagesStore((state) => state.liveBlocks);
  const liveMessageId = useMessagesStore((state) => state.liveMessageId);
  // The persisted step and the live stream may describe the same pending call.
  // Let the live card own its confirmation so the user sees one decision and
  // only one backend target preflight is started for that call.
  const livePendingCallIds = new Set(liveBlocks.flatMap((block) =>
    block.kind === 'tool_call' && block.status === 'needs_approval' ? [block.callId] : [],
  ));
  const pendingProjectMessageRef = useRef<{
    text: string; files: TextAttachment[]; workspaceId: string; sessionId: string | null; sourceSessionId?: string; sourceInputToken: object;
    resolve: (result: ComposerSendResult) => void;
  } | null>(null);
  const [routeVersion, setRouteVersion] = useState(0);
  const routingRequest = useRef<object | null>(null);
  const routeChoice = useRef<{ sessionId: string; inputToken: object; workspaceId: string | null } | null>(null);
  const [routingBusy, setRoutingBusy] = useState(false);
  const [routingDecision, setRoutingDecision] = useState<{ sessionId: string; inputToken: object; candidates: Workspace[]; error?: string } | null>(null);
  const [routingSelection, setRoutingSelection] = useState<{ sessionId: string; inputToken: object; label: string } | null>(null);
  const [routeNotice, setRouteNotice] = useState<{ workspaceId: string; name: string; sourceWorkspaceId: string; error?: string } | null>(null);
  useEffect(() => {
    routingRequest.current = null; routeChoice.current = null;
    setRoutingBusy(false); setRoutingDecision(null); setRoutingSelection(null);
  }, [session?.id]);
  useEffect(() => () => {
    const pending = pendingProjectMessageRef.current;
    pendingProjectMessageRef.current = null;
    routingRequest.current = null;
    pending?.resolve('not_sent');
  }, []);
  const composerInputRef = useRef<ComposerInputHandle>(null);
  const [resolvingCallId, setResolvingCallId] = useState<string | null>(null);
  const [isStreaming, setIsStreaming] = useState(false);
  const [historyAction, setHistoryAction] = useState<{
    sessionId: string; generation: number; pending: boolean; error?: boolean;
    kind?: 'load';
    notice?: string;
    edit?: { messageId: string; sourceContent: string; content: string };
  } | null>(null);
  const historyAttempt = useRef<symbol | null>(null);
  const [pauseAction, setPauseAction] = useState<PauseAction | null>(null);
  const pauseAttemptRef = useRef<PauseAction | null>(null);
  const [soundEnabled, setSoundEnabled] = useState(true);
  const [taskDecision, setTaskDecision] = useState<TaskDecisionProjection | null>(null);
  const isStreamingRef = useRef(false);
  // Session changes and new foreground chains take ownership of Main's UI;
  // older refreshes/finally blocks cannot overwrite that newer owner.
  const sessionGenerationRef = useRef(0);
  const stopEpochRef = useRef(0);
  const isStopping = useMessagesStore((state) => Boolean(session?.id && state.stoppingSessions.has(session.id)));
  const showRunControls = isStreaming || isStopping;
  const queuedFollowUps = useMessagesStore((state) => state.queuedFollowUps[session?.id ?? ''] ?? EMPTY_FOLLOW_UPS);
  const readyFollowUps = useRef<{ sessionId: string; generation: number; stopEpoch: number } | null>(null);
  const dispatchingFollowUps = useRef(new Set<string>());
  const [dispatchingFollowUp, setDispatchingFollowUp] = useState<string | null>(null);
  const waitingFollowUps = queuedFollowUps.filter((item) => item.id !== dispatchingFollowUp);
  isStreamingRef.current = isStreaming;

  // The backend returns a bounded, read-only projection. Personal space and
  // stale workspace state intentionally clear the card rather than showing a
  // decision from another project.
  useEffect(() => {
    if (!activeWorkspaceId || activeWorkspaceKind !== 'project') {
      setTaskDecision(null);
      return;
    }
    let cancelled = false;
    getWorkspaceTaskUnderstanding(activeWorkspaceId)
      .then((view) => {
        if (cancelled) return;
        setTaskDecision(view.action === 'ask' ? view.decision : null);
      })
      .catch(() => {
        if (!cancelled) setTaskDecision(null);
      });
    return () => { cancelled = true; };
  }, [activeWorkspaceId, activeWorkspaceKind, messages.length]);

  useEffect(() => {
    const onReferenceFile = (event: Event) => {
      const path = (event as CustomEvent<{ path?: unknown }>).detail?.path;
      if (typeof path !== 'string' || !path.trim()) return;
      const reference = `[引用文件: ${path.trim()}]`;
      const current = useMessagesStore.getState().input.trim();
      setInput(current ? `${current}\n${reference}` : reference, true);
    };
    window.addEventListener('angelbot:reference-file', onReferenceFile);
    return () => window.removeEventListener('angelbot:reference-file', onReferenceFile);
  }, [setInput]);

  const isCurrentSessionGeneration = (sessionId: string, generation: number) =>
    sessionGenerationRef.current === generation
    && useSessionsStore.getState().activeSessionId === sessionId;
  const currentPauseAction = pauseAction
    && isCurrentSessionGeneration(pauseAction.sessionId, pauseAction.generation)
    && pauseAction.stopEpoch === stopEpochRef.current ? pauseAction : null;
  const isPaused = currentPauseAction?.paused ?? false;
  const currentHistoryAction = historyAction?.sessionId === session?.id
    && historyAction?.generation === sessionGenerationRef.current ? historyAction : null;
  const historyBlocked = Boolean(currentHistoryAction?.pending || currentHistoryAction?.error);

  const [transcriptTail, setTranscriptTail] = useState<ReturnType<typeof messagePageBounds> | null>(null);
  const transcriptTailRef = useRef<typeof transcriptTail>(null);
  const preserveReadingWindowRef = useRef<((messages: Message[], bounds: NonNullable<typeof transcriptTail>) => void) | null>(null);
  const updateTranscriptTail = useCallback((tail: typeof transcriptTail) => {
    transcriptTailRef.current = tail;
    setTranscriptTail(tail);
  }, []);
  const commitTailPage = useCallback((page: MessagePage, incoming = page.messages,
    mode: 'replace' | 'refresh' | 'append' = 'replace') => {
    const bounds = messagePageBounds(page);
    const earlier = transcriptTailRef.current;
    const previous = useMessagesStore.getState().messages;
    let merged = incoming;
    if (earlier && !page.reset && mode === 'append') {
      // Replace optimistic rows by client ID; do not grow the default window
      // indefinitely when the user stays in the same conversation.
      merged = [...new Map([...previous, ...incoming].map(message => [message.id, message])).values()];
      const clipped = merged.length > TRANSCRIPT_PAGE_SIZE;
      if (clipped) merged = merged.slice(-TRANSCRIPT_PAGE_SIZE);
      bounds.firstId = clipped ? merged[0]?.id : earlier.firstId ?? bounds.firstId;
      bounds.lastId ??= earlier.lastId;
      bounds.hasOlder = clipped || earlier.hasOlder;
    }
    if (earlier && mode !== 'replace' && bounds.firstId !== earlier.firstId && bounds.lastId !== earlier.lastId) {
      preserveReadingWindowRef.current?.(previous, earlier);
    }
    updateTranscriptTail(bounds);
    loadMessages(withoutReplayedToolRows(merged));
  }, [loadMessages, updateTranscriptTail]);

  const hydrateTimelineBlocks = useCallback(async (
    sessionId: string, source: Message[], stillCurrent: () => boolean,
    onHydrated?: (saved: Message, hydrated: Message) => void,
  ) => {
    const latestId = latestConversationMessage(source, sessionId)?.id;
    const recent = new Set([...timelineCandidates(source, sessionId)].slice(0, RECENT_TIMELINE_LIMIT));
    const pending: number[] = [];
    for (let index = source.length - 1; index >= 0; index -= 1) {
      const message = source[index];
      const needsRecoveryDetails = message.id === latestId && message.taskRun?.status === 'needs_attention'
        && !message.modelRequestFailed;
      if (recent.has(message.id) && (needsRecoveryDetails || (!message.timelineBlocks?.length && !message.modelRequestFailed))) {
        pending.push(index);
      }
    }
    const hydrated = [...source];
    let next = 0;
    await Promise.all(Array.from({ length: Math.min(TIMELINE_REPLAY_CONCURRENCY, pending.length) }, async () => {
      while (next < pending.length && stillCurrent()) {
        const index = pending[next++];
        const message = source[index];
        try {
          hydrated[index] = await readMessageTimeline(sessionId, message, message.id === latestId);
          if (hydrated[index] !== message && stillCurrent()) onHydrated?.(message, hydrated[index]);
        } catch {
          // The saved reply remains usable without its optional timeline.
        }
      }
    }));
    return withoutReplayedToolRows(hydrated);
  }, []);

  // Restore this session's text before painting. Attachments have their own
  // scope and never become part of the saved text draft.
  useLayoutEffect(() => { activateInput(session?.id ?? null); }, [session?.id, activateInput]);

  // The selected conversation observes durable replies independently of the
  // local streaming turn, which may have left this view before it completed.
  useLayoutEffect(() => {
    const sessionId = session?.id;
    const generation = ++sessionGenerationRef.current;
    clearMessages();
    updateTranscriptTail(null);
    setIsStreaming(false);
    setPauseAction(null);
    pauseAttemptRef.current = null;
    setResolvingCallId(null);
    historyAttempt.current = null;
    setHistoryAction(null);

    if (!sessionId) return;
    const attempt = Symbol();
    historyAttempt.current = attempt;
    setHistoryAction({ sessionId, generation, kind: 'load', pending: true, notice: '正在读取对话记录…' });
    let disposed = false;
    let ready = false;
    let reading = false;
    let dirty = true;
    const endedMessages = new Set<string>();
    const syncHistory = async () => {
      if (reading || disposed) return;
      reading = true;
      try {
        while (dirty && !disposed) {
          dirty = false;
          const initial = historyAttempt.current === attempt;
          const readGeneration = sessionGenerationRef.current;
          const transcript = useMessagesStore.getState().messages;
          const current = () => !disposed && isCurrentSessionGeneration(sessionId, readGeneration)
            && (initial ? historyAttempt.current === attempt : !historyAttempt.current && !isStreamingRef.current
              && useMessagesStore.getState().messages === transcript);
          if (!current()) continue;
          try {
            const lastId = transcriptTailRef.current?.lastId;
            const updatesExisting = transcript.some(message => endedMessages.has(message.id));
            const afterId = !initial && !updatesExisting ? lastId : undefined;
            endedMessages.clear();
            const page = await getMessagePage(sessionId, afterId ? { afterId } : undefined);
            if (!current()) continue;
            const msgs = page.messages;
            const preserved = retainUnchangedTimelines(msgs, transcript, sessionId);
            commitTailPage(page, preserved, initial ? 'replace' : afterId ? 'append' : 'refresh');
            setHistoryAction((state) => state?.kind === 'load' && state.sessionId === sessionId
              && state.generation === readGeneration ? null : state);
            if (initial) {
              historyAttempt.current = null;
              const active = useSessionsStore.getState().activeSession;
              const firstUserMessage = msgs.find((message) => message.role === 'user');
              if (!page.hasOlder && active && !active.title.trim() && firstUserMessage) {
                const nextTitle = titleFromFirstUserMessage(firstUserMessage.content);
                if (nextTitle) useSessionsStore.getState().updateSessionTitle(sessionId, nextTitle);
              }
            }
            const snapshotCurrent = () => !disposed && !dirty
              && isCurrentSessionGeneration(sessionId, readGeneration);
            void hydrateTimelineBlocks(sessionId, preserved, snapshotCurrent, (saved, hydrated) => {
              const current = useMessagesStore.getState().messages;
              if (current.includes(saved)) {
                loadMessages(withoutReplayedToolRows(current.map((message) => message === saved ? hydrated : message)));
              }
            });
          } catch {
            // A failed background refresh does not replace visible history.
            if (initial && current()) setHistoryAction({ sessionId, generation, kind: 'load', pending: false, error: true,
              notice: '暂时无法读取对话记录。输入草稿已保留，请重新载入后继续。' });
          } finally {
            if (initial && historyAttempt.current === attempt) historyAttempt.current = null;
          }
        }
      } finally {
        reading = false;
      }
    };
    const subscription = subscribeAgentSessionEvents(sessionId, (event) => {
      // Main publishes MessageEnd only after saving the reply. One read in
      // flight plus a dirty bit closes the snapshot/event race without polling.
      if (event.type !== 'MessageEnd' || disposed || isStreamingRef.current) return;
      const id = (event.data as { message_id?: unknown } | null)?.message_id;
      if (typeof id === 'string') endedMessages.add(id);
      dirty = true;
      if (ready) void syncHistory();
    });
    const start = () => { ready = true; return syncHistory(); };
    // Dev HTTP has no native event bridge, but opening history still works.
    void subscription.ready.then(start, start);
    return () => {
      disposed = true;
      subscription.unsubscribe();
      historyAttempt.current = null;
    };
  }, [session?.id, clearMessages, loadMessages, hydrateTimelineBlocks, updateTranscriptTail, commitTailPage]);

  useEffect(() => () => unsubscribeFromAgentEvents(), [session?.id, unsubscribeFromAgentEvents]);

  const [streamingMsgId, setStreamingMsgId] = useState<string | null>(null);
  const isVisibleMessage = useCallback((message: Message) => message.sessionId === session?.id && (
    (isStreaming && message.id === streamingMsgId) || message.id === recoveryMessage?.id || message.id === resultToReview?.id
    || olderTimelineIds.has(message.id) || !isEmptySuccessfulToolTurn(message)
  ), [session?.id, isStreaming, streamingMsgId, recoveryMessage?.id, resultToReview?.id, olderTimelineIds]);
  const { containerRef, windowMessages, renderedMessages, hasOlder, hasNewer, hasMoreNewer, paging, pagingError, loadAdjacent, jumpToMessage, returnToLatest, preserveReadingWindow,
    updateHistoricalMessage, updateFollowLatest, scrollToLatestIfFollowing } = useTranscriptWindow({ sessionId: session?.id, messages, tail: transcriptTail,
      visible: isVisibleMessage,
      captureOwner: () => {
        const sessionId = session?.id;
        const generation = sessionGenerationRef.current;
        return () => Boolean(sessionId && isCurrentSessionGeneration(sessionId, generation));
      },
      onTailPage: commitTailPage,
    });
  useLayoutEffect(() => {
    preserveReadingWindowRef.current = preserveReadingWindow;
    return () => { preserveReadingWindowRef.current = null; };
  }, [preserveReadingWindow]);
  const windowDeliveries = useMemo(() => hasNewer && session?.id ? completedReplyDeliveries(windowMessages, session.id) : replyDeliveries,
    [hasNewer, session?.id, windowMessages, replyDeliveries]);
  const windowTimelineIds = useMemo(() => hasNewer && session?.id ? timelineCandidates(windowMessages, session.id) : olderTimelineIds,
    [hasNewer, session?.id, windowMessages, olderTimelineIds]);
  const readHistoricalTimeline = async (message: Message) => {
    const { sessionId } = message;
    const generation = sessionGenerationRef.current;
    try {
      const hydrated = await readMessageTimeline(sessionId, message, false);
      if (!isCurrentSessionGeneration(sessionId, generation)) return;
      const current = useMessagesStore.getState().messages;
      const inHistoricalWindow = updateHistoricalMessage(message, hydrated);
      if (current.includes(message)) {
        if (hydrated !== message) {
          loadMessages(withoutReplayedToolRows(current.map(saved => saved === message ? hydrated : saved)));
        }
        return hydrated;
      }
      if (inHistoricalWindow) return hydrated;
    } catch {
      // The saved reply is still readable. The disclosure can retry the journal.
    }
  };
  useEffect(() => {
    const onReviewResult = (event: Event) => {
      if (resultToReview && (event as CustomEvent<unknown>).detail === resultToReview.id) jumpToMessage(resultToReview.id, true, true);
    };
    window.addEventListener('angelbot:review-result-in-chat', onReviewResult);
    return () => window.removeEventListener('angelbot:review-result-in-chat', onReviewResult);
  }, [jumpToMessage, resultToReview]);
  const pendingApproval = hasNewer && !renderedMessages.some(message => message.id === latestMessage?.id)
    && (livePendingCallIds.size > 0 || latestMessage?.taskRun?.status === 'awaiting_confirmation'
    || [...authoritativeToolResultsByCallId(latestMessage?.toolResults ?? []).values()]
      .some((result) => result.confirmationRequired && result.confirmationStatus === 'pending'));

  // Auto-scroll on new messages only when the reader has not intentionally
  // navigated away from the bottom of the conversation.
  useEffect(() => {
    scrollToLatestIfFollowing();
  }, [renderedMessages, streamingText, liveStatus, liveToolSteps, scrollToLatestIfFollowing]);

  const confirmationPlaceholderRef = useRef<{ id: string; generation: number } | null>(null);
  const [failedDesktopPreview, setFailedDesktopPreview] = useState<Omit<DesktopApprovalRecheck, 'onRecheck'> | null>(null);
  const desktopRecheck = failedDesktopPreview ? {
    ...failedDesktopPreview,
    onRecheck: () => setFailedDesktopPreview((current) => current === failedDesktopPreview ? null : current),
  } : undefined;

  const onSend = async (text: string, files: TextAttachment[] = [], clearSubmittedDraft = true, queued = false): Promise<ComposerSendResult> => {
    if ((!text.trim() && !files.length) || isStreamingRef.current || historyAttempt.current || historyBlocked || routingRequest.current) return 'not_sent';
    if (session?.id && isStopPending(session.id)) return 'not_sent';
    if (!queued && session?.id && dispatchingFollowUps.current.has(session.id)) return 'not_sent';
    readyFollowUps.current = null;
    const workspaceState = useWorkspacesStore.getState();
    const secureWebSearch = extractSecureWebSearchSetup(text);
    const { inputToken, inputLocalOnly } = useMessagesStore.getState();
    const selected = routeChoice.current;
    if (!queued) routeChoice.current = null;
    let destination: Workspace | undefined;
    // A queued follow-up belongs to the conversation where it was composed.
    if (!queued && selected?.sessionId === session?.id && selected?.inputToken === inputToken) {
      destination = selected.workspaceId ? workspaceState.workspaces.find(item => item.id === selected.workspaceId && item.kind === 'project') : undefined;
      if (selected.workspaceId && !destination) return 'not_sent';
    } else if (!queued) {
      const candidates = projectCandidatesForMessage(secureWebSearch.displayContent, workspaceState.workspaces, workspaceState.activeWorkspaceId);
      if (candidates.length && session?.id) {
        const request = {};
        const sourceSession = session.id;
        const sourceGeneration = sessionGenerationRef.current;
        const ownsRequest = () => routingRequest.current === request
          && useSessionsStore.getState().activeSessionId === sourceSession
          && useWorkspacesStore.getState().activeWorkspaceId === workspaceState.activeWorkspaceId
          && sessionGenerationRef.current === sourceGeneration && useMessagesStore.getState().inputToken === inputToken;
        routingRequest.current = request; setRoutingBusy(true); setRoutingDecision(null); setRoutingSelection(null);
        try {
          const intent = await resolveWorkspaceIntent(sourceSession, secureWebSearch.displayContent);
          if (!ownsRequest()) return 'not_sent';
          if (intent.kind === 'clarify') { setRoutingDecision({ sessionId: sourceSession, inputToken, candidates }); return 'not_sent'; }
          if (intent.kind === 'project') {
            destination = useWorkspacesStore.getState().workspaces.find(item => item.id === intent.projectId && item.kind === 'project');
            if (!destination || !candidates.some(item => item.id === destination?.id)) throw new Error('项目目录已变化，请重新选择归属空间。');
          }
        } catch (cause) {
          if (ownsRequest()) setRoutingDecision({ sessionId: sourceSession, inputToken, candidates,
            error: cause instanceof Error ? cause.message : '暂时无法确认项目归属。' });
          return 'not_sent';
        } finally { if (routingRequest.current === request) { routingRequest.current = null; setRoutingBusy(false); } }
      }
    }
    if (!queued) setRoutingSelection(null);
    const route = destination;
    if (route) {
      // Routing occurs before a turn begins, so the original text and every
      // later tool/file operation belong to the project's single Main session.
      if (pendingProjectMessageRef.current) return 'not_sent';
      const sourceSessionId = session?.id;
      return new Promise<ComposerSendResult>((resolve) => {
        const pending = { text, files, workspaceId: route.id, sessionId: null as string | null, sourceSessionId,
          sourceInputToken: useMessagesStore.getState().inputToken, resolve };
        pendingProjectMessageRef.current = pending;
        // Keep the source draft until the routed turn is actually accepted.
        void workspaceState.openWorkspace(route.id).then((accepted) => {
          if (pendingProjectMessageRef.current !== pending) return;
          if (!accepted) {
            pendingProjectMessageRef.current = null;
            resolve('not_sent');
            return;
          }
          const currentWorkspace = useWorkspacesStore.getState();
          const currentSession = useSessionsStore.getState();
          const targetSessionId = currentWorkspace.workspaces.find((item) => item.id === route.id)?.activeSessionId;
          if (currentWorkspace.activeWorkspaceId !== route.id || !targetSessionId
            || currentSession.activeSessionId !== targetSessionId) {
            pendingProjectMessageRef.current = null;
            resolve('not_sent');
            return;
          }
          pending.sessionId = targetSessionId;
          setRouteNotice({ workspaceId: route.id, name: route.name, sourceWorkspaceId: workspaceState.activeWorkspaceId! });
          setRouteVersion((version) => version + 1);
        }).catch(() => {
          if (pendingProjectMessageRef.current === pending) pendingProjectMessageRef.current = null;
          resolve('not_sent');
        });
      });
    }
    if (!session?.id) return 'not_sent';
    const messageContent = secureWebSearch.displayContent;
    const sessionId = session.id;
    const generation = ++sessionGenerationRef.current;
    const stopEpoch = stopEpochRef.current;
    // Clear only the draft submitted by this caller, synchronously before
    // subscription can yield. Routed turns do not own the destination draft.
    setPauseAction(null);
    pauseAttemptRef.current = null;
    const clearedToken = {};
    if (clearSubmittedDraft) clearSubmittedInput(sessionId, text, inputToken, clearedToken);
    const restoreUnsentInput = () => {
      if (clearSubmittedDraft && !queued && useMessagesStore.getState().inputToken === clearedToken) {
        setInput(messageContent, inputLocalOnly || Boolean(secureWebSearch.setup));
      }
    };
    clearLiveActivity();
    useMessagesStore.getState().setLiveStatus('正在理解你的请求');
    try { await subscribeToAgentEvents(sessionId); }
    catch {
      if (isCurrentSessionGeneration(sessionId, generation)) restoreUnsentInput();
      return 'not_sent';
    }
    if (!isCurrentSessionGeneration(sessionId, generation)) return 'not_sent';
    returnToLatest();
    const tailId = transcriptTailRef.current?.lastId;
    // A stopped approval can settle its original row while the next turn
    // runs. Refresh that receipt too; the ordinary path reads only new rows.
    const beforeSend = useMessagesStore.getState().messages;
    const persistedTail = beforeSend.find(message => message.id === tailId);
    const previousReply = persistedTail?.role === 'assistant' ? persistedTail
      : latestConversationMessage(beforeSend.filter(message => message.id !== confirmationPlaceholderRef.current?.id), sessionId);
    const maySettleInPlace = previousReply?.taskRun?.status === 'awaiting_confirmation'
      || [...authoritativeToolResultsByCallId(previousReply?.toolResults ?? []).values()]
        .some(result => result.confirmationRequired && result.confirmationStatus === 'pending');
    const cursor = tailId && !maySettleInPlace ? { afterId: tailId } : undefined;
    const refreshMode = cursor ? 'append' : tailId ? 'refresh' : 'replace';
    isStreamingRef.current = true;
    setIsStreaming(true);
    const userMessage = {
      id: crypto.randomUUID(),
      sessionId,
      role: 'user' as const,
      content: messageContent,
      ...(files.length ? { textAttachments: files } : {}),
      createdAt: Date.now(),
    };
    appendMessage(userMessage);
    onMessageSent(userMessage);

    // Immediately insert a placeholder assistant bubble for streaming text
    const placeholderId = crypto.randomUUID();
    setStreamingMsgId(placeholderId);
    appendMessage({
      id: placeholderId,
      sessionId,
      role: 'assistant',
      content: '',
      createdAt: Date.now(),
    });

    try {
      const reply = await sendMessage({
        sessionId,
        clientMessageId: userMessage.id,
        role: 'user',
        content: messageContent,
        textAttachments: files,
        ...currentForegroundInteraction(),
        webSearchSetup: secureWebSearch.setup,
      });
      if (isCurrentSessionGeneration(sessionId, generation)) {
        // Replace the placeholder with the actual reply
        useMessagesStore.getState().removeMessage(placeholderId);
        // The backend owns durable message IDs and may enrich the reply with
        // task-run data. Reloading the completed turn keeps later edit/delete
        // actions bound to persisted rows rather than optimistic placeholders.
        try {
          const page = await getMessagePage(sessionId, cursor);
          const refreshedMessages = await hydrateTimelineBlocks(sessionId, page.messages,
            () => isCurrentSessionGeneration(sessionId, generation));
          if (isCurrentSessionGeneration(sessionId, generation)) {
            commitTailPage(page, refreshedMessages, refreshMode);
            if (!refreshedMessages.some(message => message.id === reply.id)) appendMessage(reply);
          }
        } catch {
          if (isCurrentSessionGeneration(sessionId, generation)) appendMessage(reply);
        }
      }
      if (isCurrentSessionGeneration(sessionId, generation) && stopEpoch === stopEpochRef.current && followUpsCanContinue(reply)) {
        readyFollowUps.current = { sessionId, generation, stopEpoch };
      }
      return 'accepted';
    } catch (err) {
      console.error('[ChatArea] sendMessage error:', err);
      let accepted = false;
      let durableFailureVisible = false;
      if (isCurrentSessionGeneration(sessionId, generation)) {
        useMessagesStore.getState().removeMessage(placeholderId);
        // A model/transport failure can occur after the user turn was saved.
        // Only the durable client ID decides whether the submitted snapshot
        // was accepted. Never automatically retry an uncertain send.
        try {
          const page = await getMessagePage(sessionId, cursor);
          if (page.reset && !page.messages.some(message => message.id === userMessage.id)) {
            const submitted = await getMessagePage(sessionId, { aroundId: userMessage.id });
            if (submitted.messages.some(message => message.id === userMessage.id && message.sessionId === sessionId && message.role === 'user')) accepted = true;
          }
          const durableMessages = page.messages;
          if (!isCurrentSessionGeneration(sessionId, generation)) return 'unknown';
          accepted ||= durableMessages.some((message) => message.id === userMessage.id
            && message.sessionId === sessionId && message.role === 'user');
          if (accepted) {
            const refreshedMessages = await hydrateTimelineBlocks(sessionId, durableMessages,
              () => isCurrentSessionGeneration(sessionId, generation));
            if (isCurrentSessionGeneration(sessionId, generation)) {
              commitTailPage(page, refreshedMessages, refreshMode);
              // Only this accepted user's durable failed reply can replace the
              // transport error, after its visible transcript has loaded.
              durableFailureVisible = refreshedMessages.some((message) =>
                message.sessionId === sessionId && message.role === 'assistant'
                && message.parentId === userMessage.id && Boolean(message.content.trim())
                && message.taskRun?.id === message.id
                && ['needs_attention', 'provider_unavailable', 'failed'].includes(message.taskRun.status));
            }
          } else useMessagesStore.getState().removeMessage(userMessage.id);
        } catch { /* Unknown persistence: retain attachments for user inspection. */ }
        if (!isCurrentSessionGeneration(sessionId, generation)) return 'unknown';
        if (!accepted) restoreUnsentInput();
        if (!durableFailureVisible) appendMessage({
          id: crypto.randomUUID(),
          sessionId,
          role: 'assistant',
          content: formatModelSendError(err),
          createdAt: Date.now(),
        });
      }
      return accepted ? 'accepted' : 'unknown';
    } finally {
      // A live confirmation may take over this anchor before send returns.
      // Other stale requests still clean only their own optimistic message.
      if (confirmationPlaceholderRef.current?.id !== placeholderId) useMessagesStore.getState().removeMessage(placeholderId);
      if (isCurrentSessionGeneration(sessionId, generation)) {
        setIsStreaming(false);
        isStreamingRef.current = false;
        setStreamingMsgId(null);
        window.dispatchEvent(new Event(AUTOMATIONS_UPDATED_EVENT));
      }
    }
  };

  const onAbort = async () => {
    const sessionId = session?.id;
    if (!sessionId || !isStreamingRef.current || isStopPending(sessionId)) return;
    const generation = sessionGenerationRef.current;
    useMessagesStore.setState((state) => ({ stoppingSessions: new Set(state.stoppingSessions).add(sessionId) }));
    // Fence continuation immediately; interrupt IPC may be slow or lost.
    stopEpochRef.current += 1;
    readyFollowUps.current = null;
    unsubscribeFromAgentEvents();
    setIsStreaming(false);
    setPauseAction(null);
    pauseAttemptRef.current = null;
    setResolvingCallId(null);
    try {
      await interruptAgent(sessionId);
    } catch {
      if (isCurrentSessionGeneration(sessionId, generation)) appendMessage({
        id: crypto.randomUUID(), sessionId, role: 'assistant', createdAt: Date.now(),
        content: '未能确认停止请求，Agent 可能仍在执行。请先核对运行状态与操作结果，不要重复提交原任务。',
      });
    } finally {
      useMessagesStore.setState((state) => {
        const stoppingSessions = new Set(state.stoppingSessions);
        stoppingSessions.delete(sessionId);
        return { stoppingSessions };
      });
      if (isCurrentSessionGeneration(sessionId, generation)) composerInputRef.current?.focus();
    }
  };

  const onContinueTask = async (messageId: string, action: TaskRecoveryAction) => {
    const sessionId = session?.id;
    if (!sessionId || isStreamingRef.current || historyBlocked || historyAttempt.current || routingRequest.current
      || resolvingCallId || dispatchingFollowUps.current.has(sessionId) || isStopPending(sessionId)) return;
    const source = latestConversationMessage(useMessagesStore.getState().messages, sessionId);
    if (source?.id !== messageId || taskRecoveryState(source).action !== action) return;
    const resultsOnly = action === 'resultsOnly';
    const { activeApiConfig, apiConfigLoaded, apiConfigLoadError } = useSettingsStore.getState();
    const readiness = getModelConfigReadiness(activeApiConfig, apiConfigLoaded, apiConfigLoadError);
    if (import.meta.env.VITE_ANGELBOT_DESKTOP_E2E !== '1'
      && (readiness.kind === 'needs-credentials' || readiness.kind === 'invalid')) {
      openModelSettings();
      return;
    }

    const generation = ++sessionGenerationRef.current;
    const stopEpoch = stopEpochRef.current;
    const current = () => isCurrentSessionGeneration(sessionId, generation);
    const placeholderId = crypto.randomUUID();
    readyFollowUps.current = null;
    setRecoveryNotice(null);
    setPauseAction(null);
    pauseAttemptRef.current = null;
    clearLiveActivity();
    useMessagesStore.getState().setLiveStatus(resultsOnly ? '正在根据已有结果回答' : '正在继续任务');
    isStreamingRef.current = true;
    setIsStreaming(true);
    try {
      await subscribeToAgentEvents(sessionId);
      if (!current() || stopEpoch !== stopEpochRef.current) return;
      setStreamingMsgId(placeholderId);
      appendMessage({ id: placeholderId, sessionId, role: 'assistant', content: '', createdAt: Date.now() });
      let reply: Message | undefined;
      try {
        reply = await continueAgentTask({ sessionId, messageId, interaction: currentForegroundInteraction(), ...(resultsOnly ? { resultsOnly: true } : {}) });
      } catch (error) {
        if (current() && stopEpoch === stopEpochRef.current) {
          setRecoveryNotice({ messageId, text: formatModelSendError(error) });
        }
      }
      if (!current()) return;
      try {
        const page = await getMessagePage(sessionId);
        const refreshed = await hydrateTimelineBlocks(sessionId, page.messages, current);
        if (!refreshed.some((message) => message.id === messageId)
          || (reply && !refreshed.some((message) => message.id === reply?.id))) {
          throw new Error('Continuation history is not confirmed');
        }
        if (current()) commitTailPage(page, refreshed, 'refresh');
      } catch {
        if (!current()) return;
        if (reply) appendMessage(reply);
        else setHistoryAction({ sessionId, generation, pending: false, error: true, kind: 'load',
          notice: '未能核对任务状态，请重新载入对话。' });
      }
      if (!resultsOnly && reply && current() && stopEpoch === stopEpochRef.current && followUpsCanContinue(reply)) {
        readyFollowUps.current = { sessionId, generation, stopEpoch };
      }
    } catch {
      if (current() && stopEpoch === stopEpochRef.current) {
        setRecoveryNotice({ messageId, text: '未能连接到任务运行状态，请重试。' });
      }
    } finally {
      if (confirmationPlaceholderRef.current?.id !== placeholderId) useMessagesStore.getState().removeMessage(placeholderId);
      if (current()) {
        isStreamingRef.current = false;
        setIsStreaming(false);
        setStreamingMsgId(null);
        window.dispatchEvent(new Event(AUTOMATIONS_UPDATED_EVENT));
      }
    }
  };

  const togglePause = async () => {
    if (!session?.id || !isStreamingRef.current || isStopPending(session.id)) return;
    const sessionId = session.id;
    const generation = sessionGenerationRef.current;
    const stopEpoch = stopEpochRef.current;
    const previous = pauseAttemptRef.current;
    if (previous?.sessionId === sessionId && previous.generation === generation
      && previous.stopEpoch === stopEpoch) return;
    const attempt: PauseAction = { sessionId, generation, stopEpoch, pending: true, paused: isPaused };
    const targetPaused = !isPaused;
    pauseAttemptRef.current = attempt;
    setPauseAction(attempt);
    const current = () => pauseAttemptRef.current === attempt && isStreamingRef.current
      && isCurrentSessionGeneration(sessionId, generation) && stopEpochRef.current === stopEpoch;
    try {
      if (targetPaused) await pauseAgent(sessionId); else await resumeAgent(sessionId);
      if (current()) {
        setPauseAction({ ...attempt, paused: targetPaused, pending: false });
      }
    } catch {
      if (current()) setPauseAction({ ...attempt, pending: false,
        error: `未能确认${targetPaused ? '暂停' : '继续运行'}，请核对当前任务状态；不会自动重试。`,
      });
    } finally {
      if (pauseAttemptRef.current === attempt) pauseAttemptRef.current = null;
    }
  };

  const onSteer = async (text: string) => {
    const { liveMessageId: replyId, inputToken } = useMessagesStore.getState();
    if (!session?.id || !replyId) throw new Error('当前轮暂时无法接收引导。');
    if (isStopPending(session.id)) throw new Error('正在停止，请稍候。');
    await submitSteeringMessage(session.id, replyId, text);
    clearSubmittedInput(session.id, text, inputToken);
  };
  const onFollowUp = (text: string) => {
    if (!session?.id) throw new Error('会话不可用。');
    if (isStopPending(session.id)) throw new Error('正在停止，请稍候。');
    updateFollowUps(session.id, (items) => [...items, { id: crypto.randomUUID(), text }]);
  };

  const sendNextFollowUp = async () => {
    const sessionId = session?.id;
    const next = queuedFollowUps[0];
    if (!sessionId || !next || next.unconfirmed || isStreamingRef.current
      || dispatchingFollowUps.current.has(sessionId) || isStopPending(sessionId)) return;
    dispatchingFollowUps.current.add(sessionId);
    setDispatchingFollowUp(next.id);
    try {
      const result = await onSend(next.text, [], false, true);
      if (result === 'accepted') updateFollowUps(sessionId, (items) => items.filter((item) => item.id !== next.id));
      if (result === 'unknown') updateFollowUps(sessionId, (items) => items.map((item) =>
        item.id === next.id ? { ...item, unconfirmed: true } : item));
    } finally {
      dispatchingFollowUps.current.delete(sessionId);
      setDispatchingFollowUp((id) => id === next.id ? null : id);
    }
  };

  useEffect(() => {
    if (showRunControls || dispatchingFollowUp || historyBlocked || routingBusy) return;
    const ready = readyFollowUps.current;
    if (!ready) return;
    readyFollowUps.current = null;
    if (ready.sessionId === session?.id && ready.generation === sessionGenerationRef.current && ready.stopEpoch === stopEpochRef.current) {
      void sendNextFollowUp();
    }
  });

  /**
   * Confirmation handler used by ContentBlockRenderer. It resolves the tool
   * call through the existing pipeline and waits for the backend result before
   * refreshing the inline state.
   */
  const onBlockResolveConfirmation = (
    callId: string,
    decision: 'approved' | 'rejected',
    previewId?: string,
  ): DesktopApprovalResolution | Promise<DesktopApprovalResolution> => {
    const messageId = useMessagesStore.getState().liveMessageId;
    if (messageId) return onResolveConfirmation(messageId, callId, decision, previewId);
  };

  useEffect(() => {
    const routedMessage = pendingProjectMessageRef.current;
    if (!routedMessage?.sessionId) return;
    if (currentHistoryAction?.kind === 'load' && currentHistoryAction.pending) return;
    pendingProjectMessageRef.current = null;
    if (activeWorkspaceId !== routedMessage.workspaceId || session?.id !== routedMessage.sessionId) {
      routedMessage.resolve('not_sent');
      return;
    }
    const sending = onSend(routedMessage.text, routedMessage.files, false);
    const routedGeneration = sessionGenerationRef.current;
    void sending.then((result) => {
      if (result === 'accepted') {
        const sourceSessionId = routedMessage.sourceSessionId;
        if (sourceSessionId) clearSubmittedInput(sourceSessionId, routedMessage.text, routedMessage.sourceInputToken);
      }
      if (result !== 'accepted' && isCurrentSessionGeneration(routedMessage.sessionId!, routedGeneration)
        && useWorkspacesStore.getState().activeWorkspaceId === routedMessage.workspaceId
        && useSessionsStore.getState().activeSessionId === routedMessage.sessionId) {
        composerInputRef.current?.restoreFiles(routedMessage.files);
      }
      routedMessage.resolve(result);
    });
  }, [session?.id, activeWorkspaceId, routeVersion, currentHistoryAction?.kind, currentHistoryAction?.pending, clearSubmittedInput]);

  const onResolveConfirmation = async (
    messageId: string,
    callId: string,
    decision: 'approved' | 'rejected',
    previewId?: string,
  ): Promise<DesktopApprovalResolution> => {
    if (!session?.id || resolvingCallId) return;
    if (isStopPending(session.id)) throw new Error('正在停止，请稍候。');
    const sessionId = session.id;
    const generation = ++sessionGenerationRef.current;
    const stopEpoch = stopEpochRef.current;
    const stillCurrent = () => isCurrentSessionGeneration(sessionId, generation)
      && stopEpoch === stopEpochRef.current;
    const loadConfirmationHistory = (page: MessagePage) => {
      const refreshed = page.messages;
      commitTailPage(page, refreshed, 'refresh');
      const latest = latestConversationMessage(refreshed, sessionId);
      if (latest?.role !== 'assistant' || latest.taskRun?.status !== 'needs_attention') return;
      // Show the receipt and continuation immediately. Only read the latest
      // recovery journal for the optional connection action.
      void hydrateTimelineBlocks(sessionId, [latest], stillCurrent).then(([hydrated]) => {
        if (!stillCurrent() || !hydrated.modelRequestFailed) return;
        const current = useMessagesStore.getState().messages;
        const last = latestConversationMessage(current, sessionId);
        if (last?.id !== latest.id || last.taskRun?.status !== 'needs_attention' || last.modelRequestFailed) return;
        loadMessages(current.map((message) => message === last ? { ...message, modelRequestFailed: true } : message));
      });
    };
    setPauseAction(null);
    pauseAttemptRef.current = null;
    const liveTool = useMessagesStore.getState().liveBlocks.find((block): block is Extract<LiveBlock, { kind: 'tool_call' }> =>
      block.kind === 'tool_call' && block.callId === callId);
    const toolName = useMessagesStore.getState().messages.find((message) => message.id === messageId)
      ?.toolCalls?.find((call) => call.id === callId)?.name
      ?? liveTool?.toolName;
    setResolvingCallId(callId);
    let resolved = false;
    let placeholderId: string | null = null;
    try {
      if (decision === 'approved') {
        // Resolution owns both the approved action and any backend continuation.
        // Keep the streaming anchor if its original send returns while
        // subscribing. That send must no longer clean up the shared anchor.
        const existingPlaceholder = useMessagesStore.getState().messages.find((message) =>
          message.id === streamingMsgId && message.sessionId === sessionId
          && message.role === 'assistant' && !message.content);
        if (existingPlaceholder) {
          placeholderId = existingPlaceholder.id;
          confirmationPlaceholderRef.current = { id: placeholderId, generation };
        }
        // The retained live trace can reappear inside the new placeholder.
        // This call is now resolving, not asking for approval again: otherwise
        // its remounted card starts a target read that competes with execution.
        useMessagesStore.setState((state) => ({
          liveBlocks: state.liveBlocks.map((block) => block.kind === 'tool_call'
            && block.callId === callId && block.status === 'needs_approval'
            ? { ...block, status: 'running' } : block),
        }));
        await subscribeToAgentEvents(sessionId);
        if (!isCurrentSessionGeneration(sessionId, generation) || stopEpoch !== stopEpochRef.current) return;
        if (hasNewer) returnToLatest();
        if (!placeholderId) {
          placeholderId = crypto.randomUUID();
          appendMessage({ id: placeholderId, sessionId, role: 'assistant', content: '', createdAt: Date.now() });
          confirmationPlaceholderRef.current = { id: placeholderId, generation };
        }
        setStreamingMsgId(placeholderId);
        setIsStreaming(true);
      }
      const result = await resolveAgentConfirmation({
        sessionId,
        messageId,
        callId,
        decision,
        previewId,
        ...(decision === 'approved' ? { interaction: currentForegroundInteraction() } : {}),
      });
      resolved = true;
      if (isCurrentSessionGeneration(sessionId, generation)) {
        addToolResult(messageId, result);
      }

      if (isCurrentSessionGeneration(sessionId, generation)) {
        clearLiveActivity();
        const page = await getMessagePage(sessionId);
        if (!isCurrentSessionGeneration(sessionId, generation)) return;
        loadConfirmationHistory(page);
        const reply = page.messages.find((message) => message.id === messageId);
        if (decision === 'approved' && reply?.taskRun?.status === 'completed'
          && stopEpoch === stopEpochRef.current && followUpsCanContinue(reply)) {
          readyFollowUps.current = { sessionId, generation, stopEpoch };
        }
      }
    } catch (err) {
      console.error('[ChatArea] resolve confirmation error:', err);
      if (isCurrentSessionGeneration(sessionId, generation)) {
        try {
          const page = await getMessagePage(sessionId);
          if (isCurrentSessionGeneration(sessionId, generation)) {
            const durableTurn = page.messages.find((message) => message.id === messageId && message.sessionId === sessionId);
            const durableResult = authoritativeToolResultsByCallId(durableTurn?.toolResults ?? []).get(callId);
            const stillPending = durableResult?.confirmationRequired && durableResult.confirmationStatus === 'pending';
            const recheckRequired = !resolved && decision === 'approved' && previewId && stillPending;
            // Record before replacing the live renderer with its durable card.
            // This is UI-only evidence, not a retryable/unknown tool result.
            if (recheckRequired) setFailedDesktopPreview({ sessionId, messageId, callId, previewId });
            clearLiveActivity();
            loadConfirmationHistory(page);
            if (recheckRequired) return 'recheck_required';
          }
        } catch {
          if (!isCurrentSessionGeneration(sessionId, generation)) return;
          const mayHaveExecuted = toolName === 'run_project_command' || toolName?.startsWith('mcp_')
            || Boolean(previewId && toolName && requiresDesktopActionPreview(toolName));
          if (!resolved && decision === 'approved' && (previewId || mayHaveExecuted)) {
            if (mayHaveExecuted) {
              markUnknownPersistedConfirmation(messageId, callId);
              useMessagesStore.setState((state) => ({ liveBlocks: markUnknownLiveConfirmation(state.liveBlocks, callId) }));
            }
            if (previewId) throw err;
          } else if (!resolved) {
            useMessagesStore.setState((state) => ({ liveBlocks: restoreRetryableLiveConfirmation(state.liveBlocks, callId) }));
            addToolResult(messageId, {
              callId,
              toolName: 'confirmation',
              success: false,
              output: 'Failed to resolve confirmation. Please try again.',
              error: 'Failed to resolve confirmation. Please try again.',
              confirmationRequired: true,
              confirmationStatus: 'pending',
            });
          }
        }
      }
    } finally {
      if (placeholderId) {
        const owner = confirmationPlaceholderRef.current;
        if (owner?.id !== placeholderId || owner.generation === generation) useMessagesStore.getState().removeMessage(placeholderId);
        if (owner?.id === placeholderId && owner.generation === generation) confirmationPlaceholderRef.current = null;
      }
      if (isCurrentSessionGeneration(sessionId, generation)) {
        setResolvingCallId(null);
        setIsStreaming(false);
        setStreamingMsgId(null);
        window.dispatchEvent(new Event(AUTOMATIONS_UPDATED_EVENT));
      }
    }
  };

  // ── Message action handlers ─────────────────────────────────────────────────

  const messagesRef = useRef(messages);
  messagesRef.current = windowMessages;

  const handleEditMessage = useCallback(async (messageId: string, newContent: string) => {
    if (!session?.id || isStreamingRef.current || isStopPending(session.id) || historyAttempt.current || historyBlocked) return;
    const sessionId = session.id;
    const generation = ++sessionGenerationRef.current;
    const stopEpoch = stopEpochRef.current;
    const attempt = Symbol();
    historyAttempt.current = attempt;
    const current = () => historyAttempt.current === attempt && isCurrentSessionGeneration(sessionId, generation);
    const transcript = messagesRef.current;
    const edit = { messageId, sourceContent: transcript.find((message) => message.id === messageId)?.content ?? '', content: newContent };
    const placeholderId = crypto.randomUUID();
    setHistoryAction({ sessionId, generation, pending: true, edit });
    try {
      setIsStreaming(true);
      setStreamingMsgId(placeholderId);
      clearLiveActivity();
      useMessagesStore.getState().setLiveStatus('正在基于编辑后的消息重新生成');
      await subscribeToAgentEvents(sessionId);
      if (!current()) return;
      returnToLatest();
      const editedIndex = transcript.findIndex((message) => message.id === messageId);
      if (editedIndex >= 0) {
        // Backend atomically replaces the edited turn and its later branch.
        // Keep the replacement user turn visible while its new assistant
        // response streams. This avoids a blank gap between pressing resend
        // and receiving the durable transcript back from the backend.
        const editedMessage = {
          ...transcript[editedIndex],
          content: newContent,
        };
        useMessagesStore.setState({
          messages: [
            ...transcript.slice(0, editedIndex),
            editedMessage,
            {
              id: placeholderId,
              sessionId,
              role: 'assistant',
              content: '',
              createdAt: Date.now(),
            },
          ],
        });
      }
      const reply = await editAndResendMessage({
        sessionId,
        messageId,
        content: newContent,
        ...currentForegroundInteraction(),
      });
      if (current()) {
        useMessagesStore.getState().removeMessage(placeholderId);
        try {
          const page = await getMessagePage(sessionId);
          const durableMessages = page.messages;
          if (!current()) return;

          // Never let a transient or inconsistent reload erase the local
          // replacement turn. The backend preserves `messageId` on a resend;
          // its absence is therefore a recoverable refresh failure, not an
          // empty conversation.
          if (!durableMessages.some((message) => message.id === messageId && message.role === 'user' && message.content === newContent)) {
            throw new Error('Edited turn not confirmed');
          }

          const hydrated = await hydrateTimelineBlocks(sessionId, durableMessages, current);
          if (!current()) return;
          commitTailPage(page, hydrated);
          setHistoryAction(null);
          if (stopEpoch === stopEpochRef.current && followUpsCanContinue(reply)) {
            readyFollowUps.current = { sessionId, generation, stopEpoch };
          }
        } catch {
          if (!current()) return;
          appendMessage(reply);
          setHistoryAction({ sessionId, generation, pending: false, error: true, edit });
        }
      }
    } catch {
      if (current()) {
        // A failed transport/model request may already have replaced history.
        // Read the durable branch, never restore the pre-send snapshot as fact
        // and never retry the edit automatically.
        try {
          const page = await getMessagePage(sessionId);
          const hydrated = await hydrateTimelineBlocks(sessionId, page.messages, current);
          if (!current()) return;
          clearLiveActivity();
          commitTailPage(page, hydrated);
          setHistoryAction({ sessionId, generation, pending: false,
            notice: '未能确认重新生成结果；已载入保存记录，请先核对任务状态。',
            edit: hydrated.some((message) => message.id === messageId && message.role === 'user'
              && message.content === edit.sourceContent && message.content !== newContent) ? edit : undefined });
        } catch {
          if (current()) setHistoryAction({ sessionId, generation, pending: false, error: true, edit });
        }
      }
    } finally {
      if (confirmationPlaceholderRef.current?.id !== placeholderId) useMessagesStore.getState().removeMessage(placeholderId);
      if (isCurrentSessionGeneration(sessionId, generation)) {
        setIsStreaming(false);
        setStreamingMsgId(null);
        window.dispatchEvent(new Event(AUTOMATIONS_UPDATED_EVENT));
      }
      if (historyAttempt.current === attempt) historyAttempt.current = null;
    }
  }, [session?.id, historyBlocked, clearLiveActivity, subscribeToAgentEvents, appendMessage, hydrateTimelineBlocks, commitTailPage, returnToLatest]);

  // Omitting messageId only reloads history; it never retries an edit or delete
  // whose outcome is unknown. Both paths preserve the same generation owner.
  const handleHistoryAction = useCallback(async (messageId?: string) => {
    if (!session?.id || isStreaming || isStopPending(session.id) || historyAttempt.current) return;
    if (messageId && currentHistoryAction?.error) return;
    if (messageId && !confirm('删除这条消息及其后续分支？已执行的操作不会撤销，删除记录无法恢复。')) return;
    const sessionId = session.id;
    const generation = sessionGenerationRef.current;
    const edit = messageId ? undefined : currentHistoryAction?.edit;
    const loadingHistory = currentHistoryAction?.kind === 'load';
    const attempt = Symbol();
    historyAttempt.current = attempt;
    const current = () => historyAttempt.current === attempt && isCurrentSessionGeneration(sessionId, generation);
    setHistoryAction({ sessionId, generation, pending: true, edit,
      ...(loadingHistory ? { kind: 'load', notice: '正在重新读取对话记录…' } : {}) });
    try {
      if (messageId) await deleteMessage(messageId);
      if (!current()) return;
      const page = await getMessagePage(sessionId);
      const hydrated = await hydrateTimelineBlocks(sessionId, page.messages, current);
      if (current()) {
        if (!isStreamingRef.current) {
          clearLiveActivity();
          returnToLatest();
          commitTailPage(page, hydrated);
        }
        setHistoryAction(edit && hydrated.some((message) => message.id === edit.messageId && message.role === 'user'
          && message.content === edit.sourceContent && message.content !== edit.content)
          ? { sessionId, generation, pending: false, edit, notice: '已载入保存记录；未采用的修改保留在编辑框中，发送前请先核对任务结果。' }
          : null);
      }
    } catch {
      if (current()) setHistoryAction({ sessionId, generation, pending: false, error: true, edit,
        ...(loadingHistory ? { kind: 'load', notice: '暂时无法读取对话记录。输入草稿已保留，请重新载入后继续。' } : {}) });
    } finally {
      if (historyAttempt.current === attempt) historyAttempt.current = null;
    }
  }, [session?.id, isStreaming, currentHistoryAction?.error, currentHistoryAction?.edit, currentHistoryAction?.kind, clearLiveActivity, hydrateTimelineBlocks, commitTailPage, returnToLatest]);

  const handleCopyMessage = useCallback((_messageId: string) => {
    // Clipboard write is handled inside MessageActions; this is just a callback hook
  }, []);

  const showEmptyState = messages.length === 0 && !historyBlocked;

  return (
    <main className="main-with-nav">
      <div className="chat-area">
        <ChatHeader onToggleWorkbench={onToggleWorkbench} workbenchOpen={workbenchOpen} />
        {routeNotice && routeNotice.workspaceId === activeWorkspaceId && <aside className="workspace-route-notice" role="status"><span>{routeNotice.error || <>当前位于 <strong>{routeNotice.name}</strong> 项目，使用它独立的上下文。</>}</span><button type="button" onClick={() => { void useWorkspacesStore.getState().openWorkspace(routeNotice.sourceWorkspaceId).catch(() => setRouteNotice(current => current ? { ...current, error: '暂时无法返回日常，请重试；项目任务未取消。' } : null)); }}>返回日常，项目任务继续</button><button type="button" aria-label="关闭切换提示" onClick={() => setRouteNotice(null)}>×</button></aside>}
        {routingBusy && <aside className="workspace-route-notice" role="status"><span>正在确认这条请求的项目归属，尚未开始执行。</span><button type="button" onClick={() => { routingRequest.current = null; setRoutingBusy(false); }}>取消判断</button></aside>}
        {routingDecision && routingDecision.sessionId === session?.id && routingDecision.inputToken === useMessagesStore.getState().inputToken && <aside className="workspace-route-notice workspace-route-choice" role="status">
          <span>{routingDecision.error || '这条请求的归属还不明确。'} 尚未发送，请选择后再次发送；草稿和附件仍保留。</span>
          <div><button type="button" onClick={() => { routeChoice.current = { sessionId: routingDecision.sessionId, inputToken: routingDecision.inputToken, workspaceId: null }; setRoutingSelection({ ...routingDecision, label: '日常' }); setRoutingDecision(null); }}>留在日常</button>
            {routingDecision.candidates.map(project => <button type="button" key={project.id} onClick={() => { routeChoice.current = { sessionId: routingDecision.sessionId, inputToken: routingDecision.inputToken, workspaceId: project.id }; setRoutingSelection({ ...routingDecision, label: project.name }); setRoutingDecision(null); }}>按 {project.name} 项目处理</button>)}</div>
        </aside>}
        {routingSelection && routingSelection.sessionId === session?.id && routingSelection.inputToken === useMessagesStore.getState().inputToken && <p className="workspace-route-notice" role="status">已选择 {routingSelection.label}，点击发送后继续。</p>}
        <UpdateNotice />
        <RuntimeHealthNotice />
        <ModelConfigurationNotice />
        <TaskDecisionCard decision={taskDecision} />
      <div className={`chat-main${showEmptyState ? ' chat-main--empty' : ''}`}>
      <div className={`chat-transcript${(hasOlder || renderedMessages.some(message => message.role === 'user')) ? ' chat-transcript--indexed' : ''}`}>
      {backgroundImage && <img className="chat-background-image" src={backgroundImage} alt="" aria-hidden="true" draggable={false} />}
      <div className="messages" ref={containerRef} onScroll={updateFollowLatest}>
        {showEmptyState && <ChatEmptyState />}
        {pagingError && <p className="conversation-page" role="status">{pagingError}</p>}
        {hasOlder && <button className="conversation-page" type="button" aria-disabled={paging} onClick={(event) => loadAdjacent('older', event.detail === 0, event.currentTarget)}>加载更早消息</button>}
        {renderedMessages.map((message) => {
          const reviewingResult = !showRunControls && message.id === resultToReview?.id;
          const delivery = onOpenWorkbenchView && !reviewingResult && !(isStreaming && message.id === streamingMsgId)
            ? windowDeliveries?.get(message.id) : undefined;
          return (
          <Fragment key={message.id}>
          <div className={`message-row ${message.role}`} data-conversation-message-id={message.id}>
            <div className={`message-content ${(
              message.timelineBlocks?.length
              || (isStreaming && message.role === 'assistant' && message.id === streamingMsgId && liveBlocks.length)
              || (message.role === 'assistant' && (message.toolCalls?.length ?? 0) > 0)
            ) ? 'message-content--agent-turn' : ''}`}>
              {/* Tool execution: compact inline badges, not a separate panel */}
              {message.role === 'assistant' && (message.toolCalls?.length ?? 0) > 0 && (
                !message.timelineBlocks?.length
                || message.toolResults?.some((result) =>
                  result.confirmationStatus === 'pending' || result.confirmationStatus === 'rejected'
                  || isFailedToolResult(result.toolName, result))
              ) && (
                <AgentRunSteps
                  toolCalls={isStreaming && liveMessageId
                    && (message.id === streamingMsgId || message.id === liveMessageId)
                    ? message.toolCalls?.filter((tool) => !livePendingCallIds.has(tool.id))
                    : message.toolCalls}
                  toolResults={message.toolResults}
                  taskCompleted={message.taskRun?.status === 'completed'}
                  reviewShown={!showRunControls && message.id === resultToReview?.id}
                  sessionId={session?.id}
                  messageId={message.id}
                  desktopRecheck={desktopRecheck}
                  onResolveConfirmation={isStopping ? undefined : onResolveConfirmation}
                />
              )}

              {message.role === 'user' && Boolean(message.textAttachments?.length) && (
                <div className="message-text-attachments" aria-label="已发送文本附件">
                  {message.textAttachments?.map((file, index) => (
                    <TextAttachmentDetails key={index} file={file} />
                  ))}
                </div>
              )}

              {message.role === 'assistant' && !(isStreaming && message.id === streamingMsgId) && (
                <>
                  {message.toolCalls?.filter((tool) => tool.name === 'pending_user_message').map((tool) => {
                    const { content } = parseToolArguments(tool);
                    return typeof content === 'string' && (
                      <p key={tool.id} className="composer-steering-receipt"><strong>引导</strong> {content}</p>
                    );
                  })}
                  <ToolActionReceipts toolCalls={message.toolCalls} toolResults={message.toolResults}
                    showUnknown={showRunControls || message.id !== resultToReview?.id} />
                </>
              )}

              {/* Live streaming is rendered through ContentBlockRenderer below; no separate timeline needed. */}

              {/* Live streaming: unified ContentBlock stream (text + inline tool cards).
                  Replaces both the standalone text panel and the parallel LiveAgentActivity
                  timeline so reasoning and tool calls are rendered in arrival order. */}
              {isStreaming && message.role === 'assistant' && message.id === streamingMsgId && liveBlocks.length ? (
                <div className="message-bubble message-bubble--content-stream">
                  <ContentBlockRenderer
                    blocks={liveBlocks}
                    sessionId={session?.id}
                    messageId={liveMessageId ?? undefined}
                    desktopRecheck={desktopRecheck}
                    onResolveConfirmation={onBlockResolveConfirmation}
                  />
                </div>
              ) : message.timelineBlocks?.length ? (
                <div className="message-bubble message-bubble--completed-agent-turn">
                  {message.content.trim() ? (
                    <div className="completed-agent-final-answer">
                      <MessageMarkdown content={message.content} deliveredFiles={delivery} />
                    </div>
                  ) : completedTimelineText(message.timelineBlocks) ? (
                    <div className="completed-agent-final-answer">
                      <MessageMarkdown content={completedTimelineText(message.timelineBlocks)} deliveredFiles={delivery} />
                    </div>
                  ) : null}
                </div>
              ) : (
                <div className="message-bubble">
                  <MessageMarkdown content={message.id === streamingMsgId ? streamingText : message.content} deliveredFiles={delivery} />
                </div>
              )}

              {message.metadata?.isToolExecuting && message.metadata.executingTool && (
                <ToolExecutingIndicator toolName={message.metadata.executingTool} />
              )}

              {reviewingResult ? (
                <ResultReviewCard message={message}
                  onOpenFiles={onOpenWorkbenchView ? () => onOpenWorkbenchView('files') : undefined} />
              ) : delivery ? <ReplyFileDelivery files={delivery} /> : null}

              {!showRunControls && message.id !== resultToReview?.id && message.id === recoveryMessage?.id && (
                <TaskRecoveryCard message={message} disabled={historyBlocked || routingBusy || Boolean(resolvingCallId)}
                  notice={recoveryNotice?.messageId === message.id ? recoveryNotice.text : undefined}
                  onRecover={(action) => { void onContinueTask(message.id, action); }}
                  canLocateFiles={Boolean(onToggleWorkbench || onOpenWorkbenchView)} />
              )}

              {windowTimelineIds.has(message.id) && <HistoricalRunDetails message={message} onLoad={readHistoricalTimeline} />}

              <div className="message-footer">
                <span className="message-time">
                  {message.role === 'user' && '你 · '}
                  {timestampToDate(message.createdAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
                </span>
                <MessageActions
                  message={message}
                  disabled={(showRunControls || historyBlocked) && message.role === 'user'}
                  recoveredEditContent={!historyBlocked && currentHistoryAction?.edit?.messageId === message.id
                    && currentHistoryAction.edit.sourceContent === message.content ? currentHistoryAction.edit.content : undefined}
                  isStreaming={isStreaming && message.id === streamingMsgId}
                  onEdit={handleEditMessage}
                  onDelete={handleHistoryAction}
                  onCopy={handleCopyMessage}
                />
              </div>
            </div>
          </div>
          </Fragment>
        ); })}
        {hasMoreNewer && <button className="conversation-page" type="button" aria-disabled={paging} onClick={(event) => loadAdjacent('newer', event.detail === 0, event.currentTarget)}>加载较新消息</button>}
      </div>
      <ConversationNavigation key={session?.id ?? 'none'} messages={renderedMessages} renderedMessages={renderedMessages} sessionId={session?.id}
        evidenceMessages={windowMessages} replyDeliveries={windowDeliveries ?? undefined}
        loadSearchPage={transcriptTail?.hasOlder || hasNewer ? searchConversationPage : undefined}
        containerRef={containerRef} showReturnToLatest={hasNewer} onJumpToMessage={jumpToMessage} onReturnToLatest={returnToLatest} />
      </div>
      <div className="composer">
        <WorkspaceActivityCapsule
          reviewInConversation={!showRunControls && Boolean(resultToReview)}
          workbenchOpen={workbenchOpen}
          onOpenTasks={onOpenWorkbenchView ? () => onOpenWorkbenchView('activity') : undefined} />
        {resultToReview && !showRunControls && (resultToReview.id !== latestMessage?.id || hasNewer) && (
          <div className="chat-history-feedback">
            <button type="button" onClick={() => jumpToMessage(resultToReview.id, true, true)}>查看待核对结果</button>
          </div>
        )}
        {pendingApproval && latestMessage && <div className="chat-history-feedback" role="status">
          <button type="button" onClick={() => jumpToMessage(latestMessage.id, true, true)}>查看待确认操作</button>
        </div>}
        {currentHistoryAction && <div className="chat-history-feedback" role={currentHistoryAction.pending ? 'status' : 'alert'}>
          <span>{currentHistoryAction.pending ? currentHistoryAction.notice ?? '正在更新对话记录…' : currentHistoryAction.notice
            ?? '未能确认删除或更新对话，请重新载入后核对结果，勿直接重复删除或发送。'}</span>
          {currentHistoryAction.error && <button type="button" onClick={() => { void handleHistoryAction(); }}>重新载入对话</button>}
        </div>}
        {session?.id && waitingFollowUps.length > 0 && <section className="composer-queue" aria-label="待发送队列">
          <header>
            <span>队列 <span className="composer-queue-count">{waitingFollowUps.length}</span></span>
            {!isStreaming && <button type="button" disabled={Boolean(isStopping || dispatchingFollowUp || historyBlocked || routingBusy || queuedFollowUps[0]?.unconfirmed)}
              onClick={() => { void sendNextFollowUp(); }}>发送下一条</button>}
          </header>
          <ol>
            {waitingFollowUps.map((item, index) => <li key={item.id}>
              <span className="composer-queue-text" title={item.text}>
                {item.unconfirmed && <strong>送达待核对 · </strong>}{item.text}
              </span>
              <button type="button" aria-label={`上移第 ${index + 1} 条待发消息`} title="上移"
                disabled={index === 0} onClick={() => {
                  updateFollowUps(session.id, (items) => {
                    const position = items.findIndex((candidate) => candidate.id === item.id);
                    if (position <= 0 || items[position - 1].id === dispatchingFollowUp) return items;
                    const reordered = [...items];
                    [reordered[position - 1], reordered[position]] = [reordered[position], reordered[position - 1]];
                    return reordered;
                  });
                }}>↑</button>
              <button type="button" aria-label={`编辑第 ${index + 1} 条待发消息`} title="移回空输入框编辑"
                disabled={Boolean(input.trim())} onClick={() => {
                  setInput(item.text, true);
                  updateFollowUps(session.id, (items) => items.filter((candidate) => candidate.id !== item.id));
                  composerInputRef.current?.focus();
                }}>编辑</button>
              <button type="button" aria-label={`移除第 ${index + 1} 条待发消息`} title="移除"
                onClick={() =>
                  updateFollowUps(session.id, (items) => items.filter((candidate) => candidate.id !== item.id))}>×</button>
            </li>)}
          </ol>
        </section>}
        {draftSaveFailed && <p className="chat-history-feedback" role="status">未能将草稿更新保存到本机，重新打开前请先复制需要保留的内容。</p>}
        <ComposerInput
          ref={composerInputRef}
          value={input}
          onChange={(text) => setInput(text, showRunControls)}
          onSend={onSend}
          scopeKey={`${activeWorkspaceId ?? ''}:${session?.id ?? ''}`}
          runControls={showRunControls && (
            <div className="agent-run-controls">
              <div className="agent-run-status" role="status">
                <span className="agent-run-status-dot" data-attention={isPaused || Boolean(currentPauseAction?.error)} aria-hidden="true" />
                <span>{isStopping ? '正在停止…' : currentPauseAction?.error ? '运行状态未确认'
                  : currentPauseAction?.pending ? `正在请求${isPaused ? '继续运行' : '暂停'}…`
                    : isPaused ? '已暂停' : liveRunPhase}</span>
              </div>
              <div className="agent-run-actions">
                <button type="button" onClick={togglePause} disabled={isStopping || currentPauseAction?.pending}
                  aria-busy={currentPauseAction?.pending || undefined}>{isPaused ? '继续运行' : '暂停'}</button>
                <button type="button" className="agent-run-stop" title="停止运行" aria-label="停止运行"
                  disabled={isStopping} aria-busy={isStopping || undefined} onClick={onAbort}>
                  <span className="agent-run-stop-icon" aria-hidden="true" />
                  停止
                </button>
              </div>
              {currentPauseAction?.error && <p className="agent-run-error" role="alert">{currentPauseAction.error}</p>}
            </div>
          )}
          stopPending={isStopping}
          onSteer={onSteer}
          onFollowUp={onFollowUp}
          isStreaming={showRunControls}
          soundEnabled={soundEnabled}
          onSoundToggle={() => setSoundEnabled((v) => !v)}
          disabled={!session || (historyBlocked && currentHistoryAction?.kind !== 'load')}
          sendDisabled={isStopping || historyBlocked || routingBusy || (!isStreaming && Boolean(dispatchingFollowUp))}
        />
        {showEmptyState && !showRunControls && <ChatStarterPrompts
          isProject={activeWorkspaceKind === 'project'}
          hidden={Boolean(input.trim())}
          onDraftSelect={(prompt) => composerInputRef.current?.insertIfEmpty(prompt)}
        />}
      </div>
      </div>
      </div>
    </main>
  );
}

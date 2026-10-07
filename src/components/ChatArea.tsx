import { Component, Fragment, useEffect, useRef, useState, useCallback, type ErrorInfo, type ReactNode } from 'react';
import { useMessagesStore, type LiveBlock } from '$stores/messages';
import { useSessionsStore } from '$stores/sessions';
import { projectWorkspaceForMessage, useWorkspacesStore } from '$stores/workspaces';
import { usePreferencesStore } from '$stores/preferences';
import { useSettingsStore } from '$stores/settings';
import { ComposerInput, type ComposerInputHandle } from './Composer/ComposerInput';
import { ChatEmptyState } from './ChatEmptyState';
import { WorkspaceReminders, AUTOMATIONS_UPDATED_EVENT } from './WorkspaceReminders';
import { ChatHeader } from './ChatHeader';
import { useThinkingEffortStore } from '$stores/thinkingEffort';
import { sendMessage, getMessages, resolveAgentConfirmation, interruptAgent, pauseAgent, resumeAgent, submitInProgressCommand } from '$lib/commands';
import { continueAgentTask, deleteMessage, editAndResendMessage } from '$lib/commands/message';
import { getWorkspaceTaskUnderstanding, type TaskDecisionProjection } from '$lib/commands/task-understanding';
import { MessageActions } from './MessageActions';
import { ContentBlockRenderer } from './ContentBlockRenderer';
import {
  authoritativeToolResultsByCallId,
  contentBlocksFromDurableEvents,
  getDurableAgentRunEvents,
} from '$lib/commands/agent-event';
import ReactMarkdown, { defaultUrlTransform } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { vscDarkPlus } from 'react-syntax-highlighter/dist/esm/styles/prism';
import type { Message, ToolCall, ToolResult, PersonalityTemplate, TextAttachment } from '$types';
import { textAttachmentBytes } from '$lib/text-attachments';
import { cardDescription } from '$lib/tavern-card';
import { extractSecureWebSearchSetup } from '$lib/web-search-setup';
import { WorkspaceActivityCapsule } from './WorkspaceActivityCapsule';
import { ChatToolbar } from './ChatToolbar';
import { TaskDecisionCard } from './TaskDecisionCard';
import { RuntimeHealthNotice } from './RuntimeHealthNotice';
import { UpdateNotice } from './UpdateNotice';
import { ModelConfigurationNotice } from './ModelConfigurationNotice';
import { formatModelSendError } from '$lib/model-config';
import { getToolDisplayName, parseMcpToolIdentity } from '$lib/tool-display';
import { desktopActionOperation, getToolConfirmationSummary, requiresDesktopActionPreview } from '$lib/tool-confirmation';
import { desktopActionReceipt, isResultUnknown } from '$lib/desktop-action-result';
import { DesktopActionApproval } from './DesktopActionApproval';

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

const INTERNAL_TOOL_NAMES = new Set(['record_task_understanding']);
const isInternalTool = (toolName: string) => INTERNAL_TOOL_NAMES.has(toolName);
const isRestrictedNetworkExploration = (toolName: string) => toolName === 'delegate_network_exploration';

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

const UNKNOWN_DESKTOP_ACTION_CONFIRMATION = `Error: ${JSON.stringify({
  code: 'RESULT_UNKNOWN',
  message: '确认请求状态未知；目标应用可能已改变。请先在应用中核对，勿直接重试。',
})}`;

function canAutoContinueConfirmedTurn(messages: Message[], messageId: string): boolean {
  const turn = messages.find((message) => message.id === messageId);
  return turn?.taskRun?.status === 'continue_suggested'
    && !(turn.toolResults ?? []).some(isResultUnknown);
}

function markUnknownLiveDesktopConfirmation(blocks: LiveBlock[], callId: string): LiveBlock[] {
  return blocks.map((block) => block.kind === 'tool_call' && block.callId === callId
    ? { ...block, status: 'failed', error: UNKNOWN_DESKTOP_ACTION_CONFIRMATION }
    : block);
}

function markUnknownPersistedDesktopConfirmation(messageId: string, callId: string): void {
  useMessagesStore.setState((state) => ({
    messages: state.messages.map((message) => {
      if (message.id !== messageId) return message;
      const toolName = message.toolCalls?.find((tool) => tool.id === callId)?.name;
      if (!toolName || !requiresDesktopActionPreview(toolName)) return message;
      const result: ToolResult = {
        callId,
        toolName,
        success: false,
        output: UNKNOWN_DESKTOP_ACTION_CONFIRMATION,
        error: UNKNOWN_DESKTOP_ACTION_CONFIRMATION,
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
  const pendingDesktopOperation = desktopActionOperation(tool.name, tool.arguments);
  const redactPendingDesktopAction = requiresDesktopActionPreview(tool.name)
    && result?.confirmationRequired && result.confirmationStatus === 'pending';
  const redactNetworkScope = isRestrictedNetworkExploration(tool.name) || redactPendingDesktopAction;
  const args = redactNetworkScope ? {} : parseToolArguments(tool);
  const output = result?.success ? result.output : result?.error ?? result?.output;

  return (
    <div className="agent-step-technical-details">
      {redactNetworkScope ? (
        <div className="agent-step-arguments">
          <code>{tool.name}</code>
          <div className="agent-step-args">
            <span>{redactPendingDesktopAction
              ? pendingDesktopOperation !== 'draft' && pendingDesktopOperation !== 'field'
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
        </div>
      )}

      {!redactNetworkScope && result && recalled.length === 0 && output != null && (
        <div className={`agent-step-output ${result.success ? '' : 'error'}`}>
          {!result.success && result.error && (
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

function DesktopActionReceipts({
  toolCalls = [],
  toolResults = [],
}: {
  toolCalls?: ToolCall[];
  toolResults?: ToolResult[];
}) {
  const results = authoritativeToolResultsByCallId(toolResults);
  const receipts = toolCalls.flatMap((tool) => {
    const receipt = desktopActionReceipt(tool.name, results.get(tool.id));
    return receipt ? [{ tool, receipt }] : [];
  });
  if (receipts.length === 0) return null;

  return (
    <div className="desktop-action-receipts" role="list" aria-label="桌面操作结果">
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

/** A user-visible explanation of an audited action, never hidden chain-of-thought. */
function getStepContext(tool: ToolCall, result?: ToolResult) {
  if (result?.confirmationRequired && result.confirmationStatus === 'pending') {
    const desktopOperation = desktopActionOperation(tool.name, tool.arguments);
    if (requiresDesktopActionPreview(tool.name)) return desktopOperation !== 'draft' && desktopOperation !== 'field'
      ? '等待核对目标窗口和待操作控件。'
      : '等待核对目标窗口和待填写内容。';
    return getToolConfirmationSummary(tool.name, tool.arguments);
  }
  if (isRestrictedNetworkExploration(tool.name)) {
    if (result?.success === false) return '受限联网探索委派未能建立。';
    if (result?.success) return '已创建受限联网探索委派。';
    return '正在准备受限联网探索委派。';
  }
  const desktopReceipt = desktopActionReceipt(tool.name, result);
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
  if (!trimmed || /no .*memor|没有|沒有/i.test(trimmed)) return [];
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
  sessionId,
  messageId,
  onResolveConfirmation,
}: {
  toolCalls?: ToolCall[];
  toolResults?: ToolResult[];
  sessionId?: string;
  messageId: string;
  onResolveConfirmation: (
    messageId: string,
    callId: string,
    decision: 'approved' | 'rejected',
    previewId?: string,
  ) => void;
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
  const hasUnknownDesktopResult = safeToolCalls.some((tool) =>
    desktopActionReceipt(tool.name, resultMap.get(tool.id))?.status === 'result_unknown'
  );
  const hasFailure = safeToolCalls.some((tool) => {
    const result = resultMap.get(tool.id);
    return result?.success === false
      && result.confirmationStatus !== 'pending'
      && result.confirmationStatus !== 'cancelled'
      && desktopActionReceipt(tool.name, result)?.status !== 'result_unknown';
  });
  const completedCount = effectiveToolResults.filter((r) => r.success).length;
  const terminalCount = completedCount
    + effectiveToolResults.filter((result) => result.confirmationStatus === 'cancelled').length;
  const isPassive = !hasPendingConfirmation && !hasFailure && terminalCount === safeToolCalls.length;

  // Successful implementation details are not part of the user's main
  // conversation. The final assistant response and project worktree carry the
  // result; only confirmations, failures, or genuinely incomplete work need
  // to interrupt the reading flow.
  if (isPassive) return null;

  // Compact summary shown as small inline text
  const summaryText = hasPendingConfirmation
    ? '等待确认'
    : hasFailure
      ? '有步骤失败'
      : hasUnknownDesktopResult
        ? '有操作结果待核对'
      : completedCount === safeToolCalls.length
        ? `${completedCount} 个动作已完成`
        : `${completedCount}/${safeToolCalls.length} 已完成`;

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
          const desktopOperation = desktopActionOperation(tool.name, tool.arguments);
          const needsDesktopPreview = requiresDesktopActionPreview(tool.name);

          return (
            <div key={tool.id} className={`agent-run-badge-row agent-run-badge-row--${status}`}>
              <span className={`agent-inline-badge agent-inline-badge--${status}`}>
                <InlineStatusIcon status={status} />
                <span>{getStepVerb(tool.name)}</span>
              </span>
              {needsApproval && needsDesktopPreview && (
                <DesktopActionApproval
                  sessionId={sessionId}
                  messageId={messageId}
                  callId={tool.id}
                  operation={desktopOperation}
                  layout="compact"
                  onResolve={(decision, previewId) => onResolveConfirmation(messageId, tool.id, decision, previewId)}
                />
              )}
              {needsApproval && !needsDesktopPreview && (
                <>
                  <span className="agent-run-confirmation-summary">
                    {getToolConfirmationSummary(tool.name, tool.arguments)}
                  </span>
                  <span className="agent-run-badge-actions">
                    <button
                      type="button"
                      className="agent-inline-btn agent-inline-btn--approve"
                      onClick={() => onResolveConfirmation(messageId, tool.id, 'approved')}
                    >
                      允许一次
                    </button>
                    <button
                      type="button"
                      className="agent-inline-btn agent-inline-btn--reject"
                      onClick={() => onResolveConfirmation(messageId, tool.id, 'rejected')}
                    >
                      拒绝
                    </button>
                  </span>
                </>
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
          className={`agent-run-disclosure agent-run-disclosure--${
            hasPendingConfirmation ? 'needs_approval' : hasFailure ? 'failed' : hasUnknownDesktopResult ? 'unknown' : isPassive ? 'completed' : 'running'
          }`}
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
            const recalled = tool.name === 'recall_memories' && result?.success
              ? parseRecalledMemories(result.output)
              : [];

            return (
              <li
                key={tool.id}
                className={`agent-step ${needsApproval ? 'approval' : success === false ? 'error' : success === true ? 'success' : 'pending'}`}
              >
                <div className="agent-step-index">{safeToolCalls.indexOf(tool) + 1}</div>
                <div className="agent-step-body">
                  <div className="agent-step-topline">
                    <span className="agent-step-title">{getStepVerb(tool.name)}</span>
                    <span className="agent-step-status">
                      {getStepStatusLabel(Boolean(needsApproval), success, desktopActionReceipt(tool.name, result)?.label)}
                    </span>
                  </div>
                  <p className="agent-step-context">{getStepContext(tool, result)}</p>

                  <AgentRunDetailsBoundary>
                    <AgentStepTechnicalDetails tool={tool} result={result} recalled={recalled} />
                  </AgentRunDetailsBoundary>

                  {result?.success === false && !needsApproval && !desktopActionReceipt(tool.name, result) && (
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

/**
 * Extract a project-relative workbench path from a Main-Agent file citation.
 *
 * `angelbot-file:` is deliberately the only accepted link scheme.  It avoids
 * treating model-authored `file://` or absolute Windows paths as an authority
 * to access an arbitrary host location; the workbench resolves the result
 * through the active session's existing file boundary.
 */
export function workspaceFilePathFromHref(href?: string): string | null {
  if (!href) return null;
  try {
    const parsed = new URL(href);
    if (
      parsed.protocol !== 'angelbot-file:'
      || parsed.host
      || parsed.search
      || parsed.hash
      || parsed.pathname.startsWith('/')
    ) return null;
    const path = decodeURIComponent(parsed.pathname);
    if (!path || path.includes('\\') || path.includes('\0')) return null;
    const segments = path.split('/');
    if (segments.some((segment) => !segment || segment === '.' || segment === '..')) return null;
    return path;
  } catch {
    return null;
  }
}

function MessageBubble({ content }: { content: string }) {
  return (
    <ReactMarkdown
      remarkPlugins={[remarkGfm]}
      // ReactMarkdown removes unknown schemes by default. Keep the one
      // deliberately narrow local citation scheme, while every other URL still
      // passes through the library's standard safety transform.
      urlTransform={(url) => (
        workspaceFilePathFromHref(url) ? url : defaultUrlTransform(url)
      )}
      components={{
        code({ className, children, ...props }) {
          const match = /language-(\w+)/.exec(className || '');
          const code = String(children).replace(/\n$/, '');
          if (match) {
            return (
              <SyntaxHighlighter
                style={vscDarkPlus}
                language={match[1]}
                PreTag="div"
              >
                {code}
              </SyntaxHighlighter>
            );
          }
          return <code className={className} {...props}>{children}</code>;
        },
        a({ href, children, ...props }) {
          const path = workspaceFilePathFromHref(href);
          if (!path) return <a href={href} {...props}>{children}</a>;
          return (
            <button
              type="button"
              className="message-file-link"
              title={`在工作台中定位 ${path}`}
              aria-label={`在工作台中定位 ${path}`}
              onClick={() => window.dispatchEvent(new CustomEvent('angelbot:locate-file', { detail: { path } }))}
            >
              {children}
            </button>
          );
        },
      }}
    >
      {content}
    </ReactMarkdown>
  );
}

function completedTimelineText(blocks: LiveBlock[]): string {
  return blocks
    .filter((block): block is Extract<LiveBlock, { kind: 'text' }> => block.kind === 'text')
    .map((block) => block.content.trim())
    .filter(Boolean)
    .join('\n\n');
}

// ─── Personality Mapping ──────────────────────────────────────────────────────

function mapPersonalityTraits(tone: string, personality: string) {
  const archetypeDefaults: Record<string, Record<string, number>> = {
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

// ─── Main ChatArea Component ──────────────────────────────────────────────────

function titleFromFirstUserMessage(content: string) {
  const normalized = content.replace(/\s+/g, ' ').trim();
  const characters = Array.from(normalized);
  return characters.length > 40 ? `${characters.slice(0, 40).join('')}…` : normalized;
}

export function ChatArea({ onToggleWorkbench }: { onToggleWorkbench?: () => void }) {
  const session = useSessionsStore((state) => state.activeSession);
  const activeWorkspaceId = useWorkspacesStore((state) => state.activeWorkspaceId);
  const activeWorkspaceKind = useWorkspacesStore((state) => (
    state.workspaces.find((workspace) => workspace.id === state.activeWorkspaceId)?.kind ?? null
  ));
  const messages = useMessagesStore((state) => state.messages);
  const input = useMessagesStore((state) => state.input);
  const setInput = useMessagesStore((state) => state.setInput);
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
  const liveBlocks = useMessagesStore((state) => state.liveBlocks);
  const liveMessageId = useMessagesStore((state) => state.liveMessageId);
  // The persisted step and the live stream may describe the same pending call.
  // Let the live card own its confirmation so the user sees one decision and
  // only one backend target preflight is started for that call.
  const livePendingCallIds = new Set(liveBlocks.flatMap((block) =>
    block.kind === 'tool_call' && block.status === 'needs_approval' ? [block.callId] : [],
  ));
  const containerRef = useRef<HTMLDivElement>(null);
  const pendingProjectMessageRef = useRef<{
    text: string; files: TextAttachment[]; workspaceId: string; sessionId: string | null;
    resolve: (accepted: boolean) => void;
  } | null>(null);
  const [routeVersion, setRouteVersion] = useState(0);
  // Keep following only while the reader is already at the newest content.
  // Streaming updates otherwise must not pull them away from older messages.
  const shouldFollowLatestRef = useRef(true);
  const composerInputRef = useRef<ComposerInputHandle>(null);
  const [resolvingCallId, setResolvingCallId] = useState<string | null>(null);
  const [isStreaming, setIsStreaming] = useState(false);
  const [isPaused, setIsPaused] = useState(false);
  const [soundEnabled, setSoundEnabled] = useState(true);
  const [toolPreset, setToolPreset] = useState<'none' | 'default' | 'full'>('default');
  const [taskDecision, setTaskDecision] = useState<TaskDecisionProjection | null>(null);
  const isStreamingRef = useRef(false);
  // Message loads and model calls can complete after the user has switched
  // sessions. A monotonically increasing generation prevents those stale
  // completions from overwriting the visible conversation.
  const sessionGenerationRef = useRef(0);
  isStreamingRef.current = isStreaming;
  const thinkingEffort = useThinkingEffortStore((s) => s.effort);

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
      setInput(current ? `${current}\n${reference}` : reference);
    };
    window.addEventListener('angelbot:reference-file', onReferenceFile);
    return () => window.removeEventListener('angelbot:reference-file', onReferenceFile);
  }, [setInput]);

  const isCurrentSessionGeneration = (sessionId: string, generation: number) =>
    sessionGenerationRef.current === generation
    && useSessionsStore.getState().activeSessionId === sessionId;

  const hydrateTimelineBlocks = useCallback(async (sessionId: string, source: Message[]) => {
    const hydrated = await Promise.all(source.map(async (message, index) => {
      // The persisted message list can normalize tool results into separate
      // `tool` rows, so an assistant run is not guaranteed to retain a
      // `toolCalls` array after completion. In that shape, tool results occur
      // as subsequent `tool` rows. Avoid replay queries for ordinary assistant
      // messages, which also keeps normal conversation loading lightweight.
      const hasFollowingToolRow = source.slice(index + 1).some((following) => {
        if (following.role === 'user' || following.role === 'assistant') return false;
        return following.role === 'tool';
      });
      if (message.role !== 'assistant' || (!(message.toolCalls?.length) && !hasFollowingToolRow)) return message;
      try {
        const events = await getDurableAgentRunEvents(sessionId, message.id);
        const timelineBlocks = contentBlocksFromDurableEvents(events, message.toolResults);
        if (!timelineBlocks.length) return message;

        // A terminal error or provider fallback is stored on the message rather
        // than emitted as a text delta. Keep it at the end of the same stream.
        const finalText = message.content.trim();
        const emittedText = timelineBlocks
          .filter((block) => block.kind === 'text')
          .map((block) => block.content)
          .join('');
        if (finalText && !emittedText.includes(finalText)) {
          const lastBlock = timelineBlocks[timelineBlocks.length - 1];
          timelineBlocks.push({
            kind: 'text',
            index: (lastBlock?.index ?? 0) + 1,
            iterationId: (lastBlock?.iterationId ?? 0) + 1,
            content: message.content,
          });
        }

        return { ...message, timelineBlocks };
      } catch {
        return message;
      }
    }));

    // Legacy persistence stores each inner tool result as a standalone `tool`
    // message. Once the assistant turn has a journal-backed timeline, those
    // rows would duplicate the inline tool slices, so omit them for that turn.
    let replayingAssistantTurn = false;
    return hydrated.filter((message) => {
      if (message.role === 'user') replayingAssistantTurn = false;
      if (message.role === 'assistant') {
        replayingAssistantTurn = Boolean(message.timelineBlocks?.length);
      }
      return message.role !== 'tool' || !replayingAssistantTurn;
    });
  }, []);

  // Load messages when session changes
  useEffect(() => {
    const sessionId = session?.id;
    const generation = ++sessionGenerationRef.current;
    clearMessages();
    // The composer is currently an in-memory, per-visible-session draft. A
    // workbench file reference must never be carried into another session.
    setInput('');
    setIsStreaming(false);
    setIsPaused(false);
    setResolvingCallId(null);

    if (sessionId) {
      getMessages(sessionId).then(async (msgs) => {
        const hydratedMessages = await hydrateTimelineBlocks(sessionId, msgs);
        if (isCurrentSessionGeneration(sessionId, generation)) {
          loadMessages(hydratedMessages);
          const active = useSessionsStore.getState().activeSession;
          const firstUserMessage = msgs.find((message) => message.role === 'user');
          if (active && !active.title.trim() && firstUserMessage) {
            const nextTitle = titleFromFirstUserMessage(firstUserMessage.content);
            if (nextTitle) useSessionsStore.getState().updateSessionTitle(sessionId, nextTitle);
          }
        }
      }).catch(() => {});
    }
  }, [session?.id, clearMessages, loadMessages, hydrateTimelineBlocks, setInput]);

  useEffect(() => () => unsubscribeFromAgentEvents(), [session?.id, unsubscribeFromAgentEvents]);

  const updateFollowLatest = useCallback(() => {
    const node = containerRef.current;
    if (!node) return;
    // A small tolerance prevents fractional layout changes from disabling follow.
    shouldFollowLatestRef.current = node.scrollHeight - node.scrollTop - node.clientHeight < 96;
  }, []);

  // Auto-scroll on new messages only when the reader has not intentionally
  // navigated away from the bottom of the conversation.
  useEffect(() => {
    const node = containerRef.current;
    if (!node || !shouldFollowLatestRef.current) return;
    node.scrollTop = node.scrollHeight;
  }, [messages, streamingText, liveStatus, liveToolSteps]);

  const [streamingMsgId, setStreamingMsgId] = useState<string | null>(null);

  const onSend = async (text: string, files: TextAttachment[] = [], clearSubmittedDraft = true): Promise<boolean> => {
    if ((!text.trim() && !files.length) || isStreamingRef.current) return false;
    const workspaceState = useWorkspacesStore.getState();
    const route = projectWorkspaceForMessage(
      text,
      workspaceState.workspaces,
      workspaceState.activeWorkspaceId,
    );
    if (route) {
      // Routing occurs before a turn begins, so the original text and every
      // later tool/file operation belong to the project's single Main session.
      if (pendingProjectMessageRef.current) return false;
      const sourceSessionId = session?.id;
      const sourceGeneration = sessionGenerationRef.current;
      return new Promise<boolean>((resolve) => {
        const pending = { text, files, workspaceId: route.id, sessionId: null as string | null, resolve };
        pendingProjectMessageRef.current = pending;
        setInput('');
        void workspaceState.openWorkspace(route.id).then(() => {
          if (pendingProjectMessageRef.current !== pending) return;
          const currentWorkspace = useWorkspacesStore.getState();
          const currentSession = useSessionsStore.getState();
          const targetSessionId = currentWorkspace.workspaces.find((item) => item.id === route.id)?.activeSessionId;
          if (currentWorkspace.activeWorkspaceId !== route.id || !targetSessionId
            || currentSession.activeSessionId !== targetSessionId) {
            pendingProjectMessageRef.current = null;
            resolve(false);
            return;
          }
          pending.sessionId = targetSessionId;
          setRouteVersion((version) => version + 1);
        }).catch(() => {
          if (pendingProjectMessageRef.current === pending) pendingProjectMessageRef.current = null;
          if (useWorkspacesStore.getState().activeWorkspaceId === workspaceState.activeWorkspaceId
            && sessionGenerationRef.current === sourceGeneration
            && useSessionsStore.getState().activeSessionId === sourceSessionId
            && !useMessagesStore.getState().input.trim()) setInput(text);
          resolve(false);
        });
      });
    }
    if (!session?.id) return false;
    const secureWebSearch = extractSecureWebSearchSetup(text);
    const messageContent = secureWebSearch.displayContent;
    const sessionId = session.id;
    const generation = sessionGenerationRef.current;
    // Clear only the draft submitted by this caller, synchronously before
    // subscription can yield. Routed turns do not own the destination draft.
    if (clearSubmittedDraft && useMessagesStore.getState().input.trim() === text.trim()) setInput('');
    clearLiveActivity();
    useMessagesStore.getState().setLiveStatus('正在理解你的请求');
    try { await subscribeToAgentEvents(sessionId); }
    catch {
      if (isCurrentSessionGeneration(sessionId, generation) && !useMessagesStore.getState().input.trim()) setInput(messageContent);
      return false;
    }
    if (!isCurrentSessionGeneration(sessionId, generation)) return false;
    isStreamingRef.current = true;
    setIsStreaming(true);
    const personality = useSettingsStore.getState().profile;
    const preferences = usePreferencesStore.getState().preferences;
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
        personality: {
          name: personality.name,
          description: personality.characterCard
            ? cardDescription({
              name: personality.name,
              description: personality.bio,
              firstMes: personality.greeting,
              card: personality.characterCard,
            })
            : personality.bio,
          greeting: personality.greeting,
          traits: personality.traits ?? mapPersonalityTraits(personality.tone, personality.personality),
        } as PersonalityTemplate,
        preferences: mapPreferences(preferences),
        thinkingEffort,
        webSearchSetup: secureWebSearch.setup,
      });
      if (isCurrentSessionGeneration(sessionId, generation)) {
        // Replace the placeholder with the actual reply
        useMessagesStore.getState().removeMessage(placeholderId);
        // The backend owns durable message IDs and may enrich the reply with
        // task-run data. Reloading the completed turn keeps later edit/delete
        // actions bound to persisted rows rather than optimistic placeholders.
        try {
          const refreshedMessages = await hydrateTimelineBlocks(sessionId, await getMessages(sessionId));
          if (isCurrentSessionGeneration(sessionId, generation)) loadMessages(refreshedMessages);
        } catch {
          if (isCurrentSessionGeneration(sessionId, generation)) appendMessage(reply);
        }
      }
      return true;
    } catch (err) {
      console.error('[ChatArea] sendMessage error:', err);
      let accepted = false;
      if (isCurrentSessionGeneration(sessionId, generation)) {
        useMessagesStore.getState().removeMessage(placeholderId);
        // A model/transport failure can occur after the user turn was saved.
        // Only the durable client ID decides whether the submitted snapshot
        // was accepted. Never automatically retry an uncertain send.
        try {
          const durableMessages = await getMessages(sessionId);
          if (!isCurrentSessionGeneration(sessionId, generation)) return false;
          accepted = durableMessages.some((message) => message.id === userMessage.id);
          if (accepted) {
            const refreshedMessages = await hydrateTimelineBlocks(sessionId, durableMessages);
            if (isCurrentSessionGeneration(sessionId, generation)) loadMessages(refreshedMessages);
          } else useMessagesStore.getState().removeMessage(userMessage.id);
        } catch { /* Unknown persistence: retain attachments for user inspection. */ }
        if (!isCurrentSessionGeneration(sessionId, generation)) return false;
        if (!accepted && !useMessagesStore.getState().input.trim()) setInput(messageContent);
        appendMessage({
          id: crypto.randomUUID(),
          sessionId,
          role: 'assistant',
          content: formatModelSendError(err),
          createdAt: Date.now(),
        });
      }
      return accepted;
    } finally {
      if (isCurrentSessionGeneration(sessionId, generation)) {
        setIsStreaming(false);
        isStreamingRef.current = false;
        setStreamingMsgId(null);
        window.dispatchEvent(new Event(AUTOMATIONS_UPDATED_EVENT));
      }
    }
  };

  const onAbort = async () => {
    if (session?.id) await interruptAgent(session.id).catch(() => {});
    unsubscribeFromAgentEvents();
    setIsStreaming(false);
    setIsPaused(false);
  };

  const togglePause = async () => {
    if (!session?.id) return;
    if (isPaused) await resumeAgent(session.id); else await pauseAgent(session.id);
    setIsPaused((paused) => !paused);
  };

  const onSteer = async (text: string) => {
    if (session?.id) await submitInProgressCommand(session.id, text, 'steer');
  };
  const onFollowUp = async (text: string) => {
    if (session?.id) await submitInProgressCommand(session.id, text, 'follow_up');
  };

  /**
   * Confirmation handler used by ContentBlockRenderer. It resolves the tool
   * call through the existing pipeline and waits for the backend result before
   * refreshing the inline state.
   */
  const onBlockResolveConfirmation = async (
    callId: string,
    decision: 'approved' | 'rejected',
    previewId?: string,
  ) => {
    if (!session?.id || resolvingCallId) return;
    const messageId = liveMessageId;
    if (!messageId) return;
    const sessionId = session.id;
    const generation = sessionGenerationRef.current;
    setResolvingCallId(callId);
    // Keep the confirmation pending until the backend returns its outcome.
    // Approval alone does not prove that the desktop action succeeded.
    let resolved = false;
    try {
      const result = await resolveAgentConfirmation({
        sessionId,
        messageId,
        callId,
        decision,
        previewId,
      });
      resolved = true;
      if (isCurrentSessionGeneration(sessionId, generation)) {
        addToolResult(messageId, result);
      }

      if (isCurrentSessionGeneration(sessionId, generation)) {
        clearLiveActivity();
        const refreshedMessages = await getMessages(sessionId);
        if (!isCurrentSessionGeneration(sessionId, generation)) return;
        loadMessages(refreshedMessages);
        if (decision === 'approved' && result.success
          && canAutoContinueConfirmedTurn(refreshedMessages, messageId)) {
          await subscribeToAgentEvents(sessionId);
          setIsStreaming(true);
          await continueAgentTask({ sessionId, messageId });
          if (isCurrentSessionGeneration(sessionId, generation)) {
            loadMessages(await getMessages(sessionId));
          }
        }
      }
    } catch (err) {
      console.error('[ChatArea] block resolve confirmation error:', err);
      if (isCurrentSessionGeneration(sessionId, generation)) {
        // A lost IPC response may hide a dispatched action. First ask the
        // backend; never turn an uncertain desktop action into a retry button.
        try {
          const refreshedMessages = await getMessages(sessionId);
          if (isCurrentSessionGeneration(sessionId, generation)) {
            loadMessages(refreshedMessages);
            unsubscribeFromAgentEvents();
            setIsStreaming(false);
            setStreamingMsgId(null);
          }
        } catch {
          if (!isCurrentSessionGeneration(sessionId, generation)) return;
          if (!resolved && decision === 'approved' && previewId) {
            useMessagesStore.setState((state) => ({
              liveBlocks: markUnknownLiveDesktopConfirmation(state.liveBlocks, callId),
            }));
          } else if (!resolved) {
            useMessagesStore.setState((state) => ({
              liveBlocks: restoreRetryableLiveConfirmation(state.liveBlocks, callId),
            }));
          }
        }
      }
    } finally {
      if (isCurrentSessionGeneration(sessionId, generation)) {
        setResolvingCallId(null);
        window.dispatchEvent(new Event(AUTOMATIONS_UPDATED_EVENT));
      }
    }
  };

  useEffect(() => {
    const routedMessage = pendingProjectMessageRef.current;
    if (!routedMessage?.sessionId) return;
    pendingProjectMessageRef.current = null;
    if (activeWorkspaceId !== routedMessage.workspaceId || session?.id !== routedMessage.sessionId) {
      routedMessage.resolve(false);
      return;
    }
    const routedGeneration = sessionGenerationRef.current;
    void onSend(routedMessage.text, routedMessage.files, false).then((accepted) => {
      if (!accepted && isCurrentSessionGeneration(routedMessage.sessionId!, routedGeneration)
        && useWorkspacesStore.getState().activeWorkspaceId === routedMessage.workspaceId
        && useSessionsStore.getState().activeSessionId === routedMessage.sessionId) {
        composerInputRef.current?.restoreFiles(routedMessage.files);
      }
      routedMessage.resolve(accepted);
    });
  }, [session?.id, activeWorkspaceId, routeVersion]);

  const onResolveConfirmation = async (
    messageId: string,
    callId: string,
    decision: 'approved' | 'rejected',
    previewId?: string,
  ) => {
    if (!session?.id || resolvingCallId) return;
    const sessionId = session.id;
    const generation = sessionGenerationRef.current;
    setResolvingCallId(callId);
    let resolved = false;
    try {
      const result = await resolveAgentConfirmation({
        sessionId,
        messageId,
        callId,
        decision,
        previewId,
      });
      resolved = true;
      if (isCurrentSessionGeneration(sessionId, generation)) {
        addToolResult(messageId, result);
      }

      if (isCurrentSessionGeneration(sessionId, generation)) {
        clearLiveActivity();
        const refreshedMessages = await getMessages(sessionId);
        if (!isCurrentSessionGeneration(sessionId, generation)) return;
        loadMessages(refreshedMessages);
        if (decision === 'approved' && result.success
          && canAutoContinueConfirmedTurn(refreshedMessages, messageId)) {
          // A successful step alone is not enough: another pending or unknown
          // step in this turn must keep the Main Agent paused.
          await subscribeToAgentEvents(sessionId);
          setIsStreaming(true);
          await continueAgentTask({ sessionId, messageId });
          if (isCurrentSessionGeneration(sessionId, generation)) {
            loadMessages(await getMessages(sessionId));
          }
        }
      }
    } catch (err) {
      console.error('[ChatArea] resolve confirmation error:', err);
      if (isCurrentSessionGeneration(sessionId, generation)) {
        try {
          const refreshedMessages = await getMessages(sessionId);
          if (isCurrentSessionGeneration(sessionId, generation)) {
            loadMessages(refreshedMessages);
          }
        } catch {
          if (!isCurrentSessionGeneration(sessionId, generation)) return;
          if (!resolved && decision === 'approved' && previewId) {
            markUnknownPersistedDesktopConfirmation(messageId, callId);
          } else if (!resolved) {
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
      if (isCurrentSessionGeneration(sessionId, generation)) {
        setResolvingCallId(null);
        setIsStreaming(false);
        window.dispatchEvent(new Event(AUTOMATIONS_UPDATED_EVENT));
      }
    }
  };

  // ── Message action handlers ─────────────────────────────────────────────────

  const messagesRef = useRef(messages);
  messagesRef.current = messages;

  const handleEditMessage = useCallback(async (messageId: string, newContent: string) => {
    if (!session?.id) return;
    const sessionId = session.id;
    const generation = sessionGenerationRef.current;
    const placeholderId = crypto.randomUUID();
    let messagesBeforeEdit: Message[] | null = null;
    try {
      clearLiveActivity();
      useMessagesStore.getState().setLiveStatus('正在基于编辑后的消息重新生成');
      await subscribeToAgentEvents(sessionId);
      setIsStreaming(true);
      setStreamingMsgId(placeholderId);
      const current = messagesRef.current;
      messagesBeforeEdit = current;
      const editedIndex = current.findIndex((message) => message.id === messageId);
      if (editedIndex >= 0) {
        // Backend hard-deletes the edited message and everything after it.
        // Keep the replacement user turn visible while its new assistant
        // response streams. This avoids a blank gap between pressing resend
        // and receiving the durable transcript back from the backend.
        const editedMessage = {
          ...current[editedIndex],
          content: newContent,
        };
        useMessagesStore.setState({
          messages: [
            ...current.slice(0, editedIndex),
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
      const personality = useSettingsStore.getState().profile;
      const preferences = usePreferencesStore.getState().preferences;
      const reply = await editAndResendMessage({
        sessionId,
        messageId,
        content: newContent,
        personality: {
          name: personality.name,
          description: personality.characterCard
            ? cardDescription({ name: personality.name, description: personality.bio, firstMes: personality.greeting, card: personality.characterCard })
            : personality.bio,
          greeting: personality.greeting,
          traits: personality.traits ?? mapPersonalityTraits(personality.tone, personality.personality),
        } as PersonalityTemplate,
        preferences: mapPreferences(preferences),
        thinkingEffort,
      });
      if (isCurrentSessionGeneration(sessionId, generation)) {
        useMessagesStore.getState().removeMessage(placeholderId);
        try {
          const durableMessages = await getMessages(sessionId);
          if (!isCurrentSessionGeneration(sessionId, generation)) return;

          // Never let a transient or inconsistent reload erase the local
          // replacement turn. The backend preserves `messageId` on a resend;
          // its absence is therefore a recoverable refresh failure, not an
          // empty conversation.
          if (!durableMessages.some((message) => message.id === messageId)) {
            appendMessage(reply);
            return;
          }

          loadMessages(await hydrateTimelineBlocks(sessionId, durableMessages));
        } catch {
          appendMessage(reply);
        }
      }
    } catch (err) {
      console.error('[ChatArea] edit and resend error:', err);
      if (isCurrentSessionGeneration(sessionId, generation)) {
        if (messagesBeforeEdit) {
          loadMessages(messagesBeforeEdit);
        } else {
          useMessagesStore.getState().removeMessage(placeholderId);
        }
      }
    } finally {
      if (isCurrentSessionGeneration(sessionId, generation)) {
        setIsStreaming(false);
        setStreamingMsgId(null);
        window.dispatchEvent(new Event(AUTOMATIONS_UPDATED_EVENT));
      }
    }
  }, [session?.id, thinkingEffort, clearLiveActivity, subscribeToAgentEvents, appendMessage, hydrateTimelineBlocks]);

  const handleDeleteMessage = useCallback(async (messageId: string) => {
    if (!confirm('Delete this message?')) return;
    try {
      await deleteMessage(messageId);
      useMessagesStore.getState().removeMessage(messageId);
    } catch (err) {
      console.error('[ChatArea] delete message error:', err);
    }
  }, []);

  const handleCopyMessage = useCallback((_messageId: string) => {
    // Clipboard write is handled inside MessageActions; this is just a callback hook
  }, []);

  return (
    <main className="main-with-nav">
      <div className="chat-area">
        <ChatHeader onToggleWorkbench={onToggleWorkbench} />
        <UpdateNotice />
        <RuntimeHealthNotice />
        <ModelConfigurationNotice />
        <ChatToolbar branchOnly />
        <TaskDecisionCard decision={taskDecision} />
        {messages.length > 0 && (
          <WorkspaceReminders workspaceId={activeWorkspaceKind ? activeWorkspaceId : null} />
        )}
      <div className="messages" ref={containerRef} onScroll={updateFollowLatest}>
        {messages.length === 0 && (
          <ChatEmptyState
            onDraftSelect={(prompt) => composerInputRef.current?.insertIfEmpty(prompt)}
            showStarters={!input.trim()}
          />
        )}
        {messages.map((message) => (
          <Fragment key={message.id}>
          <div className={`message-row ${message.role}`}>
            {message.role === 'user' && (
              <div className="message-avatar">
                <div className="avatar user-avatar">U</div>
              </div>
            )}
            <div className={`message-content ${(
              message.timelineBlocks?.length
              || (isStreaming && message.role === 'assistant' && message.id === streamingMsgId && liveBlocks.length)
              || (message.role === 'assistant' && (message.toolCalls?.length ?? 0) > 0)
            ) ? 'message-content--agent-turn' : ''}`}>
              {message.role === 'user' && <div className="message-role-label">你</div>}

              {/* Tool execution: compact inline badges, not a separate panel */}
              {message.role === 'assistant' && (message.toolCalls?.length ?? 0) > 0 && (
                !message.timelineBlocks?.length
                || message.toolResults?.some((result) =>
                  result.confirmationStatus === 'pending' || result.confirmationStatus === 'rejected')
              ) && (
                <AgentRunSteps
                  toolCalls={isStreaming && liveMessageId
                    && (message.id === streamingMsgId || message.id === liveMessageId)
                    ? message.toolCalls?.filter((tool) => !livePendingCallIds.has(tool.id))
                    : message.toolCalls}
                  toolResults={message.toolResults}
                  sessionId={session?.id}
                  messageId={message.id}
                  onResolveConfirmation={onResolveConfirmation}
                />
              )}

              {message.role === 'user' && Boolean(message.textAttachments?.length) && (
                <div className="message-text-attachments" aria-label="已发送文本附件">
                  {message.textAttachments?.map((file, index) => (
                    <details key={index}>
                      <summary>{file.name}<small>{textAttachmentBytes(file.text)} B · 文本快照</small></summary>
                      <pre>{file.text}</pre>
                    </details>
                  ))}
                </div>
              )}

              {message.role === 'assistant' && !(isStreaming && message.id === streamingMsgId) && (
                <DesktopActionReceipts toolCalls={message.toolCalls} toolResults={message.toolResults} />
              )}

              {/* Live streaming is rendered through ContentBlockRenderer below; no separate timeline needed. */}

              {/* Live streaming: unified ContentBlock stream (text + inline tool cards).
                  Replaces both MessageBubble text panel and the parallel LiveAgentActivity
                  timeline so reasoning and tool calls are rendered in arrival order. */}
              {isStreaming && message.role === 'assistant' && message.id === streamingMsgId && liveBlocks.length ? (
                <div className="message-bubble message-bubble--content-stream">
                  <ContentBlockRenderer
                    blocks={liveBlocks}
                    sessionId={session?.id}
                    messageId={liveMessageId ?? undefined}
                    onResolveConfirmation={onBlockResolveConfirmation}
                  />
                </div>
              ) : message.timelineBlocks?.length ? (
                <div className="message-bubble message-bubble--completed-agent-turn">
                  {message.content.trim() ? (
                    <div className="completed-agent-final-answer">
                      <MessageBubble content={message.content} />
                    </div>
                  ) : completedTimelineText(message.timelineBlocks) ? (
                    <div className="completed-agent-final-answer">
                      <MessageBubble content={completedTimelineText(message.timelineBlocks)} />
                    </div>
                  ) : null}
                </div>
              ) : (
                <div className="message-bubble">
                  <MessageBubble content={message.id === streamingMsgId ? streamingText : message.content} />
                </div>
              )}

              {message.metadata?.isToolExecuting && message.metadata.executingTool && (
                <ToolExecutingIndicator toolName={message.metadata.executingTool} />
              )}

              <div className="message-time">
                    {timestampToDate(message.createdAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
              </div>

              <MessageActions
                message={message}
                onEdit={handleEditMessage}
                onDelete={handleDeleteMessage}
                onCopy={handleCopyMessage}
              />
            </div>
          </div>
          </Fragment>
        ))}
      </div>
      <WorkspaceActivityCapsule />
      <div className="composer">
        {isStreaming && <div className="agent-run-controls" aria-label="运行控制">
          <div className="agent-run-status">
            <span className={`agent-run-status-dot${isPaused ? ' paused' : ''}`} aria-hidden="true" />
            <span>{isPaused ? '已暂停' : '正在执行'}</span>
            <small>{isPaused ? '恢复后将继续当前任务' : '可介入当前任务或排队后续指令'}</small>
          </div>
          <div className="agent-run-actions">
            <button type="button" onClick={togglePause}>{isPaused ? '继续运行' : '暂停'}</button>
            <button type="button" className="agent-run-stop" onClick={onAbort}>停止运行</button>
          </div>
        </div>}
        <ComposerInput
          ref={composerInputRef}
          value={input}
          onChange={setInput}
          onSend={onSend}
          scopeKey={`${activeWorkspaceId ?? ''}:${session?.id ?? ''}`}
          onAbort={onAbort}
          onSteer={onSteer}
          onFollowUp={onFollowUp}
          isStreaming={isStreaming}
          toolPreset={toolPreset}
          onToolPresetChange={setToolPreset}
          soundEnabled={soundEnabled}
          onSoundToggle={() => setSoundEnabled((v) => !v)}
          disabled={!session}
        />
      </div>
      </div>
    </main>
  );
}

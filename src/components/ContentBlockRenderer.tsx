import { useEffect, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { vscDarkPlus } from 'react-syntax-highlighter/dist/esm/styles/prism';
import type { LiveBlock } from '$stores/messages';
import { desktopActionOperation, getToolConfirmationSummary, requiresDesktopActionPreview } from '$lib/tool-confirmation';
import { desktopActionReceipt } from '$lib/desktop-action-result';
import { getToolDisplayName } from '$lib/tool-display';
import { DesktopActionApproval } from './DesktopActionApproval';
import '../styles/content-block-renderer.css';

interface ContentBlockRendererProps {
  blocks: LiveBlock[];
  sessionId?: string;
  messageId?: string;
  onResolveConfirmation?: (
    callId: string,
    decision: 'approved' | 'rejected',
    previewId?: string,
  ) => void;
}

/**
 * Renders a unified stream of text + tool slices in arrival order.
 *
 * Layout rules:
 *  - Consecutive text blocks (sharing the same `iterationId`) merge
 *    into one paragraph. Each new text segment starts on a new line.
 *  - Consecutive tool blocks with the same user-facing action merge
 *    into one "execution slice" card — even across agent iterations —
 *    as a collapsible panel that lists
 *    each tool call as a row. Two reasons:
 *      1. A 10-tool reading burst would otherwise create ten inline
 *         badges, drowning the model's reasoning.
 *      2. The UI gains a natural place to show progress ("3/10 done")
 *         and an aggregate status icon (running / completed / failed /
 *         paused for approval).
 *  - A slice with exactly one tool still renders as a slice so the
 *    user gets a single, consistent treatment — the inline badge is
 *    reserved for the "one quick tool call between sentences" case
 *    that no longer happens after grouping.
 *
 * Action policy:
 *  - `running` shows a spinner inline. Details hidden by default.
 *  - `completed` shows a check + condensed label. Details hidden by default.
 *  - `failed` shows an X + condensed label + recovery hint when present.
 *  - `needs_approval` pauses the slice and renders inline allow/deny buttons.
 */

type Segment =
  | {
      kind: 'text';
      iterationId: number;
      content: string;
    }
  | {
      kind: 'slice';
      iterationId: number;
      action: ToolAction;
      tools: Extract<LiveBlock, { kind: 'tool_call' }>[];
    };

type ToolAction =
  | 'explore'
  | 'edit'
  | 'command'
  | 'verify'
  | 'web'
  | 'memory'
  | 'desktop'
  | 'general';

/**
 * Group blocks into segments. The rule:
 *  - Consecutive text blocks sharing an `iterationId` merge into one segment.
 *  - Consecutive tools with the same user-facing action merge across
 *    iterations; text or an action change starts a new segment.
 *
 * This keeps the text/tool timeline intact while preventing a retry or a
 * multi-command burst from becoming a stack of one-operation cards.
 */
export function groupBlocksIntoSegments(blocks: LiveBlock[]): Segment[] {
  const segments: Segment[] = [];
  let current: Segment | null = null;

  for (const block of blocks) {
    if (block.kind === 'text') {
      if (current && current.kind === 'text' && current.iterationId === block.iterationId) {
        current = {
          kind: 'text',
          iterationId: current.iterationId,
          content: current.content + block.content,
        };
        segments[segments.length - 1] = current;
      } else {
        current = { kind: 'text', iterationId: block.iterationId, content: block.content };
        segments.push(current);
      }
    } else {
      // tool_call
      const action = actionForTool(block);
      if (current && current.kind === 'slice' && current.action === action) {
        current.tools.push(block);
        segments[segments.length - 1] = current;
      } else {
        current = {
          kind: 'slice',
          iterationId: block.iterationId,
          action,
          tools: [block],
        };
        segments.push(current);
      }
    }
  }

  return segments;
}

function aggregateStatus(
  tools: Extract<LiveBlock, { kind: 'tool_call' }>[],
): 'running' | 'completed' | 'failed' | 'needs_approval' {
  if (tools.some((t) => t.status === 'needs_approval')) return 'needs_approval';
  if (tools.some((t) => t.status === 'failed')) return 'failed';
  if (tools.some((t) => t.status === 'running')) return 'running';
  return 'completed';
}

function aggregateLabel(tools: Extract<LiveBlock, { kind: 'tool_call' }>[]): string {
  const completed = tools.filter((t) => t.status === 'completed').length;
  return `${completed}/${tools.length}`;
}

function actionLabel(action: ToolAction): string {
  switch (action) {
    case 'explore': return '探索工作区';
    case 'edit': return '编辑文件';
    case 'command': return '运行命令';
    case 'verify': return '验证结果';
    case 'web': return '浏览网页';
    case 'memory': return '整理记忆';
    case 'desktop': return '操作电脑';
    default: return '处理任务';
  }
}

function liveDesktopReceipt(tool: Extract<LiveBlock, { kind: 'tool_call' }>) {
  if (tool.status === 'running' || tool.status === 'needs_approval') return null;
  return desktopActionReceipt(tool.toolName, {
    success: tool.status === 'completed',
    output: tool.output ?? '',
    error: tool.error,
  });
}

function sliceTitle(
  status: 'running' | 'completed' | 'failed' | 'needs_approval',
  action: ToolAction,
): string {
  const label = actionLabel(action);
  if (status === 'completed') return `已完成${label}`;
  if (status === 'running') return `正在${label}`;
  if (status === 'needs_approval') return `${label}等待确认`;
  return `${label}需要处理`;
}

export function ContentBlockRenderer({
  blocks,
  sessionId,
  messageId,
  onResolveConfirmation,
}: ContentBlockRendererProps) {
  if (blocks.length === 0) return null;
  const segments = groupBlocksIntoSegments(blocks);
  return (
    <div className="content-block-stream">
      {segments.map((segment, i) =>
        segment.kind === 'text' ? (
          <TextSegment key={`text-${segment.iterationId}-${i}`} segment={segment} />
        ) : (
          <ToolSlice
            key={`slice-${segment.tools[0]?.index ?? segment.iterationId}-${i}`}
            segment={segment}
            sessionId={sessionId}
            messageId={messageId}
            onResolveConfirmation={onResolveConfirmation}
          />
        ),
      )}
    </div>
  );
}

function TextSegment({ segment }: { segment: Extract<Segment, { kind: 'text' }> }) {
  return (
    <div className="content-block-text">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
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
        }}
      >
        {segment.content}
      </ReactMarkdown>
    </div>
  );
}

function ToolSlice({
  segment,
  sessionId,
  messageId,
  onResolveConfirmation,
}: {
  segment: Extract<Segment, { kind: 'slice' }>;
  sessionId?: string;
  messageId?: string;
  onResolveConfirmation?: (callId: string, decision: 'approved' | 'rejected', previewId?: string) => void;
}) {
  const status = aggregateStatus(segment.tools);
  const receipts = segment.tools.map(liveDesktopReceipt);
  const hasUnknownDesktop = receipts.some((receipt) => receipt?.status === 'result_unknown');
  const allDesktopDispatched = segment.action === 'desktop' && status === 'completed'
    && receipts.every((receipt) => receipt?.status === 'dispatched');
  const allDesktopVerified = segment.action === 'desktop' && status === 'completed'
    && receipts.every((receipt) => receipt?.status === 'verified');
  const hasVerifiedControl = allDesktopVerified
    && segment.tools.some((tool) => tool.toolName === 'operate_trusted_app_control');
  // Keep completed work compact. A running or approval-gated slice remains
  // open so progress and its action are immediately visible.
  const [open, setOpen] = useState(
    status === 'needs_approval' || status === 'running' || hasUnknownDesktop,
  );
  useEffect(() => {
    // Live work stays visible. Once a batch reaches a terminal state it
    // returns to the compact Codex-style summary, while details remain one
    // click away.
    setOpen(status === 'needs_approval' || status === 'running' || hasUnknownDesktop);
  }, [status, hasUnknownDesktop]);
  const label = aggregateLabel(segment.tools);
  const title = hasUnknownDesktop
    ? '桌面操作结果待核对'
    : allDesktopDispatched
      ? '已发出桌面请求'
      : allDesktopVerified
          ? hasVerifiedControl ? '控件状态已确认，任务结果待检查' : '输入内容已填写并校验'
        : segment.action === 'desktop' && status === 'completed'
          ? '桌面操作状态未确认'
          : sliceTitle(status, segment.action);

  return (
    <div className={`content-block-slice content-block-slice--${status}`} aria-live="polite">
      <button
        type="button"
        className="content-block-slice-header"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
      >
        <span className="content-block-slice-chevron" aria-hidden="true">
          {open ? '▾' : '▸'}
        </span>
        <span className={`content-block-slice-icon content-block-slice-icon--${status}`}>
          <SliceStatusIcon status={status} />
        </span>
        <span className="content-block-slice-title">{title}</span>
        <span className="content-block-slice-count">{label}</span>
      </button>
      {open && (
        <div className="content-block-slice-list" role="list">
          {segment.tools.map((tool, i) => (
            <div key={`${tool.callId}-${i}`} role="listitem">
              <InlineToolRow
                tool={tool}
                sessionId={sessionId}
                messageId={messageId}
                onResolveConfirmation={onResolveConfirmation}
              />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function InlineToolRow({
  tool,
  sessionId,
  messageId,
  onResolveConfirmation,
}: {
  tool: Extract<LiveBlock, { kind: 'tool_call' }>;
  sessionId?: string;
  messageId?: string;
  onResolveConfirmation?: (callId: string, decision: 'approved' | 'rejected', previewId?: string) => void;
}) {
  const [detailsOpen, setDetailsOpen] = useState(false);
  const verb = verbForTool(tool.toolName);
  const needsApproval = tool.status === 'needs_approval';
  const desktopOperation = desktopActionOperation(tool.toolName, tool.arguments);
  const needsDesktopPreview = requiresDesktopActionPreview(tool.toolName);
  const hasDetails = Boolean(tool.error || tool.output);
  const receipt = liveDesktopReceipt(tool);

  return (
    <div className={`content-block-tool content-block-tool--${tool.status}`}>
      {hasDetails ? (
        <button
          type="button"
          className="content-block-tool-row content-block-tool-row--expandable"
          onClick={() => setDetailsOpen((v) => !v)}
          aria-expanded={detailsOpen}
          aria-label={`${detailsOpen ? '收起' : '展开'} ${verb} 的输出`}
        >
          <span className="content-block-tool-chevron" aria-hidden="true">{detailsOpen ? '▾' : '▸'}</span>
          <ToolRowLabel tool={tool} verb={verb} />
          {receipt && <span className="content-block-tool-receipt">{receipt.label}：{receipt.explanation}</span>}
        </button>
      ) : (
        <div className="content-block-tool-row">
          <ToolRowLabel tool={tool} verb={verb} />
          {receipt && <span className="content-block-tool-receipt">{receipt.label}：{receipt.explanation}</span>}
        </div>
      )}
      {needsApproval && onResolveConfirmation && needsDesktopPreview && (
        <DesktopActionApproval
          sessionId={sessionId}
          messageId={messageId}
          callId={tool.callId}
          operation={desktopOperation}
          layout="stream"
          onResolve={(decision, previewId) => onResolveConfirmation(tool.callId, decision, previewId)}
        />
      )}
      {needsApproval && onResolveConfirmation && !needsDesktopPreview && (
        <div className="content-block-tool-confirmation">
          <span className="content-block-tool-confirmation-summary">
            {getToolConfirmationSummary(tool.toolName, tool.arguments)}
          </span>
          <div className="content-block-tool-actions">
            <button
              type="button"
              className="content-block-tool-btn content-block-tool-btn--approve"
              onClick={() => onResolveConfirmation(tool.callId, 'approved')}
              title={tool.reason}
            >
              允许
            </button>
            <button
              type="button"
              className="content-block-tool-btn content-block-tool-btn--reject"
              onClick={() => onResolveConfirmation(tool.callId, 'rejected')}
              title={tool.reason}
            >
              拒绝
            </button>
          </div>
        </div>
      )}
      {detailsOpen && (tool.error || tool.output) && (
        <pre className="content-block-tool-details">{tool.error ?? tool.output}</pre>
      )}
    </div>
  );
}

function ToolRowLabel({
  tool,
  verb,
}: {
  tool: Extract<LiveBlock, { kind: 'tool_call' }>;
  verb: string;
}) {
  return (
    <span className={`content-block-tool-badge content-block-tool-badge--${tool.status}`}>
      <InlineStatusIcon status={tool.status} />
      <span className="content-block-tool-verb">{verb}</span>
    </span>
  );
}

function SliceStatusIcon({
  status,
}: {
  status: 'running' | 'completed' | 'failed' | 'needs_approval';
}) {
  if (status === 'completed') {
    return (
      <svg width="12" height="12" viewBox="0 0 10 10" fill="none" aria-hidden="true">
        <path
          d="M2 5.5L4 7.5L8 3"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    );
  }
  if (status === 'failed') {
    return (
      <svg width="12" height="12" viewBox="0 0 10 10" fill="none" aria-hidden="true">
        <path d="M3 3L7 7M7 3L3 7" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
      </svg>
    );
  }
  if (status === 'needs_approval') {
    return (
      <svg width="12" height="12" viewBox="0 0 10 10" fill="none" aria-hidden="true">
        <circle cx="5" cy="5" r="4" stroke="currentColor" strokeWidth="1.2" />
        <rect x="4" y="4" width="2" height="2" rx="0.5" fill="currentColor" />
      </svg>
    );
  }
  return (
    <svg width="12" height="12" viewBox="0 0 10 10" fill="none" aria-hidden="true">
      <circle cx="5" cy="5" r="3.5" stroke="currentColor" strokeWidth="1" opacity="0.4" />
      <circle cx="5" cy="5" r="2" fill="currentColor" />
    </svg>
  );
}

function InlineStatusIcon({
  status,
}: {
  status: 'running' | 'completed' | 'failed' | 'needs_approval';
}) {
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

function actionForTool(tool: Extract<LiveBlock, { kind: 'tool_call' }>): ToolAction {
  // Classify completed legacy receipts for rendering, without enabling their
  // retired pending actions or treating dispatch as a completed business task.
  if (liveDesktopReceipt(tool)) return 'desktop';
  switch (tool.toolName) {
    case 'list_dir':
    case 'read_file':
    case 'search_files':
    case 'grep_search':
      return 'explore';
    case 'write_file':
    case 'edit_file':
    case 'delete_file':
    case 'create_directory':
      return 'edit';
    case 'web_search':
    case 'web_fetch':
      return 'web';
    case 'recall_memories':
    case 'save_memory':
    case 'forget_memory':
    case 'remember_about_user':
      return 'memory';
    case 'execute_command':
    case 'run_project_command':
      return isVerificationCommand(tool.arguments) ? 'verify' : 'command';
    case 'open_trusted_app':
    case 'open_windows_setting':
    case 'reveal_workspace_item':
    case 'prepare_message_draft':
    case 'set_trusted_app_text':
    case 'operate_trusted_app_control':
      return 'desktop';
    default:
      return 'general';
  }
}

function isVerificationCommand(argumentsValue: unknown): boolean {
  if (!argumentsValue || typeof argumentsValue !== 'object') return false;
  const raw = JSON.stringify(argumentsValue).toLowerCase();
  return /\b(test|check|lint|verify|validate|audit|build)\b/.test(raw);
}

function verbForTool(toolName: string): string {
  switch (toolName) {
    case 'read_file': return '读取文件';
    case 'write_file': return '写入文件';
    case 'edit_file': return '编辑文件';
    case 'delete_file': return '删除文件';
    case 'list_dir': return '浏览目录';
    case 'execute_command': return '执行命令';
    case 'run_project_command': return '运行项目命令';
    case 'search_files': return '搜索文件';
    case 'grep_search': return '搜索文本';
    case 'recall_memories': return '召回记忆';
    case 'save_memory': return '写入记忆';
    case 'forget_memory': return '遗忘记忆';
    case 'remember_about_user': return '记住偏好';
    case 'create_reminder': return '创建提醒';
    case 'list_reminders': return '查看提醒';
    case 'delete_reminder': return '取消提醒';
    case 'web_search': return '联网搜索';
    case 'web_fetch': return '读取网页';
    default: return getToolDisplayName(toolName);
  }
}

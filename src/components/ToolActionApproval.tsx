import { useState } from 'react';
import { fileWriteContentForPreview, getToolConfirmationSummary, mcpArgumentsForPreview, memoryContentForPreview, projectCommandForPreview } from '$lib/tool-confirmation';
import { parseMcpToolIdentity } from '$lib/tool-display';
import { PENDING_ACTIONS_BLOCKED_MESSAGE } from '$lib/tool-action-result';
import '../styles/content-block-renderer.css';

/** Displays the pending payload, not a disk-state or permission attestation.
 * The caller retains the exact message/call identity; execution rechecks scope.
 * Desktop actions keep their separate backend-attested preview.
 */
export function ToolActionApproval({
  toolName, arguments: args, layout, reason, approvalBlocked = false, onResolve,
}: {
  toolName: string;
  arguments: unknown;
  layout: 'compact' | 'stream';
  reason?: string;
  approvalBlocked?: boolean;
  onResolve: (decision: 'approved' | 'rejected') => unknown;
}) {
  const compact = layout === 'compact';
  const [inputOpen, setInputOpen] = useState(false);
  const isMemory = toolName === 'remember_fact';
  const isWrite = toolName === 'write_file';
  const isCommand = toolName === 'run_project_command';
  const content = isMemory ? memoryContentForPreview(args)
    : isWrite ? fileWriteContentForPreview(args)
    : isCommand ? projectCommandForPreview(args) : null;
  const previewLabel = isMemory ? '拟保存的长期记忆' : isCommand ? '拟运行的命令和参数' : '拟写入的正文';
  const mcp = parseMcpToolIdentity(toolName);
  const mcpInput = mcp ? mcpArgumentsForPreview(args) : null;
  const hasPreview = isWrite || isMemory || isCommand;
  const unavailable = (hasPreview && content === null) || (mcp !== null && mcpInput === null);

  return (
    <div className={compact ? 'agent-run-text-confirmation' : 'content-block-tool-confirmation'}>
      <div className={compact ? 'agent-run-confirmation-summary' : 'content-block-tool-confirmation-summary'}>
        <span>{getToolConfirmationSummary(toolName, args)}</span>
        {mcp && (
          <div className="content-block-tool-confirmation-preview">
            <button type="button" className="agent-step-detail-toggle" aria-expanded={inputOpen} onClick={() => setInputOpen((open) => !open)}>
              {inputOpen ? '收起本次输入' : '查看本次输入'}
            </button>
            {inputOpen && <>
              <pre role="region" aria-label="本次外部工具输入" tabIndex={0}>{mcpInput ?? '无法解析本次输入，请拒绝并重新生成。'}</pre>
              <p className="content-block-tool-confirmation-warning">常见凭据字段已隐藏，其他输入可能含私人内容。输入不证明服务安全、执行后果或任务已完成。</p>
            </>}
          </div>
        )}
        {content !== null && (
          <div className="content-block-tool-confirmation-preview">
            <strong>{previewLabel}</strong>
            <pre role="region" aria-label={previewLabel} tabIndex={0}>{content}</pre>
            {!isCommand && <p className="content-block-tool-confirmation-warning">
              {isMemory ? '确认后，相关对话可能引用这条内容。可在记忆设置中暂停召回。' : content === ''
                ? '正文为空，将创建空文件或清空已有文件。'
                : '将写入以上完整正文；同名文件可能被覆盖。'}
            </p>}
          </div>
        )}
        {hasPreview && unavailable && (
          <p className="content-block-tool-confirmation-warning" role="alert">
            无法预览{previewLabel}，请拒绝这次请求并重新生成；不会自动重试。
          </p>
        )}
      </div>
      {approvalBlocked && <p role="status">{PENDING_ACTIONS_BLOCKED_MESSAGE}</p>}
      <div className={compact ? 'agent-run-badge-actions' : 'content-block-tool-actions'}>
        <button
          type="button"
          className={compact ? 'agent-inline-btn agent-inline-btn--approve' : 'content-block-tool-btn content-block-tool-btn--approve'}
          disabled={unavailable || approvalBlocked}
          onClick={() => { if (!approvalBlocked) void onResolve('approved'); }}
          title={reason}
        >
          {compact ? '允许一次' : '允许'}
        </button>
        <button
          type="button"
          className={compact ? 'agent-inline-btn agent-inline-btn--reject' : 'content-block-tool-btn content-block-tool-btn--reject'}
          onClick={() => { void onResolve('rejected'); }}
          title={reason}
        >
          拒绝
        </button>
      </div>
    </div>
  );
}

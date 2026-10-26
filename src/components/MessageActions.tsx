/**
 * MessageActions — per-message action button bar.
 * Renders below each message bubble. Role determines which buttons appear.
 */
"use client";

import { useState, useRef, useEffect } from 'react';
import type { Message } from '$types';
import { saveMessageReply } from '$lib/commands/message';

export interface MessageActionsProps {
  message: Message;
  onEdit?: (messageId: string, newContent: string) => void;
  onDelete?: (messageId: string) => void;
  onCopy?: (messageId: string) => void;
  disabled?: boolean;
  isStreaming?: boolean;
  recoveredEditContent?: string;
}

// ── Icons ───────────────────────────────────────────────────────────────────────

const EditIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <path d="M11 4H4a2 2 0 00-2 2v14a2 2 0 002 2h14a2 2 0 002-2v-7"/>
    <path d="M18.5 2.5a2.121 2.121 0 013 3L12 15l-4 1 1-4 9.5-9.5z"/>
  </svg>
);

const TrashIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <polyline points="3 6 5 6 21 6"/>
    <path d="M19 6l-1 14a2 2 0 01-2 2H8a2 2 0 01-2-2L5 6"/>
    <path d="M10 11v6M14 11v6"/>
    <path d="M9 6V4h6v2"/>
  </svg>
);

const CopyIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <rect x="9" y="9" width="13" height="13" rx="2" ry="2"/>
    <path d="M5 15H4a2 2 0 01-2-2V4a2 2 0 012-2h9a2 2 0 012 2v1"/>
  </svg>
);

const CheckIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
    <polyline points="20 6 9 17 4 12"/>
  </svg>
);

const SaveIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
    <path d="m7 10 5 5 5-5M12 15V3" />
  </svg>
);

// ── ActionButton ────────────────────────────────────────────────────────────────

interface ActionButtonProps {
  onClick: () => void;
  disabled?: boolean;
  title: string;
  children: React.ReactNode;
  variant?: 'default' | 'danger';
  active?: boolean;
  activeLabel?: string;
}

function ActionButton({ onClick, disabled, title, children, variant = 'default', active, activeLabel }: ActionButtonProps) {
  const [pressed, setPressed] = useState(false);

  const isDanger = variant === 'danger';
  const isActive = active || pressed;

  return (
    <button
      type="button"
      disabled={disabled}
      title={title}
      onClick={onClick}
      onMouseDown={() => setPressed(true)}
      onMouseUp={() => setPressed(false)}
      onMouseLeave={(e) => {
        setPressed(false);
        if (disabled) return;
        const el = e.currentTarget;
        if (isDanger) {
          el.style.color = 'var(--color-muted)';
          el.style.background = 'transparent';
        } else {
          el.style.color = 'var(--color-muted)';
          el.style.background = 'transparent';
        }
        el.style.borderColor = 'transparent';
      }}
      onMouseEnter={(e) => {
        if (disabled) return;
        const el = e.currentTarget;
        if (isDanger) {
          el.style.color = '#ef4444';
          el.style.background = 'rgba(239,68,68,0.08)';
        } else {
          el.style.color = 'var(--color-text)';
          el.style.background = 'var(--color-surface)';
        }
        el.style.borderColor = 'var(--color-border)';
      }}
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 4,
        height: 26,
        padding: '4px 8px',
        background: isActive ? 'var(--color-surface)' : 'transparent',
        border: `1px solid ${isActive ? 'var(--color-border)' : 'transparent'}`,
        borderRadius: 6,
        color: isDanger ? 'var(--color-muted)' : isActive ? 'var(--color-text)' : 'var(--color-muted)',
        cursor: disabled ? 'not-allowed' : 'pointer',
        fontSize: 11,
        fontWeight: 500,
        fontFamily: 'inherit',
        lineHeight: 1,
        whiteSpace: 'nowrap',
        userSelect: 'none',
        opacity: disabled ? 0.4 : 1,
        transition: 'background 0.12s, color 0.12s, border-color 0.12s',
        flexShrink: 0,
      }}
    >
      {activeLabel && active ? <><CheckIcon /><span>{activeLabel}</span></> : children}
    </button>
  );
}

// ── InlineEdit ─────────────────────────────────────────────────────────────────

interface InlineEditProps {
  initialContent: string;
  onConfirm: (newContent: string) => void;
  onCancel: () => void;
  disabled?: boolean;
}

function InlineEdit({ initialContent, onConfirm, onCancel, disabled }: InlineEditProps) {
  const [value, setValue] = useState(initialContent);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  const handleConfirm = () => {
    const trimmed = value.trim();
    if (disabled || !trimmed) return;
    onConfirm(trimmed);
  };

  return (
    <div className="message-inline-editor" aria-label="编辑消息">
      <textarea
        ref={textareaRef}
        className="message-inline-editor-input"
        value={value}
        disabled={disabled}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); handleConfirm(); }
          if (e.key === 'Escape') { onCancel(); }
        }}
        autoFocus
        rows={3}
        aria-label="编辑后的消息内容"
      />
      <div className="message-inline-editor-footer">
        <span className="message-inline-editor-hint">{disabled ? '当前操作处理中，暂不能重新发送' : 'Ctrl / ⌘ + Enter 重新发送'}</span>
        <div className="message-inline-editor-actions">
          <button type="button" className="message-inline-editor-cancel" onClick={onCancel}>
            取消
          </button>
          <button
            type="button"
            className="message-inline-editor-submit"
            onClick={handleConfirm}
            disabled={disabled || !value.trim()}
          >
            重新发送
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Main Component ─────────────────────────────────────────────────────────────

export function MessageActions({ message, onEdit, onDelete, onCopy, disabled, isStreaming, recoveredEditContent }: MessageActionsProps) {
  const [editing, setEditing] = useState<{ source: Message; content: string } | null>(null);
  const [copyState, setCopyState] = useState<{
    source: Message; status: 'pending' | 'copied' | 'error';
  } | null>(null);
  const copyAttempt = useRef<symbol | null>(null);
  const copyTimer = useRef<ReturnType<typeof setTimeout> | undefined>();
  const [exportState, setExportState] = useState<{
    source: Message; pending: boolean; notice?: string; error?: string;
  } | null>(null);
  const exportAttempt = useRef<symbol | null>(null);
  useEffect(() => {
    setEditing(recoveredEditContent !== undefined && message.role === 'user'
      ? { source: message, content: recoveredEditContent } : null);
    setCopyState(null);
    setExportState(null);
    return () => {
      exportAttempt.current = null;
      copyAttempt.current = null;
      clearTimeout(copyTimer.current);
    };
  }, [message.id, message.sessionId, message.content, recoveredEditContent]);
  const isCurrent = (source?: Message | null) => source?.id === message.id
    && source.sessionId === message.sessionId && source.content === message.content;
  const activeExport = isCurrent(exportState?.source) ? exportState : null;
  const activeCopy = isCurrent(copyState?.source) ? copyState : null;
  const copied = activeCopy?.status === 'copied';

  const handleSave = async () => {
    if (disabled || isStreaming || exportAttempt.current || message.role !== 'assistant' || !message.content.trim()) return;
    const attempt = Symbol();
    exportAttempt.current = attempt;
    setExportState({ source: message, pending: true });
    try {
      const result = await saveMessageReply(message.sessionId, message.id);
      if (exportAttempt.current !== attempt) return;
      setExportState({ source: message, pending: false,
        notice: result.status === 'saved' ? `已保存回复快照：${result.name}` : undefined });
    } catch (error) {
      if (exportAttempt.current === attempt) setExportState({ source: message, pending: false,
        error: error instanceof Error ? error.message : '未能确认保存结果，请先检查所选位置；原回复未改变。' });
    } finally {
      if (exportAttempt.current === attempt) exportAttempt.current = null;
    }
  };

  const handleCopy = async () => {
    if (disabled || copyAttempt.current || !message.content.trim()) return;
    const attempt = Symbol();
    copyAttempt.current = attempt;
    clearTimeout(copyTimer.current);
    setCopyState({ source: message, status: 'pending' });
    try {
      await navigator.clipboard.writeText(message.content);
    } catch {
      if (copyAttempt.current === attempt) {
        copyAttempt.current = null;
        setCopyState({ source: message, status: 'error' });
      }
      return;
    }
    if (copyAttempt.current !== attempt) return;
    copyAttempt.current = null;
    setCopyState({ source: message, status: 'copied' });
    copyTimer.current = setTimeout(() => setCopyState(null), 1800);
    onCopy?.(message.id);
  };

  const handleEditConfirm = (newContent: string) => {
    if (disabled || !isCurrent(editing?.source)) return;
    onEdit?.(message.id, newContent);
    setEditing(null);
  };

  if (editing && isCurrent(editing.source)) {
    return (
      <InlineEdit
        initialContent={editing.content}
        onConfirm={handleEditConfirm}
        onCancel={() => setEditing(null)}
        disabled={disabled}
      />
    );
  }

  const isUser = message.role === 'user';
  const isAssistant = message.role === 'assistant';

  return (
    <div className="message-actions" style={{ display: 'flex', alignItems: 'center', gap: 4, marginTop: 6, flexWrap: 'wrap' }}>
      {isUser && (
        <>
          <ActionButton onClick={() => setEditing({ source: message, content: message.content })} disabled={disabled} title={disabled ? '请先等待当前操作结束后编辑消息' : '编辑消息'}>
            <EditIcon />
            <span>编辑</span>
          </ActionButton>
          <ActionButton onClick={() => onDelete?.(message.id)} disabled={disabled} title={disabled ? '请先等待当前操作结束后删除消息' : '删除消息'} variant="danger">
            <TrashIcon />
            <span>删除</span>
          </ActionButton>
        </>
      )}
      {isAssistant && (
        <>
          <ActionButton onClick={() => { void handleCopy(); }} disabled={disabled || activeCopy?.status === 'pending' || !message.content.trim()} title="复制回复" active={copied} activeLabel="已复制">
            {copied ? <CheckIcon /> : <CopyIcon />}
            <span>{activeCopy?.status === 'pending' ? '正在复制…' : '复制'}</span>
          </ActionButton>
          {activeCopy?.status === 'error' && <span role="alert"
            style={{ fontSize: 12, overflowWrap: 'anywhere', color: 'var(--color-danger)' }}>
            无法复制到剪贴板，请手动选择正文复制；原回复未改变。
          </span>}
          {!isStreaming && message.content.trim() && <ActionButton onClick={() => { void handleSave(); }}
            disabled={disabled || activeExport?.pending} title="保存为 Markdown 或文本新文件，不覆盖已有文件">
            <SaveIcon /><span>{activeExport?.pending ? '正在保存…' : '保存回复…'}</span>
          </ActionButton>}
          {(activeExport?.notice || activeExport?.error) && <span role={activeExport.error ? 'alert' : 'status'}
            style={{ fontSize: 12, overflowWrap: 'anywhere', color: activeExport.error ? 'var(--color-danger)' : 'var(--color-muted)' }}>
            {activeExport.error ?? activeExport.notice}
          </span>}
        </>
      )}
    </div>
  );
}

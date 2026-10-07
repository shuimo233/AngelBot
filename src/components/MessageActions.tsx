/**
 * MessageActions — per-message action button bar.
 * Renders below each message bubble. Role determines which buttons appear.
 */
"use client";

import { useState, useCallback, useRef } from 'react';
import type { Message } from '$types';

export interface MessageActionsProps {
  message: Message;
  onEdit?: (messageId: string, newContent: string) => void;
  onDelete?: (messageId: string) => void;
  onCopy?: (messageId: string) => void;
  disabled?: boolean;
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
      {activeLabel && isActive ? <CheckIcon /> : children}
      {activeLabel && isActive && <span>{activeLabel}</span>}
    </button>
  );
}

// ── InlineEdit ─────────────────────────────────────────────────────────────────

interface InlineEditProps {
  initialContent: string;
  onConfirm: (newContent: string) => void;
  onCancel: () => void;
}

function InlineEdit({ initialContent, onConfirm, onCancel }: InlineEditProps) {
  const [value, setValue] = useState(initialContent);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  const handleConfirm = () => {
    const trimmed = value.trim();
    if (!trimmed) return;
    onConfirm(trimmed);
  };

  return (
    <div className="message-inline-editor" aria-label="编辑消息">
      <textarea
        ref={textareaRef}
        className="message-inline-editor-input"
        value={value}
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
        <span className="message-inline-editor-hint">Ctrl / ⌘ + Enter 重新发送</span>
        <div className="message-inline-editor-actions">
          <button type="button" className="message-inline-editor-cancel" onClick={onCancel}>
            取消
          </button>
          <button
            type="button"
            className="message-inline-editor-submit"
            onClick={handleConfirm}
            disabled={!value.trim()}
          >
            重新发送
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Main Component ─────────────────────────────────────────────────────────────

export function MessageActions({ message, onEdit, onDelete, onCopy, disabled }: MessageActionsProps) {
  const [editing, setEditing] = useState(false);
  const [copied, setCopied] = useState(false);

  const handleCopy = useCallback(() => {
    navigator.clipboard.writeText(message.content).then(() => {
      setCopied(true);
      onCopy?.(message.id);
      setTimeout(() => setCopied(false), 1800);
    }).catch(() => {
      // clipboard not available
    });
  }, [message.content, message.id, onCopy]);

  const handleEditConfirm = (newContent: string) => {
    onEdit?.(message.id, newContent);
    setEditing(false);
  };

  if (editing) {
    return (
      <InlineEdit
        initialContent={message.content}
        onConfirm={handleEditConfirm}
        onCancel={() => setEditing(false)}
      />
    );
  }

  const isUser = message.role === 'user';
  const isAssistant = message.role === 'assistant';

  return (
    <div className="message-actions" style={{ display: 'flex', alignItems: 'center', gap: 4, marginTop: 6, flexWrap: 'wrap' }}>
      {isUser && (
        <>
          <ActionButton onClick={() => setEditing(true)} disabled={disabled} title="编辑消息">
            <EditIcon />
            <span>编辑</span>
          </ActionButton>
          <ActionButton onClick={() => onDelete?.(message.id)} disabled={disabled} title="删除消息" variant="danger">
            <TrashIcon />
            <span>删除</span>
          </ActionButton>
        </>
      )}
      {isAssistant && (
        <>
          <ActionButton onClick={handleCopy} disabled={disabled} title="复制回复" active={copied} activeLabel="已复制">
            {copied ? <CheckIcon /> : <CopyIcon />}
            <span>{copied ? '已复制' : '复制'}</span>
          </ActionButton>
        </>
      )}
    </div>
  );
}

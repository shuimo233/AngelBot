/**
 * ComposerInput — Three-section input bar inspired by pi-web ChatInput.
 *
 * Layout:
 *   [attachment]  |  [textarea]  |  [Send/Stop]
 *   [运行环境 ▾] [Sound]
 *
 * 运行环境弹层内包含：模型、思考强度、工具预设、权限确认方式。
 *
 * Streaming state:
 *   [attachment]  |  [textarea]  |  [Stop]
 *   [引导] [排队]
 */
"use client";

import { useRef, useState, useCallback, useEffect, forwardRef, useImperativeHandle } from 'react';
import { useThinkingEffortStore, type ThinkingEffort } from '$stores/thinkingEffort';
import { useSettingsStore } from '$stores/settings';
import type { AgentExecutionPermission } from '$lib/commands/settings';
import { useExecutionPermissionStore } from '$stores/executionPermission';
import type { TextAttachment } from '$types';
import { loadTextAttachments, TEXT_ATTACHMENT_ACCEPT, textAttachmentBytes, validateTextAttachments } from '$lib/text-attachments';
import './ComposerInput.css';

// ── Icons ───────────────────────────────────────────────────────────────────────

const AttachIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
    <path d="M21.44 11.05l-9.19 9.19a6 6 0 01-8.49-8.49l9.19-9.19a4 4 0 015.66 5.66l-9.2 9.19a2 2 0 01-2.83-2.83l8.49-8.48"/>
  </svg>
);



const SendIcon = () => (
  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
    <line x1="22" y1="2" x2="11" y2="13"/>
    <polygon points="22 2 15 22 11 13 2 9 22 2"/>
  </svg>
);

const StopIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
    <rect x="3" y="3" width="18" height="18" rx="2"/>
  </svg>
);

const SoundOnIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
    <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/>
    <path d="M19.07 4.93a10 10 0 010 14.14M15.54 8.46a5 5 0 010 7.07"/>
  </svg>
);

const SoundOffIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
    <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/>
    <line x1="23" y1="9" x2="17" y2="15"/>
    <line x1="17" y1="9" x2="23" y2="15"/>
  </svg>
);

// ── Types ───────────────────────────────────────────────────────────────────────

export type ToolPreset = 'none' | 'default' | 'full';

export interface ComposerInputHandle {
  insertText: (text: string) => void;
  insertIfEmpty: (text: string) => void;
  prependText: (text: string) => void;
  addFiles: (files: File[]) => void;
  restoreFiles: (files: TextAttachment[]) => boolean;
}

export interface ComposerInputProps {
  /** Text input value */
  value: string;
  onChange: (value: string) => void;
  onSend: (message: string, files?: TextAttachment[]) => boolean | void | Promise<boolean | void>;
  onAbort: () => void;
  onSteer?: (message: string) => void;
  onFollowUp?: (message: string) => void;
  /** Attachments never cross a session or workspace switch except explicit routing. */
  scopeKey?: string;
  /** Agent is currently running */
  isStreaming: boolean;
  /** Tool preset (none / default / full) */
  toolPreset: ToolPreset;
  onToolPresetChange: (preset: ToolPreset) => void;
  soundEnabled: boolean;
  onSoundToggle: () => void;
  disabled?: boolean;
}

// ── Tool Preset config ──────────────────────────────────────────────────────────

const TOOL_PRESET_OPTIONS: Array<{ value: ToolPreset; label: string; desc: string }> = [
  { value: 'none',    label: '关闭工具', desc: '只读对话，不调用工具' },
  { value: 'default', label: '默认工具', desc: '内置工具' },
  { value: 'full',   label: '全部工具', desc: '所有内置工具' },
];


const PERMISSION_OPTIONS: Array<{ value: AgentExecutionPermission; label: string; desc: string }> = [
  { value: 'ask', label: '请求批准', desc: '按工具规则逐次确认需要批准的操作' },
  { value: 'workspace_auto', label: '工作区自动', desc: '新建或覆盖项目文件、已启用的 MCP 工具可自动执行；其他文件操作仍按规则确认' },
  { value: 'full_access', label: '完全访问', desc: '所有工作区中已准入的文件、桌面、MCP 与外部服务工具可自动执行；保护操作仍会询问' },
];

// ── Thinking Effort config ──────────────────────────────────────────────────────

const THINKING_OPTIONS: Array<{ value: ThinkingEffort; label: string; desc: string }> = [
  { value: 'low',    label: '低', desc: '快速回复，适合简单任务' },
  { value: 'medium', label: '中', desc: '速度与质量平衡' },
  { value: 'high',   label: '高', desc: '深度推理，适合复杂任务' },
];

// ── Sub-components ──────────────────────────────────────────────────────────────

function PButton({
  children,
  onClick,
  disabled,
  title,
  className,
  style,
}: {
  children: React.ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  title?: string;
  className?: string;
  style?: React.CSSProperties;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      title={title}
      className={className}
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        gap: 5,
        padding: '7px 12px',
        height: 36,
        background: 'none',
        border: '1px solid var(--color-border-muted)',
        borderRadius: 'var(--radius-control)',
        color: 'var(--color-muted)',
        cursor: disabled ? 'not-allowed' : 'pointer',
        fontSize: 12,
        fontWeight: 500,
        letterSpacing: '-0.01em',
        opacity: disabled ? 0.45 : 1,
        transition: 'background-color 120ms ease, color 120ms ease, border-color 120ms ease',
        whiteSpace: 'nowrap',
        userSelect: 'none',
        flexShrink: 0,
        ...style,
      }}
      onMouseEnter={(e) => {
        if (disabled) return;
        const el = e.currentTarget;
        el.style.background = 'var(--color-surface)';
        el.style.color = 'var(--color-text)';
        el.style.borderColor = 'var(--color-border)';
      }}
      onMouseLeave={(e) => {
        if (disabled) return;
        const el = e.currentTarget;
        el.style.background = 'none';
        el.style.color = 'var(--color-muted)';
        el.style.borderColor = 'var(--color-border-muted)';
      }}
    >
      {children}
    </button>
  );
}

function TooltipPButton({
  children,
  onClick,
  disabled,
  title,
  active,
  activeStyle,
}: {
  children: React.ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  title?: string;
  active?: boolean;
  activeStyle?: React.CSSProperties;
}) {
  return (
    <PButton
      onClick={onClick}
      disabled={disabled}
      title={title}
      style={active ? activeStyle : undefined}
      className={active ? 'p-btn-active' : undefined}
    >
      {children}
    </PButton>
  );
}


/** 运行环境弹层里的选项分组：标题 + 单选选项列表 */


interface EnvDropdownOption<T extends string> {
  value: T;
  label: string;
  desc?: string;
}

function EnvironmentChoiceGroup<T extends string>({
  label,
  value,
  options,
  onSelect,
  disabled,
}: {
  label: string;
  value: T;
  options: EnvDropdownOption<T>[];
  onSelect: (value: T) => void;
  disabled?: boolean;
}) {
  return (
    <div className="environment-panel-group">
      <div className="environment-panel-label">{label}</div>
      <div className="environment-panel-choices">
        {options.map((option) => (
          <button
            key={option.value}
            type="button"
            className="environment-panel-choice"
            aria-pressed={option.value === value}
            disabled={disabled}
            title={option.desc}
            onClick={() => onSelect(option.value)}
          >
            {option.label}
          </button>
        ))}
      </div>
      {options.find((option) => option.value === value)?.desc && (
        <div className="environment-panel-description">
          {options.find((option) => option.value === value)?.desc}
        </div>
      )}
    </div>
  );
}

function EnvDropdown<T extends string>({
  label,
  value,
  options,
  onSelect,
  disabled,
  renderValue,
  loadingLabel,
}: {
  label: string;
  value: T;
  options: EnvDropdownOption<T>[];
  onSelect: (value: T) => void;
  disabled?: boolean;
  renderValue: (v: T) => string;
  loadingLabel?: string;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const activeOption = options.find((option) => option.value === value);

  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('click', handler);
    return () => document.removeEventListener('click', handler);
  }, [open]);

  return (
    <div ref={ref} style={{ position: 'relative' }}>
      <button
        type="button"
        onClick={() => !disabled && setOpen((v) => !v)}
        disabled={disabled}
        className="env-dropdown-btn"
        title={loadingLabel ?? activeOption?.desc}
      >
        <span className="env-dropdown-btn-label">{label}</span>
        <span className="env-dropdown-btn-value">{loadingLabel ?? renderValue(value)}</span>
      </button>
      {open && (
        <div className="env-dropdown-panel">
          {options.map((opt) => {
            const isActive = opt.value === value;
            return (
              <button
                key={opt.value}
                type="button"
                onClick={() => { onSelect(opt.value); setOpen(false); }}
                className={`env-dropdown-item ${isActive ? 'active' : ''}`}
              >
                <span className="env-dropdown-item-label">{opt.label}</span>
                {opt.desc && <span className="env-dropdown-item-desc">{opt.desc}</span>}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}


// ── Main Component ─────────────────────────────────────────────────────────────

export const ComposerInput = forwardRef<ComposerInputHandle, ComposerInputProps>(
  function ComposerInput({
    value,
    onChange,
    onSend,
    onAbort,
    onSteer,
    onFollowUp,
    isStreaming,
    toolPreset,
    onToolPresetChange,
    soundEnabled,
    onSoundToggle,
    disabled,
    scopeKey = '',
  }, ref) {

    const { effort, setEffort } = useThinkingEffortStore();
    const apiConfig = useSettingsStore((state) => state.activeApiConfig);

    const textareaRef = useRef<HTMLTextAreaElement>(null);
    const fileInputRef = useRef<HTMLInputElement>(null);
    const scopeRef = useRef({ key: scopeKey, generation: 0 });
    const filesRef = useRef<TextAttachment[]>([]);
    const readsRef = useRef(0);
    const sendingRef = useRef(false);
    const mountedRef = useRef(true);
    const valueRef = useRef(value);
    const valueRevisionRef = useRef(0);
    if (valueRef.current !== value) valueRevisionRef.current++;
    valueRef.current = value;
    if (scopeRef.current.key !== scopeKey) {
      scopeRef.current = { key: scopeKey, generation: scopeRef.current.generation + 1 };
      filesRef.current = [];
      readsRef.current = 0;
      sendingRef.current = false;
    }
    const generation = scopeRef.current.generation;
    const [fileDraft, setFileDraft] = useState({ generation, files: [] as TextAttachment[] });
    const [fileStatus, setFileStatus] = useState({ generation, reading: false, sending: false, error: '' });
    const attachedFiles = fileDraft.generation === generation ? fileDraft.files : [];
    const status = fileStatus.generation === generation ? fileStatus : { reading: false, sending: false, error: '' };
    const executionPermission = useExecutionPermissionStore((state) => state.permission);
    const permissionLoading = useExecutionPermissionStore((state) => state.loading);
    const permissionSaving = useExecutionPermissionStore((state) => state.saving);
    const loadExecutionPermission = useExecutionPermissionStore((state) => state.load);
    const changeExecutionPermission = useExecutionPermissionStore((state) => state.save);
    const permissionLoaded = executionPermission !== null;
    const [environmentOpen, setEnvironmentOpen] = useState(false);
    const environmentRef = useRef<HTMLDivElement>(null);

    useEffect(() => {
      if (!environmentOpen) return;
      const closeOnOutsideClick = (event: MouseEvent) => {
        if (environmentRef.current && !environmentRef.current.contains(event.target as Node)) {
          setEnvironmentOpen(false);
        }
      };
      const closeOnEscape = (event: KeyboardEvent) => {
        if (event.key === 'Escape') setEnvironmentOpen(false);
      };
      document.addEventListener('click', closeOnOutsideClick);
      document.addEventListener('keydown', closeOnEscape);
      return () => {
        document.removeEventListener('click', closeOnOutsideClick);
        document.removeEventListener('keydown', closeOnEscape);
      };
    }, [environmentOpen]);

    useEffect(() => {
      if (import.meta.env.MODE === 'test') return;
      let cancelled = false;
      let retryTimer: ReturnType<typeof setTimeout> | undefined;

      const loadWithRetry = async (attempt: number) => {
        const loaded = await loadExecutionPermission();
        if (!loaded && !cancelled && attempt < 4) {
          retryTimer = setTimeout(() => void loadWithRetry(attempt + 1), 250 * (attempt + 1));
        }
      };

      void loadWithRetry(0);
      return () => {
        cancelled = true;
        if (retryTimer) clearTimeout(retryTimer);
      };
    }, [loadExecutionPermission]);

    // One bounded loader serves file selection, drop and paste. Concurrent
    // reads merge against the current draft and must not outlive their scope.
    useEffect(() => {
      mountedRef.current = true;
      return () => { mountedRef.current = false; scopeRef.current.generation++; };
    }, []);
    const processFiles = useCallback(async (files: File[]) => {
      if (!files.length || disabled) return;
      const readGeneration = scopeRef.current.generation;
      const current = () => mountedRef.current && scopeRef.current.generation === readGeneration;
      readsRef.current++;
      setFileStatus({ generation: readGeneration, reading: true, sending: sendingRef.current, error: '' });
      try {
        const loaded = await loadTextAttachments(files, filesRef.current);
        if (!current()) return;
        const combined = [...filesRef.current, ...loaded];
        validateTextAttachments(combined);
        filesRef.current = combined;
        setFileDraft({ generation: readGeneration, files: combined });
      } catch (error) {
        if (current()) setFileStatus({ generation: readGeneration, reading: readsRef.current > 1,
          sending: sendingRef.current, error: error instanceof Error ? error.message : '无法读取文件，请重新选择。' });
      } finally {
        if (current()) {
          readsRef.current--;
          setFileStatus((previous) => ({ ...previous, generation: readGeneration,
            reading: readsRef.current > 0, sending: sendingRef.current }));
        }
      }
    }, [disabled]);

    const restoreFiles = useCallback((files: TextAttachment[]) => {
      const combined = [...filesRef.current, ...files.filter((file) => !filesRef.current.includes(file))];
      try { validateTextAttachments(combined); }
      catch {
        setFileStatus({ generation: scopeRef.current.generation, reading: readsRef.current > 0,
          sending: sendingRef.current, error: '未发送附件无法全部恢复：当前草稿已达到附件限制。现有草稿未被覆盖，请先处理当前附件，再重新选择未发送的文件。' });
        return false;
      }
      filesRef.current = combined;
      setFileDraft({ generation: scopeRef.current.generation, files: filesRef.current });
      return true;
    }, []);

    const removeFile = (file: TextAttachment) => {
      filesRef.current = filesRef.current.filter((candidate) => candidate !== file);
      setFileDraft({ generation, files: filesRef.current });
    };

    // ── Imperative handle ───────────────────────────────────────────────────

    useImperativeHandle(ref, () => ({
      insertIfEmpty(text: string) {
        if (value.trim()) return;
        onChange(text);
        requestAnimationFrame(() => textareaRef.current?.focus());
      },
      insertText(text: string) {
        const ta = textareaRef.current;
        if (!ta) { onChange(value + (value ? ' ' : '') + text); return; }
        const start = ta.selectionStart ?? value.length;
        const end = ta.selectionEnd ?? value.length;
        const before = value.slice(0, start);
        const after = value.slice(end);
        const sep = before.length > 0 && !before.endsWith(' ') ? ' ' : '';
        onChange(before + sep + text + after);
        requestAnimationFrame(() => {
          const pos = start + sep.length + text.length;
          ta.setSelectionRange(pos, pos);
          ta.focus();
        });
      },
      prependText(text: string) {
        if (!text.trim()) return;
        const combined = [text, value].filter((t) => t.trim()).join('\n\n');
        onChange(combined);
        requestAnimationFrame(() => textareaRef.current?.focus());
      },
      addFiles(files: File[]) { void processFiles(files); },
      restoreFiles,
    }), [value, onChange, processFiles, restoreFiles]);

    // ── Send ────────────────────────────────────────────────────────────────

    const canSend = Boolean(value.trim() || attachedFiles.length > 0) && !isStreaming && !disabled && !status.reading && !status.sending;
    const canQueue = Boolean(value.trim()) && attachedFiles.length === 0 && !status.reading && !disabled;

    const handleSend = useCallback(async () => {
      const msg = value.trim();
      const submittedFiles = filesRef.current;
      if ((!msg && submittedFiles.length === 0) || isStreaming || disabled || readsRef.current || sendingRef.current) return;
      const sendGeneration = scopeRef.current.generation;
      const submittedRevision = valueRevisionRef.current;
      sendingRef.current = true;
      // Retain the submitted snapshots in this closure for failed admission,
      // but do not treat them as the next draft or block text-only steering.
      filesRef.current = filesRef.current.filter((file) => !submittedFiles.includes(file));
      setFileDraft({ generation: sendGeneration, files: filesRef.current });
      setFileStatus({ generation: sendGeneration, reading: false, sending: true, error: '' });
      try {
        const accepted = await onSend(msg, submittedFiles.length ? submittedFiles : undefined);
        if (!mountedRef.current || scopeRef.current.generation !== sendGeneration) return;
        if (accepted === false) {
          const restored = restoreFiles(submittedFiles);
          if (restored) setFileStatus({ generation: sendGeneration, reading: readsRef.current > 0, sending: false,
            error: '消息未确认送达，附件仍保留。请检查会话记录后再重试。' });
        } else {
          if (valueRevisionRef.current === submittedRevision && valueRef.current.trim() === msg) onChange('');
        }
      } catch {
        if (mountedRef.current && scopeRef.current.generation === sendGeneration) {
          const restored = restoreFiles(submittedFiles);
          if (restored) setFileStatus({ generation: sendGeneration, reading: readsRef.current > 0, sending: false,
            error: '发送失败，文本和附件仍保留。请检查会话记录后再重试。' });
        }
      } finally {
        if (mountedRef.current && scopeRef.current.generation === sendGeneration) {
          sendingRef.current = false;
          setFileStatus((previous) => ({ ...previous, sending: false }));
        }
      }
      requestAnimationFrame(() => {
        if (textareaRef.current) {
          textareaRef.current.style.height = 'auto';
          textareaRef.current.style.height = `${Math.min(textareaRef.current.scrollHeight, 220)}px`;
        }
      });
    }, [value, isStreaming, disabled, onSend, onChange, restoreFiles]);

    const handleKeyDown = useCallback((e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        if (isStreaming && (onSteer || onFollowUp)) {
          if (onSteer && canQueue) {
            const msg = value.trim();
            if (msg) { onSteer(msg); onChange(''); }
          }
        } else {
          void handleSend();
        }
      }
    }, [isStreaming, onSteer, onFollowUp, value, onChange, handleSend, canQueue]);

    const handleInput = useCallback(() => {
      const ta = textareaRef.current;
      if (!ta) return;
      ta.style.height = 'auto';
      ta.style.height = `${Math.min(ta.scrollHeight, 220)}px`;
    }, []);

    const handlePaste = useCallback((e: React.ClipboardEvent) => {
      const items = Array.from(e.clipboardData?.items ?? []);
      const fileItems = items.filter((item) => item.kind === 'file');
      if (!fileItems.length) return;
      e.preventDefault();
      const files = fileItems.map((item) => item.getAsFile()).filter((f): f is File => f !== null);
      void processFiles(files);
    }, [processFiles]);

    // Auto-resize textarea on external value changes
    useEffect(() => {
      const ta = textareaRef.current;
      if (!ta) return;
      ta.style.height = 'auto';
      ta.style.height = `${Math.min(ta.scrollHeight, 220)}px`;
    }, [value]);

    // ── Render ──────────────────────────────────────────────────────────────

    return (
      <div className="composer-input-root"
        onDragOver={(event) => { if (event.dataTransfer.types.includes('Files')) event.preventDefault(); }}
        onDrop={(event) => {
          if (!event.dataTransfer.files.length) return;
          event.preventDefault();
          void processFiles(Array.from(event.dataTransfer.files));
        }}>
        {/* Hidden file input */}
        <input
          ref={fileInputRef}
          type="file"
          accept={TEXT_ATTACHMENT_ACCEPT}
          aria-label="选择文本附件"
          multiple
          style={{ display: 'none' }}
          onChange={(e) => {
            const files = Array.from(e.target.files ?? []);
            void processFiles(files);
            e.target.value = '';
          }}
        />

        {attachedFiles.length > 0 && (
          <div className="composer-files-strip" aria-label="待发送文本附件">
            {attachedFiles.map((file, index) => (
              <div key={index} className="composer-file-chip">
                <span title={file.name}>{file.name}</span>
                <small>{textAttachmentBytes(file.text)} B</small>
                <button type="button" onClick={() => removeFile(file)} aria-label={`移除附件 ${file.name}`} title="移除附件">×</button>
              </div>
            ))}
          </div>
        )}
        {(attachedFiles.length > 0 || status.reading) && <div className="composer-attachment-note">
          {status.reading ? '正在读取文本文件… ' : ''}内容将发送给当前配置的模型；附件是只读快照，不授予原文件访问或修改权限。
        </div>}
        {isStreaming && (attachedFiles.length > 0 || status.reading) && <div className="composer-attachment-note">附件已暂存，请等待当前任务结束后发送；引导和排队暂不支持附件。</div>}
        {status.error && <div role="alert" className="composer-attachment-error">{status.error}</div>}

        {/* Main input row */}
        <div className="composer-bar">

          {/* ── LEFT: attachment only; model selection lives in the toolbar ── */}
          <div className="composer-bar-left">
            {/* Explicit UTF-8 text snapshots, not a general file permission. */}
            <TooltipPButton
              onClick={() => fileInputRef.current?.click()}
              disabled={disabled}
              title="添加文本文件（每个 16 KiB，合计 32 KiB，最多 4 个）"
            >
              <AttachIcon />
            </TooltipPButton>

          </div>

          {/* ── CENTER: textarea ── */}
          <textarea
            ref={textareaRef}
            value={value}
            onChange={(e) => onChange(e.target.value)}
            onKeyDown={handleKeyDown}
            onInput={handleInput}
            onPaste={handlePaste}
            placeholder={
              isStreaming && (onSteer || onFollowUp)
                ? '输入引导，将在当前步骤结束后交给 AngelBot…'
                : isStreaming
                ? 'AngelBot 正在运行…'
                : '给 AngelBot 发消息…'
            }
            rows={1}
            disabled={disabled}
            style={{
              flex: 1,
              background: 'none',
              border: 'none',
              outline: 'none',
              resize: 'none',
              color: 'var(--color-text)',
              fontSize: 15,
              lineHeight: 1.65,
              fontFamily: 'inherit',
              minHeight: 24,
              maxHeight: 220,
              overflow: 'auto',
              padding: '4px 0',
            }}
          />

          {/* ── RIGHT: send / stop ── */}
          <div className="composer-bar-right">
            {isStreaming ? (
              <TooltipPButton
                onClick={onAbort}
                title="停止运行"
                active
                activeStyle={{
                  background: 'color-mix(in srgb, var(--color-danger) 10%, var(--color-surface))',
                  border: '1px solid color-mix(in srgb, var(--color-danger) 34%, var(--color-border))',
                  color: 'var(--color-danger)',
                }}
              >
                <StopIcon />
              </TooltipPButton>
            ) : (
              <button
                type="button"
                disabled={!canSend}
                onClick={handleSend}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                  width: 36,
                  height: 32,
                  padding: 0,
                  border: '1px solid var(--color-border-muted)',
                  borderRadius: 'var(--radius-control)',
                  background: canSend ? 'var(--color-accent)' : 'var(--color-surface)',
                  color: canSend ? '#fff' : 'var(--color-muted)',
                  cursor: canSend ? 'pointer' : 'not-allowed',
                  fontSize: 13,
                  opacity: canSend ? 1 : 0.45,
                  transition: 'background 0.12s, color 0.12s, border-color 0.12s',
                  flexShrink: 0,
                }}
                onMouseEnter={(e) => {
                  if (!canSend) return;
                  e.currentTarget.style.background = 'var(--color-accent-hover)';
                }}
                onMouseLeave={(e) => {
                  if (!canSend) return;
                  e.currentTarget.style.background = 'var(--color-accent)';
                }}
                title="发送"
              >
                <SendIcon />
              </button>
            )}
          </div>
        </div>

        {/* ── TOOLBAR ROW: environment options split into separate sections below input ── */}
        <div className="composer-toolbar-row">
          {isStreaming ? (
            <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
              {onSteer && (
                <TooltipPButton
                  onClick={() => {
                    const msg = value.trim();
                    if (msg) { onSteer(msg); onChange(''); }
                  }}
                  disabled={!canQueue}
                  title={attachedFiles.length ? '附件暂存，任务结束后再发送' : '立即注入消息'}
                  active
                  activeStyle={{
                    background: 'var(--color-warning-surface)',
                    border: '1px solid var(--color-warning-border)',
                    color: 'var(--color-warning)',
                  }}
                >
                  <span style={{ fontWeight: 600, fontSize: 12 }}>引导</span>
                </TooltipPButton>
              )}
              {onFollowUp && (
                <TooltipPButton
                  onClick={() => {
                    const msg = value.trim();
                    if (msg) { onFollowUp(msg); onChange(''); }
                  }}
                  disabled={!canQueue}
                  title={attachedFiles.length ? '附件暂存，任务结束后再发送' : '排队此消息'}
                  active
                  activeStyle={{
                    background: 'color-mix(in srgb, var(--color-cool-accent) 10%, var(--color-surface))',
                    border: '1px solid color-mix(in srgb, var(--color-cool-accent) 34%, var(--color-border))',
                    color: 'var(--color-cool-accent)',
                  }}
                >
                  <span style={{ fontWeight: 600, fontSize: 12 }}>排队</span>
                </TooltipPButton>
              )}
            </div>
          ) : (
            <div className="env-options-row">
              <div className="environment-disclosure" ref={environmentRef}>
                <button
                  type="button"
                  className="env-dropdown-btn environment-disclosure-trigger"
                  aria-expanded={environmentOpen}
                  aria-controls="composer-environment-panel"
                  onClick={() => setEnvironmentOpen((open) => !open)}
                >
                  <span className="env-dropdown-btn-label">运行配置</span>
                  <span className="env-dropdown-btn-value">
                    {THINKING_OPTIONS.find((option) => option.value === effort)?.label ?? effort}
                    {' · '}
                    {TOOL_PRESET_OPTIONS.find((option) => option.value === toolPreset)?.label ?? toolPreset}
                  </span>
                </button>

                {environmentOpen && (
                  <div id="composer-environment-panel" className="environment-panel">
                    <button
                      type="button"
                      className="environment-panel-model"
                      onClick={() => {
                        setEnvironmentOpen(false);
                        window.dispatchEvent(new CustomEvent('open-settings', { detail: 'api' }));
                      }}
                    >
                      <span>
                        <span className="environment-panel-label">模型</span>
                        <strong>{apiConfig.model || '尚未配置'}</strong>
                      </span>
                      <span className="environment-panel-link">更改</span>
                    </button>
                    <EnvironmentChoiceGroup<ThinkingEffort>
                      label="思考强度"
                      value={effort}
                      options={THINKING_OPTIONS}
                      onSelect={setEffort}
                      disabled={disabled}
                    />
                    <EnvironmentChoiceGroup<ToolPreset>
                      label="可用工具"
                      value={toolPreset}
                      options={TOOL_PRESET_OPTIONS}
                      onSelect={onToolPresetChange}
                      disabled={disabled}
                    />
                  </div>
                )}
              </div>

              {/* 权限 - 下拉选择 */}
              <EnvDropdown<AgentExecutionPermission>
                label="执行权限"
                value={executionPermission ?? 'ask'}
                options={PERMISSION_OPTIONS}
                onSelect={(v) => void changeExecutionPermission(v)}
                disabled={disabled || permissionLoading || !permissionLoaded || permissionSaving}
                loadingLabel={!permissionLoaded ? (permissionLoading ? '读取中…' : '不可用，请到设置中重试') : undefined}
                renderValue={(v) => PERMISSION_OPTIONS.find((o) => o.value === v)?.label ?? v}
              />

              {/* Sound toggle */}
              <TooltipPButton
                onClick={onSoundToggle}
                title={soundEnabled ? '关闭完成音效' : '开启完成音效'}
                active={soundEnabled}
                activeStyle={{ color: 'var(--color-text)' }}
              >
                {soundEnabled ? <SoundOnIcon /> : <SoundOffIcon />}
              </TooltipPButton>
            </div>
          )}
        </div>

      </div>
    );
  }
);

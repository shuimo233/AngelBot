/**
 * ComposerInput — Three-section input bar inspired by pi-web ChatInput.
 *
 * Layout:
 *   [attachment]  |  [textarea]  |  [Send]
 *   [对话选项 ▾] [Sound]
 *
 * 运行环境弹层内包含模型、思考强度和权限确认方式。
 *
 * Streaming state:
 *   [status]                       [Pause] [Stop]
 *   [attachment]                  [排队 / 引导] [Send]
 */
"use client";

import { useRef, useState, useCallback, useEffect, forwardRef, useImperativeHandle, type ReactNode } from 'react';
import { useThinkingEffortStore, type ThinkingEffort } from '$stores/thinkingEffort';
import { useSettingsStore } from '$stores/settings';
import { useAppearanceStore } from '$stores/appearance';
import { usePreferencesStore } from '$stores/preferences';
import { getModelConfigReadiness, openModelSettings } from '$lib/model-config';
import type { AgentExecutionPermission } from '$lib/commands/settings';
import { EXECUTION_PERMISSION_OPTIONS } from '$lib/execution-permission-copy';
import { useExecutionPermissionStore } from '$stores/executionPermission';
import type { TextAttachment } from '$types';
import { loadTextAttachments, TEXT_ATTACHMENT_ACCEPT, validateTextAttachments } from '$lib/text-attachments';
import { TextAttachmentDetails } from '../TextAttachmentDetails';
import './ComposerInput.css';

// ── Icons ───────────────────────────────────────────────────────────────────────

const AttachIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
    <path d="M21.44 11.05l-9.19 9.19a6 6 0 01-8.49-8.49l9.19-9.19a4 4 0 015.66 5.66l-9.2 9.19a2 2 0 01-2.83-2.83l8.49-8.48"/>
  </svg>
);



const SendIcon = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d="M12 19V5m-6 6 6-6 6 6" />
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


export interface ComposerInputHandle {
  focus: () => void;
  insertText: (text: string) => void;
  insertIfEmpty: (text: string) => void;
  prependText: (text: string) => void;
  addFiles: (files: File[]) => void;
  restoreFiles: (files: TextAttachment[]) => boolean;
}

/** Message admission only; acceptance does not imply task completion. */
export type ComposerSendResult = 'accepted' | 'not_sent' | 'unknown';

export interface ComposerInputProps {
  /** Text input value */
  value: string;
  onChange: (value: string) => void;
  onSend: (message: string, files?: TextAttachment[]) => ComposerSendResult | Promise<ComposerSendResult>;
  runControls?: ReactNode;
  stopPending?: boolean;
  onSteer?: (message: string) => void | Promise<void>;
  onFollowUp?: (message: string) => void | Promise<void>;
  /** Attachments never cross a session or workspace switch except explicit routing. */
  scopeKey?: string;
  /** Agent is currently running */
  isStreaming: boolean;
  soundEnabled: boolean;
  onSoundToggle: () => void;
  disabled?: boolean;
  /** Keep drafting available while history is being checked; block admission only. */
  sendDisabled?: boolean;
}

// ── Thinking Effort config ──────────────────────────────────────────────────────

const THINKING_OPTIONS: readonly { value: ThinkingEffort; label: string }[] = [
  { value: 'low', label: '快速' },
  { value: 'medium', label: '平衡' },
  { value: 'high', label: '深入' },
];

// ── Sub-components ──────────────────────────────────────────────────────────────

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
  describeOptions = false,
}: {
  label: string;
  value: T | null;
  options: readonly EnvDropdownOption<T>[];
  onSelect: (value: T) => void;
  disabled?: boolean;
  describeOptions?: boolean;
}) {
  return (
    <div className="environment-panel-group" role="group" aria-label={label}>
      <div className="environment-panel-label">{label}</div>
      <div className={`environment-panel-choices${describeOptions ? ' environment-panel-choices--described' : ''}`}>
        {options.map((option) => (
          <button
            key={option.value}
            type="button"
            className="environment-panel-choice"
            aria-pressed={option.value === value}
            aria-label={describeOptions ? option.label : undefined}
            aria-description={describeOptions ? option.desc : undefined}
            disabled={disabled}
            onClick={() => onSelect(option.value)}
          >
            <span>{option.label}</span>
            {describeOptions && option.desc && <small>{option.desc}</small>}
          </button>
        ))}
      </div>
    </div>
  );
}


// ── Main Component ─────────────────────────────────────────────────────────────

export const ComposerInput = forwardRef<ComposerInputHandle, ComposerInputProps>(
  function ComposerInput({
    value,
    onChange,
    onSend,
    runControls,
    stopPending = false,
    onSteer,
    onFollowUp,
    isStreaming,
    soundEnabled,
    onSoundToggle,
    disabled,
    sendDisabled = false,
    scopeKey = '',
  }, ref) {

    const { effort, setEffort } = useThinkingEffortStore();
    const apiConfig = useSettingsStore((state) => state.activeApiConfig);
    const chatFontSize = useAppearanceStore((state) => state.chatFontSize);
    const chatFontFamily = useAppearanceStore((state) => state.chatFontFamily);
    const attachmentReaderEnabled = usePreferencesStore((state) => state.preferences.habits.useLongTermMemory ?? true);

    const textareaRef = useRef<HTMLTextAreaElement>(null);
    const fileInputRef = useRef<HTMLInputElement>(null);
    const scopeRef = useRef({ key: scopeKey, generation: 0 });
    const filesRef = useRef<TextAttachment[]>([]);
    const readsRef = useRef(0);
    const sendingRef = useRef(false);
    const runSubmission = useRef<object | null>(null);
    const [preferredRunMode, setPreferredRunMode] = useState<'queue' | 'steer'>('queue');
    const [runFeedback, setRunFeedback] = useState({ generation: 0, pending: false, error: '' });
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
      runSubmission.current = null;
    }
    const generation = scopeRef.current.generation;
    const [fileDraft, setFileDraft] = useState({ generation, files: [] as TextAttachment[] });
    const [fileStatus, setFileStatus] = useState({ generation, reading: false, sending: false, error: '' });
    const attachedFiles = fileDraft.generation === generation ? fileDraft.files : [];
    const hasReferenceAttachments = attachedFiles.some((file) => file.referenceOnly);
    const hasInlineAttachments = attachedFiles.some((file) => !file.referenceOnly);
    const status = fileStatus.generation === generation ? fileStatus : { reading: false, sending: false, error: '' };
    const executionPermission = useExecutionPermissionStore((state) => state.permission);
    const permissionLoading = useExecutionPermissionStore((state) => state.loading);
    const permissionSaving = useExecutionPermissionStore((state) => state.saving);
    const permissionError = useExecutionPermissionStore((state) => state.error);
    const loadExecutionPermission = useExecutionPermissionStore((state) => state.load);
    const changeExecutionPermission = useExecutionPermissionStore((state) => state.save);
    const permissionLoaded = executionPermission !== null;
    const permissionLabel = executionPermission !== 'ask'
      ? EXECUTION_PERMISSION_OPTIONS.find((option) => option.value === executionPermission)?.label
      : undefined;
    const [environmentOpen, setEnvironmentOpen] = useState(false);
    const environmentRef = useRef<HTMLDivElement>(null);
    const environmentTriggerRef = useRef<HTMLButtonElement>(null);

    useEffect(() => {
      setEnvironmentOpen(false);
    }, [scopeKey, isStreaming]);

    useEffect(() => {
      if (!environmentOpen) return;
      const closeOnOutsideClick = (event: MouseEvent) => {
        if (environmentRef.current && !environmentRef.current.contains(event.target as Node)) {
          setEnvironmentOpen(false);
        }
      };
      document.addEventListener('click', closeOnOutsideClick);
      return () => document.removeEventListener('click', closeOnOutsideClick);
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
          sending: sendingRef.current, error: `${error instanceof Error ? error.message : '无法读取文件，请重新选择。'} 本次选择未添加任何附件，已有附件和输入文字未改变。` });
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

    const setAttachmentReferenceOnly = (file: TextAttachment, referenceOnly: boolean) => {
      if (referenceOnly && !attachmentReaderEnabled) return;
      filesRef.current = filesRef.current.map((candidate) => {
        if (candidate !== file) return candidate;
        const next = { ...candidate };
        if (referenceOnly) next.referenceOnly = true;
        else delete next.referenceOnly;
        return next;
      });
      setFileDraft({ generation, files: filesRef.current });
    };

    // ── Imperative handle ───────────────────────────────────────────────────

    useImperativeHandle(ref, () => ({
      focus() { textareaRef.current?.focus({ preventScroll: true }); },
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

    const runPending = runFeedback.generation === generation && runFeedback.pending;
    const canSend = Boolean(value.trim() || attachedFiles.length > 0) && !isStreaming && !disabled && !sendDisabled && !status.reading && !status.sending && !runPending;
    const hasRunMessageActions = Boolean(onSteer || onFollowUp);
    const runMode = onSteer && onFollowUp ? preferredRunMode : onFollowUp ? 'queue' : 'steer';
    const runActionLabel = runMode === 'queue' ? '加入队列' : '引导本轮';
    const runPlaceholder = runMode === 'queue' ? '排到下一轮…' : '补充本轮指令…';
    const canSubmitRunText = Boolean(value.trim()) && attachedFiles.length === 0 && !status.reading && !disabled && !sendDisabled && !runPending;

    const handleSend = useCallback(async () => {
      const msg = value.trim();
      const submittedFiles = filesRef.current;
      if ((!msg && submittedFiles.length === 0) || isStreaming || disabled || sendDisabled || readsRef.current || sendingRef.current || runSubmission.current) return;
      const { activeApiConfig, apiConfigLoaded, apiConfigLoadError } = useSettingsStore.getState();
      const readiness = getModelConfigReadiness(activeApiConfig, apiConfigLoaded, apiConfigLoadError);
      // Scripted desktop tests provide their own model without saving credentials.
      if (import.meta.env.VITE_ANGELBOT_DESKTOP_E2E !== '1'
        && (readiness.kind === 'needs-credentials' || readiness.kind === 'invalid')) {
        openModelSettings();
        return;
      }
      const retainedDraftNotice = submittedFiles.length ? '草稿和附件仍保留。' : '草稿仍保留。';
      const unconfirmedSendNotice = `消息未确认送达，${retainedDraftNotice}请先核对会话记录。`;
      const sendGeneration = scopeRef.current.generation;
      const submittedRevision = valueRevisionRef.current;
      sendingRef.current = true;
      // Retain the submitted snapshots in this closure for failed admission,
      // but do not treat them as the next draft or block text-only steering.
      filesRef.current = filesRef.current.filter((file) => !submittedFiles.includes(file));
      setFileDraft({ generation: sendGeneration, files: filesRef.current });
      setFileStatus({ generation: sendGeneration, reading: false, sending: true, error: '' });
      try {
        const result = await onSend(msg, submittedFiles.length ? submittedFiles : undefined);
        if (!mountedRef.current || scopeRef.current.generation !== sendGeneration) return;
        if (result !== 'accepted') {
          const restored = restoreFiles(submittedFiles);
          if (restored) setFileStatus({ generation: sendGeneration, reading: readsRef.current > 0, sending: false,
            error: result === 'not_sent' ? `消息尚未发送，${retainedDraftNotice}` : unconfirmedSendNotice });
        } else {
          if (valueRevisionRef.current === submittedRevision && valueRef.current.trim() === msg) onChange('');
        }
      } catch {
        if (mountedRef.current && scopeRef.current.generation === sendGeneration) {
          const restored = restoreFiles(submittedFiles);
          if (restored) setFileStatus({ generation: sendGeneration, reading: readsRef.current > 0, sending: false,
            error: unconfirmedSendNotice });
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
    }, [value, isStreaming, disabled, sendDisabled, onSend, onChange, restoreFiles]);

    const submitRunText = async (mode: 'queue' | 'steer') => {
      const submit = mode === 'queue' ? onFollowUp : onSteer;
      if (!submit || !canSubmitRunText || runSubmission.current) return;
      const request = {};
      const revision = valueRevisionRef.current;
      runSubmission.current = request;
      setRunFeedback({ generation, pending: true, error: '' });
      let error = '';
      try {
        const result = submit(value.trim());
        if (result) await result;
        if (mountedRef.current && runSubmission.current === request && valueRevisionRef.current === revision) onChange('');
      } catch (cause) {
        error = `${cause instanceof Error ? cause.message : typeof cause === 'string' ? cause : '未能送达引导。'} 草稿已保留。`;
      } finally {
        if (mountedRef.current && runSubmission.current === request) {
          runSubmission.current = null;
          setRunFeedback({ generation, pending: false, error });
        }
      }
    };

    const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      const alternateMode = isStreaming && Boolean(onSteer && onFollowUp) && e.shiftKey && (e.ctrlKey || e.metaKey);
      if (e.key !== 'Enter' || (e.shiftKey && !alternateMode) || e.nativeEvent.isComposing) return;
      e.preventDefault();
      if (isStreaming) void submitRunText(alternateMode ? (runMode === 'queue' ? 'steer' : 'queue') : runMode);
      else void handleSend();
    };

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

    // Reflow the draft when input wraps differently after a window resize.
    useEffect(() => {
      window.addEventListener('resize', handleInput);
      return () => window.removeEventListener('resize', handleInput);
    }, [handleInput]);

    useEffect(() => {
      handleInput();
    }, [value, chatFontSize, chatFontFamily, handleInput]);

    // ── Render ──────────────────────────────────────────────────────────────

    return (
      <div className={`composer-input-root${isStreaming ? ' composer-input-root--running' : ''}`}
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

        <div className="composer-bar" role={isStreaming ? 'group' : undefined} aria-label={isStreaming ? '运行控制' : undefined}>
          {isStreaming && runControls}
          {attachedFiles.length > 0 && (
            <div className="composer-files-strip" aria-label="待发送文本附件">
              {attachedFiles.map((file, index) => (
                <div key={index} className="composer-file-chip">
                  <TextAttachmentDetails file={file} />
                  <label className="composer-file-mode" title={attachmentReaderEnabled
                    ? '模型需要时再读取附件正文'
                    : '读取附件正文需要开启长期记忆'}>
                    <input type="checkbox" checked={file.referenceOnly === true}
                      aria-label={`按需读取 ${file.name}`}
                      disabled={disabled || status.sending || (!attachmentReaderEnabled && !file.referenceOnly)}
                      onChange={(event) => setAttachmentReferenceOnly(file, event.currentTarget.checked)} />
                    按需读取
                  </label>
                  <button type="button" onClick={() => removeFile(file)} aria-label={`移除附件 ${file.name}`} title="移除附件">×</button>
                </div>
              ))}
            </div>
          )}
          {(attachedFiles.length > 0 || status.reading) && <div className="composer-attachment-note">
            {status.reading ? '正在读取文件文字… ' : ''}点击文件名可核对文字。
            {hasInlineAttachments && '普通附件发送后内容将交给当前配置的模型。'}
            {hasReferenceAttachments && (attachmentReaderEnabled
              ? '按需附件的正文会在模型读取时发送。'
              : '长期记忆已关闭。按需附件只提供引用，模型无法读取正文。')}
            附件是只读快照，不授予原文件访问或修改权限。
            {attachedFiles.some((file) => file.name.toLowerCase().endsWith('.docx')) && ' DOCX 只读取正文和表格文字，不含图片、页眉页脚或版式；复杂域与修订暂不支持。'}
            {attachedFiles.some((file) => file.name.toLowerCase().endsWith('.xlsx')) && ' XLSX 读取保存的原始单元格值，不按日期、百分比或货币格式换算，不含图表或样式；公式及隐藏内容暂不支持。'}
            {attachedFiles.some((file) => file.name.toLowerCase().endsWith('.pdf')) && ' PDF 按页提取文字，扫描件需先 OCR。'}
          </div>}
          {isStreaming && (attachedFiles.length > 0 || status.reading) && <div className="composer-attachment-note">附件已暂存，可在本轮结束后发送。</div>}
          {status.error && <div role="alert" className="composer-attachment-error">{status.error}</div>}
          {runFeedback.generation === generation && runFeedback.error && <div role="alert" className="composer-attachment-error">{runFeedback.error}</div>}
          <textarea
            className="composer-textarea"
            aria-label="消息输入"
            ref={textareaRef}
            value={value}
            onChange={(e) => {
              if (!runPending && runFeedback.error) setRunFeedback({ generation, pending: false, error: '' });
              onChange(e.target.value);
            }}
            onKeyDown={handleKeyDown}
            onInput={handleInput}
            onPaste={handlePaste}
            placeholder={
              stopPending ? '准备下一条消息…'
                : isStreaming && hasRunMessageActions
                ? runPlaceholder
                : isStreaming
                ? 'AngelBot 正在运行…'
                : '给 AngelBot 发消息…'
            }
            rows={1}
            disabled={disabled}
          />

          <div className="composer-toolbar-row">
            <div className="env-options-row">
              <button
                type="button"
                className="composer-icon-button"
                onClick={() => fileInputRef.current?.click()}
                disabled={disabled}
                aria-label="添加文本附件"
                title="添加文本、DOCX 或 XLSX（文档原文件最多 4 MiB；文字每个 16 KiB、合计 32 KiB，最多 4 个）"
              >
                <AttachIcon />
              </button>
              {!isStreaming && <>
                <div className="environment-disclosure" ref={environmentRef}
                  onBlur={(event) => {
                    if (!event.currentTarget.contains(event.relatedTarget)) setEnvironmentOpen(false);
                  }}
                  onKeyDown={(event) => {
                    if (event.key === 'Escape' && environmentOpen) {
                      event.preventDefault();
                      event.stopPropagation();
                      setEnvironmentOpen(false);
                      environmentTriggerRef.current?.focus();
                    }
                  }}>
                  <button
                    type="button"
                    ref={environmentTriggerRef}
                    className="env-dropdown-btn environment-disclosure-trigger"
                    aria-expanded={environmentOpen}
                    aria-controls="composer-environment-panel"
                    onClick={() => setEnvironmentOpen((open) => !open)}
                  >
                    <span>对话选项</span>
                    {permissionLabel && <span className="composer-permission-mode">{permissionLabel}</span>}
                  </button>

                  {environmentOpen && (
                    <div id="composer-environment-panel" className="environment-panel">
                      <button
                        type="button"
                        className="environment-panel-model"
                        onClick={() => {
                          setEnvironmentOpen(false);
                          openModelSettings();
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
                      <EnvironmentChoiceGroup<AgentExecutionPermission>
                        label="执行权限"
                        value={executionPermission}
                        options={EXECUTION_PERMISSION_OPTIONS}
                        onSelect={(next) => void changeExecutionPermission(next)}
                        disabled={disabled || permissionLoading || !permissionLoaded || permissionSaving}
                        describeOptions
                      />
                      {permissionLoading && <div className="environment-panel-status" role="status">正在读取执行权限…</div>}
                      {permissionSaving && <div className="environment-panel-status" role="status">正在保存执行权限…</div>}
                      {permissionError && !permissionLoading && !permissionSaving && (
                        <div className="environment-panel-status" role="alert">
                          {permissionLoaded ? '执行权限未能保存，请重试。' : '无法读取执行权限。'}
                          {!permissionLoaded && <button type="button" onClick={() => void loadExecutionPermission()}>重新读取</button>}
                        </div>
                      )}
                    </div>
                  )}
                </div>

                <button
                  type="button"
                  className="composer-icon-button"
                  onClick={onSoundToggle}
                  aria-label="完成音效"
                  aria-pressed={soundEnabled}
                  title={soundEnabled ? '关闭完成音效' : '开启完成音效'}
                >
                  {soundEnabled ? <SoundOnIcon /> : <SoundOffIcon />}
                </button>
              </>}
            </div>
            <div className="composer-bar-right">
              {isStreaming ? (
                <>
                  {onSteer && onFollowUp && <select className="composer-run-mode" aria-label="指令发送方式"
                    disabled={stopPending}
                    value={runMode} onChange={(event) => setPreferredRunMode(event.target.value === 'steer' ? 'steer' : 'queue')}>
                    <option value="queue">排到下一轮</option>
                    <option value="steer">引导本轮</option>
                  </select>}
                  {hasRunMessageActions && <button type="button" className="composer-submit" disabled={!canSubmitRunText}
                    onClick={() => { void submitRunText(runMode); }} aria-label={runActionLabel}
                    aria-busy={runPending || undefined} title={runPending ? `正在提交 · ${runActionLabel}` : runActionLabel}>
                    <SendIcon />
                  </button>}
                </>
              ) : (
                <button
                  type="button"
                  className="composer-submit"
                  disabled={!canSend}
                  onClick={handleSend}
                  title="发送"
                >
                  <SendIcon />
                </button>
              )}
            </div>
          </div>
        </div>
      </div>
    );
  }
);

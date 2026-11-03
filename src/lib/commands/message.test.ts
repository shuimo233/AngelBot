import { describe, expect, it, vi } from 'vitest';
import type { Message } from '$types';
import { extractDocumentText, normalizeMessageContract, preflightPendingDesktopAction, saveMessageReply, type DesktopActionPreview } from './message';
import { invoke, isTauriRuntime } from '../invoke';

vi.mock('../invoke', () => ({ invoke: vi.fn(), isTauriRuntime: vi.fn() }));

describe('saveMessageReply', () => {
  it('requires the native picker, checks receipts and projects only fixed error explanations', async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(isTauriRuntime).mockReturnValue(false);
    await expect(saveMessageReply('session', 'reply')).rejects.toThrow('桌面应用');
    expect(invoke).not.toHaveBeenCalled();
    vi.mocked(isTauriRuntime).mockReturnValue(true);
    vi.mocked(invoke).mockResolvedValueOnce({ status: 'cancelled', privatePath: 'PRIVATE_PATH' });
    expect(await saveMessageReply('session', 'reply')).toEqual({ status: 'cancelled' });
    vi.mocked(invoke).mockResolvedValueOnce({ status: 'saved', name: '清单.md', bytes: 12, privatePath: 'PRIVATE_PATH' });
    expect(await saveMessageReply('session', 'reply')).toEqual({ status: 'saved', name: '清单.md', bytes: 12 });
    expect(invoke).toHaveBeenLastCalledWith('save_message_reply', { sessionId: 'session', messageId: 'reply' });
    for (const [code, hint] of [['REPLY_UNAVAILABLE', '刷新会话'], ['REPLY_TOO_LARGE', '2 MiB'], ['INVALID_DESTINATION', 'Markdown'], ['FILE_EXISTS', '未覆盖']]) {
      for (const error of [code, new Error(code)]) {
        vi.mocked(invoke).mockRejectedValueOnce(error);
        await expect(saveMessageReply('session', 'reply')).rejects.toThrow(hint);
      }
    }
    for (const failure of ['PRIVATE_RAW_ERROR', new Error('PRIVATE_RAW_ERROR'), { privatePath: 'PRIVATE_PATH' }]) {
      vi.mocked(invoke).mockRejectedValueOnce(failure);
      await expect(saveMessageReply('session', 'reply')).rejects.toThrow('未能确认保存结果，请先检查所选位置；原回复未改变。');
    }
    for (const receipt of [null, {}, { status: 'saved', name: '../private.md', bytes: 1 },
      { status: 'saved', name: 'run.ps1', bytes: 1 }, { status: 'saved', name: 'bad\u202e.md', bytes: 1 },
      { status: 'saved', name: 'reply.md', bytes: 0 }, { status: 'saved', name: 'reply.md', bytes: Infinity },
      { status: 'saved', name: 'reply.md', bytes: 2 * 1024 * 1024 + 1 }]) {
      vi.mocked(invoke).mockResolvedValueOnce(receipt);
      await expect(saveMessageReply('session', 'reply')).rejects.toThrow('未能确认保存结果');
    }
  });
});

describe('preflightPendingDesktopAction', () => {
  it('shares only an in-flight exact-scope read and releases successful or rejected requests', async () => {
    vi.mocked(invoke).mockReset();
    const scope = { sessionId: 'session', messageId: 'reply', callId: 'field' };
    const preview: DesktopActionPreview = { previewId: 'read-1', operation: 'field', appDisplayName: '便笺',
      executableName: 'notes.exe', windowTitle: '记录', controlName: '正文', text: '事项', expiresAt: Date.now() + 60_000 };
    let release!: (value: DesktopActionPreview) => void;
    const reading = new Promise<DesktopActionPreview>((resolve) => { release = resolve; });
    vi.mocked(invoke).mockReturnValue(reading);
    const requests = [scope, { ...scope }, { ...scope, sessionId: 'other' },
      { ...scope, messageId: 'other' }, { ...scope, callId: 'other' }].map(preflightPendingDesktopAction);
    try {
      expect(invoke).toHaveBeenCalledTimes(4);
      release(preview);
      expect(await Promise.all(requests)).toEqual(Array(5).fill(preview));
      // A settled preview may already be consumed. A later recheck must read again.
      vi.mocked(invoke).mockResolvedValueOnce({ ...preview, previewId: 'fresh' });
      expect((await preflightPendingDesktopAction(scope)).previewId).toBe('fresh');
      expect(invoke).toHaveBeenCalledTimes(5);
      vi.mocked(invoke).mockRejectedValueOnce('TARGET_CHANGED: inspect the new page');
      expect(await Promise.allSettled([preflightPendingDesktopAction(scope), preflightPendingDesktopAction({ ...scope })]))
        .toEqual(Array(2).fill({ status: 'rejected', reason: 'TARGET_CHANGED: inspect the new page' }));
      expect(invoke).toHaveBeenCalledTimes(6);
      vi.mocked(invoke).mockResolvedValueOnce({ ...preview, previewId: 'after-failure' });
      expect((await preflightPendingDesktopAction(scope)).previewId).toBe('after-failure');
      expect(invoke).toHaveBeenCalledTimes(7);
      expect(invoke).toHaveBeenLastCalledWith('preflight_pending_desktop_action', scope);
    } finally {
      release(preview);
      await Promise.allSettled(requests);
      vi.mocked(invoke).mockReset();
    }
  });
});

describe('extractDocumentText', () => {
  it('normalizes native string rejection without hiding its explanation or altering valid snapshots', async () => {
    vi.mocked(invoke).mockRejectedValueOnce('DOCX 文件损坏，请重新选择。');
    await expect(extractDocumentText('broken.docx', 'eA==')).rejects.toBeInstanceOf(Error);
    for (const rejection of ['DOCX 文件损坏', new Error('DOCX 文件损坏')]) {
      vi.mocked(invoke).mockRejectedValueOnce(rejection);
      await expect(extractDocumentText('broken.docx', 'eA==')).rejects.toThrow('DOCX 文件损坏');
    }
    expect(invoke).toHaveBeenLastCalledWith('extract_document_text', { req: { name: 'broken.docx', dataBase64: 'eA==' } });
    vi.mocked(invoke).mockRejectedValueOnce({ privateDetail: 'do not expose' });
    await expect(extractDocumentText('broken.docx', 'eA==')).rejects.toThrow('文档读取未能完成，请重新选择。');
    for (const extension of ['docx', 'xlsx']) {
      const snapshot = { name: `valid.${extension}`, text: '文字快照' };
      vi.mocked(invoke).mockResolvedValueOnce(snapshot);
      expect(await extractDocumentText(snapshot.name, 'UEs=')).toEqual(snapshot);
      expect(invoke).toHaveBeenLastCalledWith('extract_document_text', { req: { name: snapshot.name, dataBase64: 'UEs=' } });
    }
  });
});

describe('normalizeMessageContract', () => {
  it('projects durable snake-case task facts into a visible pending confirmation', () => {
    const raw = {
      id: 'assistant-1',
      sessionId: 'session-1',
      role: 'assistant',
      content: 'Needs approval',
      createdAt: 1,
      toolCalls: [{
        id: 'call-1',
        name: 'open_windows_setting',
        arguments: JSON.stringify({ page: 'sound' }),
      }],
      taskFacts: {
        goal: 'Open sound settings',
        completed_steps: [],
        failed_steps: [],
        modified_files: ['notes/todo.md'],
        verification_evidence: [{
          command: 'verify', exit_code: 0, summary: 'passed', verified_at: 12,
        }],
        pending_confirmation: {
          call_id: 'call-1',
          tool_name: 'open_windows_setting',
          arguments: { page: 'sound' },
          requested_at: 10,
        },
      },
    } as unknown as Message;

    const message = normalizeMessageContract(raw);

    expect(message.taskFacts?.modifiedFiles).toEqual(['notes/todo.md']);
    expect(message.taskFacts?.verificationEvidence?.[0]).toEqual({
      command: 'verify', exitCode: 0, summary: 'passed', verifiedAt: 12,
    });
    expect(message.taskFacts?.pendingConfirmation).toEqual({
      callId: 'call-1',
      toolName: 'open_windows_setting',
      arguments: { page: 'sound' },
      requestedAt: 10,
    });
    expect(message.toolResults).toEqual([expect.objectContaining({
      callId: 'call-1',
      confirmationRequired: true,
      confirmationStatus: 'pending',
    })]);
  });

  it('does not duplicate a backend-provided result', () => {
    const message = normalizeMessageContract({
      id: 'assistant-1', sessionId: 'session-1', role: 'assistant', content: '', createdAt: 1,
      toolCalls: [{ id: 'call-1', name: 'write_file', arguments: '{}' }],
      toolResults: [{ callId: 'call-1', toolName: 'write_file', success: false, output: 'pending', confirmationRequired: true, confirmationStatus: 'pending' }],
      taskFacts: {
        goal: 'Write',
        pendingConfirmation: { callId: 'call-1', toolName: 'write_file', arguments: {}, requestedAt: 1 },
      },
    });

    expect(message.toolResults).toHaveLength(1);
  });

  it('projects a durable rejection when the backend stores no tool-result row', () => {
    const message = normalizeMessageContract({
      id: 'assistant-1', sessionId: 'session-1', role: 'assistant', content: '', createdAt: 1,
      toolCalls: [{ id: 'call-1', name: 'open_windows_setting', arguments: '{}' }],
      taskRun: {
        id: 'run-1', goal: 'Open settings', status: 'needs_attention', plan: ['open_windows_setting'],
        confirmationState: 'rejected', resumable: true, stepCount: 1, completedStepCount: 0,
      },
      taskFacts: {
        goal: 'Open settings',
        // Compatibility with pre-fix durable rows: the task-run verdict wins.
        pendingConfirmation: {
          callId: 'call-1', toolName: 'open_windows_setting', arguments: {}, requestedAt: 1,
        },
      },
    });

    expect(message.toolResults).toEqual([expect.objectContaining({
      callId: 'call-1', confirmationRequired: false, confirmationStatus: 'rejected',
    })]);
  });
});

import { createSession, getSessions, sendMessage, getMessages, resolveAgentConfirmation, preflightPendingDesktopAction } from './commands';
import { beforeEach, describe, expect, it, vi } from 'vitest';

describe('commands', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('creates and loads a session via Tauri', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd } = JSON.parse(init?.body as string);
      const data = (() => {
        if (cmd === 'create_session') return Promise.resolve({ id: 'session-1', title: 'New', createdAt: 1, updatedAt: 1 });
        if (cmd === 'get_sessions') return Promise.resolve([{ id: 'session-1', title: 'New', createdAt: 1, updatedAt: 1 }]);
        return Promise.resolve([]);
      })();
      return new Response(JSON.stringify({ ok: true, data: await data }));
    });

    const created = await createSession('New');
    const sessions = await getSessions();
    expect(created.id).toBe('session-1');
    expect(sessions[0].title).toBe('New');
  });

  it('sends and loads messages via Tauri', async () => {
    const stored: any[] = [];
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd, args } = JSON.parse(init?.body as string);
      const data = (() => {
        if (cmd === 'send_message') {
          const message = { id: 'm1', sessionId: args.sessionId, role: 'assistant', content: 'hi', createdAt: 1 };
          stored.push(message);
          return Promise.resolve(message);
        }
        if (cmd === 'get_messages') return Promise.resolve([...stored]);
        return Promise.resolve([]);
      })();
      return new Response(JSON.stringify({ ok: true, data: await data }));
    });

    const reply = await sendMessage({ sessionId: 's', role: 'user', content: 'hi' });
    const messages = await getMessages('s');
    expect(reply.role).toBe('assistant');
    expect(messages[0].content).toBe('hi');
  });

  it('wraps confirmation decisions in the Tauri request payload', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd, args } = JSON.parse(init?.body as string);
      expect(cmd).toBe('resolve_agent_confirmation');
      expect(args).toEqual({
        req: {
          sessionId: 'session-1',
          messageId: 'message-1',
          callId: 'call-1',
          decision: 'approved',
        },
      });
      return new Response(JSON.stringify({
        ok: true,
        data: { callId: 'call-1', toolName: 'write_file', success: true, output: '文件写入成功' },
      }));
    });

    const result = await resolveAgentConfirmation({
      sessionId: 'session-1',
      messageId: 'message-1',
      callId: 'call-1',
      decision: 'approved',
    });
    expect(result.success).toBe(true);
  });

  it('gets a persisted desktop action preflight and passes its ID only on approval', async () => {
    const calls: Array<{ cmd: string; args: unknown }> = [];
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const request = JSON.parse(init?.body as string) as { cmd: string; args: unknown };
      calls.push(request);
      if (request.cmd === 'preflight_pending_desktop_action') {
        return new Response(JSON.stringify({ ok: true, data: {
          previewId: 'preview-1', operation: 'draft', appDisplayName: '便笺', executableName: 'notes.exe',
          windowTitle: '当前便笺', controlName: '正文', text: '精确内容',
          expiresAt: Date.now() + 30_000,
        } }));
      }
      return new Response(JSON.stringify({ ok: true, data: {
        callId: 'call-1', toolName: 'prepare_message_draft', success: true, output: 'verified',
      } }));
    });

    const preview = await preflightPendingDesktopAction({
      sessionId: 'session-1', messageId: 'message-1', callId: 'call-1',
    });
    await resolveAgentConfirmation({
      sessionId: 'session-1', messageId: 'message-1', callId: 'call-1',
      decision: 'approved', previewId: preview.previewId,
    });

    expect(calls).toEqual([
      { cmd: 'preflight_pending_desktop_action', args: {
        sessionId: 'session-1', messageId: 'message-1', callId: 'call-1',
      } },
      { cmd: 'resolve_agent_confirmation', args: { req: {
        sessionId: 'session-1', messageId: 'message-1', callId: 'call-1',
        decision: 'approved', previewId: 'preview-1',
      } } },
    ]);
  });
});

import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ContentBlockRenderer, groupBlocksIntoSegments } from './ContentBlockRenderer';
import type { LiveBlock } from '$stores/messages';

afterEach(() => vi.restoreAllMocks());

function text(index: number, iterationId: number, content: string): LiveBlock {
  return { kind: 'text', index, iterationId, content };
}

function tool(
  index: number,
  iterationId: number,
  partial: Partial<Extract<LiveBlock, { kind: 'tool_call' }>> = {},
): LiveBlock {
  return {
    kind: 'tool_call',
    index,
    iterationId,
    status: 'completed',
    callId: `call-${index}`,
    toolName: 'read_file',
    arguments: {},
    ...partial,
  };
}

describe('groupBlocksIntoSegments', () => {
  it('returns an empty list when there are no blocks', () => {
    expect(groupBlocksIntoSegments([])).toEqual([]);
  });

  it('keeps a single text block as one text segment', () => {
    const segments = groupBlocksIntoSegments([text(1, 0, 'hello')]);
    expect(segments).toEqual([
      { kind: 'text', iterationId: 0, content: 'hello' },
    ]);
  });

  it('merges consecutive text blocks sharing iterationId', () => {
    const segments = groupBlocksIntoSegments([
      text(1, 0, 'foo '),
      text(2, 0, 'bar '),
      text(3, 0, 'baz'),
    ]);
    expect(segments).toEqual([
      { kind: 'text', iterationId: 0, content: 'foo bar baz' },
    ]);
  });

  it('splits text when iterationId changes', () => {
    const segments = groupBlocksIntoSegments([
      text(1, 0, 'first '),
      text(2, 0, 'thought'),
      text(3, 1, 'second '),
      text(4, 1, 'thought'),
    ]);
    expect(segments).toEqual([
      { kind: 'text', iterationId: 0, content: 'first thought' },
      { kind: 'text', iterationId: 1, content: 'second thought' },
    ]);
  });

  it('groups consecutive tool blocks into one slice', () => {
    const segments = groupBlocksIntoSegments([
      tool(1, 0, { callId: 'a', toolName: 'read_file' }),
      tool(2, 0, { callId: 'b', toolName: 'list_dir' }),
      tool(3, 0, { callId: 'c', toolName: 'grep_search' }),
    ]);
    expect(segments).toHaveLength(1);
    expect(segments[0]).toMatchObject({
      kind: 'slice',
      iterationId: 0,
      action: 'explore',
    });
    if (segments[0].kind === 'slice') {
      expect(segments[0].tools).toHaveLength(3);
      expect(segments[0].tools.map((t) => t.callId)).toEqual(['a', 'b', 'c']);
    }
  });

  it('keeps text and tool interleaving as separate segments in order', () => {
    const segments = groupBlocksIntoSegments([
      text(1, 0, 'let me check'),
      tool(2, 0, { callId: 'a' }),
      tool(3, 0, { callId: 'b' }),
      text(4, 1, 'found it'),
    ]);
    expect(segments.map((s) => s.kind)).toEqual(['text', 'slice', 'text']);
    expect(segments.map((s) => s.iterationId)).toEqual([0, 0, 1]);
  });

  it('does not merge text and tool blocks even when iterationId matches', () => {
    const segments = groupBlocksIntoSegments([
      text(1, 0, 'a'),
      tool(2, 0, { callId: 'x' }),
      text(3, 0, 'b'),
    ]);
    expect(segments.map((s) => s.kind)).toEqual(['text', 'slice', 'text']);
  });

  it('groups related calls across agent iterations until text resumes', () => {
    const segments = groupBlocksIntoSegments([
      tool(1, 0, { toolName: 'run_project_command' }),
      tool(2, 1, { toolName: 'run_project_command' }),
      tool(3, 2, { toolName: 'run_project_command' }),
    ]);
    expect(segments).toHaveLength(1);
    expect(segments[0]).toMatchObject({ kind: 'slice', action: 'command' });
    if (segments[0].kind === 'slice') expect(segments[0].tools).toHaveLength(3);
  });

  it('starts a new slice when the user-facing action changes', () => {
    const segments = groupBlocksIntoSegments([
      tool(1, 0, { toolName: 'read_file' }),
      tool(2, 1, { toolName: 'write_file' }),
      tool(3, 2, { toolName: 'run_project_command', arguments: { command: 'cargo', args: ['test'] } }),
    ]);
    expect(segments.map((segment) => segment.kind === 'slice' ? segment.action : segment.kind))
      .toEqual(['explore', 'edit', 'verify']);
  });
});

describe('ContentBlockRenderer', () => {
  it('renders nothing for an empty block list', () => {
    const { container } = render(<ContentBlockRenderer blocks={[]} />);
    expect(container.firstChild).toBeNull();
  });

  it('renders text segments as paragraphs', () => {
    render(
      <ContentBlockRenderer
        blocks={[text(1, 0, '你好'), text(2, 0, '世界')]}
      />,
    );
    expect(screen.getByText(/你好世界/)).toBeInTheDocument();
  });

  it('renders GFM pipe tables as semantic tables instead of plain text', () => {
    render(
      <ContentBlockRenderer
        blocks={[text(1, 0, '| File | Purpose |\n| --- | --- |\n| README.md | Project guide |')]}
      />,
    );
    expect(screen.getByRole('table')).toBeInTheDocument();
    expect(screen.getByRole('columnheader', { name: 'File' })).toBeInTheDocument();
    expect(screen.getByRole('cell', { name: 'Project guide' })).toBeInTheDocument();
  });

  it('renders a single-tool slice as a collapsed card', () => {
    render(
      <ContentBlockRenderer
        blocks={[tool(1, 0, { toolName: 'read_file' })]}
      />,
    );
    expect(screen.getByText('已完成探索工作区')).toBeInTheDocument();
    expect(screen.getByText('1/1')).toBeInTheDocument();
    expect(screen.queryByText('读取文件')).not.toBeInTheDocument();
  });

  it('keeps completed multi-tool slices compact until expanded', async () => {
    render(
      <ContentBlockRenderer
        blocks={[
          tool(1, 0, { callId: 'a', toolName: 'read_file' }),
          tool(2, 0, { callId: 'b', toolName: 'list_dir' }),
          tool(3, 0, { callId: 'c', toolName: 'grep_search' }),
        ]}
      />,
    );
    expect(screen.getByText('3/3')).toBeInTheDocument();
    expect(screen.queryByText(/读取文件|浏览目录|搜索文本/)).not.toBeInTheDocument();
    expect(screen.getByText('已完成探索工作区')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: /已完成探索工作区/ }));
    expect(screen.getAllByText(/读取文件|浏览目录|搜索文本/)).toHaveLength(3);
  });

  it('updates one live slice in place and folds it after the last call finishes', async () => {
    const blocks = [
      tool(1, 0, { callId: 'a', toolName: 'run_project_command', status: 'completed' }),
      tool(2, 1, { callId: 'b', toolName: 'run_project_command', status: 'running' }),
    ];
    const { rerender } = render(<ContentBlockRenderer blocks={blocks} />);
    expect(screen.getByText('正在运行命令')).toBeInTheDocument();
    expect(screen.getAllByText('运行项目命令')).toHaveLength(2);

    rerender(
      <ContentBlockRenderer
        blocks={blocks.map((block) => block.kind === 'tool_call' && block.callId === 'b'
          ? { ...block, status: 'completed' as const }
          : block)}
      />,
    );
    await waitFor(() => expect(screen.getByText('已完成运行命令')).toBeInTheDocument());
    expect(screen.queryByText('运行项目命令')).not.toBeInTheDocument();
  });

  it('calls onResolveConfirmation when the user approves a paused tool', async () => {
    const onResolveConfirmation = vi.fn();
    render(
      <ContentBlockRenderer
        blocks={[
          tool(1, 0, {
            callId: 'a',
            toolName: 'write_file',
            status: 'needs_approval',
            reason: '需要确认',
          }),
        ]}
        onResolveConfirmation={onResolveConfirmation}
      />,
    );
    await userEvent.click(screen.getByRole('button', { name: '允许' }));
    expect(onResolveConfirmation).toHaveBeenCalledWith('a', 'approved');
  });

  it('shows a safe external-boundary summary for a live Web Search approval', () => {
    render(
      <ContentBlockRenderer
        blocks={[tool(1, 0, {
          callId: 'web-search',
          toolName: 'web_search',
          status: 'needs_approval',
          arguments: { query: 'private medical question' },
          reason: 'Confirmation required',
        })]}
        onResolveConfirmation={vi.fn()}
      />,
    );

    expect(screen.getByText('向已配置的搜索服务发送一次公开网页查询')).toBeInTheDocument();
    expect(screen.queryByText('private medical question')).not.toBeInTheDocument();
  });

  it('shows the exact desktop draft before allowing it to be written', async () => {
    const onResolveConfirmation = vi.fn();
    const draft = '请先核对金额。\n第二行 <不发送>';
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const { cmd, args } = JSON.parse(init?.body as string);
      expect(cmd).toBe('preflight_pending_desktop_action');
      expect(args).toEqual({ sessionId: 'session-1', messageId: 'message-1', callId: 'desktop-draft' });
      return new Response(JSON.stringify({ ok: true, data: {
        previewId: 'preview-1', operation: 'draft', appDisplayName: '便笺', executableName: 'notes.exe',
        windowTitle: '当前便笺', controlName: '正文', text: draft, expiresAt: Date.now() + 60_000,
      } }));
    });
    render(
      <ContentBlockRenderer
        sessionId="session-1"
        messageId="message-1"
        blocks={[tool(1, 0, {
          callId: 'desktop-draft',
          toolName: 'prepare_message_draft',
          status: 'needs_approval',
          arguments: { app_id: 'forged-app', text: 'forged model text' },
        })]}
        onResolveConfirmation={onResolveConfirmation}
      />,
    );

    expect(screen.getByRole('button', { name: '允许' })).toBeDisabled();
    expect(await screen.findByText((_, element) => element?.tagName === 'PRE' && element.textContent === draft)).toBeInTheDocument();
    expect(screen.getByText(/便笺（notes.exe）/)).toBeInTheDocument();
    expect(screen.queryByText('forged model text')).not.toBeInTheDocument();
    expect(screen.queryByText('forged-app')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: '允许' }));
    expect(onResolveConfirmation).toHaveBeenCalledWith('desktop-draft', 'approved', 'preview-1');
  });

  it('uses the same attested approval for a generic field without claiming it is a draft', async () => {
    const onResolveConfirmation = vi.fn();
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const request = JSON.parse(init?.body as string) as { cmd: string; args: unknown };
      expect(request).toEqual({
        cmd: 'preflight_pending_desktop_action',
        args: { sessionId: 'session-1', messageId: 'message-1', callId: 'field-call' },
      });
      return new Response(JSON.stringify({ ok: true, data: {
        previewId: 'field-preview', operation: 'field', appDisplayName: '记事本',
        executableName: 'notepad.exe', windowTitle: '工作记录', controlName: '正文',
        text: '由后端确认的文本', expiresAt: Date.now() + 60_000,
      } }));
    });
    render(<ContentBlockRenderer
      sessionId="session-1"
      messageId="message-1"
      blocks={[tool(1, 0, {
        callId: 'field-call', toolName: 'set_trusted_app_text', status: 'needs_approval',
        arguments: { app_id: 'forged-app', field_ref: 'forged-ref', text: 'model-authored text' },
      })]}
      onResolveConfirmation={onResolveConfirmation}
    />);

    expect(await screen.findByText('由后端确认的文本')).toBeInTheDocument();
    expect(screen.getByText(/将修改这一个输入框，不点击提交/)).toBeInTheDocument();
    expect(screen.queryByText(/仅填写草稿/)).not.toBeInTheDocument();
    expect(screen.queryByText('model-authored text')).not.toBeInTheDocument();
    expect(screen.queryByText('forged-app')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: '允许' }));
    expect(onResolveConfirmation).toHaveBeenCalledWith('field-call', 'approved', 'field-preview');
  });

  it.each([
    { action: 'invoke', label: '调用控件' }, { action: 'select', label: '选中控件' },
    { action: 'expand', label: '展开控件' }, { action: 'collapse', label: '收起控件' },
    { action: 'scrollup', label: '向上小幅滚动' }, { action: 'scrolldown', label: '向下小幅滚动' },
  ])('uses the attested $action target, action-specific approval and no text preview', async ({ action, label }) => {
    const onResolveConfirmation = vi.fn();
    const targetName = action === 'scrollup' || action === 'scrolldown' ? '正文浏览区' : '发送';
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_input, init) => {
      const request = JSON.parse(init?.body as string) as { cmd: string; args: unknown };
      expect(request).toEqual({
        cmd: 'preflight_pending_desktop_action',
        args: { sessionId: 'session-1', messageId: 'message-1', callId: 'invoke-call' },
      });
      return new Response(JSON.stringify({ ok: true, data: {
        previewId: 'invoke-preview', operation: action, appDisplayName: '邮件',
        executableName: 'mail.exe', windowTitle: '新邮件', controlName: targetName,
        text: null, expiresAt: Date.now() + 60_000,
      } }));
    });
    render(<ContentBlockRenderer
      sessionId="session-1" messageId="message-1"
      blocks={[tool(1, 0, {
        callId: 'invoke-call', toolName: 'operate_trusted_app_control', status: 'needs_approval',
        arguments: { app_id: 'forged-app', control_ref: 'forged-control', action, name: 'Safe button', text: 'forged text' },
      })]}
      onResolveConfirmation={onResolveConfirmation}
    />);

    expect(screen.getByRole('button', { name: `允许${label}一次` })).toBeDisabled();
    expect(await screen.findByText(new RegExp(`邮件（mail.exe） · 新邮件 · ${targetName}`))).toBeInTheDocument();
    expect(screen.getByText(/可能触发发送、删除等后果/)).toBeInTheDocument();
    expect(screen.getByText(/控件名称不代表安全或授权/)).toBeInTheDocument();
    expect(screen.getByText(`待确认操作：${label}`)).toBeInTheDocument();
    expect(screen.getByText(action === 'invoke'
      ? /仅表示操作请求已发出，不代表目标已完成/
      : action === 'scrollup' || action === 'scrolldown'
        ? /仅确认滚动状态，仍需检查任务结果，不代表目标已完成；请重新观察页面/
      : /仅确认控件状态，仍需检查任务结果，不代表目标已完成/)).toBeInTheDocument();
    expect(screen.queryByText('将填写的内容')).not.toBeInTheDocument();
    expect(screen.queryByText('forged-app')).not.toBeInTheDocument();
    expect(screen.queryByText('forged-control')).not.toBeInTheDocument();
    expect(screen.queryByText('Safe button')).not.toBeInTheDocument();
    expect(screen.queryByText('forged text')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: `允许${label}一次` }));
    expect(onResolveConfirmation).toHaveBeenCalledWith('invoke-call', 'approved', 'invoke-preview');
  });

  it.each([
    { action: 'invoke', label: '调用控件' },
    { action: 'scrollup', label: '向上小幅滚动' }, { action: 'scrolldown', label: '向下小幅滚动' },
  ])('does not allow $action with a mismatched text-bearing preflight', async ({ action, label }) => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({ ok: true, data: {
      previewId: 'invoke-invalid', operation: action, appDisplayName: '邮件',
      executableName: 'mail.exe', windowTitle: '新邮件', controlName: '发送',
      text: 'unexpected text', expiresAt: Date.now() + 60_000,
    } })));
    const onResolveConfirmation = vi.fn();
    render(<ContentBlockRenderer
      sessionId="session-1" messageId="message-1"
      blocks={[tool(1, 0, { toolName: 'operate_trusted_app_control', arguments: { action }, status: 'needs_approval' })]}
      onResolveConfirmation={onResolveConfirmation}
    />);

    expect(await screen.findByRole('alert')).toHaveTextContent('无法核对目标窗口或待操作控件');
    expect(screen.getByRole('button', { name: `允许${label}一次` })).toBeDisabled();
    expect(screen.queryByText('unexpected text')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: '拒绝' }));
    expect(onResolveConfirmation).toHaveBeenCalledWith('call-1', 'rejected', undefined);
  });

  it.each([
    { action: 'invoke', previewAction: 'select' }, { action: 'select', previewAction: 'expand' },
    { action: 'expand', previewAction: 'collapse' }, { action: 'collapse', previewAction: 'invoke' },
    { action: 'scrollup', previewAction: 'scrolldown' }, { action: 'scrolldown', previewAction: 'scrollup' },
  ])('cannot approve a forged $previewAction preview for a requested $action action', async ({ action, previewAction }) => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({ ok: true, data: {
      previewId: 'forged-action', operation: previewAction, appDisplayName: 'Editor', executableName: 'editor.exe',
      windowTitle: 'Document', controlName: 'Unknown label', text: null, expiresAt: Date.now() + 60_000,
    } })));
    render(<ContentBlockRenderer sessionId="session-1" messageId="message-1"
      blocks={[tool(1, 0, { toolName: 'operate_trusted_app_control', arguments: { action }, status: 'needs_approval' })]}
      onResolveConfirmation={vi.fn()} />);
    expect(await screen.findByRole('alert')).toHaveTextContent('无法核对目标窗口或待操作控件');
    expect(screen.getByRole('button', { name: /^允许/ })).toBeDisabled();
    expect(screen.queryByText(/Editor（editor.exe）/)).not.toBeInTheDocument();
  });

  it.each([
    { controlName: '', expiresAt: Date.now() + 60_000 },
    { controlName: '   ', expiresAt: Date.now() + 60_000 },
    { controlName: null, expiresAt: Date.now() + 60_000 },
    { controlName: undefined, expiresAt: Date.now() + 60_000 },
    { controlName: 'Control', expiresAt: Infinity },
    { controlName: 'Control', expiresAt: NaN },
  ])('blocks an unclear label or nonfinite deadline in a native preview: %j', async (invalid) => {
    const preview = Object.assign({
      previewId: 'invalid-target', operation: 'select', appDisplayName: 'Editor', executableName: 'editor.exe',
      windowTitle: null, controlName: 'Option', text: null, expiresAt: Date.now() + 60_000,
    }, invalid);
    // Return decoded data directly so NaN/Infinity exercise the runtime guard,
    // rather than being converted to null by JSON.stringify in the fixture.
    vi.spyOn(globalThis, 'fetch').mockResolvedValue({
      ok: true, json: async () => ({ ok: true, data: preview }),
    } as Response);
    render(<ContentBlockRenderer sessionId="session-1" messageId="message-1"
      blocks={[tool(1, 0, { toolName: 'operate_trusted_app_control', arguments: { action: 'select' }, status: 'needs_approval' })]}
      onResolveConfirmation={vi.fn()} />);
    expect(await screen.findByRole('alert')).toHaveTextContent('无法核对目标窗口或待操作控件');
    expect(screen.getByRole('button', { name: '允许选中控件一次' })).toBeDisabled();
    expect(screen.queryByText(/已核对目标：/)).not.toBeInTheDocument();
  });

  it.each([
    { toolName: 'operate_trusted_app_control', arguments: {} },
    { toolName: 'operate_trusted_app_control', arguments: { action: 'unknown' } },
    { toolName: 'operate_trusted_app_control', arguments: { action: 'ScrollUp' } },
    { toolName: 'operate_trusted_app_control', arguments: { action: 'scroll_up' } },
    { toolName: 'invoke_trusted_app_control', arguments: { action: 'invoke' } },
  ])('blocks unsupported or legacy pending $toolName without inspection or generic approval', async ({ toolName, arguments: args }) => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockRejectedValue(new Error('must not inspect'));
    const onResolveConfirmation = vi.fn();
    render(<ContentBlockRenderer sessionId="session-1" messageId="message-1"
      blocks={[tool(1, 0, { toolName, arguments: args, status: 'needs_approval' })]}
      onResolveConfirmation={onResolveConfirmation} />);
    expect(await screen.findByRole('alert')).toHaveTextContent('请求动作缺失、无效或已停用');
    expect(screen.getByRole('button', { name: /^允许/ })).toBeDisabled();
    expect(screen.queryByRole('button', { name: '重新核对' })).not.toBeInTheDocument();
    expect(fetchMock).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: '拒绝' }));
    expect(onResolveConfirmation).toHaveBeenCalledWith('call-1', 'rejected', undefined);
  });

  it('rejects an attested preview for the wrong desktop text operation', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({ ok: true, data: {
      previewId: 'wrong-operation', operation: 'draft', appDisplayName: '记事本',
      executableName: 'notepad.exe', windowTitle: null, controlName: '正文',
      text: 'not for generic approval', expiresAt: Date.now() + 60_000,
    } })));
    render(<ContentBlockRenderer
      sessionId="session-1" messageId="message-1"
      blocks={[tool(1, 0, { toolName: 'set_trusted_app_text', status: 'needs_approval' })]}
      onResolveConfirmation={vi.fn()}
    />);

    expect(await screen.findByRole('alert')).toHaveTextContent('无法核对目标窗口或待填写内容');
    expect(screen.getByRole('button', { name: '允许' })).toBeDisabled();
    expect(screen.queryByText('not for generic approval')).not.toBeInTheDocument();
  });

  it('does not allow a draft when backend preflight cannot attest its target', async () => {
    const onResolveConfirmation = vi.fn();
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({
      ok: false, error: 'target is unavailable',
    })));
    render(
      <ContentBlockRenderer
        sessionId="session-1"
        messageId="message-1"
        blocks={[tool(1, 0, {
          toolName: 'prepare_message_draft',
          status: 'needs_approval',
          arguments: { app_id: 'mail', text: 'model claim' },
        })]}
        onResolveConfirmation={onResolveConfirmation}
      />,
    );

    expect(await screen.findByRole('alert')).toHaveTextContent('无法核对目标窗口或草稿内容');
    expect(screen.getByRole('button', { name: '允许' })).toBeDisabled();
    expect(screen.queryByText('model claim')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: '拒绝' }));
    expect(onResolveConfirmation).toHaveBeenCalledWith('call-1', 'rejected', undefined);
  });

  it('refuses an approval if its attested target expires before the click', async () => {
    const onResolveConfirmation = vi.fn();
    const issuedAt = Date.now();
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({ ok: true, data: {
      previewId: 'preview-expiring', operation: 'draft', appDisplayName: '便笺', executableName: 'notes.exe',
      windowTitle: null, controlName: '正文', text: '待填写文本', expiresAt: issuedAt + 60_000,
    } })));
    render(<ContentBlockRenderer
      sessionId="session-1"
      messageId="message-1"
      blocks={[tool(1, 0, { toolName: 'prepare_message_draft', status: 'needs_approval' })]}
      onResolveConfirmation={onResolveConfirmation}
    />);

    expect(await screen.findByText('待填写文本')).toBeInTheDocument();
    vi.spyOn(Date, 'now').mockReturnValue(issuedAt + 61_000);
    await userEvent.click(screen.getByRole('button', { name: '允许' }));
    expect(onResolveConfirmation).not.toHaveBeenCalled();
    expect(screen.getByRole('alert')).toHaveTextContent('目标预检已过期');
    expect(screen.getByRole('button', { name: '允许' })).toBeDisabled();
  });

  it('keeps a dispatched desktop launch distinct from a completed task', async () => {
    render(<ContentBlockRenderer blocks={[tool(1, 0, {
      toolName: 'open_trusted_app',
      output: '{"status":"dispatched","target":"notes"}',
    })]} />);

    expect(screen.getByText('已发出桌面请求')).toBeInTheDocument();
    expect(screen.queryByText('已完成处理任务')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: /已发出桌面请求/ }));
    expect(screen.getByText('打开应用')).toBeInTheDocument();
    expect(screen.getByText(/尚未确认目标窗口已就绪/)).toBeInTheDocument();
  });

  it('keeps control dispatch visible without claiming completion', async () => {
    render(<ContentBlockRenderer blocks={[tool(1, 0, {
      toolName: 'operate_trusted_app_control',
      output: '{"status":"dispatched","action":"invoke"}',
    })]} />);
    expect(screen.getByText('已发出桌面请求')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: /已发出桌面请求/ }));
    expect(screen.getByText(/操作已发出，需重新观察目标确认结果/)).toBeInTheDocument();
    expect(screen.queryByText(/尚未确认目标窗口已就绪/)).not.toBeInTheDocument();
  });

  it.each([
    { action: 'select', state: '选中' }, { action: 'expand', state: '展开' }, { action: 'collapse', state: '收起' },
  ])('shows only the verified $action control state, not a text write or completed task', async ({ action, state }) => {
    render(<ContentBlockRenderer blocks={[tool(1, 0, {
      toolName: 'operate_trusted_app_control', output: JSON.stringify({ status: 'verified', action }),
    })]} />);
    expect(screen.getByText('控件状态已确认，任务结果待检查')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: /控件状态已确认，任务结果待检查/ }));
    expect(screen.getByText(new RegExp(`已确认控件${state}状态，仍需检查任务结果`))).toBeInTheDocument();
    expect(screen.queryByText('输入内容已填写并校验')).not.toBeInTheDocument();
    expect(screen.queryByText('已完成处理任务')).not.toBeInTheDocument();
  });

  it.each([
    { action: 'scrollup', direction: '向上' }, { action: 'scrolldown', direction: '向下' },
  ])('shows a verified $action as scroll state only and calls for fresh observation', async ({ action, direction }) => {
    render(<ContentBlockRenderer blocks={[tool(1, 0, {
      toolName: 'operate_trusted_app_control', output: JSON.stringify({ status: 'verified', action }),
    })]} />);
    await userEvent.click(screen.getByRole('button', { name: /控件状态已确认，任务结果待检查/ }));
    expect(screen.getByText(new RegExp(`已确认${direction}滚动方向或已到边界；这不代表任务完成`))).toBeInTheDocument();
    expect(screen.getByText(/重新观察页面并获取新的控件引用/)).toBeInTheDocument();
    expect(screen.queryByText('输入内容已填写并校验')).not.toBeInTheDocument();
    expect(screen.queryByText('已完成处理任务')).not.toBeInTheDocument();
  });

  it('does not promote a verified invoke result to confirmed task or control state', async () => {
    render(<ContentBlockRenderer blocks={[tool(1, 0, {
      toolName: 'operate_trusted_app_control', output: '{"status":"verified","action":"invoke"}',
    })]} />);
    expect(screen.getByText('桌面操作状态未确认')).toBeInTheDocument();
    expect(screen.queryByText('输入内容已填写并校验')).not.toBeInTheDocument();
    expect(screen.queryByText('控件状态已确认，任务结果待检查')).not.toBeInTheDocument();
  });

  it('keeps an uncertain invocation open for manual inspection', () => {
    render(<ContentBlockRenderer blocks={[tool(1, 0, {
      toolName: 'operate_trusted_app_control', status: 'failed',
      error: 'Error: {"code":"RESULT_UNKNOWN"}',
    })]} />);
    expect(screen.getByText('桌面操作结果待核对')).toBeInTheDocument();
    expect(screen.getByText(/控件可能已被触发；请重新观察目标确认结果，勿自动重试/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '允许' })).not.toBeInTheDocument();
  });

  it('keeps an uncertain desktop draft open for manual review', () => {
    render(<ContentBlockRenderer blocks={[tool(1, 0, {
      toolName: 'prepare_message_draft',
      status: 'failed',
      error: 'Error: {"code":"RESULT_UNKNOWN","message":"inspect first"}',
    })]} />);

    expect(screen.getByText('桌面操作结果待核对')).toBeInTheDocument();
    expect(screen.getByText(/请先去目标应用核对，勿自动重试/)).toBeInTheDocument();
  });

  it('uses the whole tool row as the only output disclosure control', async () => {
    render(
      <ContentBlockRenderer
        blocks={[tool(1, 0, { status: 'running', output: 'directory contents' })]}
      />,
    );
    expect(screen.queryByText('directory contents')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: '展开 读取文件 的输出' }));
    expect(screen.getByText('directory contents')).toBeInTheDocument();
    expect(screen.queryByText('详情')).not.toBeInTheDocument();
  });

  it('alternates between text paragraphs and slice cards in arrival order', () => {
    render(
      <ContentBlockRenderer
        blocks={[
          text(1, 0, 'before'),
          tool(2, 0, { callId: 'a' }),
          tool(3, 0, { callId: 'b' }),
          text(4, 1, 'after'),
        ]}
      />,
    );
    expect(screen.getByText('before')).toBeInTheDocument();
    expect(screen.getByText('after')).toBeInTheDocument();
    expect(screen.getByText('2/2')).toBeInTheDocument();
  });
});

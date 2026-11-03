import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { MessageActions } from './MessageActions';
import type { Message } from '$types';
import { saveMessageReply } from '$lib/commands/message';

vi.mock('$lib/commands/message', () => ({ saveMessageReply: vi.fn() }));

function message(role: Message['role']): Message {
  return {
    id: `${role}-message`,
    sessionId: 'session-1',
    role,
    content: 'test message',
    createdAt: 0,
  };
}

describe('MessageActions', () => {
  it('user message shows edit and delete buttons', () => {
    render(<MessageActions message={message('user')} />);

    expect(screen.getByText('编辑')).toBeInTheDocument();
    expect(screen.getByText('删除')).toBeInTheDocument();
    expect(screen.queryByText('保存回复…')).not.toBeInTheDocument();
  });

  it('assistant message shows copy button and explicitly saves only its own durable reply', async () => {
    const user = userEvent.setup();
    const save = vi.mocked(saveMessageReply);
    const onCopy = vi.fn();
    const reply = message('assistant');
    const { rerender } = render(<MessageActions message={reply} onCopy={onCopy} />);

    expect(screen.getByText('复制')).toBeInTheDocument();
    expect(save).not.toHaveBeenCalled();
    save.mockResolvedValueOnce({ status: 'cancelled' });
    await user.click(screen.getByText('保存回复…'));
    expect(save).toHaveBeenLastCalledWith(reply.sessionId, reply.id);
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();

    let finish: (result: Awaited<ReturnType<typeof saveMessageReply>>) => void = () => {};
    save.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    await user.click(screen.getByText('保存回复…'));
    expect(screen.getByText('正在保存…').closest('button')).toBeDisabled();
    await user.click(screen.getByText('正在保存…'));
    expect(save).toHaveBeenCalledTimes(2);
    await act(async () => finish({ status: 'saved', name: '清单.md', bytes: 12 }));
    expect(screen.getByRole('status')).toHaveTextContent('已保存回复快照：清单.md');
    expect(onCopy).not.toHaveBeenCalled();

    save.mockRejectedValueOnce(new Error('目标文件已存在，未覆盖任何内容；请换一个文件名。'));
    await user.click(screen.getByText('保存回复…'));
    expect(screen.getByRole('alert')).toHaveTextContent('未覆盖任何内容');
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    save.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    await user.click(screen.getByText('保存回复…'));
    rerender(<MessageActions message={{ ...reply, id: 'other-message', sessionId: 'other-session', content: 'other reply' }} />);
    await act(async () => finish({ status: 'saved', name: 'OLD_PRIVATE_RESULT.md', bytes: 12 }));
    expect(screen.queryByText(/OLD_PRIVATE_RESULT/)).not.toBeInTheDocument();
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByText('保存回复…').closest('button')).toBeEnabled());
    rerender(<MessageActions message={reply} disabled />);
    await user.click(screen.getByText('保存回复…'));
    expect(save).toHaveBeenCalledTimes(4);
    rerender(<MessageActions message={reply} isStreaming />);
    expect(screen.queryByText('保存回复…')).not.toBeInTheDocument();
    expect(screen.getByText('复制')).toBeVisible();
  });

  it('uses a labeled editor with cancel and resend actions for user messages', async () => {
    const onEdit = vi.fn();
    const user = userEvent.setup();
    const original = message('user');
    const { rerender } = render(<MessageActions message={original} onEdit={onEdit} />);

    await user.click(screen.getByText('编辑'));
    const editor = screen.getByLabelText('编辑后的消息内容');
    expect(screen.getByRole('button', { name: '取消' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '重新发送' })).toBeInTheDocument();

    await user.clear(editor);
    await user.type(editor, 'updated message');
    rerender(<MessageActions message={original} onEdit={onEdit} disabled />);
    expect(editor).toBeDisabled();
    expect(screen.getByRole('button', { name: '重新发送' })).toBeDisabled();
    expect(screen.getByText('当前操作处理中，暂不能重新发送')).toBeVisible();
    expect(screen.getByRole('button', { name: '取消' })).toBeEnabled();
    await user.click(screen.getByRole('button', { name: '重新发送' }));
    await user.keyboard('{Control>}{Enter}{/Control}');
    expect(onEdit).not.toHaveBeenCalled();
    expect(editor).toHaveValue('updated message');

    rerender(<MessageActions message={original} onEdit={onEdit} />);
    await user.click(editor);
    await user.keyboard('{Control>}{Enter}{/Control}');

    expect(onEdit).toHaveBeenCalledWith('user-message', 'updated message');
    await user.click(screen.getByText('编辑'));
    await user.type(screen.getByLabelText('编辑后的消息内容'), ' old draft');
    const other = { ...original, id: 'another-message', sessionId: 'another-session', content: 'another message' };
    rerender(<MessageActions message={other} onEdit={onEdit} />);
    expect(screen.queryByLabelText('编辑后的消息内容')).not.toBeInTheDocument();
    await user.click(screen.getByText('编辑'));
    expect(screen.getByLabelText('编辑后的消息内容')).toHaveValue('another message');
    await user.click(screen.getByRole('button', { name: '取消' }));
    expect(onEdit).toHaveBeenCalledTimes(1);
    rerender(<MessageActions message={other} onEdit={onEdit} recoveredEditContent="Retained failed edit" />);
    expect(screen.getByLabelText('编辑后的消息内容')).toHaveValue('Retained failed edit');
    await user.click(screen.getByRole('button', { name: '取消' }));
    rerender(<MessageActions message={other} onEdit={onEdit} recoveredEditContent="Retained failed edit" />);
    expect(screen.queryByLabelText('编辑后的消息内容')).not.toBeInTheDocument();
    await user.click(screen.getByText('编辑'));
    expect(screen.getByLabelText('编辑后的消息内容')).toHaveValue('another message');
  });

  it('confirms only completed clipboard writes and discards stale copy feedback', async () => {
    vi.useFakeTimers();
    userEvent.setup(); // Install the same deterministic clipboard as other UI tests.
    const clipboard = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
    const write = vi.spyOn(navigator.clipboard, 'writeText');
    const onCopy = vi.fn();
    const original = message('assistant');
    const { rerender, unmount } = render(<MessageActions message={original} onCopy={onCopy} />);
    try {
      let finish = () => {};
      write.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
      const button = screen.getByRole('button', { name: '复制' });
      fireEvent.mouseDown(button);
      expect(screen.queryByText('已复制')).not.toBeInTheDocument();
      fireEvent.mouseUp(button);
      fireEvent.click(button);
      expect(screen.getByRole('button', { name: '正在复制…' })).toBeDisabled();
      fireEvent.click(screen.getByRole('button', { name: '正在复制…' }));
      expect(write).toHaveBeenCalledTimes(1);
      expect(write).toHaveBeenLastCalledWith(original.content);
      expect(onCopy).not.toHaveBeenCalled();
      await act(async () => finish());
      expect(screen.getByRole('button', { name: '已复制' })).toBeVisible();
      expect(onCopy).toHaveBeenCalledTimes(1);
      expect(onCopy).toHaveBeenLastCalledWith(original.id);

      await act(async () => vi.advanceTimersByTime(900));
      const changed = { ...original, content: 'new streaming content' };
      rerender(<MessageActions message={changed} onCopy={onCopy} isStreaming />);
      expect(screen.queryByText('已复制')).not.toBeInTheDocument();
      write.mockResolvedValueOnce();
      await act(async () => fireEvent.click(screen.getByRole('button', { name: '复制' })));
      expect(write).toHaveBeenLastCalledWith(changed.content);
      await act(async () => vi.advanceTimersByTime(900));
      expect(screen.getByRole('button', { name: '已复制' })).toBeVisible();
      await act(async () => vi.advanceTimersByTime(900));
      expect(screen.getByRole('button', { name: '复制' })).toBeVisible();

      write.mockRejectedValueOnce(new Error('PRIVATE_CLIPBOARD_DIAGNOSTIC'));
      await act(async () => fireEvent.click(screen.getByRole('button', { name: '复制' })));
      expect(screen.getByRole('alert')).toHaveTextContent('无法复制');
      expect(screen.queryByText(/PRIVATE_CLIPBOARD_DIAGNOSTIC/)).not.toBeInTheDocument();
      expect(onCopy).toHaveBeenCalledTimes(2);
      write.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
      fireEvent.click(screen.getByRole('button', { name: '复制' }));
      const other = { ...original, id: 'other-message', sessionId: 'other-session' };
      rerender(<MessageActions message={other} onCopy={onCopy} />);
      await act(async () => finish());
      expect(screen.queryByText('已复制')).not.toBeInTheDocument();
      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
      expect(onCopy).toHaveBeenCalledTimes(2);

      Object.defineProperty(navigator, 'clipboard', { configurable: true, value: undefined });
      await act(async () => fireEvent.click(screen.getByRole('button', { name: '复制' })));
      expect(screen.getByRole('alert')).toHaveTextContent('无法复制');
      expect(onCopy).toHaveBeenCalledTimes(2);
      rerender(<MessageActions message={other} onCopy={onCopy} disabled />);
      expect(screen.getByRole('button', { name: '复制' })).toBeDisabled();
      rerender(<MessageActions message={{ ...other, content: ' \n' }} onCopy={onCopy} isStreaming />);
      expect(screen.getByRole('button', { name: '复制' })).toBeDisabled();
      fireEvent.click(screen.getByRole('button', { name: '复制' }));
      expect(onCopy).toHaveBeenCalledTimes(2);
      if (clipboard) Object.defineProperty(navigator, 'clipboard', clipboard);
      rerender(<MessageActions message={other} onCopy={onCopy} />);
      write.mockResolvedValueOnce();
      await act(async () => fireEvent.click(screen.getByRole('button', { name: '复制' })));
      expect(vi.getTimerCount()).toBe(1);
      unmount();
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      unmount();
      write.mockRestore();
      if (clipboard) Object.defineProperty(navigator, 'clipboard', clipboard);
      else Reflect.deleteProperty(navigator, 'clipboard');
      vi.useRealTimers();
    }
  });
});

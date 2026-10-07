import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { MessageActions } from './MessageActions';
import type { Message } from '$types';

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
  });

  it('assistant message shows copy button', () => {
    render(<MessageActions message={message('assistant')} />);

    expect(screen.getByText('复制')).toBeInTheDocument();
  });

  it('uses a labeled editor with cancel and resend actions for user messages', async () => {
    const onEdit = vi.fn();
    const user = userEvent.setup();
    render(<MessageActions message={message('user')} onEdit={onEdit} />);

    await user.click(screen.getByText('编辑'));
    const editor = screen.getByLabelText('编辑后的消息内容');
    expect(screen.getByRole('button', { name: '取消' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '重新发送' })).toBeInTheDocument();

    await user.clear(editor);
    await user.type(editor, 'updated message');
    await user.keyboard('{Control>}{Enter}{/Control}');

    expect(onEdit).toHaveBeenCalledWith('user-message', 'updated message');
  });
});

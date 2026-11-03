import { createRef } from 'react';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import type { Message } from '$types';
import { ConversationNavigation, conversationIndex } from './ConversationNavigation';

const message = (id: string, role: Message['role'], content: string, sessionId = 'main'): Message => ({ id, role, content, sessionId, createdAt: 1 });

it('indexes only the newest 50 visible user turns, keeping their final text preview without tool rows or mutations', () => {
  const messages = Array.from({ length: 65 }, (_, index) => [
    message(`user-${index}`, 'user', `## 提问 ${index}`),
    message(`tool-${index}`, 'tool', '{"internal_tool_result":true}'),
    message(`reply-${index}`, 'assistant', `回答 ${index}`),
  ]).flat();
  messages.push(message('other', 'user', '另一个项目', 'other'));
  messages.push(message('last-reply', 'assistant', '最终回答 64'));
  const original = structuredClone(messages);
  const entries = conversationIndex(messages, 'main');
  expect(entries).toHaveLength(50);
  expect(entries[0]).toMatchObject({ id: 'user-15', title: '提问 15', reply: '回答 15' });
  expect(entries[entries.length - 1]).toMatchObject({ id: 'user-64', reply: '最终回答 64' });
  expect(JSON.stringify(entries)).not.toContain('internal_tool_result');
  expect(JSON.stringify(entries)).not.toContain('另一个项目');
  expect(messages).toEqual(original);
  expect(conversationIndex(messages)).toEqual([]);
  expect(conversationIndex([
    { ...message('file', 'user', ''), textAttachments: [{ name: 'notes.txt', text: '不复制附件正文' }] },
    { ...message('timeline', 'assistant', ''), timelineBlocks: [{ kind: 'text', index: 0, iterationId: 0, content: '来自已展示正文的结果' }] },
  ], 'main')).toMatchObject([{ title: '附件：notes.txt', reply: '来自已展示正文的结果' }]);
});

it('previews via hover and keyboard, jumps locally, and returns to the bottom without switching a branch', async () => {
  const messages = [message('one', 'user', '第一项要求'), message('answer', 'assistant', '已完成的第一项结果'), message('two', 'user', '第二项要求')];
  const containerRef = createRef<HTMLDivElement>();
  const leave = vi.fn();
  const latest = vi.fn(() => { if (containerRef.current) containerRef.current.scrollTop = containerRef.current.scrollHeight; });
  const user = userEvent.setup();
  const { container } = render(<div><div ref={containerRef}>{messages.map((entry, index) => <div key={entry.id} data-conversation-message-id={entry.id} data-row={index}>{entry.content}</div>)}</div><ConversationNavigation messages={messages} sessionId="main" containerRef={containerRef} onNavigateAway={leave} onReturnToLatest={latest} /></div>);
  const transcript = containerRef.current!;
  Object.defineProperties(transcript, { scrollHeight: { configurable: true, value: 1600 }, clientHeight: { configurable: true, value: 400 } });
  vi.spyOn(transcript, 'getBoundingClientRect').mockImplementation(() => new DOMRect(0, 100, 600, 400));
  container.querySelectorAll<HTMLElement>('[data-row]').forEach((row, index) => {
    vi.spyOn(row, 'getBoundingClientRect').mockImplementation(() => new DOMRect(0, 100 + index * 600 - transcript.scrollTop, 600, 400));
  });
  fireEvent.scroll(transcript);
  expect(await screen.findByRole('button', { name: '回到最底部' })).toBeVisible();
  const first = screen.getByRole('button', { name: '定位第 1 条：第一项要求' });
  await user.hover(first);
  expect(within(screen.getByRole('tooltip')).getByText('已完成的第一项结果')).toBeVisible();
  await user.click(first);
  expect(leave).toHaveBeenCalledOnce();
  expect(latest).not.toHaveBeenCalled();
  expect(transcript.scrollTop).toBe(0);
  await user.keyboard('{End}');
  const second = screen.getByRole('button', { name: '定位第 2 条：第二项要求' });
  expect(second).toHaveFocus();
  expect(within(screen.getByRole('tooltip')).getByText('暂无文字回复')).toBeVisible();
  await user.keyboard('{Enter}');
  expect(transcript.scrollTop).toBe(1180);
  expect(second).toHaveAttribute('aria-current', 'location');
  expect(screen.queryByRole('button', { name: '回到最底部' })).not.toBeInTheDocument();
  transcript.scrollTop = 0;
  fireEvent.scroll(transcript);
  await user.click(await screen.findByRole('button', { name: '回到最底部' }));
  expect(latest).toHaveBeenCalledOnce();
  expect(transcript.scrollTop).toBe(1600);
  await waitFor(() => expect(screen.queryByRole('button', { name: '回到最底部' })).not.toBeInTheDocument());
});

it('keeps a single keyboard entry when the oldest indexed turn falls outside the limit', () => {
  const containerRef = createRef<HTMLDivElement>();
  const messages = Array.from({ length: 51 }, (_, index) => message(String(index), 'user', `要求 ${index}`));
  const { rerender } = render(<ConversationNavigation messages={messages.slice(0, 50)} sessionId="main" containerRef={containerRef} onNavigateAway={() => {}} onReturnToLatest={() => {}} />);
  fireEvent.focus(screen.getByRole('button', { name: '定位第 1 条：要求 0' }));
  rerender(<ConversationNavigation messages={messages} sessionId="main" containerRef={containerRef} onNavigateAway={() => {}} onReturnToLatest={() => {}} />);
  const buttons = within(screen.getByRole('navigation', { name: '对话定位' })).getAllByRole('button');
  expect(buttons).toHaveLength(50);
  expect(buttons.filter(button => button.tabIndex === 0)).toHaveLength(1);
  expect(screen.queryByRole('button', { name: '定位第 1 条：要求 0' })).not.toBeInTheDocument();
});

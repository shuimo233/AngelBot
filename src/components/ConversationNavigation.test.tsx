import { createRef } from 'react';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import type { Message } from '$types';
import type { ConversationSearchHit, ConversationSearchPage, searchConversationPage } from '$lib/commands/message';
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
  const jump = vi.fn((id: string) => {
    if (containerRef.current) containerRef.current.scrollTop = id === 'one' ? 0 : 1180;
    return true;
  });
  const latest = vi.fn(() => { if (containerRef.current) containerRef.current.scrollTop = containerRef.current.scrollHeight; });
  const user = userEvent.setup();
  const view = (showReturnToLatest: boolean) => <div>
    <div ref={containerRef}>{messages.map((entry, index) => <div key={entry.id} data-conversation-message-id={entry.id} data-row={index}>{entry.content}</div>)}</div>
    <ConversationNavigation messages={messages} renderedMessages={messages} sessionId="main" containerRef={containerRef}
      onJumpToMessage={jump} onReturnToLatest={latest} showReturnToLatest={showReturnToLatest} />
  </div>;
  const { container, rerender } = render(view(false));
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
  expect(jump.mock.calls).toEqual([['one', false]]);
  expect(latest).not.toHaveBeenCalled();
  expect(transcript.scrollTop).toBe(0);
  await user.keyboard('{End}');
  const second = screen.getByRole('button', { name: '定位第 2 条：第二项要求' });
  expect(second).toHaveFocus();
  expect(within(screen.getByRole('tooltip')).getByText('暂无文字回复')).toBeVisible();
  await user.keyboard('{Enter}');
  expect(jump).toHaveBeenLastCalledWith('two', false);
  expect(transcript.scrollTop).toBe(1180);
  expect(second).toHaveAttribute('aria-current', 'location');
  expect(screen.queryByRole('button', { name: '回到最底部' })).not.toBeInTheDocument();
  rerender(view(true));
  await user.click(screen.getByRole('button', { name: '回到最底部' }));
  expect(latest).toHaveBeenCalledOnce();
  expect(transcript.scrollTop).toBe(1600);
  rerender(view(false));
  await waitFor(() => expect(screen.queryByRole('button', { name: '回到最底部' })).not.toBeInTheDocument());
});

it('shows the original short turn with only its own task or delivered file hint', async () => {
  const task = (goal: string, status = 'completed') => ({ id: goal, goal, status, plan: [], confirmationState: 'none', resumable: false, stepCount: 1, completedStepCount: 1 });
  const oldGoal = '清理旧回合票据';
  const write = { id: 'put', name: 'write_file', arguments: '{"path":"docs/周末清单.md"}' };
  const messages = [
    message('old', 'user', '请整理旧项目的所有票据'),
    { ...message('old-result', 'assistant', '旧任务完成。'), parentId: 'old', taskRun: task(oldGoal) },
    message('yes', 'user', '可以。'),
    { ...message('late-old', 'assistant', '晚到的旧结果'), parentId: 'old-result', taskRun: task(oldGoal) },
    message('continue', 'user', '继续'),
    message('optimize', 'user', '那就优化'),
    { ...message('approval', 'assistant', ''), parentId: 'optimize', toolCalls: [write], taskRun: task('那就优化', 'awaiting_confirmation') },
    { ...message('done', 'assistant', '整理好了。'), parentId: 'approval', taskRun: task('那就优化'),
      toolResults: [{ toolName: write.name, callId: write.id, success: true, output: '', confirmationStatus: 'approved' }] },
    message('next', 'user', '继续'),
    { ...message('next-reply', 'assistant', '正在处理这一轮。'), parentId: 'next', taskRun: task('核对模板字段', 'running') },
    message('no-evidence', 'user', '同意'),
    { ...message('guess', 'assistant', '也许是在修改[配置](angelbot-file:conf.json)。'), parentId: 'no-evidence', taskFacts: { goal: '同意', modifiedFiles: ['conf.json'] } },
    message('foreign', 'user', '继续', 'other'),
    { ...message('foreign-reply', 'assistant', '属于另一会话。', 'other'), parentId: 'foreign', taskRun: task('只属于另一会话的目标') },
  ];
  const jump = vi.fn().mockReturnValue(true);
  const user = userEvent.setup();
  const containerRef = createRef<HTMLDivElement>();
  const view = (sessionId: string) => <>
    <div ref={containerRef} />
    <ConversationNavigation key={sessionId} messages={messages} renderedMessages={messages} sessionId={sessionId} containerRef={containerRef}
      onJumpToMessage={jump} onReturnToLatest={() => {}} showReturnToLatest={false} />
  </>;
  const { rerender } = render(view('main'));
  expect(screen.getByRole('button', { name: '定位第 1 条：请整理旧项目的所有票据' })).toBeVisible();
  expect(screen.getByRole('button', { name: '定位第 2 条：可以。' })).toBeVisible();
  expect(screen.getByRole('button', { name: '定位第 3 条：继续' })).toBeVisible();
  expect(screen.getByRole('button', { name: '定位第 6 条：同意' })).toBeVisible();
  const file = screen.getByRole('button', { name: '定位第 4 条：那就优化 · 周末清单.md' });
  await user.hover(file);
  expect(screen.getByRole('tooltip')).toHaveTextContent('那就优化 · 周末清单.md');
  await user.click(file);
  expect(jump).toHaveBeenLastCalledWith('optimize', false);
  fireEvent.focus(screen.getByRole('button', { name: '定位第 5 条：继续 · 核对模板字段' }));
  expect(screen.getByRole('tooltip')).toHaveTextContent('继续 · 核对模板字段');

  await user.click(screen.getByRole('button', { name: '搜索当前对话' }));
  const search = screen.getByRole('searchbox');
  await user.type(search, '可以。');
  expect(screen.getByRole('status')).toHaveTextContent('1 条匹配');
  await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: /可以。/ }));
  expect(jump).toHaveBeenLastCalledWith('yes', true);

  rerender(view('other'));
  expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /周末清单/ })).not.toBeInTheDocument();
  const foreign = screen.getByRole('button', { name: '定位第 1 条：继续 · 只属于另一会话的目标' });
  fireEvent.focus(foreign);
  expect(screen.getByRole('tooltip')).toHaveTextContent('继续 · 只属于另一会话的目标');
});

it('keeps a single keyboard entry when the oldest indexed turn falls outside the limit', () => {
  const containerRef = createRef<HTMLDivElement>();
  const messages = Array.from({ length: 51 }, (_, index) => message(String(index), 'user', `要求 ${index}`));
  const view = (entries: Message[]) => <ConversationNavigation messages={entries} renderedMessages={entries} sessionId="main" containerRef={containerRef}
    onJumpToMessage={() => false} onReturnToLatest={() => {}} showReturnToLatest={false} />;
  const { rerender } = render(view(messages.slice(0, 50)));
  fireEvent.focus(screen.getByRole('button', { name: '定位第 1 条：要求 0' }));
  rerender(view(messages));
  const buttons = within(screen.getByRole('navigation', { name: '对话定位' })).getAllByRole('button');
  expect(buttons).toHaveLength(50);
  expect(buttons.filter(button => button.tabIndex === 0)).toHaveLength(1);
  expect(screen.queryByRole('button', { name: '定位第 1 条：要求 0' })).not.toBeInTheDocument();
});

it('shows one search result per exchange and jumps to the matching message', async () => {
  const messages = [
    message('question', 'user', '请查项目进度'),
    message('first-reply', 'assistant', '项目进度已更新，详见执行记录'),
    message('next-question', 'user', '附件在哪'),
    message('next-reply', 'assistant', '项目进度表已保存到资料库'),
  ];
  const jump = vi.fn().mockReturnValue(true);
  const user = userEvent.setup();
  render(<ConversationNavigation messages={messages} renderedMessages={messages} sessionId="main"
    containerRef={createRef<HTMLDivElement>()} onJumpToMessage={jump}
    onReturnToLatest={() => {}} showReturnToLatest={false} />);

  const trigger = screen.getByRole('button', { name: '搜索当前对话' });
  await user.click(trigger);
  let dialog = screen.getByRole('dialog');
  await waitFor(() => expect(within(dialog).getByRole('searchbox')).toHaveFocus());
  await user.paste('项目进度');
  const [newest, earlier] = within(dialog).getAllByRole('listitem');
  expect(within(dialog).getByRole('status')).toHaveTextContent('2 条匹配');
  expect(newest).toHaveTextContent('项目进度表已保存到资料库');
  await user.click(within(earlier).getByRole('button'));
  expect(jump).toHaveBeenLastCalledWith('question', true);

  await user.click(trigger);
  dialog = screen.getByRole('dialog');
  await waitFor(() => expect(within(dialog).getByRole('searchbox')).toHaveFocus());
  await user.paste('执行记录');
  await user.click(within(dialog).getByRole('button', { name: /执行记录/ }));
  expect(jump).toHaveBeenLastCalledWith('first-reply', true);
});

it('searches saved messages outside the mounted rows and delegates the exact jump by keyboard', async () => {
  const user = userEvent.setup();
  const containerRef = createRef<HTMLDivElement>();
  const jump = vi.fn().mockReturnValueOnce(false).mockReturnValue(true);
  const early = {
    ...message('early', 'assistant', '这段旧回复提到了旧计划。'),
    timelineBlocks: [{ kind: 'text' as const, index: 0, iterationId: 0,
      content: '收据核对结果 [查看旧文件](angelbot-file:archive%2Fold-plan.md) [无效](angelbot-file:..%2Fout.md)' }],
    taskFacts: { goal: '旧计划', modifiedFiles: ['reports/old-check.txt'] },
  };
  const messages = [
    { ...message('early-user', 'user', '最早的消息'), textAttachments: [{ name: '旧附件 [原始].txt', text: '附件秘密' }] },
    early,
    ...Array.from({ length: 60 }, (_, index) => message(String(index), 'user', `新消息 ${index}`)),
    message('tool', 'tool', 'old-check.txt'),
    message('foreign', 'user', 'old-check.txt', 'other'),
  ];
  const renderedMessages = messages.slice(2, 8);
  render(<div>
    <div ref={containerRef}>{renderedMessages.map(entry =>
      <div key={entry.id} data-conversation-message-id={entry.id}>{entry.content}</div>)}</div>
    <ConversationNavigation messages={messages} renderedMessages={renderedMessages} sessionId="main" containerRef={containerRef}
      onJumpToMessage={jump} onReturnToLatest={() => {}} showReturnToLatest={false} />
  </div>);
  expect(screen.queryByText(early.content)).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /定位.*最早的消息/ })).not.toBeInTheDocument();
  const trigger = screen.getByRole('button', { name: '搜索当前对话' });
  await user.click(trigger);
  const dialog = screen.getByRole('dialog', { name: '搜索当前对话' });
  const input = within(dialog).getByRole('searchbox', { name: '关键词或文件名' });
  await waitFor(() => expect(input).toHaveFocus());
  await user.type(input, '附件秘密');
  expect(within(dialog).getByRole('status')).toHaveTextContent('没有匹配');
  await user.clear(input);
  await user.paste('旧附件 [原始].TXT');
  expect(within(dialog).getByRole('button', { name: /旧附件 \[原始\]\.txt/ })).toBeVisible();
  await user.clear(input);
  await user.type(input, '新消息');
  expect(within(dialog).getByRole('status')).toHaveTextContent('显示最近 20 条匹配');
  expect(within(dialog).getAllByRole('listitem')).toHaveLength(20);
  await user.clear(input);
  await user.type(input, '收据核对结果');
  expect(within(dialog).getByRole('button', { name: /收据核对结果/ })).toBeVisible();
  await user.clear(input);
  await user.type(input, '../out.md');
  expect(within(dialog).getByRole('status')).toHaveTextContent('没有匹配');
  await user.clear(input);
  await user.type(input, 'archive/old-plan.md');
  expect(within(dialog).getByRole('button', { name: /archive\/old-plan\.md/ })).toBeVisible();
  await user.clear(input);
  await user.type(input, 'OLD-CHECK.TXT');
  expect(within(dialog).getByRole('status')).toHaveTextContent('1 条匹配');
  expect(within(dialog).getByRole('button', { name: /reports\/old-check.txt/ })).toBeVisible();
  await user.keyboard('{Enter}');
  expect(jump.mock.calls).toEqual([['early', true]]);
  expect(screen.getByRole('dialog')).toBeVisible();
  await user.keyboard('{Enter}');
  expect(jump).toHaveBeenLastCalledWith('early', true);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await user.click(trigger);
  expect(within(screen.getByRole('dialog')).getByRole('searchbox')).toHaveValue('');
  await user.keyboard('{Escape}');
  expect(trigger).toHaveFocus();
});

it('appends server search hits in keyboard-accessible batches and jumps to the exact older reply', async () => {
  const user = userEvent.setup();
  const matches = Array.from({ length: 45 }, (_, index): ConversationSearchHit => {
    const number = String(45 - index).padStart(4, '0');
    const answer = number === '0001';
    return { id: `${answer ? 'a' : 'q'}-${number}`, role: answer ? 'assistant' : 'user',
      text: `归档案例${answer ? '结果' : '请求'} ${number}`, createdAt: 1 };
  });
  const loadSearchPage = vi.fn<typeof searchConversationPage>().mockImplementation(async ({ beforeId }) => {
    const from = beforeId ? matches.findIndex(hit => hit.id === beforeId) + 1 : 0;
    const results = matches.slice(from, from + 20);
    return { results, nextBeforeId: from + results.length < matches.length ? results.at(-1)!.id : null };
  });
  const jump = vi.fn().mockResolvedValue(true);
  const latest = [message('recent', 'user', '尚无归档案例的正文，这行不参与远程排序')];
  render(<ConversationNavigation messages={latest} renderedMessages={latest} sessionId="main"
    containerRef={createRef<HTMLDivElement>()} loadSearchPage={loadSearchPage}
    onJumpToMessage={jump} onReturnToLatest={() => {}} showReturnToLatest={false} />);

  const trigger = screen.getByRole('button', { name: '搜索当前对话' });
  await user.click(trigger);
  let dialog = within(screen.getByRole('dialog'));
  let input = dialog.getByRole('searchbox');
  expect(loadSearchPage).not.toHaveBeenCalled();
  await user.type(input, '归档');
  await waitFor(() => expect(dialog.getByRole('status')).toHaveTextContent('显示最近 20 条匹配'));
  expect(dialog.getAllByRole('listitem')).toHaveLength(20);
  expect(dialog.getByRole('button', { name: /归档案例请求 0045/ })).toBeVisible();
  expect(dialog.queryByRole('button', { name: /归档案例请求 0025/ })).not.toBeInTheDocument();
  expect(loadSearchPage.mock.calls).toEqual([[{ sessionId: 'main', query: '归档' }]]);

  const more = () => dialog.getByRole('button', { name: '查看更多匹配' });
  more().focus();
  await user.keyboard('{Enter}');
  await waitFor(() => expect(dialog.getAllByRole('listitem')).toHaveLength(40));
  await waitFor(() => expect(dialog.getByRole('button', { name: /归档案例请求 0025/ })).toHaveFocus());
  expect(dialog.getByRole('status')).toHaveTextContent('显示最近 40 条匹配');
  expect(loadSearchPage).toHaveBeenLastCalledWith({ sessionId: 'main', query: '归档', beforeId: 'q-0026' });

  await user.clear(input);
  await user.type(input, '归档案例');
  await waitFor(() => expect(dialog.getAllByRole('listitem')).toHaveLength(20));
  expect(loadSearchPage).toHaveBeenLastCalledWith({ sessionId: 'main', query: '归档案例' });
  await user.click(more());
  await waitFor(() => expect(more()).toBeEnabled());
  await user.click(more());
  await waitFor(() => expect(dialog.getAllByRole('listitem')).toHaveLength(45));
  expect(dialog.getByRole('status')).toHaveTextContent('45 条匹配');
  expect(dialog.queryByRole('button', { name: '查看更多匹配' })).not.toBeInTheDocument();
  expect(dialog.getByRole('button', { name: /归档案例请求 0005/ })).toHaveFocus();

  await user.keyboard('{Escape}');
  await user.click(trigger);
  dialog = within(screen.getByRole('dialog'));
  input = dialog.getByRole('searchbox');
  expect(input).toHaveValue('');
  await user.type(input, '归档案例');
  await waitFor(() => expect(dialog.getAllByRole('listitem')).toHaveLength(20));
  await user.click(more());
  await waitFor(() => expect(more()).toBeEnabled());
  await user.click(more());
  (await dialog.findByRole('button', { name: /归档案例结果 0001/ })).focus();
  await user.keyboard('{Enter}');
  expect(jump.mock.calls).toEqual([['a-0001', true]]);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

it('ignores superseded keywords and in-flight older pages across a session switch', async () => {
  const user = userEvent.setup();
  const replies: ((page: ConversationSearchPage) => void)[] = [];
  const loadSearchPage = vi.fn<typeof searchConversationPage>().mockImplementation(() => new Promise(resolve => replies.push(resolve)));
  const hit = (id: string, text: string, nextBeforeId: string | null = null): ConversationSearchPage => ({
    results: [{ id, text, role: 'assistant', createdAt: 1 }], nextBeforeId,
  });
  const containerRef = createRef<HTMLDivElement>();
  const view = (sessionId: string) => <ConversationNavigation messages={[]} renderedMessages={[]} sessionId={sessionId}
    containerRef={containerRef} loadSearchPage={loadSearchPage} onJumpToMessage={() => true}
    onReturnToLatest={() => {}} showReturnToLatest={false} />;
  const { rerender } = render(view('main'));
  await user.click(screen.getByRole('button', { name: '搜索当前对话' }));
  const dialog = within(screen.getByRole('dialog'));
  const input = dialog.getByRole('searchbox');
  fireEvent.change(input, { target: { value: '旧词' } });
  await waitFor(() => expect(loadSearchPage).toHaveBeenCalledWith({ sessionId: 'main', query: '旧词' }));
  fireEvent.change(input, { target: { value: '新词' } });
  await waitFor(() => expect(loadSearchPage).toHaveBeenCalledWith({ sessionId: 'main', query: '新词' }));
  await act(async () => replies[1](hit('fresh', '新词的匹配', 'cursor-main')));
  await act(async () => replies[0](hit('late', '迟到的旧词匹配')));
  expect(dialog.getByRole('button', { name: /新词的匹配/ })).toBeVisible();
  expect(dialog.queryByRole('button', { name: /迟到的旧词匹配/ })).not.toBeInTheDocument();
  await user.click(dialog.getByRole('button', { name: '查看更多匹配' }));
  expect(loadSearchPage).toHaveBeenLastCalledWith({ sessionId: 'main', query: '新词', beforeId: 'cursor-main' });

  rerender(view('other'));
  await waitFor(() => expect(loadSearchPage).toHaveBeenLastCalledWith({ sessionId: 'other', query: '新词' }));
  await act(async () => replies[3](hit('other', '另一个项目的匹配')));
  await act(async () => replies[2](hit('older-main', '上一个项目的匹配')));
  expect(dialog.getAllByRole('listitem')).toHaveLength(1);
  expect(dialog.getByRole('button', { name: /另一个项目的匹配/ })).toBeVisible();
  expect(dialog.queryByRole('button', { name: /上一个项目的匹配/ })).not.toBeInTheDocument();
});

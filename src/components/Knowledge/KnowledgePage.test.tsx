import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { KnowledgePage } from './KnowledgePage';
import { useKnowledgeStore } from '$stores/knowledge';
import { deleteMemory, getMemories, saveMemory, updateMemory, type RustMemory } from '$lib/commands/memory';

vi.mock('$lib/commands/memory', () => ({ getMemories: vi.fn(), saveMemory: vi.fn(), updateMemory: vi.fn(), deleteMemory: vi.fn() }));
const entry: RustMemory = { id: 'one', content: '# 旅行清单\n\n**先核对日期**\n\n![外部图片](https://example.com/private.png)', scope: 'project-one', category: 'knowledge', importance: 4, source: 'user', is_permanent: true, frequency: 0, last_mentioned: null, embedding: null, decay_factor: 1, forget_stage: 'active', created_at: 1, updated_at: 5 };
const other: RustMemory = { ...entry, id: 'two', content: '# 常用地址\n\n仅工作日配送', scope: 'global', source: 'knowledge', updated_at: 2 };
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getMemories).mockResolvedValue([entry, other]);
  vi.mocked(saveMemory).mockResolvedValue(undefined);
  vi.mocked(updateMemory).mockResolvedValue(undefined);
  vi.mocked(deleteMemory).mockResolvedValue(undefined);
  useKnowledgeStore.setState({ entries: [], isLoading: false, error: null, query: '' });
});

it('browses compact summaries, filters trimmed text, and previews Markdown without remote images', async () => {
  const user = userEvent.setup();
  render(<KnowledgePage />);
  const detail = await screen.findByRole('article', { name: '资料详情' });
  expect(within(detail).getByText('先核对日期')).toHaveProperty('tagName', 'STRONG');
  expect(detail.querySelector('img')).toBeNull();
  await user.click(within(detail).getByText('资料信息'));
  expect(within(detail).getByText(/限定范围资料/)).toBeVisible();
  await user.type(screen.getByRole('searchbox', { name: '搜索资料' }), ' 配送 ');
  expect(screen.getByRole('article', { name: '资料详情' })).toHaveTextContent('常用地址');
  await user.click(screen.getByRole('button', { name: '筛选与排序' }));
  await user.selectOptions(screen.getByLabelText('资料来源'), 'user');
  expect(screen.getByText(/没有匹配资料/)).toBeInTheDocument();
});

it('edits the exact record while preserving scope and flags, and retains failed edits', async () => {
  const user = userEvent.setup();
  vi.mocked(updateMemory).mockRejectedValueOnce(new Error('写入失败'));
  render(<KnowledgePage />);
  await user.click(await screen.findByRole('button', { name: '编辑' }));
  const field = screen.getByLabelText('编辑资料内容');
  await user.clear(field); await user.type(field, '# 更正后的日期');
  // A concurrent catalogue reload must never redirect the edit to a new first row.
  act(() => useKnowledgeStore.setState(state => ({ entries: state.entries.filter(item => item.id === 'two') })));
  await user.click(screen.getByRole('button', { name: '保存修改' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('写入失败');
  expect(field).toHaveValue('# 更正后的日期');
  expect(updateMemory).toHaveBeenCalledWith({ id: 'one', scope: 'project-one', category: 'knowledge', importance: 4, isPermanent: true, content: '# 更正后的日期' });
  expect(saveMemory).not.toHaveBeenCalled();
});

it('imports a bounded text snapshot into a draft and only saves after confirmation', async () => {
  const user = userEvent.setup();
  render(<KnowledgePage />);
  await screen.findByRole('article', { name: '资料详情' });
  await user.click(screen.getByRole('button', { name: '添加资料' }));
  await user.upload(screen.getByLabelText('导入资料文件'), new File(['hello notes'], 'notes.md', { type: 'text/markdown' }));
  const draft = await screen.findByRole('textbox', { name: '资料内容' });
  await waitFor(() => expect(draft).toHaveValue('# notes.md\n\nhello notes'));
  expect(saveMemory).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: '收起添加' }));
  await user.click(screen.getByRole('button', { name: '添加资料' }));
  expect(screen.getByLabelText('资料内容')).toHaveValue('# notes.md\n\nhello notes');
  await user.click(screen.getByRole('button', { name: '保存资料' }));
  await waitFor(() => expect(saveMemory).toHaveBeenCalledWith(expect.objectContaining({ scope: 'global', source: 'user', category: 'knowledge', isPermanent: true, userConfirmed: true, content: '# notes.md\n\nhello notes' })));
});

it('requires a separate destructive confirmation and distinguishes read failures from empty data', async () => {
  const user = userEvent.setup();
  render(<KnowledgePage />);
  await user.click(await screen.findByText('更多操作'));
  await user.click(screen.getByRole('button', { name: '删除资料' }));
  expect(deleteMemory).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: '保留资料' }));
  expect(deleteMemory).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: '删除资料' }));
  await user.click(screen.getByRole('button', { name: '确认删除' }));
  await waitFor(() => expect(deleteMemory).toHaveBeenCalledWith('one'));
  vi.mocked(getMemories).mockRejectedValueOnce(new Error('暂不可读'));
  await act(async () => { await useKnowledgeStore.getState().loadKnowledge(); });
  expect(screen.getByRole('alert')).toHaveTextContent('上次读取');
  expect(screen.queryByText(/还没有参考资料/)).not.toBeInTheDocument();
});

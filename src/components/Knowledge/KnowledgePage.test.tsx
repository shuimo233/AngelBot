import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { KnowledgePage } from './KnowledgePage';
import { useKnowledgeStore } from '$stores/knowledge';
import { deleteMemory, getMemories, saveMemory, updateMemory, type RustMemory } from '$lib/commands/memory';
import { extractDocumentText } from '$lib/commands/message';

vi.mock('$lib/commands/memory', () => ({ getMemories: vi.fn(), saveMemory: vi.fn(), updateMemory: vi.fn(), deleteMemory: vi.fn() }));
vi.mock('$lib/commands/message', () => ({ extractDocumentText: vi.fn() }));
const entry: RustMemory = { id: 'one', content: '# 旅行清单\n\n**先核对日期**\n\n![外部图片](https://example.com/private.png)', scope: 'project-one', category: 'knowledge', importance: 4, source: 'user', is_permanent: true, frequency: 0, last_mentioned: null, embedding: null, decay_factor: 1, forget_stage: 'active', created_at: 1, updated_at: 5 };
const other: RustMemory = { ...entry, id: 'two', content: '# 常用地址\n\n仅工作日配送', scope: 'global', source: 'knowledge', updated_at: 2 };
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getMemories).mockResolvedValue([entry, other]);
  vi.mocked(saveMemory).mockImplementation(async memory => memory.id);
  vi.mocked(updateMemory).mockResolvedValue(undefined);
  vi.mocked(deleteMemory).mockResolvedValue(undefined);
  useKnowledgeStore.setState({ entries: [], isLoading: false, error: null, query: '' });
});
afterEach(() => { vi.restoreAllMocks(); });

it('browses compact summaries, filters trimmed text, and previews Markdown without remote images', async () => {
  const user = userEvent.setup();
  render(<KnowledgePage />);
  const detail = await screen.findByRole('article', { name: '资料详情' });
  expect(within(detail).getByText('先核对日期')).toHaveProperty('tagName', 'STRONG');
  expect(detail.querySelector('img')).toBeNull();
  const list = screen.getByLabelText('资料列表');
  await user.click(within(list).getByRole('button', { name: /常用地址/ }));
  expect(detail).toHaveTextContent('常用地址');
  expect(screen.queryByRole('button', { name: '返回列表' })).not.toBeInTheDocument();
  await user.click(within(list).getByRole('button', { name: /旅行清单/ }));
  await user.click(within(detail).getByText('资料信息'));
  expect(within(detail).getByText(/限定范围资料/)).toBeVisible();
  await user.type(screen.getByRole('searchbox', { name: '搜索资料' }), ' 配送 ');
  expect(screen.getByRole('article', { name: '资料详情' })).toHaveTextContent('常用地址');
  await user.click(screen.getByRole('button', { name: '筛选与排序' }));
  await user.selectOptions(screen.getByLabelText('资料来源'), 'user');
  expect(screen.getByText(/没有匹配资料/)).toBeInTheDocument();
});

it('opens a focused detail on narrow layouts and restores the filtered list on Back and Escape', async () => {
  const user = userEvent.setup();
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 0, 420, 600));
  render(<div data-testid="scroll-pane"><KnowledgePage /></div>);
  const search = await screen.findByRole('searchbox', { name: '搜索资料' });
  await user.type(search, ' 配送 ');
  await user.click(screen.getByRole('button', { name: '筛选与排序' }));
  await user.selectOptions(screen.getByLabelText('资料来源'), 'other');
  const list = screen.getByLabelText('资料列表');
  const row = within(list).getByRole('button', { name: /常用地址/ });
  const pane = screen.getByTestId('scroll-pane');
  list.scrollTop = 45; pane.scrollTop = 180;
  row.focus();
  await user.keyboard('{Enter}');
  const detail = screen.getByRole('article', { name: '资料详情' });
  expect(detail).toHaveFocus();
  expect(detail).toHaveTextContent('常用地址');
  expect(screen.queryByRole('searchbox')).not.toBeInTheDocument();
  expect(list).not.toBeVisible();

  list.scrollTop = 0; pane.scrollTop = 0;
  await user.keyboard('{Escape}');
  expect(row).toHaveFocus();
  expect(row).toHaveAttribute('aria-pressed', 'true');
  expect(search).toHaveValue(' 配送 ');
  expect(screen.getByLabelText('资料来源')).toHaveValue('other');
  expect(list.scrollTop).toBe(45);
  expect(pane.scrollTop).toBe(180);

  await user.keyboard('{Enter}');
  await user.click(screen.getByRole('button', { name: '返回列表' }));
  expect(row).toHaveFocus();
  expect(screen.queryByRole('article', { name: '资料详情' })).not.toBeInTheDocument();

  await user.keyboard('{Enter}');
  act(() => useKnowledgeStore.setState(state => ({ entries: state.entries.filter(item => item.id !== other.id) })));
  expect(screen.queryByRole('article', { name: '资料详情' })).not.toBeInTheDocument();
  expect(screen.getByRole('searchbox', { name: '搜索资料' })).toHaveFocus();
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
  vi.mocked(getMemories).mockResolvedValue([]);
  render(<KnowledgePage />);
  await screen.findByText('暂无参考资料');
  expect(screen.queryByRole('searchbox')).not.toBeInTheDocument();
  expect(screen.getAllByRole('button', { name: '添加资料' })).toHaveLength(1);
  await user.click(screen.getByRole('button', { name: '添加资料' }));
  expect(screen.getByRole('region', { name: '添加资料' })).toHaveTextContent('日常与项目均可检索');
  expect(screen.getByRole('textbox', { name: '资料内容' })).toHaveFocus();
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

it('uses the host PDF snapshot in the editable draft and keeps a failed import unsaved', async () => {
  const user = userEvent.setup();
  vi.mocked(getMemories).mockResolvedValue([]);
  vi.mocked(extractDocumentText)
    .mockRejectedValueOnce(new Error('PDF 中未找到可提取的文字。'))
    .mockResolvedValueOnce({ name: 'terms.pdf', text: '【第 2 页】\nCoupon 12' });
  render(<KnowledgePage />);
  await user.click(await screen.findByRole('button', { name: '添加资料' }));
  const file = new File(['%PDF'], 'terms.pdf', { type: 'application/pdf' });
  const picker = screen.getByLabelText('导入资料文件');
  const draft = screen.getByRole('textbox', { name: '资料内容' });
  await user.upload(picker, file);
  expect(await screen.findByRole('alert')).toHaveTextContent('未找到可提取的文字');
  expect(draft).toHaveValue('');
  await user.upload(picker, file);
  await waitFor(() => expect(draft).toHaveValue('# terms.pdf\n\n【第 2 页】\nCoupon 12'));
  expect(saveMemory).not.toHaveBeenCalled();
});

it('keeps a rejected save editable and opens the existing record after a duplicate save', async () => {
  const user = userEvent.setup();
  vi.mocked(saveMemory).mockRejectedValueOnce('相同正文已存在，但未参与召回。').mockResolvedValueOnce(other.id);
  render(<KnowledgePage />);
  await user.click(await screen.findByRole('button', { name: '添加资料' }));
  await user.type(screen.getByRole('textbox', { name: '资料内容' }), '常用地址');
  await user.click(screen.getByRole('button', { name: '保存资料' }));
  expect(await screen.findByText('相同正文已存在，但未参与召回。')).toBeVisible();
  expect(screen.getByRole('textbox', { name: '资料内容' })).toHaveValue('常用地址');
  await user.click(screen.getByRole('button', { name: '保存资料' }));
  await waitFor(() => expect(within(screen.getByRole('article', { name: '资料详情' })).getByRole('heading', { name: '常用地址' })).toBeVisible());
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

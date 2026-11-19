import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { MemoryGovernanceSettings } from './MemoryGovernanceSettings';
import {
  deleteMemory,
  findRelatedMemories,
  getMemories,
  getMemoryHistory,
  replaceMemory,
  restoreReplacedMemory,
  setMemoryPaused,
  setMemoryPermanent,
  updateMemory,
} from '$lib/commands/memory';

vi.mock('$lib/commands/memory', () => ({
  deleteMemory: vi.fn(),
  findRelatedMemories: vi.fn(),
  getMemories: vi.fn(),
  getMemoryHistory: vi.fn(),
  replaceMemory: vi.fn(),
  restoreReplacedMemory: vi.fn(),
  setMemoryPaused: vi.fn(),
  setMemoryPermanent: vi.fn(),
  updateMemory: vi.fn(),
}));

const memories = [
  {
    id: 'mem-1',
    scope: 'global',
    category: 'preference',
    content: 'User likes Rust',
    importance: 7,
    source: 'test',
    frequency: 3,
    last_mentioned: null,
    is_permanent: false,
    embedding: null,
    decay_factor: 1,
    forget_stage: 'active',
    created_at: 1,
    updated_at: 2,
  },
  {
    id: 'mem-2',
    scope: 'global',
    category: 'fact',
    content: 'User uses Windows',
    importance: 4,
    source: 'test',
    frequency: 1,
    last_mentioned: null,
    is_permanent: true,
    embedding: null,
    decay_factor: 1,
    forget_stage: 'active',
    created_at: 1,
    updated_at: 2,
  },
];
const customMemory = { ...memories[1], id: 'mem-custom', scope: 'custom-project-scope', category: 'constructor', content: '自定义范围中的内容' };

describe('MemoryGovernanceSettings', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getMemories).mockResolvedValue([...memories, customMemory]);
    vi.mocked(updateMemory).mockResolvedValue(undefined);
    vi.mocked(setMemoryPermanent).mockResolvedValue(undefined);
    vi.mocked(deleteMemory).mockResolvedValue(undefined);
    vi.mocked(getMemoryHistory).mockResolvedValue([
      {
        id: 'h1',
        memoryId: 'mem-1',
        operation: 'update',
        previousContent: 'old',
        newContent: 'User likes Rust',
        details: 'importance:4->7',
        createdAt: 3,
      },
    ]);
    vi.mocked(findRelatedMemories).mockResolvedValue([
      {
        memory_id: 'mem-2',
        content: 'User likes Rust programming',
        source: 'agent',
        updated_at: 3,
      },
    ]);
    vi.mocked(replaceMemory).mockResolvedValue(undefined);
  });

  it('shows only the selected record and can pause it without editing its contents', async () => {
    vi.mocked(setMemoryPaused).mockResolvedValue(undefined);
    render(<MemoryGovernanceSettings focusedMemoryId="mem-2" />);
    expect(await screen.findByDisplayValue('User uses Windows')).toBeVisible();
    expect(screen.queryByDisplayValue('User likes Rust')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('搜索记忆')).not.toBeInTheDocument();
    vi.mocked(getMemories).mockResolvedValue([memories[0], { ...memories[1], forget_stage: 'paused' }]);
    await userEvent.click(screen.getByRole('button', { name: '暂时不用这条记忆' }));
    expect(await screen.findByRole('button', { name: '恢复使用这条记忆' })).toBeVisible();
    expect(setMemoryPaused).toHaveBeenCalledWith('mem-2', true);
    expect(updateMemory).not.toHaveBeenCalled();
  });

  it('filters memories and saves edited memory fields', async () => {
    render(<MemoryGovernanceSettings />);

    expect(await screen.findByDisplayValue('User likes Rust')).toBeInTheDocument();
    expect(screen.getByRole('option', { name: '偏好' })).toHaveValue('preference');
    expect(screen.getByRole('option', { name: 'constructor' })).toHaveValue('constructor');
    await userEvent.type(screen.getByLabelText('搜索记忆'), 'windows');
    expect(screen.queryByDisplayValue('User likes Rust')).not.toBeInTheDocument();
    expect(screen.getByDisplayValue('User uses Windows')).toBeInTheDocument();

    await userEvent.clear(screen.getByLabelText('搜索记忆'));
    const content = await screen.findByDisplayValue('User likes Rust');
    await userEvent.clear(content);
    await userEvent.type(content, 'User really likes Rust');

    const item = content.closest('.memory-item') as HTMLElement;
    expect(within(item).getByText('所有对话')).toBeVisible();
    expect(within(item).getByLabelText('重要度')).not.toBeVisible();
    await userEvent.click(within(item).getByText('更多设置与操作', { exact: true }));
    expect(within(item).getByLabelText('适用范围标识')).toHaveValue('global');
    expect(within(item).getByLabelText('分类标识')).toHaveValue('preference');
    await userEvent.click(within(item).getByText('更多设置与操作', { exact: true }));
    await userEvent.click(within(item).getByLabelText('长期保留'));
    vi.mocked(updateMemory).mockRejectedValueOnce(new Error('暂时无法保存'));
    await userEvent.click(within(item).getByRole('button', { name: '保存' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('暂时无法保存');
    expect(content).toHaveValue('User really likes Rust');
    expect(within(item).getByText('有未保存的修改')).toBeVisible();
    vi.mocked(updateMemory).mockRejectedValueOnce('数据库正忙，请稍后保存');
    await userEvent.click(within(item).getByRole('button', { name: '保存' }));
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('数据库正忙，请稍后保存'));
    expect(content).toHaveValue('User really likes Rust');
    expect(within(item).getByText('有未保存的修改')).toBeVisible();
    vi.mocked(getMemories).mockResolvedValue([{ ...memories[0], content: 'User really likes Rust', is_permanent: true }, memories[1], customMemory]);
    await userEvent.click(within(item).getByRole('button', { name: '保存' }));

    await waitFor(() => {
      expect(updateMemory).toHaveBeenCalledWith({
        id: 'mem-1',
        scope: 'global',
        category: 'preference',
        content: 'User really likes Rust',
        importance: 7,
        isPermanent: true,
      });
    });
    await waitFor(() => expect(within(item).getByText('已保存')).toBeVisible());

    const custom = screen.getByDisplayValue(customMemory.content).closest('.memory-item') as HTMLElement;
    await userEvent.click(within(custom).getByText('更多设置与操作', { exact: true }));
    expect(within(custom).getByLabelText('分类标识')).toHaveValue('constructor');
    const scope = within(custom).getByLabelText('适用范围标识');
    await userEvent.clear(scope);
    await userEvent.type(scope, 'custom-project-scope-2');
    await userEvent.click(within(custom).getByText('更多设置与操作', { exact: true }));
    await userEvent.click(within(custom).getByRole('button', { name: '保存' }));
    await waitFor(() => expect(updateMemory).toHaveBeenCalledWith(expect.objectContaining({ id: 'mem-custom', scope: 'custom-project-scope-2', category: 'constructor', content: customMemory.content, importance: 4, isPermanent: true })));
  });

  it('keeps labels, descriptions and datalists within each simultaneously mounted editor', async () => {
    render(<>
      <section aria-label="资料库编辑器"><MemoryGovernanceSettings /></section>
      <div role="dialog" aria-label="设置中的记忆"><MemoryGovernanceSettings /></div>
    </>);
    const library = screen.getByRole('region', { name: '资料库编辑器' });
    const settings = screen.getByRole('dialog', { name: '设置中的记忆' });
    const libraryContent = await within(library).findByDisplayValue('User likes Rust');
    const settingsContent = await within(settings).findByDisplayValue('User likes Rust');
    const settingsItem = settingsContent.closest('.memory-item') as HTMLElement;
    expect(within(settingsItem).getByLabelText('要记住的内容')).toBe(settingsContent);
    await userEvent.click(within(settingsItem).getByText('要记住的内容'));
    expect(settingsContent).toHaveFocus();
    expect(libraryContent).not.toHaveFocus();

    const ids = [...document.querySelectorAll('.memory-governance [id]')].map(element => element.id);
    expect(ids.length).toBeGreaterThan(0);
    expect(new Set(ids).size).toBe(ids.length);
    for (const editor of [library, settings]) {
      for (const control of editor.querySelectorAll('[aria-describedby], [list]')) {
        const references = [control.getAttribute('aria-describedby'), control.getAttribute('list')].filter(Boolean).join(' ').split(/\s+/);
        for (const id of references) {
          const target = document.getElementById(id);
          expect(target).not.toBeNull();
          expect(editor.contains(target)).toBe(true);
        }
      }
    }
  });

  it('reviews related records, requires a replacement decision, shows history, pins and deletes', async () => {
    render(<MemoryGovernanceSettings />);

    const content = await screen.findByDisplayValue('User likes Rust');
    const item = content.closest('.memory-item') as HTMLElement;

    await userEvent.click(within(item).getByText('更多设置与操作', { exact: true }));
    await userEvent.click(within(item).getByRole('button', { name: '历史记录' }));
    expect(await screen.findByText('importance:4->7')).toBeInTheDocument();
    expect(getMemoryHistory).toHaveBeenCalledWith('mem-1');

    await userEvent.click(within(item).getByRole('button', { name: '查找相关记忆' }));
    expect(await screen.findByText('User likes Rust programming')).toBeInTheDocument();
    expect(findRelatedMemories).toHaveBeenCalledWith('mem-1');
    await userEvent.click(screen.getByText('用当前记忆替代此条'));
    expect(replaceMemory).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: '确认替代' }));
    await waitFor(() => expect(replaceMemory).toHaveBeenCalledWith({
      sourceId: 'mem-2', sourceContent: 'User likes Rust programming',
      targetId: 'mem-1', targetContent: 'User likes Rust',
    }));

    await userEvent.click(within(item).getByRole('button', { name: '立即设为长期保留' }));
    await waitFor(() => expect(setMemoryPermanent).toHaveBeenCalledWith('mem-1', true));

    await userEvent.click(within(item).getByText('删除这条记忆', { exact: true }));
    expect(deleteMemory).not.toHaveBeenCalled();
    await userEvent.click(within(item).getByRole('button', { name: '确认删除' }));
    await waitFor(() => expect(deleteMemory).toHaveBeenCalledWith('mem-1'));
  });

  it('pauses recall without deleting the content and resumes it explicitly', async () => {
    const paused = { ...memories[0], forget_stage: 'paused' };
    vi.mocked(setMemoryPaused).mockResolvedValue(undefined);
    vi.mocked(getMemories).mockResolvedValueOnce([memories[0]])
      .mockResolvedValueOnce([paused]).mockResolvedValueOnce([memories[0]]);
    render(<MemoryGovernanceSettings />);

    await screen.findByDisplayValue(memories[0].content);
    await userEvent.click(screen.getByText('更多设置与操作', { exact: true }));
    await userEvent.click(screen.getByRole('button', { name: '暂停召回' }));
    expect(await screen.findByText('已暂停召回', { selector: '.memory-badge' })).toBeVisible();
    expect(setMemoryPaused).toHaveBeenCalledWith('mem-1', true);
    expect(screen.getByDisplayValue(memories[0].content)).toBeVisible();

    await userEvent.click(screen.getByRole('button', { name: '恢复召回' }));
    await waitFor(() => expect(setMemoryPaused).toHaveBeenLastCalledWith('mem-1', false));
    await waitFor(() => expect(screen.getByRole('button', { name: '暂停召回' })).toBeVisible());
    expect(deleteMemory).not.toHaveBeenCalled();
  });

  it('shows replaced records on demand and returns them to paused recall', async () => {
    const original = { ...memories[0], id: 'old', content: 'Old Rust preference' };
    const replaced = { ...original, superseded_by: 'mem-1' };
    const paused = { ...original, forget_stage: 'paused' };
    vi.mocked(getMemories).mockResolvedValueOnce([memories[0], replaced])
      .mockResolvedValueOnce([memories[0], paused]);
    vi.mocked(restoreReplacedMemory).mockResolvedValue(undefined);
    render(<MemoryGovernanceSettings />);

    await screen.findByDisplayValue(memories[0].content);
    expect(getMemories).toHaveBeenCalledWith(true);
    expect(screen.queryByDisplayValue(original.content)).not.toBeInTheDocument();
    const filter = screen.getByLabelText('按阶段筛选');
    await userEvent.selectOptions(filter, 'replaced');
    expect(screen.getByLabelText('原记录')).toHaveValue(original.content);
    expect(screen.getByLabelText('原记录')).toHaveAttribute('readonly');
    await userEvent.click(screen.getByText('查看替代记录'));
    expect(screen.getByLabelText('替代记录的当前内容')).toHaveValue(memories[0].content);

    await userEvent.click(screen.getByRole('button', { name: '撤销替代' }));
    await waitFor(() => expect(filter).toHaveValue('paused'));
    expect(restoreReplacedMemory).toHaveBeenCalledWith('old', 'mem-1');
    expect(screen.getByDisplayValue(original.content)).toBeVisible();
    expect(setMemoryPaused).not.toHaveBeenCalled();
  });

});

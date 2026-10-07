import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { MemoryGovernanceSettings } from './MemoryGovernanceSettings';
import {
  deleteMemory,
  detectMemoryConflicts,
  getMemories,
  getMemoryHistory,
  mergeMemories,
  setMemoryPermanent,
  updateMemory,
} from '$lib/commands/memory';

vi.mock('$lib/commands/memory', () => ({
  deleteMemory: vi.fn(),
  detectMemoryConflicts: vi.fn(),
  getMemories: vi.fn(),
  getMemoryHistory: vi.fn(),
  mergeMemories: vi.fn(),
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

describe('MemoryGovernanceSettings', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getMemories).mockResolvedValue(memories);
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
    vi.mocked(detectMemoryConflicts).mockResolvedValue([
      {
        memory_id: 'mem-2',
        content: 'User likes Rust programming',
        category: 'preference',
        similarity: 0.92,
      },
    ]);
    vi.mocked(mergeMemories).mockResolvedValue(undefined);
  });

  it('filters memories and saves edited memory fields', async () => {
    render(<MemoryGovernanceSettings />);

    expect(await screen.findByDisplayValue('User likes Rust')).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText('搜索记忆'), 'windows');
    expect(screen.queryByDisplayValue('User likes Rust')).not.toBeInTheDocument();
    expect(screen.getByDisplayValue('User uses Windows')).toBeInTheDocument();

    await userEvent.clear(screen.getByLabelText('搜索记忆'));
    const content = await screen.findByDisplayValue('User likes Rust');
    await userEvent.clear(content);
    await userEvent.type(content, 'User really likes Rust');

    const item = content.closest('.memory-item') as HTMLElement;
    await userEvent.click(within(item).getByLabelText('Never forget'));
    await userEvent.click(within(item).getByRole('button', { name: 'Save' }));

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
  });

  it('shows history, detects conflicts, merges, pins, and deletes', async () => {
    render(<MemoryGovernanceSettings />);

    const content = await screen.findByDisplayValue('User likes Rust');
    const item = content.closest('.memory-item') as HTMLElement;

    await userEvent.click(within(item).getByRole('button', { name: 'History' }));
    expect(await screen.findByText('importance:4->7')).toBeInTheDocument();
    expect(getMemoryHistory).toHaveBeenCalledWith('mem-1');

    await userEvent.click(within(item).getByRole('button', { name: 'Conflicts' }));
    expect(await screen.findByText('User likes Rust programming')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Merge into this' }));
    await waitFor(() => expect(mergeMemories).toHaveBeenCalledWith('mem-2', 'mem-1'));

    await userEvent.click(within(item).getByRole('button', { name: 'Pin' }));
    await waitFor(() => expect(setMemoryPermanent).toHaveBeenCalledWith('mem-1', true));

    await userEvent.click(within(item).getByRole('button', { name: 'Delete' }));
    await waitFor(() => expect(deleteMemory).toHaveBeenCalledWith('mem-1'));
  });
});

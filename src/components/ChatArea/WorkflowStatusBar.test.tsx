import { render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { WorkflowStatusBar } from './WorkflowStatusBar';
import { useMessagesStore } from '$stores/messages';
import { useSettingsStore } from '$stores/settings';
import { getMemoryStats } from '$lib/commands/memory';

vi.mock('$lib/commands/memory', () => ({ getMemoryStats: vi.fn() }));

// MemoryStats is a frozen runtime contract; cast through unknown so the
// test fixture doesn't have to mirror every field the production type
// adds over time (deleted, withEmbeddings, …).
const EMPTY_STATS = {
  total: 0, active: 0, permanent: 0, summarized: 0, archived: 0,
  deleted: 0, withEmbeddings: 0,
} as unknown as Awaited<ReturnType<typeof getMemoryStats>>;

describe('WorkflowStatusBar', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getMemoryStats).mockResolvedValue(EMPTY_STATS);
    useSettingsStore.setState({ profile: { name: 'Test', bio: 'Bio' } as never });
    useMessagesStore.setState({ messages: [] });
  });

  it('returns null when not expanded', () => {
    const { container } = render(<WorkflowStatusBar expanded={false} />);
    expect(container.firstChild).toBeNull();
  });

  it('shows the modified-files summary when the latest assistant message has task facts', async () => {
    // After C, durable task facts surface modified files on Message.taskFacts.
    // The status bar must mirror them so a glance at the workflow area
    // tells the user what changed in this turn.
    useMessagesStore.setState({
      messages: [{
        id: 'assistant-1', sessionId: 's1', role: 'assistant', content: '', createdAt: 1,
        taskFacts: {
          goal: 'Refactor',
          modifiedFiles: ['src/a.ts', 'src/b.ts'],
          terminalReason: 'completed',
        },
      }],
    });
    render(<WorkflowStatusBar expanded />);

    expect(screen.getByText('修改 2 个文件')).toBeInTheDocument();
    await waitFor(() => expect(getMemoryStats).toHaveBeenCalled());
  });
});
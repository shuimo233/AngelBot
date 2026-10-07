import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { LibraryPage } from './LibraryPage';
import { getSkills } from '$lib/commands/skill';

vi.mock('../Knowledge/KnowledgePage', () => ({ KnowledgePage: () => <div>knowledge</div> }));
vi.mock('../Memory/MemoryPage', () => ({ MemoryPage: () => <div>memory</div> }));
vi.mock('./ActivityPage', () => ({ ActivityPage: () => <div>activity</div> }));
vi.mock('$lib/commands/skill', () => ({ getSkills: vi.fn() }));

const getSkillsMock = vi.mocked(getSkills);

describe('LibraryPage skills', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('shows installed skills without exposing a second, competing import flow', async () => {
    getSkillsMock.mockResolvedValue([
      {
        id: 'web-research',
        name: 'Web research',
        description: 'Research a topic from trusted sources.',
        version: '1.0.0',
        actions: [],
        dependencies: [],
        permissions: [],
      },
    ]);
    const user = userEvent.setup();
    render(<LibraryPage />);

    await user.click(screen.getByRole('tab', { name: '技能' }));

    await waitFor(() => expect(screen.getByText('Web research')).toBeInTheDocument());
    expect(screen.queryByRole('button', { name: '添加技能' })).not.toBeInTheDocument();
    expect(getSkillsMock).toHaveBeenCalledTimes(1);
  });

  it('explains the URL install path and the named-install prerequisite when none are installed', async () => {
    getSkillsMock.mockResolvedValue([]);
    const user = userEvent.setup();
    render(<LibraryPage />);

    await user.click(screen.getByRole('tab', { name: '技能' }));

    expect(
      await screen.findByText(/发送公开 GitHub 仓库链接即可安装/),
    ).toBeInTheDocument();
  });
});

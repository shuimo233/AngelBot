import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { LibraryPage } from './LibraryPage';
import { getSkills } from '$lib/commands/skill';
import { getMcpServers } from '$lib/commands/mcp';

vi.mock('../Knowledge/KnowledgePage', () => ({ KnowledgePage: () => <input aria-label="knowledge draft" defaultValue="" /> }));
vi.mock('../Memory/MemoryPage', () => ({ MemoryPage: () => <div>memory</div> }));
vi.mock('./ActivityPage', () => ({ ActivityPage: () => <div>activity</div> }));
vi.mock('$lib/commands/skill', () => ({ getSkills: vi.fn() }));
vi.mock('$lib/commands/mcp', () => ({ getMcpServers: vi.fn(() => Promise.resolve([])) }));

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
    expect(getMcpServers).not.toHaveBeenCalled();
  });

  it('supports keyboard categories and retries a failed skills read without calling it empty', async () => {
    getSkillsMock.mockRejectedValueOnce(new Error('offline')).mockResolvedValueOnce([]);
    const user = userEvent.setup();
    render(<LibraryPage />);
    const first = screen.getByRole('tab', { name: '参考资料' });
    first.focus();
    await user.keyboard('{ArrowDown}{ArrowDown}');
    expect(screen.getByRole('tab', { name: '技能' })).toHaveFocus();
    expect(await screen.findByRole('alert')).toHaveTextContent('无法加载已添加的技能');
    expect(screen.queryByText('还没有已添加的技能')).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '重新加载' }));
    expect(await screen.findByText('还没有已添加的技能')).toBeInTheDocument();
    expect(getSkillsMock).toHaveBeenCalledTimes(2);
  });

  it('explains the URL install path and the named-install prerequisite when none are installed', async () => {
    getSkillsMock.mockResolvedValue([]);
    const user = userEvent.setup();
    render(<LibraryPage />);

    await user.click(screen.getByRole('tab', { name: '技能' }));
    await user.click(screen.getByRole('button', { name: '如何添加技能' }));

    expect(
      await screen.findByText(/复制公开 GitHub 仓库链接/),
    ).toBeInTheDocument();
  });

  it('keeps the knowledge draft when switching categories', async () => {
    getSkillsMock.mockResolvedValue([]);
    const user = userEvent.setup();
    render(<LibraryPage />);
    await user.type(screen.getByLabelText('knowledge draft'), '尚未保存的资料');
    await user.click(screen.getByRole('tab', { name: '技能' }));
    expect(screen.queryByRole('textbox', { name: 'knowledge draft' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('tab', { name: '参考资料' }));
    expect(screen.getByLabelText('knowledge draft')).toHaveValue('尚未保存的资料');
  });

  it('separates purpose-labelled views and lazily loads only the selected capability', async () => {
    getSkillsMock.mockResolvedValue([]);
    const user = userEvent.setup();
    render(<LibraryPage />);
    expect(screen.getAllByRole('tab').map(tab => tab.textContent)).toEqual([
      '参考资料我主动保存的内容', '关于我的记忆交流中保留的偏好和事实', '技能教助手如何做事', '应用连接让助手使用外部工具', '使用记录回看任务结果与记忆变更',
    ]);
    expect(getSkills).not.toHaveBeenCalled();
    expect(getMcpServers).not.toHaveBeenCalled();
    await user.click(screen.getByRole('tab', { name: '应用连接' }));
    await screen.findByText('还没有应用连接');
    expect(getMcpServers).toHaveBeenCalledTimes(1);
    expect(getSkills).not.toHaveBeenCalled();
    expect(screen.queryByRole('heading', { name: '技能' })).not.toBeInTheDocument();
  });
});

import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { LibraryPage } from './LibraryPage';
import { getSkills } from '$lib/commands/skill';
import { getMcpServers } from '$lib/commands/mcp';
import { useSettingsStore } from '$stores/settings';
import { applyTavernCard, parseTavernCard } from '$lib/tavern-card';

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
    await user.keyboard('{ArrowDown}{ArrowDown}{ArrowDown}');
    expect(screen.getByRole('tab', { name: '技能' })).toHaveFocus();
    expect(await screen.findByRole('alert')).toHaveTextContent('无法加载已添加的技能');
    expect(screen.queryByText('暂无技能')).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '重新加载' }));
    expect(await screen.findByText('暂无技能')).toBeInTheDocument();
    expect(getSkillsMock).toHaveBeenCalledTimes(2);
  });

  it('explains the URL install path and the named-install prerequisite when none are installed', async () => {
    getSkillsMock.mockResolvedValue([]);
    const user = userEvent.setup();
    render(<LibraryPage />);

    await user.click(screen.getByRole('tab', { name: '技能' }));
    await user.click(screen.getByRole('button', { name: '如何添加技能' }));

    expect(
      await screen.findByText(/在主对话贴上公开仓库链接/),
    ).toBeInTheDocument();
    expect(screen.getByText(/按名称查找需要启用联网搜索/)).toBeInTheDocument();
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

  it('uses concise tab labels and lazily loads only the selected capability', async () => {
    getSkillsMock.mockResolvedValue([]);
    const user = userEvent.setup();
    render(<LibraryPage />);
    expect(screen.getAllByRole('tab').map(tab => tab.textContent)).toEqual([
      '参考资料', '人物卡与世界书', '关于我的记忆', '技能', '应用连接', '使用记录',
    ]);
    expect(getSkills).not.toHaveBeenCalled();
    expect(getMcpServers).not.toHaveBeenCalled();
    await user.click(screen.getByRole('tab', { name: '应用连接' }));
    await screen.findByText('暂无应用连接');
    expect(getMcpServers).toHaveBeenCalledTimes(1);
    expect(getSkills).not.toHaveBeenCalled();
    expect(screen.queryByRole('heading', { name: '技能' })).not.toBeInTheDocument();
  });

  it('reads the saved persona and bounded book pages, routing changes through shared settings', async () => {
    const user = userEvent.setup();
    const original = useSettingsStore.getState().profile;
    const originalActive = useSettingsStore.getState().activeProfile;
    const stored = localStorage.getItem('angelbot_profile');
    const openSettings = vi.fn();
    window.addEventListener('open-settings', openSettings);
    try {
      const entries = Array.from({ length: 25 }, (_, index) => ({ name: '条目 ' + (index + 1), keys: ['key' + index], secondary_keys: index === 2 ? ['secondary2'] : [], selective: index === 2, content: '合成条目内容 ' + (index + 1) + (index === 24 ? ' 尾部测试标记' : ''), enabled: index !== 1, constant: index === 0 }));
      const imported = applyTavernCard(parseTavernCard({ spec: 'chara_card_v2', spec_version: '2.0', data: { name: '资料页测试人设', description: '主助手设定', character_book: { name: '合成世界书', entries: [...entries, { keys: ['/正则规则/'], content: '仅留存', use_regex: true }] } } }));
      const activeProfile = { ...original, ...imported };
      useSettingsStore.setState({ activeProfile, profile: { ...activeProfile, name: '未保存的名字' } });
      render(<LibraryPage />);
      await user.click(screen.getByRole('tab', { name: '人物卡与世界书' }));
      expect(screen.queryByText('未保存的名字')).not.toBeInTheDocument();
      expect(screen.getByRole('heading', { name: '资料页测试人设' })).toBeVisible();
      expect(screen.getByText('可摘录条目中的文字，保存为对话背景。')).toBeVisible();
      const list = screen.getByLabelText('世界书条目列表');
      expect(within(list).getAllByRole('button')).toHaveLength(20);
      expect(screen.getByRole('article', { name: '世界书条目详情' })).toHaveTextContent(entries[0].content);
      await user.click(screen.getByRole('button', { name: '下一页' }));
      expect(within(list).getAllByRole('button')).toHaveLength(6);
      await user.type(screen.getByRole('searchbox', { name: '搜索世界书条目' }), '尾部测试标记');
      expect(within(list).getAllByRole('button')).toHaveLength(1);
      const entry = screen.getByRole('article', { name: '世界书条目详情' });
      expect(entry).toHaveTextContent('character_book.entries.24');
      await user.click(within(entry).getByRole('button', { name: '编辑摘录' }));
      const excerpt = within(entry).getByRole('textbox', { name: '摘录' });
      expect(excerpt).toHaveValue(entries[24].content);
      expect(excerpt).toHaveFocus();
      fireEvent.change(excerpt, { target: { value: '超'.repeat(4096) } });
      expect(within(entry).getByRole('button', { name: '加入人设草稿' })).toBeDisabled();
      expect(excerpt).toHaveAccessibleDescription(/超出人设补充上限/);
      await user.click(within(entry).getByRole('button', { name: '取消摘录' }));
      const originalText = within(entry).getByText(entries[24].content).firstChild!;
      const selectedText = '尾部测试标记';
      const range = document.createRange();
      range.setStart(originalText, entries[24].content.length - selectedText.length);
      range.setEnd(originalText, entries[24].content.length);
      const selection = window.getSelection()!;
      selection.removeAllRanges();
      selection.addRange(range);
      expect(selection.toString()).toBe(selectedText);
      const editExcerpt = within(entry).getByRole('button', { name: '编辑摘录' });
      fireEvent.pointerDown(editExcerpt);
      selection.removeAllRanges();
      fireEvent.click(editExcerpt, { detail: 1 });
      expect(within(entry).getByRole('textbox', { name: '摘录' })).toHaveValue(selectedText);
      await user.click(within(entry).getByRole('button', { name: '加入人设草稿' }));
      expect(screen.getByRole('textbox', { name: '人设补充草稿' })).toHaveValue('条目 25：\n尾部测试标记');
      expect(screen.getByText(/未保存 · 剩余/)).toBeVisible();
      expect(useSettingsStore.getState().activeProfile.personaNotes).toBe('');
      expect(localStorage.getItem('angelbot_profile')).toBe(stored);
      await user.click(screen.getByRole('button', { name: '前往性格设置保存' }));
      expect(openSettings.mock.calls[0][0].detail).toBe('personality');
      expect(useSettingsStore.getState().profile.name).toBe('未保存的名字');
      act(() => { useSettingsStore.getState().persistProfile(); useSettingsStore.getState().loadProfile(); });
      expect(screen.getByRole('article', { name: '世界书条目详情' })).toHaveTextContent('尾部测试标记');
      expect(screen.getByText(/当前生效 · 剩余/)).toBeVisible();
      expect(useSettingsStore.getState().activeProfile.personaNotes).toBe('条目 25：\n尾部测试标记');
    } finally {
      window.removeEventListener('open-settings', openSettings);
      act(() => useSettingsStore.setState({ profile: original, activeProfile: originalActive }));
      if (stored === null) localStorage.removeItem('angelbot_profile'); else localStorage.setItem('angelbot_profile', stored);
    }
  });
});

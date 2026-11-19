import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { PersonalitySettings } from './PersonalitySettings';
import { usePreferencesStore } from '$stores/preferences';
import { useSettingsStore } from '$stores/settings';
import { defaultTraits } from '$lib/presets';
import { matchPersonalityDirection } from '$lib/commands';
import {
  acceptEvolutionProposal,
  dismissLearnedPreference,
  getEvolutionProposals,
  getMemories,
  getLearnedPreferences,
  initializeMemoryControls,
  rejectEvolutionProposal,
  saveMemoryControls,
} from '$lib/commands/memory';

vi.mock('$lib/commands/memory', () => ({
  acceptEvolutionProposal: vi.fn(),
  getEvolutionProposals: vi.fn(),
  getMemories: vi.fn(),
  getLearnedPreferences: vi.fn(),
  dismissLearnedPreference: vi.fn(),
  initializeMemoryControls: vi.fn(),
  rejectEvolutionProposal: vi.fn(),
  saveMemoryControls: vi.fn(),
  setMemoryPermanent: vi.fn(),
}));

vi.mock('$lib/commands', () => ({
  matchPersonalityDirection: vi.fn(),
}));

const defaultPreferences = {
  communication: {
    preferredTone: [],
    dislikedWords: [],
    petPeeves: [],
  },
  habits: {
    greetingStyle: '',
    responseLength: 'medium' as const,
    responseLanguage: 'auto' as const,
  },
  topics: {
    interests: [],
    avoidTopics: [],
  },
  learnedAt: 1,
  evolutionEnabled: true,
};
const characterCard = {
  specVersion: '2.0', personality: '耐心', scenario: '', mesExample: '',
  systemPrompt: '已有的角色设定', postHistoryInstructions: '',
  alternateGreetings: [], creatorNotes: '', creator: '', tags: [],
  rawData: { character_book: { name: '随卡参考', entries: [{ keys: ['书店'], content: '角色经常去的书店。', enabled: true }] } },
};

describe('PersonalitySettings', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    useSettingsStore.getState().loadProfile();
    usePreferencesStore.setState({ preferences: defaultPreferences });
    vi.mocked(initializeMemoryControls).mockImplementation(async (legacy) => legacy);
    vi.mocked(saveMemoryControls).mockImplementation(async (update) => {
      const { habits, evolutionEnabled } = usePreferencesStore.getState().preferences;
      return { useLongTermMemory: habits.useLongTermMemory ?? true, evolutionEnabled, ...update };
    });
    useSettingsStore.setState({
      profile: {
        name: 'AngelBot',
        avatar: '',
        bio: '',
        languageStyle: 'casual',
        tone: 'friendly',
        responseFormats: ['text'],
        keywords: [],
        greeting: '',
        personality: 'balanced',
        speechBubble: 'default',
      },
    });
    vi.mocked(getMemories).mockResolvedValue([]);
    vi.mocked(getLearnedPreferences).mockResolvedValue([]);
    vi.mocked(dismissLearnedPreference).mockResolvedValue(undefined);
    vi.mocked(getEvolutionProposals).mockResolvedValue([
      {
        id: 'proposal-1',
        proposalType: 'memory',
        sessionId: null,
        category: 'preference',
        content: '用户喜欢 Rust',
        importance: 8,
        preferencesJson: null,
        summary: '学到用户喜欢 Rust',
        source: 'manual_evolution_review',
        status: 'pending',
        createdAt: 1,
        reviewedAt: null,
      },
    ]);
    vi.mocked(acceptEvolutionProposal).mockResolvedValue({
      id: 'proposal-1',
      proposalType: 'memory',
      sessionId: null,
      category: 'preference',
      content: '用户喜欢 Rust',
      importance: 8,
      preferencesJson: null,
      summary: '学到用户喜欢 Rust',
      source: 'manual_evolution_review',
      status: 'accepted',
      createdAt: 1,
      reviewedAt: 2,
    });
    vi.mocked(rejectEvolutionProposal).mockResolvedValue({
      id: 'proposal-1',
      proposalType: 'memory',
      sessionId: null,
      category: 'preference',
      content: '用户喜欢 Rust',
      importance: 8,
      preferencesJson: null,
      summary: '学到用户喜欢 Rust',
      source: 'manual_evolution_review',
      status: 'rejected',
      createdAt: 1,
      reviewedAt: 2,
    });
  });

  it('starts with everyday fields and shows personality fields directly and preserves edits across saving and reopening', async () => {
    const user = userEvent.setup();
    useSettingsStore.getState().updateProfile({
      bio: '原有的相处方式', greeting: '欢迎回来',
      traits: { ...defaultTraits, humor: 4 }, characterCard,
    });
    usePreferencesStore.setState({ preferences: { ...defaultPreferences, habits: { ...defaultPreferences.habits, useLongTermMemory: false } } });
    const { unmount } = render(<PersonalitySettings />);

    expect(screen.getByLabelText('名字')).toBeVisible();
    expect(screen.getByRole('checkbox', { name: '启用偏好学习' })).toBeVisible();
    expect(screen.getByLabelText('相处方式')).toBeVisible();
    expect(screen.getByRole('button', { name: '导入人物卡或配置' })).toBeVisible();
    expect(screen.getByLabelText('人设补充')).toHaveValue('');
    await user.type(screen.getByLabelText('人设补充'), '喜欢星空与热茶');
    expect(usePreferencesStore.getState().preferences.habits.useLongTermMemory).toBe(false);
    expect(useSettingsStore.getState().profile.characterCard).toBe(characterCard);
    expect(localStorage.getItem('angelbot_profile')).toBeNull();
    expect(screen.queryByLabelText(/头像/i)).not.toBeInTheDocument();
    expect(screen.queryByRole('slider', { name: '无趣 — 幽默' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '显示滑杆' })).toHaveAttribute('aria-expanded', 'false');
    expect(screen.getByText('已导入角色卡')).toBeVisible();
    expect(matchPersonalityDirection).not.toHaveBeenCalled();

    await user.clear(screen.getByLabelText('相处方式'));
    await user.type(screen.getByLabelText('相处方式'), '自然一点，先说结论');
    expect(screen.getByLabelText('系统提示')).toBeVisible();
    await user.click(screen.getByRole('button', { name: '显示滑杆' }));
    expect(screen.getByRole('slider', { name: '聚焦卡点 — 循序渐进' })).toHaveValue('5');
    const humor = screen.getByRole('slider', { name: '无趣 — 幽默' });
    expect(humor).toHaveValue('4');
    fireEvent.change(humor, { target: { value: '2' } });
    expect(screen.getByLabelText('相处方式')).toHaveValue('自然一点，先说结论');
    expect(screen.queryByRole('button', { name: '保存所有设置' })).not.toBeInTheDocument();

    act(() => useSettingsStore.getState().persistProfile());
    unmount();
    act(() => useSettingsStore.getState().loadProfile());
    render(<PersonalitySettings />);
    expect(screen.getByLabelText('相处方式')).toHaveValue('自然一点，先说结论');
    expect(useSettingsStore.getState().profile).toMatchObject({
      traits: { ...defaultTraits, humor: 2 }, characterCard, greeting: '欢迎回来', personaNotes: '喜欢星空与热茶',
    });
    expect(screen.getByLabelText('人设补充')).toHaveValue('喜欢星空与热茶');
    await user.click(screen.getByRole('button', { name: '显示滑杆' }));
    expect(screen.getByRole('slider', { name: '无趣 — 幽默' })).toHaveValue('2');
    await user.click(screen.getByRole('button', { name: '收起滑杆' }));
    expect(screen.queryByRole('slider', { name: '无趣 — 幽默' })).not.toBeInTheDocument();
  });

  it('separates saved dialogue settings from edits and appends a starter only to the draft', async () => {
    const user = userEvent.setup();
    useSettingsStore.getState().updateProfile({
      name: '北辰',
      bio: '先听我说。',
      greeting: '今天从哪里开始？',
      personaNotes: '只用选定的角色背景。',
    });
    useSettingsStore.getState().persistProfile();
    render(<PersonalitySettings />);

    const current = screen.getByRole('complementary', { name: '试聊示例' });
    expect(current).toHaveTextContent('北辰');
    expect(current).toHaveTextContent('本地固定示例');
    expect(current).toHaveTextContent('我今天有点累。');
    expect(current).not.toHaveTextContent('未保存草稿');
    const activeReply = within(current).getByText('先歇一会儿吧。还有什么事是今天一定要做的？');
    await user.click(screen.getByRole('button', { name: '简洁协作' }));
    const draft = useSettingsStore.getState().profile;
    expect(draft.bio).toMatch(/^先听我说。\n先说结论和下一步/);
    expect(screen.getByLabelText('相处方式')).toHaveValue(draft.bio);
    expect(current).toHaveTextContent('未保存草稿');
    expect(activeReply).toBeVisible();
    expect(within(current).getByText('先歇一会儿。今天只留一件必须做的事。')).toBeVisible();
    await user.click(within(current).getByText('查看当前设定'));
    expect(within(current).getByText('今天从哪里开始？')).toBeVisible();
    expect(within(current).getByText('只用选定的角色背景。')).toBeVisible();
    expect(current).not.toHaveTextContent('先说结论和下一步');
    expect(matchPersonalityDirection).not.toHaveBeenCalled();

    act(() => useSettingsStore.getState().persistProfile());
    expect(current).not.toHaveTextContent('未保存草稿');
    expect(current).toHaveTextContent('先歇一会儿。今天只留一件必须做的事。');
  });

  it('previews model suggestions without overwriting settings and applies all traits through the existing save path', async () => {
    const user = userEvent.setup();
    useSettingsStore.getState().updateProfile({ bio: '保持原样', greeting: '欢迎回来' });
    const before = useSettingsStore.getState().profile;
    const suggestedTraits = { ...defaultTraits, humor: 2, verbosity: -2 };
    vi.mocked(matchPersonalityDirection).mockResolvedValue({
      direction_id: 'gentle', direction_name: '温和的倾听者', direction_avatar: '',
      suggested_description: '耐心倾听，简洁回应', suggested_traits: JSON.stringify(suggestedTraits),
      suggested_greeting: '另一个开场白',
    });
    render(<PersonalitySettings />);
    await user.type(screen.getByLabelText('希望的性格'), '耐心温和');
    await user.click(screen.getByRole('button', { name: '生成建议' }));
    expect(await screen.findByText('建议：温和的倾听者')).toBeVisible();
    expect(matchPersonalityDirection).toHaveBeenCalledWith('耐心温和');
    expect(useSettingsStore.getState().profile).toEqual(before);
    expect(localStorage.getItem('angelbot_profile')).toBeNull();

    await user.click(screen.getByRole('button', { name: '应用建议' }));
    expect(screen.getByLabelText('相处方式')).toHaveValue('耐心倾听，简洁回应');
    act(() => useSettingsStore.getState().persistProfile());
    expect(JSON.parse(localStorage.getItem('angelbot_profile')!)).toMatchObject({
      name: before.name, greeting: before.greeting, bio: '耐心倾听，简洁回应', traits: suggestedTraits,
    });
  });

  it('imports existing profile traits into the shared draft without saving behind the user’s back', async () => {
    const user = userEvent.setup();
    let picker: HTMLInputElement | undefined;
    const click = vi.spyOn(HTMLInputElement.prototype, 'click').mockImplementation(function (this: HTMLInputElement) {
      picker = this;
    });
    try {
      useSettingsStore.getState().updateProfile({ bio: '尚未保存的相处方式', characterCard, personaNotes: '旧卡补充' });
      const before = useSettingsStore.getState().profile;
      render(<PersonalitySettings />);
      await user.click(screen.getByRole('button', { name: '导入人物卡或配置' }));
      const imported = {
        profile: { name: '原来的助手', bio: '旧配置的相处方式', greeting: '欢迎回来' },
        traits: { ...defaultTraits, tone: -3, humor: 4 },
      };
      const file = new File([], 'angelbot-persona.json', { type: 'application/json' });
      Object.assign(file, { text: async () => JSON.stringify(imported) });
      if (!picker) throw new Error('Expected the existing file picker to open');
      fireEvent.change(picker, { target: { files: [file] } });
      const preview = await screen.findByRole('region', { name: '导入预览' });
      expect(within(preview).getByLabelText('预览名字')).toHaveValue(imported.profile.name);
      expect(preview).toHaveTextContent('导入配置会移除原角色卡');
      expect(useSettingsStore.getState().profile).toBe(before);
      await user.click(within(preview).getByRole('button', { name: '应用到草稿' }));
      expect(screen.getByText(/已应用到草稿/)).toHaveAttribute('role', 'status');
      expect(screen.getByLabelText('名字')).toHaveValue(imported.profile.name);
      expect(screen.getByLabelText('相处方式')).toHaveValue(imported.profile.bio);
      expect(useSettingsStore.getState().profile.traits).toEqual(imported.traits);
      expect(useSettingsStore.getState().profile.characterCard).toBeUndefined();
      expect(useSettingsStore.getState().profile.personaNotes).toBe('');
      expect(localStorage.getItem('angelbot_profile')).toBeNull();
      act(() => useSettingsStore.getState().persistProfile());
      expect(JSON.parse(localStorage.getItem('angelbot_profile')!).traits).toEqual(imported.traits);
      expect(JSON.parse(localStorage.getItem('angelbot_profile')!)).not.toHaveProperty('characterCard');
      act(() => useSettingsStore.getState().loadProfile());
      expect(useSettingsStore.getState().profile).toMatchObject({ name: imported.profile.name, traits: imported.traits });
      expect(useSettingsStore.getState().profile.characterCard).toBeUndefined();
      expect(useSettingsStore.getState().profile.personaNotes).toBe('');
    } finally {
      click.mockRestore();
    }
  });

  it('previews a V3 text card, cancels without changing drafts, and explicitly applies before saving', async () => {
    const user = userEvent.setup();
    let picker: HTMLInputElement | undefined;
    const click = vi.spyOn(HTMLInputElement.prototype, 'click').mockImplementation(function (this: HTMLInputElement) { picker = this; });
    try {
      useSettingsStore.getState().updateProfile({ bio: '已有的未保存草稿', traits: { ...defaultTraits, humor: 2 } });
      const before = useSettingsStore.getState().profile;
      render(<PersonalitySettings />);
      const data = {
        name: '星汐', description: '自然陪伴，先听我说', personality: '安静耐心',
        scenario: '{{char}}与{{user}}在图书馆。', first_mes: '欢迎回来，{{user}}。',
        system_prompt: 'CARDSYS_ONLY_ARCHIVE', post_history_instructions: 'CARDPOST_ONLY_ARCHIVE',
        mes_example: '<START>\n{{char}}: 你好', alternate_greetings: ['你好呀'],
        character_book: { entries: [{ keys: ['地图'], content: '世界书条目' }] }, extensions: { sample: '保留原文' },
      };
      const file = new File([], 'xingxi.json', { type: 'application/json' });
      Object.assign(file, { text: async () => JSON.stringify({ spec: 'chara_card_v3', spec_version: '3.0', data }) });
      const choose = async () => {
        await user.click(screen.getByRole('button', { name: '导入人物卡或配置' }));
        if (!picker) throw new Error('Expected the existing file picker to open');
        fireEvent.change(picker, { target: { files: [file] } });
        return screen.findByRole('region', { name: '导入预览' });
      };
      let preview = await choose();
      expect(within(preview).getByLabelText('预览名字')).toHaveValue('星汐');
      expect(within(preview).getByLabelText('预览场景')).toHaveValue(data.scenario);
      expect(within(preview).getByLabelText('预览场景')).toHaveProperty('readOnly', true);
      expect(preview).toHaveTextContent('保存后作为对话背景，不执行世界书规则或写入个人记忆');
      expect(within(preview).getByLabelText('预览人设补充')).toHaveValue('');
      await user.click(within(preview).getByText(/从随卡世界书挑选内容/));
      await user.click(within(preview).getByRole('checkbox', { name: '条目 1' }));
      await user.click(within(preview).getByRole('button', { name: '将所选文字加入人设补充' }));
      expect(within(preview).getByLabelText('预览人设补充')).toHaveValue('条目 1：\n世界书条目');
      expect(useSettingsStore.getState().profile).toBe(before);
      expect(localStorage.getItem('angelbot_profile')).toBeNull();
      await user.click(within(preview).getByRole('button', { name: '取消导入' }));
      expect(screen.queryByRole('region', { name: '导入预览' })).not.toBeInTheDocument();
      expect(useSettingsStore.getState().profile).toBe(before);

      preview = await choose();
      expect(within(preview).getByLabelText('预览人设补充')).toHaveValue('');
      await user.type(within(preview).getByLabelText('预览人设补充'), '只保留我编辑的角色背景');
      await user.click(within(preview).getByRole('button', { name: '应用到草稿' }));
      expect(localStorage.getItem('angelbot_profile')).toBeNull();
      expect(useSettingsStore.getState().profile).toMatchObject({
        name: data.name, bio: data.description, greeting: data.first_mes, traits: before.traits, personaNotes: '只保留我编辑的角色背景',
        characterCard: { specVersion: '3.0', personality: data.personality, scenario: data.scenario, systemPrompt: data.system_prompt, postHistoryInstructions: data.post_history_instructions, mesExample: data.mes_example, alternateGreetings: data.alternate_greetings },
      });
      act(() => useSettingsStore.getState().persistProfile());
      act(() => useSettingsStore.getState().loadProfile());
      expect(useSettingsStore.getState().profile.characterCard?.rawData).toMatchObject({ character_book: data.character_book, extensions: data.extensions });
      expect(useSettingsStore.getState().profile.characterCard?.rawData).not.toHaveProperty('personaNotes');
      expect(useSettingsStore.getState().profile.personaNotes).toBe('只保留我编辑的角色背景');
      expect(matchPersonalityDirection).not.toHaveBeenCalled();
    } finally {
      click.mockRestore();
    }
  });

  it('shows pending proposals and accepts edited memory proposals', async () => {
    render(<PersonalitySettings />);

    await userEvent.click(screen.getByRole('button', { name: '查看详情' }));

    expect(screen.getByText(/会使用当前模型额度/)).toBeInTheDocument();
    expect(screen.getByText('当前设定的偏好')).toBeInTheDocument();
    expect(screen.queryByText('我学到的关于你')).not.toBeInTheDocument();
    expect(await screen.findByText('待确认的学习项')).toBeInTheDocument();
    const contentInput = await screen.findByDisplayValue('用户喜欢 Rust');
    await userEvent.clear(contentInput);
    await userEvent.type(contentInput, '用户非常喜欢 Rust');
    await userEvent.click(screen.getByLabelText('不遗忘'));
    await userEvent.click(screen.getByRole('button', { name: '接受' }));

    await waitFor(() => {
      expect(acceptEvolutionProposal).toHaveBeenCalledWith({
        id: 'proposal-1',
        content: '用户非常喜欢 Rust',
        importance: 8,
        permanent: true,
      });
    });
  });

  it('keeps learning status aligned with both switches without changing the saved choice or hiding owner controls', async () => {
    const user = userEvent.setup();
    const habit = { id: 'ready-habit', content: '喜欢先说结论', ready: true, updatedAt: 1 };
    vi.mocked(getLearnedPreferences).mockResolvedValue([
      habit,
      { id: 'pending-habit', content: '喜欢简短问候', ready: false, updatedAt: 2 },
    ]);
    vi.mocked(getEvolutionProposals).mockResolvedValue([]);
    render(<PersonalitySettings />);
    await user.click(screen.getByRole('button', { name: '查看详情' }));
    expect(await screen.findByText('可作为回应参考')).toBeInTheDocument();
    expect(screen.getByText('还在了解')).toBeInTheDocument();

    const setMemory = (enabled: boolean) => act(() => usePreferencesStore.setState((state) => ({
      preferences: { ...state.preferences, habits: { ...state.preferences.habits, useLongTermMemory: enabled } },
    })));
    const learning = screen.getByRole('checkbox', { name: '启用偏好学习' });
    setMemory(false);
    expect(learning).toBeChecked();
    expect(learning).toBeEnabled();
    expect(screen.getByText(/长期记忆已关闭，偏好学习暂停/)).toBeInTheDocument();
    expect(screen.getAllByText('已暂停沿用')).toHaveLength(2);
    expect(screen.queryByText('可作为回应参考')).not.toBeInTheDocument();

    await user.click(learning);
    await waitFor(() => expect(learning).not.toBeChecked());
    expect(saveMemoryControls).toHaveBeenLastCalledWith({ evolutionEnabled: false });
    expect(screen.getByText(/偏好学习已关闭/)).toBeInTheDocument();
    setMemory(true);
    expect(learning).not.toBeChecked();
    expect(screen.getAllByText('已暂停沿用')).toHaveLength(2);
    await user.click(learning);
    expect(await screen.findByText('可作为回应参考')).toBeInTheDocument();
    expect(saveMemoryControls).toHaveBeenLastCalledWith({ evolutionEnabled: true });
    expect(screen.getByText('还在了解')).toBeInTheDocument();
    expect(screen.getByText(/会使用当前模型额度/)).toBeInTheDocument();
    expect(getLearnedPreferences).toHaveBeenCalledTimes(1);

    setMemory(false);
    await user.click(screen.getByRole('button', { name: `不再沿用：${habit.content}` }));
    await waitFor(() => expect(dismissLearnedPreference).toHaveBeenCalledWith(habit.id));
    expect(screen.queryByText(habit.content)).not.toBeInTheDocument();
    expect(screen.getByText('喜欢简短问候')).toBeInTheDocument();
    expect(screen.getByText('已暂停沿用')).toBeInTheDocument();
  });

  it('rejects pending proposals', async () => {
    render(<PersonalitySettings />);

    await userEvent.click(screen.getByRole('button', { name: '查看详情' }));
    await screen.findByDisplayValue('用户喜欢 Rust');
    await userEvent.click(screen.getByRole('button', { name: '拒绝' }));

    await waitFor(() => {
      expect(rejectEvolutionProposal).toHaveBeenCalledWith('proposal-1');
    });
  });
});

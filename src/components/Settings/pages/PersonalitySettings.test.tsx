import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
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
  rejectEvolutionProposal,
} from '$lib/commands/memory';

vi.mock('$lib/commands/memory', () => ({
  acceptEvolutionProposal: vi.fn(),
  getEvolutionProposals: vi.fn(),
  getMemories: vi.fn(),
  getLearnedPreferences: vi.fn(),
  dismissLearnedPreference: vi.fn(),
  rejectEvolutionProposal: vi.fn(),
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

describe('PersonalitySettings', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    usePreferencesStore.setState({ preferences: defaultPreferences });
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

  it('starts with everyday fields and preserves advanced settings across disclosure, saving and reopening', async () => {
    const user = userEvent.setup();
    const characterCard = {
      specVersion: '2.0', personality: '耐心', scenario: '', mesExample: '',
      systemPrompt: '已有的角色设定', postHistoryInstructions: '',
      alternateGreetings: [], creatorNotes: '', creator: '', tags: [],
    };
    useSettingsStore.getState().updateProfile({
      bio: '原有的相处方式', greeting: '欢迎回来',
      traits: { ...defaultTraits, humor: 4 }, characterCard,
    });
    const { unmount } = render(<PersonalitySettings />);

    expect(screen.getByLabelText('名字')).toBeVisible();
    expect(screen.getByLabelText('相处方式')).toBeVisible();
    expect(screen.getByRole('checkbox', { name: '启用偏好学习' })).toBeVisible();
    expect(screen.queryByLabelText(/头像/i)).not.toBeInTheDocument();
    expect(screen.queryByRole('slider')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('系统提示')).not.toBeInTheDocument();
    expect(screen.getByText('已导入角色卡')).toBeVisible();
    expect(matchPersonalityDirection).not.toHaveBeenCalled();

    await user.clear(screen.getByLabelText('相处方式'));
    await user.type(screen.getByLabelText('相处方式'), '自然一点，先说结论');
    await user.click(screen.getByText('高级设置', { exact: true }));
    expect(screen.getByLabelText('系统提示')).toBeVisible();
    await user.click(screen.getByRole('button', { name: '性格微调' }));
    const humor = screen.getByRole('slider', { name: '无趣 — 幽默' });
    expect(humor).toHaveValue('4');
    fireEvent.change(humor, { target: { value: '2' } });
    await user.click(screen.getByText('高级设置', { exact: true }));
    expect(humor).not.toBeInTheDocument();
    expect(screen.getByLabelText('相处方式')).toHaveValue('自然一点，先说结论');
    expect(screen.queryByRole('button', { name: '保存所有设置' })).not.toBeInTheDocument();

    act(() => useSettingsStore.getState().persistProfile());
    unmount();
    act(() => useSettingsStore.getState().loadProfile());
    render(<PersonalitySettings />);
    expect(screen.getByLabelText('相处方式')).toHaveValue('自然一点，先说结论');
    expect(useSettingsStore.getState().profile).toMatchObject({
      traits: { ...defaultTraits, humor: 2 }, characterCard, greeting: '欢迎回来',
    });
    expect(screen.queryByRole('slider')).not.toBeInTheDocument();
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
    await user.click(screen.getByText('高级设置', { exact: true }));
    await user.click(screen.getByRole('button', { name: '模型辅助设定' }));
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
      render(<PersonalitySettings />);
      await user.click(screen.getByText('高级设置', { exact: true }));
      await user.click(screen.getByRole('button', { name: '导入/导出' }));
      await user.click(screen.getByRole('button', { name: '导入配置' }));
      const imported = {
        profile: { name: '原来的助手', bio: '旧配置的相处方式', greeting: '欢迎回来' },
        traits: { ...defaultTraits, tone: -3, humor: 4 },
      };
      const file = new File([], 'angelbot-persona.json', { type: 'application/json' });
      Object.assign(file, { text: async () => JSON.stringify(imported) });
      if (!picker) throw new Error('Expected the existing file picker to open');
      fireEvent.change(picker, { target: { files: [file] } });
      expect(await screen.findByRole('status')).toHaveTextContent('配置已导入');
      expect(screen.getByLabelText('名字')).toHaveValue(imported.profile.name);
      expect(screen.getByLabelText('相处方式')).toHaveValue(imported.profile.bio);
      expect(useSettingsStore.getState().profile.traits).toEqual(imported.traits);
      expect(localStorage.getItem('angelbot_profile')).toBeNull();
      act(() => useSettingsStore.getState().persistProfile());
      expect(JSON.parse(localStorage.getItem('angelbot_profile')!).traits).toEqual(imported.traits);
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
    expect(learning).not.toBeChecked();
    expect(screen.getByText(/偏好学习已关闭/)).toBeInTheDocument();
    setMemory(true);
    expect(learning).not.toBeChecked();
    expect(screen.getAllByText('已暂停沿用')).toHaveLength(2);
    await user.click(learning);
    expect(screen.getByText('可作为回应参考')).toBeInTheDocument();
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

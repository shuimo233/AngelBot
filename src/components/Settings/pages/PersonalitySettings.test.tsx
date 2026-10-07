import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { PersonalitySettings } from './PersonalitySettings';
import { usePreferencesStore } from '$stores/preferences';
import { useSettingsStore } from '$stores/settings';
import {
  acceptEvolutionProposal,
  getEvolutionProposals,
  getMemories,
  rejectEvolutionProposal,
} from '$lib/commands/memory';

vi.mock('$lib/commands/memory', () => ({
  acceptEvolutionProposal: vi.fn(),
  getEvolutionProposals: vi.fn(),
  getMemories: vi.fn(),
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

describe('PersonalitySettings evolution proposals', () => {
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

  it('keeps the identity settings to a name and does not expose avatar editing', async () => {
    const user = userEvent.setup();
    render(<PersonalitySettings />);

    await user.click(screen.getByRole('button', { name: '基础信息' }));

    expect(screen.getByLabelText('名字')).toBeInTheDocument();
    expect(screen.queryByLabelText(/头像/i)).not.toBeInTheDocument();
  });

  it('shows pending proposals and accepts edited memory proposals', async () => {
    render(<PersonalitySettings />);

    await userEvent.click(screen.getByText('自进化').closest('.settings-section-header')!);
    await userEvent.click(screen.getByRole('button', { name: '查看详情' }));

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

  it('rejects pending proposals', async () => {
    render(<PersonalitySettings />);

    await userEvent.click(screen.getByText('自进化').closest('.settings-section-header')!);
    await userEvent.click(screen.getByRole('button', { name: '查看详情' }));
    await screen.findByDisplayValue('用户喜欢 Rust');
    await userEvent.click(screen.getByRole('button', { name: '拒绝' }));

    await waitFor(() => {
      expect(rejectEvolutionProposal).toHaveBeenCalledWith('proposal-1');
    });
  });
});

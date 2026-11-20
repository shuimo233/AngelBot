import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { initializeMemoryControls, saveMemoryControls } from '$lib/commands/memory';
import { DefaultWorkModeSettings } from './DefaultWorkModeSettings';
import { usePreferencesStore } from '$stores/preferences';
import { useThinkingEffortStore } from '$stores/thinkingEffort';

vi.mock('$lib/commands/memory', () => ({
  initializeMemoryControls: vi.fn(),
  saveMemoryControls: vi.fn(),
}));

const preferences = {
  communication: { preferredTone: [], dislikedWords: [], petPeeves: [] },
  habits: {
    greetingStyle: '',
    responseLength: 'medium' as const,
    responseLanguage: 'auto' as const,
    useLongTermMemory: true,
  },
  topics: { interests: [], avoidTopics: [] },
  learnedAt: 0,
  evolutionEnabled: true,
};

describe('DefaultWorkModeSettings', () => {
  beforeEach(() => {
    localStorage.clear();
    usePreferencesStore.setState({ preferences });
    useThinkingEffortStore.setState({ effort: 'medium' });
    vi.mocked(initializeMemoryControls).mockImplementation(async (legacy) => legacy);
    vi.mocked(saveMemoryControls).mockResolvedValue({ useLongTermMemory: false, evolutionEnabled: true });
  });

  it('persists response defaults and changes the thinking default', async () => {
    const user = userEvent.setup();
    render(<DefaultWorkModeSettings />);

    await user.click(screen.getByRole('radio', { name: '简洁' }));
    await user.click(screen.getByRole('radio', { name: 'English' }));
    await user.click(screen.getByRole('radio', { name: '深入' }));
    await user.click(screen.getByLabelText('使用长期记忆'));

    await waitFor(() => expect(usePreferencesStore.getState().preferences.habits).toMatchObject({
      responseLength: 'short',
      responseLanguage: 'en-US',
      useLongTermMemory: false,
    }));
    expect(saveMemoryControls).toHaveBeenCalledWith({ useLongTermMemory: false });
    expect(useThinkingEffortStore.getState().effort).toBe('high');
    expect(JSON.parse(localStorage.getItem('angelbot_preferences') ?? '{}').habits).toMatchObject({
      responseLanguage: 'en-US',
    });
  });
});

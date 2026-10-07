import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it } from 'vitest';
import { DefaultWorkModeSettings } from './DefaultWorkModeSettings';
import { usePreferencesStore } from '$stores/preferences';
import { useThinkingEffortStore } from '$stores/thinkingEffort';

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
  });

  it('persists response defaults and changes the thinking default', async () => {
    const user = userEvent.setup();
    render(<DefaultWorkModeSettings />);

    await user.selectOptions(screen.getByLabelText('默认回复长度'), 'short');
    await user.selectOptions(screen.getByLabelText('默认回复语言'), 'en-US');
    await user.selectOptions(screen.getByLabelText('默认推理强度'), 'high');
    await user.click(screen.getByLabelText('使用长期记忆'));

    expect(usePreferencesStore.getState().preferences.habits).toMatchObject({
      responseLength: 'short',
      responseLanguage: 'en-US',
      useLongTermMemory: false,
    });
    expect(useThinkingEffortStore.getState().effort).toBe('high');
    expect(JSON.parse(localStorage.getItem('angelbot_preferences') ?? '{}').habits).toMatchObject({
      responseLanguage: 'en-US',
    });
  });
});

import { create } from 'zustand';
import type { UserPreferences } from '$types';

interface PreferencesState {
  preferences: UserPreferences;
  loadPreferences: () => void;
  updatePreferences: (partial: Partial<UserPreferences>) => void;
  persistPreferences: () => void;
  setEvolutionEnabled: (enabled: boolean) => void;
  addInterest: (topic: string) => void;
  removeInterest: (topic: string) => void;
  addAvoidTopic: (topic: string) => void;
  removeAvoidTopic: (topic: string) => void;
  addDislikedWord: (word: string) => void;
  removeDislikedWord: (word: string) => void;
}

const STORAGE_KEY = 'angelbot_preferences';

const defaultPreferences: UserPreferences = {
  communication: {
    preferredTone: [],
    dislikedWords: [],
    petPeeves: [],
  },
  habits: {
    greetingStyle: '',
    responseLength: 'medium',
    responseLanguage: 'auto',
    useLongTermMemory: true,
  },
  topics: {
    interests: [],
    avoidTopics: [],
  },
  learnedAt: Date.now(),
  evolutionEnabled: true,
};

function loadFromStorage(): UserPreferences {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored) {
      const parsed = JSON.parse(stored) as Partial<UserPreferences>;
      return {
        ...defaultPreferences,
        ...parsed,
        communication: { ...defaultPreferences.communication, ...parsed.communication },
        habits: { ...defaultPreferences.habits, ...parsed.habits },
        topics: { ...defaultPreferences.topics, ...parsed.topics },
      };
    }
  } catch {
    // ignore
  }
  return defaultPreferences;
}

export const usePreferencesStore = create<PreferencesState>((set, get) => ({
  preferences: loadFromStorage(),

  loadPreferences: () => {
    set({ preferences: loadFromStorage() });
  },

  updatePreferences: (partial) => {
    set((state) => ({
      preferences: { ...state.preferences, ...partial },
    }));
  },

  persistPreferences: () => {
    const { preferences } = get();
    localStorage.setItem(STORAGE_KEY, JSON.stringify(preferences));
  },

  setEvolutionEnabled: (enabled) => {
    set((state) => ({
      preferences: { ...state.preferences, evolutionEnabled: enabled },
    }));
    get().persistPreferences();
  },

  addInterest: (topic) => {
    const { preferences } = get();
    if (!preferences.topics.interests.includes(topic)) {
      set({
        preferences: {
          ...preferences,
          topics: {
            ...preferences.topics,
            interests: [...preferences.topics.interests, topic],
          },
          learnedAt: Date.now(),
        },
      });
      get().persistPreferences();
    }
  },

  removeInterest: (topic) => {
    const { preferences } = get();
    set({
      preferences: {
        ...preferences,
        topics: {
          ...preferences.topics,
          interests: preferences.topics.interests.filter((t) => t !== topic),
        },
        learnedAt: Date.now(),
      },
    });
    get().persistPreferences();
  },

  addAvoidTopic: (topic) => {
    const { preferences } = get();
    if (!preferences.topics.avoidTopics.includes(topic)) {
      set({
        preferences: {
          ...preferences,
          topics: {
            ...preferences.topics,
            avoidTopics: [...preferences.topics.avoidTopics, topic],
          },
          learnedAt: Date.now(),
        },
      });
      get().persistPreferences();
    }
  },

  removeAvoidTopic: (topic) => {
    const { preferences } = get();
    set({
      preferences: {
        ...preferences,
        topics: {
          ...preferences.topics,
          avoidTopics: preferences.topics.avoidTopics.filter((t) => t !== topic),
        },
        learnedAt: Date.now(),
      },
    });
    get().persistPreferences();
  },

  addDislikedWord: (word) => {
    const { preferences } = get();
    if (!preferences.communication.dislikedWords.includes(word)) {
      set({
        preferences: {
          ...preferences,
          communication: {
            ...preferences.communication,
            dislikedWords: [...preferences.communication.dislikedWords, word],
          },
          learnedAt: Date.now(),
        },
      });
      get().persistPreferences();
    }
  },

  removeDislikedWord: (word) => {
    const { preferences } = get();
    set({
      preferences: {
        ...preferences,
        communication: {
          ...preferences.communication,
          dislikedWords: preferences.communication.dislikedWords.filter((w) => w !== word),
        },
        learnedAt: Date.now(),
      },
    });
    get().persistPreferences();
  },
}));

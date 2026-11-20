import { create } from 'zustand';
import { initializeMemoryControls, saveMemoryControls, type MemoryControls } from '$lib/commands/memory';
import type { UserPreferences } from '$types';

interface PreferencesState {
  preferences: UserPreferences;
  loadPreferences: () => Promise<void>;
  updatePreferences: (partial: Partial<UserPreferences>) => void;
  persistPreferences: () => void;
  setMemoryControls: (partial: Partial<MemoryControls>) => Promise<void>;
  memoryControlsBusy: boolean;
  memoryControlsError: string | null;
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

function controlsFromPreferences(preferences: UserPreferences): MemoryControls {
  return {
    useLongTermMemory: preferences.habits.useLongTermMemory ?? true,
    evolutionEnabled: preferences.evolutionEnabled,
  };
}

function withMemoryControls(preferences: UserPreferences, controls: MemoryControls): UserPreferences {
  return {
    ...preferences,
    habits: { ...preferences.habits, useLongTermMemory: controls.useLongTermMemory },
    evolutionEnabled: controls.evolutionEnabled,
  };
}

let initializingMemoryControls: Promise<void> | undefined;
let memoryControlsLoaded = false;

export const usePreferencesStore = create<PreferencesState>((set, get) => ({
  preferences: loadFromStorage(),
  memoryControlsBusy: false,
  memoryControlsError: null,

  loadPreferences: () => {
    if (memoryControlsLoaded) return Promise.resolve();
    if (initializingMemoryControls) return initializingMemoryControls;

    set({ memoryControlsBusy: true, memoryControlsError: null });
    initializingMemoryControls = Promise.resolve()
      .then(() => initializeMemoryControls(controlsFromPreferences(get().preferences)))
      .then((controls) => {
        set((state) => ({ preferences: withMemoryControls(state.preferences, controls) }));
        memoryControlsLoaded = true;
      })
      .catch((error: unknown) => {
        set({ memoryControlsError: String(error) });
      })
      .finally(() => {
        initializingMemoryControls = undefined;
        set({ memoryControlsBusy: false });
      });
    return initializingMemoryControls;
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

  setMemoryControls: async (partial) => {
    await get().loadPreferences();
    if (!memoryControlsLoaded || get().memoryControlsBusy) return;
    set({ memoryControlsBusy: true, memoryControlsError: null });
    try {
      const controls = await saveMemoryControls(partial);
      set((state) => ({ preferences: withMemoryControls(state.preferences, controls) }));
    } catch (error) {
      set({ memoryControlsError: String(error) });
    } finally {
      set({ memoryControlsBusy: false });
    }
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

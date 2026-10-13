import { create } from 'zustand';

interface AppearancePreferences {
  chatFontSize: number;
  reduceMotion: boolean;
  bgImage: string;
  bgOpacity: number;
}

function read(key: string): string | null {
  try { return localStorage.getItem(key); } catch { return null; }
}

function bounded(value: number, min: number, max: number, fallback: number) {
  return Number.isFinite(value) ? Math.round(Math.min(max, Math.max(min, value))) : fallback;
}

function readPreferences(): AppearancePreferences {
  return {
    chatFontSize: bounded(Number(read('chatFontSize') ?? 14), 12, 20, 14),
    reduceMotion: read('reduceMotion') === 'true',
    bgImage: read('bgImage') ?? '',
    bgOpacity: bounded(Number(read('bgOpacity') ?? 30), 0, 100, 30),
  };
}

function apply(preferences: AppearancePreferences) {
  const root = document.documentElement;
  root.style.setProperty('--ui-chat-font-size', `${preferences.chatFontSize}px`);
  root.style.setProperty('--bg-image', preferences.bgImage ? `url(${JSON.stringify(preferences.bgImage)})` : 'none');
  root.style.setProperty('--bg-opacity', String(preferences.bgOpacity / 100));
  root.toggleAttribute('data-reduced-motion', preferences.reduceMotion);
}

interface AppearanceState extends AppearancePreferences {
  persistenceError: boolean;
  update: (patch: Partial<AppearancePreferences>) => void;
}

export const useAppearanceStore = create<AppearanceState>((set, get) => ({
  ...readPreferences(),
  persistenceError: false,
  update: (patch) => {
    const current = get();
    const preferences = { ...current, ...patch };
    preferences.chatFontSize = bounded(preferences.chatFontSize, 12, 20, 14);
    preferences.bgOpacity = bounded(preferences.bgOpacity, 0, 100, 30);
    apply(preferences);
    let persistenceError = false;
    try {
      // Retry the complete state after a failed write; a later slider edit must
      // not clear the warning while an earlier background change remains unsaved.
      if ('chatFontSize' in patch || current.persistenceError) localStorage.setItem('chatFontSize', String(preferences.chatFontSize));
      if ('reduceMotion' in patch || current.persistenceError) localStorage.setItem('reduceMotion', String(preferences.reduceMotion));
      if ('bgOpacity' in patch || current.persistenceError) localStorage.setItem('bgOpacity', String(preferences.bgOpacity));
      if ('bgImage' in patch || current.persistenceError) {
        if (preferences.bgImage) localStorage.setItem('bgImage', preferences.bgImage);
        else localStorage.removeItem('bgImage');
      }
    } catch { persistenceError = true; }
    set({ ...preferences, persistenceError });
  },
}));

/** Restore before opening Settings; background and reading preferences belong to the app. */
export function initializeAppearance() {
  const preferences = readPreferences();
  useAppearanceStore.setState({ ...preferences, persistenceError: false });
  apply(preferences);
}

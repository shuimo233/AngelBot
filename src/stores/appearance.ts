import { create } from 'zustand';

export const CHAT_TYPOGRAPHY = { minSize: 12, maxSize: 20, defaultSize: 14, maxFamilyLength: 80 };
const DEFAULT_BACKGROUND_OPACITY = 15;

interface AppearancePreferences {
  chatFontSize: number;
  chatFontFamily: string;
  reduceMotion: boolean;
  bgImage: string;
  bgOpacity: number;
}

function read(key: string): string | null {
  try { return localStorage.getItem(key); } catch { return null; }
}

export function readReducedMotionPreference(): boolean {
  return read('reduceMotion') === 'true';
}

function bounded(value: number, min: number, max: number, fallback: number) {
  return Number.isFinite(value) ? Math.round(Math.min(max, Math.max(min, value))) : fallback;
}

function fontSize(value: number) {
  const { minSize, maxSize, defaultSize } = CHAT_TYPOGRAPHY;
  return bounded(value, minSize, maxSize, defaultSize);
}

function fontFamily(value: string) {
  return value.replace(/[\x00-\x1f\x7f]/g, '').slice(0, CHAT_TYPOGRAPHY.maxFamilyLength);
}

function readPreferences(): AppearancePreferences {
  return {
    chatFontSize: fontSize(Number(read('chatFontSize') ?? CHAT_TYPOGRAPHY.defaultSize)),
    chatFontFamily: fontFamily(read('chatFontFamily') ?? ''),
    reduceMotion: readReducedMotionPreference(),
    bgImage: read('bgImage') ?? '',
    bgOpacity: bounded(Number(read('bgOpacity') ?? DEFAULT_BACKGROUND_OPACITY), 0, 100, DEFAULT_BACKGROUND_OPACITY),
  };
}

function apply(preferences: AppearancePreferences) {
  const root = document.documentElement;
  root.style.setProperty('--ui-chat-font-size', `${preferences.chatFontSize}px`);
  const family = preferences.chatFontFamily.trim();
  if (family) root.style.setProperty('--ui-chat-font-family', `${JSON.stringify(family)}, var(--ui-font-sans)`);
  else root.style.removeProperty('--ui-chat-font-family');
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
    preferences.chatFontSize = fontSize(preferences.chatFontSize);
    preferences.chatFontFamily = fontFamily(preferences.chatFontFamily);
    preferences.bgOpacity = bounded(preferences.bgOpacity, 0, 100, DEFAULT_BACKGROUND_OPACITY);
    apply(preferences);
    let persistenceError = false;
    try {
      // Retry the complete state after a failed write; a later slider edit must
      // not clear the warning while an earlier background change remains unsaved.
      if ('chatFontSize' in patch || current.persistenceError) localStorage.setItem('chatFontSize', String(preferences.chatFontSize));
      if ('chatFontFamily' in patch || current.persistenceError) {
        if (preferences.chatFontFamily.trim()) localStorage.setItem('chatFontFamily', preferences.chatFontFamily);
        else localStorage.removeItem('chatFontFamily');
      }
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

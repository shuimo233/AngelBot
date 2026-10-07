import { create } from 'zustand';

export type Theme = 'dark' | 'light' | 'system';
type ResolvedTheme = Exclude<Theme, 'system'>;

interface ThemeState {
  theme: Theme;
  resolvedTheme: ResolvedTheme;
  setTheme: (theme: Theme) => void;
  toggleTheme: () => void;
}

function readTheme(): Theme {
  try {
    const stored = localStorage.getItem('theme');
    if (stored === 'dark' || stored === 'light' || stored === 'system') return stored;
  } catch {
    // Appearance remains usable when browser storage is unavailable.
  }
  return 'light';
}

function systemThemeQuery(): MediaQueryList | undefined {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return undefined;
  return window.matchMedia('(prefers-color-scheme: dark)');
}

function resolveTheme(theme: Theme, systemDark = systemThemeQuery()?.matches ?? false): ResolvedTheme {
  return theme === 'system' ? (systemDark ? 'dark' : 'light') : theme;
}

function applyTheme(theme: Theme, systemDark?: boolean) {
  const resolvedTheme = resolveTheme(theme, systemDark);
  useThemeStore.setState({ theme, resolvedTheme });
  if (typeof document !== 'undefined') {
    document.documentElement.classList.toggle('dark', resolvedTheme === 'dark');
  }
}

const initialTheme = readTheme();

export const useThemeStore = create<ThemeState>((_set, get) => ({
  theme: initialTheme,
  resolvedTheme: resolveTheme(initialTheme),
  setTheme: (theme) => {
    applyTheme(theme);
    try {
      localStorage.setItem('theme', theme);
    } catch {
      // Keep the current appearance even if it cannot be persisted.
    }
  },
  toggleTheme: () => get().setTheme(get().resolvedTheme === 'dark' ? 'light' : 'dark'),
}));

/** Attach system-theme changes for the app lifetime, including StrictMode remounts. */
export function initializeTheme(): () => void {
  const media = systemThemeQuery();
  applyTheme(readTheme(), media?.matches ?? false);

  const onSystemThemeChange = (event: MediaQueryListEvent) => {
    if (useThemeStore.getState().theme === 'system') applyTheme('system', event.matches);
  };

  if (media?.addEventListener) {
    media.addEventListener('change', onSystemThemeChange);
    return () => media.removeEventListener('change', onSystemThemeChange);
  }
  if (media?.addListener) {
    media.addListener(onSystemThemeChange);
    return () => media.removeListener(onSystemThemeChange);
  }
  return () => {};
}

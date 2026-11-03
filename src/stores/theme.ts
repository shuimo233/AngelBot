import { create } from 'zustand';

export type Theme = 'dark' | 'light' | 'system';
export type AppLayout = 'A' | 'B';
type ResolvedTheme = Exclude<Theme, 'system'>;

interface ThemeState {
  theme: Theme;
  resolvedTheme: ResolvedTheme;
  layout: AppLayout;
  setLayout: (layout: AppLayout) => void;
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

function readLayout(): AppLayout {
  try {
    if (localStorage.getItem('ui-layout') === 'B') return 'B';
  } catch {
    // A remains usable without browser storage.
  }
  return 'A';
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
  layout: readLayout(),
  setLayout: (layout) => {
    useThemeStore.setState({ layout });
    try { localStorage.setItem('ui-layout', layout); } catch {
      // The current layout still changes when persistence is unavailable.
    }
  },
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
  useThemeStore.setState({ layout: readLayout() });
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

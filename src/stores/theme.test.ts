import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { initializeTheme, useThemeStore } from './theme';

function mockSystemTheme(dark: boolean) {
  const listeners = new Set<(event: MediaQueryListEvent) => void>();
  const media = {
    matches: dark,
    addEventListener: vi.fn((_name: string, listener: (event: MediaQueryListEvent) => void) => {
      listeners.add(listener);
    }),
    removeEventListener: vi.fn((_name: string, listener: (event: MediaQueryListEvent) => void) => {
      listeners.delete(listener);
    }),
  };
  vi.stubGlobal('matchMedia', vi.fn(() => media));
  return {
    media,
    change(matches: boolean) {
      media.matches = matches;
      listeners.forEach((listener) => listener({ matches } as MediaQueryListEvent));
    },
  };
}

describe('theme', () => {
  let dispose: (() => void) | undefined;

  beforeEach(() => {
    localStorage.clear();
    document.documentElement.classList.remove('dark');
    useThemeStore.setState({ theme: 'light', resolvedTheme: 'light' });
  });

  afterEach(() => {
    dispose?.();
    dispose = undefined;
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it.each(['light', 'dark'] as const)('restores a persisted %s theme', (theme) => {
    mockSystemTheme(theme !== 'dark');
    localStorage.setItem('theme', theme);

    dispose = initializeTheme();

    expect(useThemeStore.getState().theme).toBe(theme);
    expect(useThemeStore.getState().resolvedTheme).toBe(theme);
    expect(document.documentElement.classList.contains('dark')).toBe(theme === 'dark');
  });

  it('defaults to light for an invalid preference and removes a stale dark class', () => {
    localStorage.setItem('theme', 'unknown');
    document.documentElement.classList.add('dark');

    dispose = initializeTheme();

    expect(useThemeStore.getState().theme).toBe('light');
    expect(document.documentElement).not.toHaveClass('dark');
  });

  it('follows live system changes without overwriting the system preference', () => {
    const system = mockSystemTheme(true);
    localStorage.setItem('theme', 'system');
    dispose = initializeTheme();
    expect(useThemeStore.getState().resolvedTheme).toBe('dark');
    expect(document.documentElement).toHaveClass('dark');

    system.change(false);

    expect(useThemeStore.getState().theme).toBe('system');
    expect(useThemeStore.getState().resolvedTheme).toBe('light');
    expect(localStorage.getItem('theme')).toBe('system');
    expect(document.documentElement).not.toHaveClass('dark');
  });

  it('makes the quick toggle the explicit opposite of the resolved system theme', () => {
    const system = mockSystemTheme(true);
    localStorage.setItem('theme', 'system');
    dispose = initializeTheme();

    useThemeStore.getState().toggleTheme();
    system.change(false);
    system.change(true);

    expect(useThemeStore.getState().theme).toBe('light');
    expect(useThemeStore.getState().resolvedTheme).toBe('light');
    expect(localStorage.getItem('theme')).toBe('light');
    expect(document.documentElement).not.toHaveClass('dark');
    useThemeStore.getState().toggleTheme();
    expect(useThemeStore.getState().theme).toBe('dark');
    expect(localStorage.getItem('theme')).toBe('dark');
    expect(document.documentElement).toHaveClass('dark');
  });

  it('removes its system listener on cleanup and supports remounting', () => {
    const system = mockSystemTheme(false);
    localStorage.setItem('theme', 'system');
    dispose = initializeTheme();
    dispose();
    expect(system.media.removeEventListener).toHaveBeenCalledWith('change', expect.any(Function));

    system.change(true);
    expect(document.documentElement).not.toHaveClass('dark');

    dispose = initializeTheme();
    expect(document.documentElement).toHaveClass('dark');
    expect(system.media.addEventListener).toHaveBeenCalledTimes(2);
  });

  it('resolves system to light when matchMedia is unavailable', () => {
    vi.stubGlobal('matchMedia', undefined);
    localStorage.setItem('theme', 'system');

    dispose = initializeTheme();

    expect(useThemeStore.getState().theme).toBe('system');
    expect(useThemeStore.getState().resolvedTheme).toBe('light');
    useThemeStore.getState().setTheme('dark');
    expect(document.documentElement).toHaveClass('dark');
  });

  it('still applies a user choice when browser storage is unavailable', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('disabled'); });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('disabled'); });

    dispose = initializeTheme();
    useThemeStore.getState().setTheme('dark');

    expect(useThemeStore.getState().theme).toBe('dark');
    expect(document.documentElement).toHaveClass('dark');
  });
});

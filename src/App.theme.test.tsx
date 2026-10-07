import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { StrictMode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';
import { AppearanceSettings } from './components/Settings/pages/AppearanceSettings';
import { useThemeStore } from './stores/theme';

vi.mock('./components/Sidebar', () => ({
  Sidebar: ({ dark, onToggleTheme }: { dark: boolean; onToggleTheme: () => void }) => (
    <button type="button" aria-label={dark ? '浅色模式' : '深色模式'} onClick={onToggleTheme} />
  ),
}));
vi.mock('./components/ChatArea', () => ({ ChatArea: () => <main /> }));
vi.mock('./components/RightPanel', () => ({ RightPanel: () => null }));
vi.mock('./components/Automation/AutomationPage', () => ({ AutomationPage: () => null }));
vi.mock('./components/Library/LibraryPage', () => ({ LibraryPage: () => null }));
vi.mock('./components/Settings', () => ({ SettingsModal: () => null }));
vi.mock('./components/CommandPalette', () => ({ CommandPalette: () => null }));
vi.mock('./stores/settings', () => ({
  useSettingsStore: (selector: (state: unknown) => unknown) => selector({
    apiConfigLoaded: true,
    loadApiConfig: vi.fn(),
  }),
}));
vi.mock('./stores/navigation', () => ({
  useNavigationStore: (selector: (state: unknown) => unknown) => selector({ currentPage: 'chat' }),
}));
vi.mock('./stores/workspaces', () => ({
  useWorkspacesStore: (selector: (state: unknown) => unknown) => selector({
    workspaces: [],
    activeWorkspaceId: null,
  }),
}));
vi.mock('./lib/commands/automation', () => ({ runDueAutomations: vi.fn().mockResolvedValue(undefined) }));

describe('App theme synchronization', () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.classList.remove('dark');
    useThemeStore.setState({ theme: 'light', resolvedTheme: 'light' });
    vi.stubGlobal('matchMedia', undefined);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('restores dark mode and gives the sidebar a matching quick-toggle state', async () => {
    const user = userEvent.setup();
    localStorage.setItem('theme', 'dark');
    render(<App />);

    expect(document.documentElement).toHaveClass('dark');
    await user.click(screen.getByRole('button', { name: '浅色模式' }));

    expect(document.documentElement).not.toHaveClass('dark');
    expect(screen.getByRole('button', { name: '深色模式' })).toBeInTheDocument();
    expect(localStorage.getItem('theme')).toBe('light');
  });

  it('keeps settings, sidebar and the document in sync across system changes and StrictMode cleanup', async () => {
    const user = userEvent.setup();
    const listeners = new Set<(event: MediaQueryListEvent) => void>();
    const media = {
      matches: false,
      addEventListener: vi.fn((_name: string, listener: (event: MediaQueryListEvent) => void) => {
        listeners.add(listener);
      }),
      removeEventListener: vi.fn((_name: string, listener: (event: MediaQueryListEvent) => void) => {
        listeners.delete(listener);
      }),
    };
    vi.stubGlobal('matchMedia', vi.fn(() => media));
    localStorage.setItem('theme', 'system');
    const view = render(<StrictMode><App /><AppearanceSettings /></StrictMode>);
    await user.click(screen.getByRole('button', { name: '主题' }));
    expect(screen.getByLabelText('颜色主题')).toHaveValue('system');

    act(() => {
      media.matches = true;
      listeners.forEach((listener) => listener({ matches: true } as MediaQueryListEvent));
    });

    expect(document.documentElement).toHaveClass('dark');
    expect(screen.getByRole('button', { name: '浅色模式' })).toBeInTheDocument();
    expect(screen.getByLabelText('颜色主题')).toHaveValue('system');

    await user.click(screen.getByRole('button', { name: '浅色模式' }));
    expect(screen.getByLabelText('颜色主题')).toHaveValue('light');
    expect(document.documentElement).not.toHaveClass('dark');

    view.unmount();
    expect(listeners.size).toBe(0);
    expect(media.removeEventListener).toHaveBeenCalledTimes(media.addEventListener.mock.calls.length);
  });
});

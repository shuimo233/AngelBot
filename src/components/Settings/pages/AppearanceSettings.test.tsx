import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { initializeTheme, useThemeStore } from '$stores/theme';
import { AppearanceSettings } from './AppearanceSettings';

describe('AppearanceSettings', () => {
  let dispose: (() => void) | undefined;

  beforeEach(() => {
    localStorage.clear();
    document.documentElement.classList.remove('dark');
    useThemeStore.setState({ theme: 'light', resolvedTheme: 'light' });
    vi.stubGlobal('matchMedia', vi.fn(() => ({
      matches: true,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    })));
    dispose = initializeTheme();
  });

  afterEach(() => {
    dispose?.();
    vi.unstubAllGlobals();
  });

  it('uses the shared preference and reflects a quick theme toggle while open', async () => {
    const user = userEvent.setup();
    render(<AppearanceSettings />);
    await user.click(screen.getByRole('button', { name: '主题' }));
    const select = screen.getByLabelText('颜色主题');
    expect(select).toHaveValue('light');

    await user.selectOptions(select, 'system');
    expect(useThemeStore.getState().theme).toBe('system');
    expect(document.documentElement).toHaveClass('dark');
    expect(localStorage.getItem('theme')).toBe('system');

    act(() => useThemeStore.getState().toggleTheme());

    expect(select).toHaveValue('light');
    expect(document.documentElement).not.toHaveClass('dark');
  });

  it('applies an explicit dark selection immediately', async () => {
    const user = userEvent.setup();
    render(<AppearanceSettings />);
    await user.click(screen.getByRole('button', { name: '主题' }));

    await user.selectOptions(screen.getByLabelText('颜色主题'), 'dark');

    expect(useThemeStore.getState().resolvedTheme).toBe('dark');
    expect(localStorage.getItem('theme')).toBe('dark');
    expect(document.documentElement).toHaveClass('dark');
  });
});

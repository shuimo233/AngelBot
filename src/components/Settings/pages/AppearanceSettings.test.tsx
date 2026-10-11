import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { initializeTheme, useThemeStore } from '$stores/theme';
import { initializeAppearance, useAppearanceStore } from '$stores/appearance';
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
    initializeAppearance();
  });

  afterEach(() => {
    dispose?.();
    vi.unstubAllGlobals();
  });

  it('uses the shared preference and reflects a quick theme toggle while open', async () => {
    const user = userEvent.setup();
    render(<AppearanceSettings />);
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

    await user.selectOptions(screen.getByLabelText('颜色主题'), 'dark');

    expect(useThemeStore.getState().resolvedTheme).toBe('dark');
    expect(localStorage.getItem('theme')).toBe('dark');
    expect(document.documentElement).toHaveClass('dark');
  });

  it('makes reading size and reduced motion effective without saving', async () => {
    const user = userEvent.setup();
    const { unmount } = render(<AppearanceSettings />);
    const slider = screen.getByRole('slider', { name: '对话文字大小' });
    slider.focus();
    // jsdom does not synthesize native range keyboard increments.
    fireEvent.change(slider, { target: { value: '18' } });
    expect(screen.getByLabelText('对话文字预览')).toHaveStyle({ fontSize: '18px' });
    expect(document.documentElement.style.getPropertyValue('--ui-chat-font-size')).toBe('18px');
    await user.click(screen.getByRole('checkbox', { name: '减少动态效果' }));
    expect(document.documentElement).toHaveAttribute('data-reduced-motion');
    unmount();
    render(<AppearanceSettings />);
    expect(screen.getByRole('slider', { name: '对话文字大小' })).toHaveValue('18');
    expect(screen.getByRole('checkbox', { name: '减少动态效果' })).toBeChecked();
    expect(screen.queryByLabelText('界面语言')).not.toBeInTheDocument();
  });

  it('uses an accessible upload button and rejects unsupported images', async () => {
    const user = userEvent.setup({ applyAccept: false });
    render(<AppearanceSettings />);
    await user.click(screen.getByRole('button', { name: '背景图片' }));
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    const openPicker = vi.spyOn(input, 'click');
    await user.click(screen.getByRole('button', { name: '选择背景图片' }));
    expect(openPicker).toHaveBeenCalledOnce();
    await user.upload(input, new File(['svg'], 'test.svg', { type: 'image/svg+xml' }));
    expect(screen.getByRole('alert')).toHaveTextContent('不超过 2 MB');
    expect(useAppearanceStore.getState().bgImage).toBe('');
  });
});

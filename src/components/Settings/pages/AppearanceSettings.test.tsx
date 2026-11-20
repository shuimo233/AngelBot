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
    const light = screen.getByRole('radio', { name: '浅色' });
    expect(light).toBeChecked();

    await user.click(screen.getByRole('radio', { name: '跟随系统' }));
    expect(useThemeStore.getState().theme).toBe('system');
    expect(document.documentElement).toHaveClass('dark');
    expect(localStorage.getItem('theme')).toBe('system');

    act(() => useThemeStore.getState().toggleTheme());

    expect(light).toBeChecked();
    expect(document.documentElement).not.toHaveClass('dark');
  });

  it('applies an explicit dark selection immediately', async () => {
    const user = userEvent.setup();
    render(<AppearanceSettings />);
    const light = screen.getByRole('radio', { name: '浅色' });
    light.focus();
    await user.keyboard('{ArrowRight}');

    expect(screen.getByRole('radio', { name: '深色' })).toBeChecked();
    expect(useThemeStore.getState().resolvedTheme).toBe('dark');
    expect(localStorage.getItem('theme')).toBe('dark');
    expect(document.documentElement).toHaveClass('dark');
  });

  it('applies and remembers reading preferences and can reset typography alone', async () => {
    const user = userEvent.setup();
    const { unmount } = render(<AppearanceSettings />);
    const slider = screen.getByRole('slider', { name: '对话文字大小' });
    slider.focus();
    // jsdom does not synthesize native range keyboard increments.
    fireEvent.change(slider, { target: { value: '18' } });
    expect(screen.getByText('18px')).toBeVisible();
    expect(document.documentElement.style.getPropertyValue('--ui-chat-font-size')).toBe('18px');
    await user.type(screen.getByLabelText('对话字体'), 'Microsoft YaHei');
    expect(document.documentElement.style.getPropertyValue('--ui-chat-font-family')).toBe('"Microsoft YaHei", var(--ui-font-sans)');
    expect(localStorage.getItem('chatFontFamily')).toBe('Microsoft YaHei');
    await user.click(screen.getByRole('checkbox', { name: '减少动态效果' }));
    expect(document.documentElement).toHaveAttribute('data-reduced-motion');
    unmount();
    render(<AppearanceSettings />);
    expect(screen.getByRole('slider', { name: '对话文字大小' })).toHaveValue('18');
    expect(screen.getByLabelText('对话字体')).toHaveValue('Microsoft YaHei');
    expect(screen.getByRole('checkbox', { name: '减少动态效果' })).toBeChecked();
    await user.click(screen.getByRole('button', { name: '恢复默认字体和字号' }));
    expect(screen.getByRole('slider', { name: '对话文字大小' })).toHaveValue('14');
    expect(screen.getByLabelText('对话字体')).toHaveValue('');
    expect(screen.getByRole('checkbox', { name: '减少动态效果' })).toBeChecked();
    expect(screen.queryByLabelText('界面语言')).not.toBeInTheDocument();
  });

  it('uses an accessible upload button and rejects unsupported images', async () => {
    const user = userEvent.setup({ applyAccept: false });
    render(<AppearanceSettings />);
    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    const openPicker = vi.spyOn(input, 'click');
    await user.click(screen.getByRole('button', { name: '选择背景图片' }));
    expect(openPicker).toHaveBeenCalledOnce();
    await user.upload(input, new File(['svg'], 'test.svg', { type: 'image/svg+xml' }));
    expect(screen.getByRole('alert')).toHaveTextContent('不超过 3.5 MB');
    await user.upload(input, new File([new Uint8Array(3.5 * 1024 * 1024 + 1)], 'large.png', { type: 'image/png' }));
    expect(screen.getByRole('alert')).toHaveTextContent('不超过 3.5 MB');
    expect(useAppearanceStore.getState().bgImage).toBe('');
  });

  it('keeps the displayed and stored background when a replacement cannot be decoded', async () => {
    const previous = 'data:image/png;base64,previous';
    useAppearanceStore.getState().update({ bgImage: previous });
    vi.stubGlobal('Image', class {
      decode = () => Promise.reject(new Error('Invalid image'));
    });
    const user = userEvent.setup();
    const { container } = render(<AppearanceSettings />);
    await user.upload(container.querySelector<HTMLInputElement>('input[type="file"]')!,
      new File(['broken'], 'broken.png', { type: 'image/png' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('无法打开这张图片');
    expect(screen.getByRole('img', { name: '背景预览' })).toHaveAttribute('src', previous);
    expect(localStorage.getItem('bgImage')).toBe(previous);
  });

  it('retains the shared A/B layout preference while changing reading settings', async () => {
    const user = userEvent.setup();
    render(<AppearanceSettings />);
    const iconLayout = screen.getByRole('radio', { name: '图标侧栏' });
    await user.click(iconLayout);
    expect(useThemeStore.getState().layout).toBe('B');
    expect(localStorage.getItem('ui-layout')).toBe('B');
    fireEvent.change(screen.getByRole('slider', { name: '对话文字大小' }), { target: { value: '18' } });
    expect(iconLayout).toBeChecked();
    expect(useAppearanceStore.getState().chatFontSize).toBe(18);
  });

  it('ignores superseded background decoding and aborts the pending read when leaving', async () => {
    const readers: ReadProbe[] = [];
    class ReadProbe {
      result = '';
      onload: (() => Promise<void>) | null = null;
      onerror: (() => void) | null = null;
      abort = vi.fn();
      readAsDataURL = vi.fn();
      constructor() { readers.push(this); }
    }
    vi.stubGlobal('FileReader', ReadProbe);
    let finishFirst!: () => void;
    const decode = vi.fn()
      .mockImplementationOnce(() => new Promise<void>((resolve) => { finishFirst = resolve; }))
      .mockResolvedValue(undefined);
    vi.stubGlobal('Image', class { decode = decode; });
    const view = render(<AppearanceSettings />);
    const input = view.container.querySelector('input[type="file"]');
    if (!input) throw new Error('Background file picker is missing');
    const choose = (name: string) => fireEvent.change(input, { target: { files: [new File(['png'], name, { type: 'image/png' })] } });
    choose('first.png');
    readers[0].result = 'data:image/png;base64,stale';
    const firstRead = readers[0].onload?.();
    choose('second.png');
    expect(readers[0].abort).toHaveBeenCalledOnce();
    expect(useAppearanceStore.getState().bgImage).toBe('');
    readers[1].result = 'data:image/png;base64,current';
    await act(async () => readers[1].onload?.());
    await act(async () => { finishFirst(); await firstRead; });
    expect(useAppearanceStore.getState().bgImage).toBe(readers[1].result);
    choose('pending.png');
    view.unmount();
    expect(readers[2].abort).toHaveBeenCalledOnce();
    readers[2].result = 'data:image/png;base64,unmounted';
    await act(async () => readers[2].onload?.());
    expect(useAppearanceStore.getState().bgImage).toBe(readers[1].result);
  });
});

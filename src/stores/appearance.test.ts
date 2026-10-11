import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import { initializeAppearance, useAppearanceStore } from './appearance';

describe('appearance preferences', () => {
  beforeEach(() => {
    localStorage.clear();
    initializeAppearance();
  });
  afterEach(() => vi.restoreAllMocks());

  it('restores existing background keys and reading preferences at app startup', () => {
    localStorage.setItem('bgImage', 'data:image/png;base64,example');
    localStorage.setItem('bgOpacity', '45');
    localStorage.setItem('chatFontSize', '18');
    localStorage.setItem('reduceMotion', 'true');
    initializeAppearance();
    expect(document.documentElement.style.getPropertyValue('--bg-image')).toContain('data:image/png;base64,example');
    expect(document.documentElement.style.getPropertyValue('--bg-opacity')).toBe('0.45');
    expect(document.documentElement.style.getPropertyValue('--ui-chat-font-size')).toBe('18px');
    expect(document.documentElement).toHaveAttribute('data-reduced-motion');
  });

  it('applies edits immediately, persists them, and removes stale background state', () => {
    useAppearanceStore.getState().update({ bgImage: 'data:image/png;base64,example', bgOpacity: 65 });
    useAppearanceStore.getState().update({ chatFontSize: 20, reduceMotion: true });
    initializeAppearance();
    expect(useAppearanceStore.getState()).toMatchObject({ chatFontSize: 20, reduceMotion: true, bgOpacity: 65 });
    useAppearanceStore.getState().update({ bgImage: '', reduceMotion: false });
    expect(localStorage.getItem('bgImage')).toBeNull();
    expect(document.documentElement.style.getPropertyValue('--bg-image')).toBe('none');
    expect(document.documentElement).not.toHaveAttribute('data-reduced-motion');
  });

  it('does not rewrite image data for every range-input change', () => {
    const write = vi.spyOn(Storage.prototype, 'setItem');
    useAppearanceStore.getState().update({ chatFontSize: 16 });
    expect(write).toHaveBeenCalledOnce();
    expect(write).toHaveBeenCalledWith('chatFontSize', '16');
  });

  it('bounds malformed stored values', () => {
    localStorage.setItem('chatFontSize', 'NaN');
    localStorage.setItem('bgOpacity', '999');
    initializeAppearance();
    expect(useAppearanceStore.getState()).toMatchObject({ chatFontSize: 14, bgOpacity: 100 });
  });

  it('keeps controls usable when storage is blocked and reports failed persistence', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('blocked'); });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('quota'); });
    expect(initializeAppearance).not.toThrow();
    useAppearanceStore.getState().update({ chatFontSize: 18 });
    expect(useAppearanceStore.getState()).toMatchObject({ chatFontSize: 18, persistenceError: true });
    expect(document.documentElement.style.getPropertyValue('--ui-chat-font-size')).toBe('18px');
  });

  it('retries earlier failed preferences before clearing the warning', () => {
    const write = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('quota'); });
    useAppearanceStore.getState().update({ bgImage: 'data:image/png;base64,example' });
    expect(useAppearanceStore.getState().persistenceError).toBe(true);
    write.mockRestore();
    useAppearanceStore.getState().update({ chatFontSize: 16 });
    expect(localStorage.getItem('bgImage')).toBe('data:image/png;base64,example');
    expect(useAppearanceStore.getState().persistenceError).toBe(false);
  });
});

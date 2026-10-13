import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { SettingsModal } from './index';
import { SettingsNav } from './components/SettingsNav';
import { useDialogFocus } from '$lib/useDialogFocus';

const settingsState = vi.hoisted(() => ({
  loadProfile: vi.fn(),
  loadApiConfig: vi.fn(),
  persistProfile: vi.fn(),
  persistApiConfig: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('$stores/settings', () => ({
  useSettingsStore: (selector: (state: typeof settingsState) => unknown) => selector(settingsState),
}));

vi.mock('./SettingsRouter', () => ({
  SettingsRouter: () => (
    <main>
      <button type="button">第一项</button>
      <button type="button">第二项</button>
    </main>
  ),
}));

describe('SettingsModal', () => {
  it('traps keyboard focus and restores the settings trigger on close', async () => {
    const user = userEvent.setup();
    const Harness = () => {
      const [isOpen, setIsOpen] = useState(false);
      return (
        <>
          <button type="button" onClick={() => setIsOpen(true)}>打开设置</button>
          <SettingsModal isOpen={isOpen} onClose={() => setIsOpen(false)} />
        </>
      );
    };
    render(<Harness />);

    const trigger = screen.getByRole('button', { name: '打开设置' });
    await user.click(trigger);
    const closeButton = screen.getByRole('button', { name: '关闭' });
    await waitFor(() => expect(closeButton).toHaveFocus());

    await user.tab();
    expect(screen.getByRole('button', { name: '第一项' })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole('button', { name: '第二项' })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole('button', { name: '保存' })).toHaveFocus();
    await user.tab({ shift: true });
    expect(screen.getByRole('button', { name: '第二项' })).toHaveFocus();

    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog', { name: '设置' })).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it('exposes the selected settings page to assistive technology', () => {
    render(<SettingsNav activePage="api" onNavigate={vi.fn()} />);

    expect(screen.getByRole('navigation', { name: '设置分类' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '模型 API' })).toHaveAttribute('aria-current', 'page');
    expect(screen.getByRole('button', { name: '性格' })).not.toHaveAttribute('aria-current');
  });

  it('keeps compact navigation on the same page and callback as the sidebar', async () => {
    const user = userEvent.setup();
    const onNavigate = vi.fn();
    const { rerender } = render(<SettingsNav activePage="api" onNavigate={onNavigate} />);
    const select = screen.getByRole('combobox', { name: '设置页面' });
    expect(select).toHaveValue('api');
    await user.selectOptions(select, 'appearance');
    expect(onNavigate).toHaveBeenCalledWith('appearance');
    rerender(<SettingsNav activePage="appearance" onNavigate={onNavigate} />);
    expect(screen.getByRole('button', { name: '外观' })).toHaveAttribute('aria-current', 'page');
    expect(select).toHaveValue('appearance');
  });

  it('does not trap focus on hidden responsive branches or native file inputs', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    function Harness() {
      const ref = useDialogFocus<HTMLDivElement>({ isOpen: true, onClose });
      return <div ref={ref} role="dialog" tabIndex={-1}>
        <button>可见的第一项</button>
        <div style={{ display: 'none' }}><button>隐藏导航</button></div>
        <button>可见的最后一项</button>
        <input type="file" hidden />
        <div style={{ visibility: 'hidden' }}><button>隐藏项</button></div>
      </div>;
    }
    render(<Harness />);
    const first = screen.getByRole('button', { name: '可见的第一项' });
    const last = screen.getByRole('button', { name: '可见的最后一项' });
    await waitFor(() => expect(screen.getByRole('dialog')).toHaveFocus());
    first.focus();
    await user.tab({ shift: true });
    expect(last).toHaveFocus();
    await user.tab();
    expect(first).toHaveFocus();
  });
});

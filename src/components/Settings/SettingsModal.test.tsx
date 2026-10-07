import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { SettingsModal } from './index';
import { SettingsNav } from './components/SettingsNav';

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

    await user.tab({ shift: true });
    expect(screen.getByRole('button', { name: '第二项' })).toHaveFocus();
    await user.tab();
    expect(closeButton).toHaveFocus();

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
});

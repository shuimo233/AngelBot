import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { SettingsModal } from './index';
import { SettingsNav } from './components/SettingsNav';
import { useDialogFocus } from '$lib/useDialogFocus';
import { Dialog } from '../../ui';

const settingsState = vi.hoisted(() => ({
  loadProfile: vi.fn(),
  loadApiConfig: vi.fn(),
  persistProfile: vi.fn(),
  persistApiConfig: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('$stores/settings', () => ({
  hasUnsavedApiConfig: () => false,
  useSettingsStore: Object.assign((selector: (state: typeof settingsState) => unknown) => selector(settingsState), {
    getState: () => settingsState,
  }),
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
    const { rerender } = render(<Harness />);

    const trigger = screen.getByRole('button', { name: '打开设置' });
    await user.click(trigger);
    const closeButton = screen.getByRole('button', { name: '关闭' });
    await waitFor(() => expect(closeButton).toHaveFocus());

    await user.tab({ shift: true });
    expect(screen.getByRole('button', { name: '第二项' })).toHaveFocus();
    rerender(<Harness />);
    expect(screen.getByRole('button', { name: '第二项' })).toHaveFocus();
    await user.tab();
    expect(closeButton).toHaveFocus();

    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog', { name: '设置' })).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it('exposes the selected settings page to assistive technology', () => {
    const nav = render(<SettingsNav activePage="api" onNavigate={vi.fn()} />);

    expect(screen.getByRole('navigation', { name: '设置分类' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '模型与 API' })).toHaveAttribute('aria-current', 'page');
    const personality = screen.getByRole('button', { name: '性格' });
    expect(personality.closest('[role="group"]')).toHaveAccessibleName('常规');
    expect(personality).not.toHaveAttribute('aria-current');
    nav.unmount();
    render(<SettingsModal isOpen initialPage="memory" onClose={vi.fn()} />);
    expect(screen.getByRole('dialog', { name: '设置' })).not.toHaveAttribute('aria-describedby');
    expect(screen.queryByText('此页设置即时生效')).not.toBeInTheDocument();
  });

  it('shares page selection and the current five-group order with compact navigation', async () => {
    const user = userEvent.setup();
    const onNavigate = vi.fn();
    const { rerender } = render(<SettingsNav activePage="api" onNavigate={onNavigate} />);
    const select = screen.getByRole('combobox', { name: '设置页面' });
    expect(select).toHaveValue('api');
    expect(Array.from(select.querySelectorAll('optgroup')).map(group => group.label)).toEqual([
      '常规', '模型与连接', '工具与应用', '隐私与权限', '记忆与数据',
    ]);
    await user.selectOptions(select, 'appearance');
    expect(onNavigate).toHaveBeenCalledWith('appearance');
    rerender(<SettingsNav activePage="appearance" onNavigate={onNavigate} />);
    expect(screen.getByRole('button', { name: '外观' })).toHaveAttribute('aria-current', 'page');
    expect(select).toHaveValue('appearance');
    expect(screen.getByRole('button', { name: '模型与 API' })).not.toHaveAttribute('aria-current');
  });

  it('skips hidden responsive branches, inert controls and native file inputs when trapping focus', async () => {
    const user = userEvent.setup();
    function Harness() {
      const ref = useDialogFocus<HTMLDivElement>({ isOpen: true, onClose: vi.fn() });
      return <div ref={ref} role="dialog" tabIndex={-1}>
        <button>可见的第一项</button>
        <div style={{ display: 'none' }}><button>隐藏导航</button></div>
        <button>可见的最后一项</button>
        <input type="file" hidden />
        <div ref={element => element?.setAttribute('inert', '')}><button>不可交互项</button></div>
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

  it('closes only the inner dialog with Escape and restores its trigger', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    function Harness() {
      const [open, setOpen] = useState(false);
      return <Dialog open title="设置" onClose={onClose}>
        <button type="button" onClick={() => setOpen(true)}>清空数据…</button>
        <Dialog open={open} title="确认清空" role="alertdialog" onClose={() => setOpen(false)}>
          <button type="button" onClick={() => setOpen(false)}>取消</button>
        </Dialog>
      </Dialog>;
    }
    render(<Harness />);
    const trigger = screen.getByRole('button', { name: '清空数据…' });
    await user.click(trigger);
    const inner = screen.getByRole('alertdialog');
    await waitFor(() => expect(inner).toHaveFocus());
    await user.tab();
    expect(screen.getByRole('button', { name: '取消' })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole('button', { name: '取消' })).toHaveFocus();
    await user.keyboard('{Escape}');
    expect(inner).not.toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();
    expect(trigger).toHaveFocus();
  });

  it('wraps Tab at a closed details summary and includes its fields after opening it', async () => {
    const user = userEvent.setup();
    function Harness() {
      const ref = useDialogFocus<HTMLDivElement>({ isOpen: true, onClose: vi.fn() });
      return <div ref={ref} role="dialog" tabIndex={-1}>
        <button>第一项</button>
        <details>
          <summary>生成参数</summary>
          <input aria-label="输出长度" />
        </details>
        <button disabled>测试连接</button>
      </div>;
    }
    render(<Harness />);
    const first = screen.getByRole('button', { name: '第一项' });
    const summary = screen.getByText('生成参数');
    await waitFor(() => expect(screen.getByRole('dialog')).toHaveFocus());
    first.focus();
    await user.tab({ shift: true });
    expect(summary).toHaveFocus();
    await user.tab();
    expect(first).toHaveFocus();

    summary.parentElement?.setAttribute('open', '');
    await user.tab({ shift: true });
    expect(screen.getByRole('textbox', { name: '输出长度' })).toHaveFocus();
    await user.tab();
    expect(first).toHaveFocus();
  });
});

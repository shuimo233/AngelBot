import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { ContextDrawer } from './ContextDrawer';

vi.mock('./ContextDrawerContent', () => ({
  ContextDrawerContent: ({ onClose }: { onClose?: () => void }) => (
    <section>
      <button type="button" onClick={onClose}>关闭</button>
      <button type="button">上下文操作</button>
    </section>
  ),
}));

describe('ContextDrawer', () => {
  it('is not rendered while closed', () => {
    render(<ContextDrawer isOpen={false} onClose={vi.fn()} />);

    expect(screen.queryByRole('dialog', { name: '本轮上下文' })).not.toBeInTheDocument();
  });

  it('closes on Escape and restores focus to the trigger', async () => {
    const user = userEvent.setup();
    const Harness = () => {
      const [isOpen, setIsOpen] = useState(false);
      return (
        <>
          <button type="button" onClick={() => setIsOpen(true)}>打开上下文</button>
          <ContextDrawer isOpen={isOpen} onClose={() => setIsOpen(false)} />
        </>
      );
    };
    render(<Harness />);

    const trigger = screen.getByRole('button', { name: '打开上下文' });
    await user.click(trigger);
    expect(screen.getByRole('dialog', { name: '本轮上下文' })).toBeInTheDocument();
    await user.keyboard('{Escape}');

    expect(screen.queryByRole('dialog', { name: '本轮上下文' })).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it('closes when the overlay is clicked', async () => {
    const onClose = vi.fn();
    const user = userEvent.setup();
    render(<ContextDrawer isOpen onClose={onClose} />);

    await user.click(screen.getByRole('button', { name: '关闭上下文面板' }));

    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

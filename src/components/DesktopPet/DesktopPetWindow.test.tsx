import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { hideDesktopPet, openMainFromDesktopPet, startDesktopPetDrag } from '$lib/commands/desktop-pet';
import { DesktopPetWindow } from './DesktopPetWindow';

vi.mock('$lib/commands/desktop-pet', () => ({
  openMainFromDesktopPet: vi.fn(), hideDesktopPet: vi.fn(), startDesktopPetDrag: vi.fn(),
}));

describe('DesktopPetWindow', () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(openMainFromDesktopPet).mockResolvedValue();
    vi.mocked(hideDesktopPet).mockResolvedValue();
    vi.mocked(startDesktopPetDrag).mockResolvedValue();
  });

  it('opens the existing conversation or hides the pet using its buttons', async () => {
    const user = userEvent.setup();
    render(<DesktopPetWindow />);
    screen.getByRole('button', { name: '打开 AngelBot 对话' }).focus();
    await user.keyboard('{Enter}');
    expect(openMainFromDesktopPet).toHaveBeenCalledOnce();
    await user.click(screen.getByRole('button', { name: '隐藏桌宠' }));
    expect(hideDesktopPet).toHaveBeenCalledOnce();
    expect(startDesktopPetDrag).not.toHaveBeenCalled();
  });

  it('asks the host to drag once and suppresses the click following the drag', () => {
    render(<DesktopPetWindow />);
    const pet = screen.getByRole('button', { name: '打开 AngelBot 对话' });
    pet.setPointerCapture = vi.fn();
    pet.releasePointerCapture = vi.fn();
    const pointer = (type: string, clientX: number) => {
      const event = new MouseEvent(type, { bubbles: true, button: 0, buttons: 1, clientX });
      Object.defineProperties(event, { pointerId: { value: 1 }, isPrimary: { value: true } });
      fireEvent(pet, event);
    };

    pointer('pointerdown', 10);
    pointer('pointermove', 12);
    expect(startDesktopPetDrag).not.toHaveBeenCalled();
    pointer('pointermove', 18);
    pointer('pointermove', 22);
    fireEvent.click(pet, { detail: 1 });
    expect(pet.releasePointerCapture).toHaveBeenCalledWith(1);
    expect(startDesktopPetDrag).toHaveBeenCalledOnce();
    expect(openMainFromDesktopPet).not.toHaveBeenCalled();
  });

  it('alternates the petting response, clears it, and leaves the chat closed', () => {
    vi.useFakeTimers();
    try {
      const { container } = render(<DesktopPetWindow />);
      const pat = screen.getByRole('button', { name: '摸摸头' });
      fireEvent.click(pat);
      expect(screen.getByRole('main')).toHaveAttribute('data-reaction', 'hop');
      fireEvent.click(pat);
      expect(screen.getByRole('main')).toHaveAttribute('data-reaction', 'wiggle');
      expect(container.querySelector('.desktop-pet-hearts')).toBeInTheDocument();
      expect(openMainFromDesktopPet).not.toHaveBeenCalled();
      expect(startDesktopPetDrag).not.toHaveBeenCalled();
      act(() => vi.advanceTimersByTime(1100));
      expect(screen.getByRole('main')).not.toHaveAttribute('data-reaction');
      expect(container.querySelector('.desktop-pet-hearts')).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it('applies changes to the app-wide motion preference from the main window', () => {
    localStorage.removeItem('reduceMotion');
    try {
      render(<DesktopPetWindow />);
      const pet = screen.getByRole('main');
      expect(pet).not.toHaveAttribute('data-reduced-motion');
      act(() => {
        localStorage.setItem('reduceMotion', 'true');
        window.dispatchEvent(new StorageEvent('storage', { key: 'reduceMotion' }));
      });
      expect(pet).toHaveAttribute('data-reduced-motion', 'true');
    } finally {
      localStorage.removeItem('reduceMotion');
    }
  });
});

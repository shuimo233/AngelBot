import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { listen } from '@tauri-apps/api/event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { DESKTOP_PET_CHANGED, getDesktopPet, resetDesktopPetPosition, setDesktopPet, type DesktopPetStatus } from '$lib/commands/desktop-pet';
import { isTauriRuntime } from '$lib/invoke';
import { DesktopPetSettingsSection } from './DesktopPetSettingsSection';

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
vi.mock('$lib/invoke', () => ({ isTauriRuntime: vi.fn() }));
vi.mock('$lib/commands/desktop-pet', async importOriginal => ({
  ...await importOriginal<typeof import('$lib/commands/desktop-pet')>(),
  getDesktopPet: vi.fn(), setDesktopPet: vi.fn(), resetDesktopPetPosition: vi.fn(),
}));

describe('DesktopPetSettingsSection', () => {
  const unlisten = vi.fn();
  const disabled: DesktopPetStatus = { supported: true, enabled: false };
  const enabled: DesktopPetStatus = { supported: true, enabled: true };

  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(isTauriRuntime).mockReturnValue(true);
    vi.mocked(listen).mockResolvedValue(unlisten);
    vi.mocked(getDesktopPet).mockResolvedValue(disabled);
  });

  it('enables only after a successful reply and follows changes from the tray', async () => {
    const user = userEvent.setup();
    vi.mocked(setDesktopPet).mockResolvedValue(enabled);
    vi.mocked(resetDesktopPetPosition).mockResolvedValue();
    const { unmount } = render(<DesktopPetSettingsSection />);
    const control = screen.getByRole('checkbox', { name: '显示桌宠' });
    await waitFor(() => expect(control).toBeEnabled());
    expect(control).not.toBeChecked();
    await user.click(control);
    expect(setDesktopPet).toHaveBeenCalledWith(true);
    expect(control).toBeChecked();
    await user.click(screen.getByRole('button', { name: '恢复默认位置' }));
    expect(resetDesktopPetPosition).toHaveBeenCalledOnce();
    expect(screen.getByRole('status')).toHaveTextContent('已恢复默认位置');

    const [event, handler] = vi.mocked(listen).mock.calls[0];
    expect(event).toBe(DESKTOP_PET_CHANGED);
    act(() => handler({ event: DESKTOP_PET_CHANGED, id: 1, payload: disabled }));
    expect(control).not.toBeChecked();
    unmount();
    expect(unlisten).toHaveBeenCalledOnce();
  });

  it('leaves desktop controls disabled in a browser', () => {
    vi.mocked(isTauriRuntime).mockReturnValue(false);
    render(<DesktopPetSettingsSection />);
    expect(screen.getByRole('checkbox', { name: '显示桌宠' })).toBeDisabled();
    expect(getDesktopPet).not.toHaveBeenCalled();
  });
});

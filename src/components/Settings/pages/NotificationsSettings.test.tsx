import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { NotificationsSettings } from './NotificationsSettings';
import {
  getNotificationSettings,
  saveNotificationSettings,
} from '$lib/commands/settings';

vi.mock('$lib/commands/settings', () => ({
  getNotificationSettings: vi.fn(),
  saveNotificationSettings: vi.fn(),
}));

describe('NotificationsSettings', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getNotificationSettings).mockResolvedValue({
      chat_reply: true,
      reminder: true,
      task_complete: true,
      file_change: false,
      system: true,
    });
    vi.mocked(saveNotificationSettings).mockResolvedValue(undefined);
  });

  it('loads notification channel settings from the backend', async () => {
    render(<NotificationsSettings />);

    await waitFor(() => expect(getNotificationSettings).toHaveBeenCalledTimes(1));
    expect(screen.getByLabelText('文件变更')).not.toBeChecked();
    expect(screen.getByLabelText('定时任务提醒')).toBeChecked();
  });

  it('saves channel toggles when changed', async () => {
    render(<NotificationsSettings />);

    await userEvent.click(await screen.findByLabelText('文件变更'));

    expect(saveNotificationSettings).toHaveBeenCalledWith({
      chat_reply: true,
      reminder: true,
      task_complete: true,
      file_change: true,
      system: true,
    });
  });
});

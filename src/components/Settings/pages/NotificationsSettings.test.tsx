import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
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

  it('blocks default writes while reading and offers a safe retry after a failed read', async () => {
    let rejectRead!: (reason: Error) => void;
    vi.mocked(getNotificationSettings).mockReturnValueOnce(new Promise((_resolve, reject) => { rejectRead = reject; }));
    render(<NotificationsSettings />);

    expect(screen.getAllByRole('checkbox').every(input => input.hasAttribute('disabled'))).toBe(true);
    fireEvent.click(screen.getByLabelText('定时任务提醒'));
    expect(saveNotificationSettings).not.toHaveBeenCalled();

    await act(async () => rejectRead(new Error('PRIVATE_READ_DIAGNOSTIC')));
    expect(screen.getByRole('alert')).toHaveTextContent('无法读取通知设置');
    expect(screen.queryByText(/PRIVATE_/)).not.toBeInTheDocument();
    expect(screen.getByLabelText('定时任务提醒')).toBeDisabled();

    await userEvent.click(screen.getByRole('button', { name: '重新读取通知设置' }));
    await waitFor(() => expect(screen.getByLabelText('文件变更')).toBeEnabled());
    expect(screen.getByLabelText('文件变更')).not.toBeChecked();
    expect(getNotificationSettings).toHaveBeenCalledTimes(2);
    expect(saveNotificationSettings).not.toHaveBeenCalled();
  });

  it('serializes complete settings writes and rereads an uncertain save without replaying it', async () => {
    let finishSave!: () => void;
    vi.mocked(saveNotificationSettings).mockReturnValueOnce(new Promise(resolve => { finishSave = resolve; }));
    render(<NotificationsSettings />);
    await waitFor(() => expect(screen.getByLabelText('文件变更')).not.toBeChecked());

    act(() => {
      fireEvent.click(screen.getByLabelText('文件变更'));
      fireEvent.click(screen.getByLabelText('定时任务提醒'));
    });
    expect(saveNotificationSettings).toHaveBeenCalledTimes(1);
    expect(screen.getAllByRole('checkbox').every(input => input.hasAttribute('disabled'))).toBe(true);
    expect(screen.getByRole('status')).toHaveTextContent('正在保存通知设置');

    await act(async () => finishSave());
    expect(screen.getByLabelText('文件变更')).toBeChecked();
    expect(screen.getByLabelText('定时任务提醒')).toBeChecked();

    vi.mocked(saveNotificationSettings).mockRejectedValueOnce(new Error('PRIVATE_SAVE_DIAGNOSTIC'));
    await userEvent.click(screen.getByLabelText('定时任务提醒'));
    expect(await screen.findByRole('alert')).toHaveTextContent('未能确认通知设置已保存');
    expect(screen.queryByText(/PRIVATE_/)).not.toBeInTheDocument();
    expect(screen.getByLabelText('文件变更')).toBeDisabled();
    vi.mocked(getNotificationSettings).mockResolvedValueOnce({
      chat_reply: true, reminder: false, task_complete: true, file_change: true, system: true,
    });
    await userEvent.click(screen.getByRole('button', { name: '重新读取通知设置' }));
    await waitFor(() => expect(screen.getByLabelText('定时任务提醒')).toBeEnabled());
    expect(screen.getByLabelText('定时任务提醒')).not.toBeChecked();
    expect(screen.getByLabelText('文件变更')).toBeChecked();
    expect(saveNotificationSettings).toHaveBeenCalledTimes(2);
  });
});

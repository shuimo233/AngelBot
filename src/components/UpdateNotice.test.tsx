import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { relaunch } from '@tauri-apps/plugin-process';
import { checkForAppUpdate } from '$lib/updates';
import { UpdateNotice, UpdateNoticePanel } from './UpdateNotice';

vi.mock('@tauri-apps/plugin-process', () => ({ relaunch: vi.fn() }));
vi.mock('$lib/updates', () => ({ checkForAppUpdate: vi.fn() }));

describe('UpdateNoticePanel', () => {
  it('keeps installing an update an explicit user action', async () => {
    const onInstall = vi.fn();
    const onDismiss = vi.fn();
    render(
      <UpdateNoticePanel
        update={{ version: '1.2.3', body: 'Reliability improvements' }}
        state="available"
        onInstall={onInstall}
        onDismiss={onDismiss}
      />,
    );

    expect(screen.getByText('版本 1.2.3 已可用')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '更新并重启' }));
    expect(onInstall).not.toHaveBeenCalled();
    expect(screen.getByRole('alertdialog')).toHaveTextContent('未发送');
    expect(screen.getByRole('alertdialog')).toHaveTextContent('自动化');
    expect(screen.getByRole('alertdialog')).toHaveTextContent('不会撤回');
    await waitFor(() => expect(screen.getByRole('button', { name: '暂不更新' })).toHaveFocus());
    fireEvent.click(screen.getByRole('button', { name: '暂不更新' }));
    expect(onInstall).not.toHaveBeenCalled();
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '更新并重启' }));
    fireEvent.click(screen.getByRole('button', { name: '确认更新并重启' }));
    expect(onInstall).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    expect(onDismiss).not.toHaveBeenCalled();
  });

  it('shows a retry without blocking the current version after failure', async () => {
    const onInstall = vi.fn();
    const panel = render(
      <UpdateNoticePanel
        update={{ version: '1.2.3' }}
        state="failed"
        onInstall={onInstall}
        onDismiss={vi.fn()}
      />,
    );

    expect(screen.getByText('更新未能完成，当前版本仍可继续使用')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '重新尝试' }));
    expect(onInstall).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: '确认更新并重启' }));
    expect(onInstall).toHaveBeenCalledTimes(1);
    panel.unmount();

    // Exercise the real owner: a completed install must not be repeated merely
    // because its separate relaunch request failed.
    const install = vi.fn().mockRejectedValueOnce(new Error('PRIVATE_INSTALL_ERROR')).mockResolvedValue(undefined);
    const close = vi.fn().mockResolvedValue(undefined);
    let finishRestart: () => void = () => {};
    vi.mocked(relaunch).mockRejectedValueOnce(new Error('PRIVATE_RELAUNCH_ERROR'))
      .mockImplementationOnce(() => new Promise<void>((resolve) => { finishRestart = resolve; }));
    vi.mocked(checkForAppUpdate).mockResolvedValue({ version: '1.2.3', install, close });
    vi.stubEnv('MODE', 'development');
    vi.stubEnv('VITE_ANGELBOT_DESKTOP_E2E', '0');
    vi.useFakeTimers();
    const owner = render(<UpdateNotice />);
    try {
      await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
      vi.useRealTimers();
      fireEvent.click(screen.getByRole('button', { name: '更新并重启' }));
      expect(install).not.toHaveBeenCalled();
      fireEvent.click(screen.getByRole('button', { name: '确认更新并重启' }));
      await screen.findByText('更新未能完成，当前版本仍可继续使用');
      expect(relaunch).not.toHaveBeenCalled();
      expect(screen.queryByText(/PRIVATE_INSTALL_ERROR/)).not.toBeInTheDocument();
      fireEvent.click(screen.getByRole('button', { name: '重新尝试' }));
      fireEvent.click(screen.getByRole('button', { name: '确认更新并重启' }));
      await screen.findByText('更新已安装，尚未完成重新启动');
      expect(install).toHaveBeenCalledTimes(2);
      expect(relaunch).toHaveBeenCalledTimes(1);
      expect(screen.queryByText(/PRIVATE_RELAUNCH_ERROR/)).not.toBeInTheDocument();
      fireEvent.click(screen.getByRole('button', { name: '重新启动' }));
      fireEvent.keyDown(document, { key: 'Escape' });
      expect(relaunch).toHaveBeenCalledTimes(1);
      fireEvent.click(screen.getByRole('button', { name: '重新启动' }));
      fireEvent.click(screen.getByRole('button', { name: '确认重启' }));
      expect(screen.getByText('更新已安装，正在重新启动')).toBeVisible();
      expect(screen.queryByRole('button')).not.toBeInTheDocument();
      expect(relaunch).toHaveBeenCalledTimes(2);
      expect(install).toHaveBeenCalledTimes(2);
      await act(async () => finishRestart());
      expect(screen.queryByRole('button')).not.toBeInTheDocument();
    } finally {
      owner.unmount();
      vi.useRealTimers();
      vi.unstubAllEnvs();
    }
    expect(close).toHaveBeenCalledTimes(1);
  });
});

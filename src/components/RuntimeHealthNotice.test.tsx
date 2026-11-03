import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { relaunch } from '@tauri-apps/plugin-process';
import { RuntimeHealthNotice, RuntimeHealthPanel } from './RuntimeHealthNotice';
import { getRuntimeHealth, type RuntimeHealth } from '$lib/commands/runtime-health';

vi.mock('@tauri-apps/plugin-process', () => ({ relaunch: vi.fn() }));
vi.mock('$lib/commands/runtime-health', () => ({ getRuntimeHealth: vi.fn() }));

describe('RuntimeHealthPanel', () => {
  it('把主动关闭子代理显示为安静提示而非系统故障', () => {
    const health: RuntimeHealth = {
      status: 'ready',
      delegationAvailable: false,
      issues: [{
        code: 'delegation_disabled',
        severity: 'notice',
        title: '子代理已按当前配置停用',
        detail: '主 Agent 仍可独立完成对话与工具任务。',
        recoveryAction: 'none',
      }],
    };

    const { container } = render(<RuntimeHealthPanel health={health} onRestart={vi.fn()} />);
    expect(screen.getByText('运行提示')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '重新启动 AngelBot' })).not.toBeInTheDocument();
    expect(container.querySelector('details')).not.toHaveAttribute('open');
  });

  it('关键持久化故障默认展开并提供明确的重启恢复操作', async () => {
    let failRestart: (reason: unknown) => void = () => {};
    const onRestart = vi.fn(() => new Promise<void>((_, reject) => { failRestart = reject; }));
    const health: RuntimeHealth = {
      status: 'degraded',
      delegationAvailable: true,
      issues: [{
        code: 'ephemeral_storage',
        severity: 'critical',
        title: '本地数据未连接',
        detail: '当前对话可以继续，但重启前的新内容不会保存。',
        recoveryAction: 'restart',
      }],
    };

    const { container, unmount } = render(<RuntimeHealthPanel health={health} onRestart={onRestart} />);
    expect(screen.getByText('部分能力受限')).toBeInTheDocument();
    expect(container.querySelector('details')).toHaveAttribute('open');
    screen.getByRole('button', { name: '重新启动 AngelBot' }).focus();
    fireEvent.click(screen.getByRole('button', { name: '重新启动 AngelBot' }));
    expect(onRestart).not.toHaveBeenCalled();
    expect(screen.getByRole('alertdialog')).toHaveTextContent('未发送');
    expect(screen.getByRole('alertdialog')).toHaveTextContent('自动化');
    expect(screen.getByRole('alertdialog')).toHaveTextContent('新内容');
    expect(screen.getByRole('alertdialog')).toHaveTextContent('复制保存');
    await waitFor(() => expect(screen.getByRole('button', { name: '暂不重启' })).toHaveFocus());
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    expect(onRestart).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: '重新启动 AngelBot' }));
    fireEvent.click(screen.getByRole('button', { name: '确认重启' }));
    expect(onRestart).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('button', { name: '处理中…' })).toBeDisabled();
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.getByRole('alertdialog')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '处理中…' }));
    expect(onRestart).toHaveBeenCalledTimes(1);
    await act(async () => failRestart(new Error('RAW_PRIVATE_RESTART_ERROR')));
    expect(await screen.findByRole('alert')).toHaveTextContent('重新启动未能完成');
    expect(screen.queryByText(/RAW_PRIVATE_RESTART_ERROR/)).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '确认重启' })).toBeEnabled();
    fireEvent.click(screen.getByRole('button', { name: '暂不重启' }));
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole('button', { name: '重新启动 AngelBot' })).toHaveFocus());
    fireEvent.click(screen.getByRole('button', { name: '重新启动 AngelBot' }));
    fireEvent.click(screen.getByRole('button', { name: '确认重启' }));
    expect(onRestart).toHaveBeenCalledTimes(2);
    unmount();

    // The real notice must return the native promise to the guarded panel,
    // not detach a rejection with `void relaunch()`.
    vi.mocked(getRuntimeHealth).mockResolvedValue(health);
    vi.mocked(relaunch).mockRejectedValueOnce(new Error('PRIVATE_NATIVE_RESTART_ERROR'));
    vi.stubEnv('MODE', 'development');
    const notice = render(<RuntimeHealthNotice />);
    try {
      fireEvent.click(await screen.findByRole('button', { name: '重新启动 AngelBot' }));
      expect(relaunch).not.toHaveBeenCalled();
      fireEvent.click(screen.getByRole('button', { name: '确认重启' }));
      expect(await screen.findByRole('alert')).toHaveTextContent('重新启动未能完成');
      expect(relaunch).toHaveBeenCalledTimes(1);
      expect(screen.queryByText(/PRIVATE_NATIVE_RESTART_ERROR/)).not.toBeInTheDocument();
    } finally {
      notice.unmount();
      vi.unstubAllEnvs();
    }
  });
});

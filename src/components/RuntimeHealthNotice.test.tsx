import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { RuntimeHealthPanel } from './RuntimeHealthNotice';
import type { RuntimeHealth } from '$lib/commands/runtime-health';

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

  it('关键持久化故障默认展开并提供明确的重启恢复操作', () => {
    const onRestart = vi.fn();
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

    const { container } = render(<RuntimeHealthPanel health={health} onRestart={onRestart} />);
    expect(screen.getByText('部分能力受限')).toBeInTheDocument();
    expect(container.querySelector('details')).toHaveAttribute('open');
    fireEvent.click(screen.getByRole('button', { name: '重新启动 AngelBot' }));
    expect(onRestart).toHaveBeenCalledTimes(1);
  });
});

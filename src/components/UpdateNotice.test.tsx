import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { UpdateNoticePanel } from './UpdateNotice';

describe('UpdateNoticePanel', () => {
  it('keeps installing an update an explicit user action', () => {
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
    expect(onInstall).toHaveBeenCalledTimes(1);
    expect(onDismiss).not.toHaveBeenCalled();
  });

  it('shows a retry without blocking the current version after failure', () => {
    const onInstall = vi.fn();
    render(
      <UpdateNoticePanel
        update={{ version: '1.2.3' }}
        state="failed"
        onInstall={onInstall}
        onDismiss={vi.fn()}
      />,
    );

    expect(screen.getByText('更新未能完成，当前版本仍可继续使用')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '重新尝试' }));
    expect(onInstall).toHaveBeenCalledTimes(1);
  });
});

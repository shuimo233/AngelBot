import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { ProjectNetworkApprovalsSettings } from './pages/ProjectNetworkApprovalsSettings';
import { ExecutionPermissionSettings } from './pages/ExecutionPermissionSettings';
import { SettingsNav } from './components/SettingsNav';
import { getSettingsNavItems, settingsRegistry } from './SettingsRouter';

describe('SettingsRouter project network permissions integration', () => {
  it('uses the localized page heading for channel settings', () => {
    expect(settingsRegistry.pages.channels.title).toBe('渠道与远程访问');
  });

  it('registers project network permissions as a distinct settings page', () => {
    expect(settingsRegistry.pages.projectNetwork).toMatchObject({
      title: '项目联网权限',
      component: ProjectNetworkApprovalsSettings,
    });
    expect(getSettingsNavItems()).toContainEqual(expect.objectContaining({
      id: 'projectNetwork',
      label: '项目联网权限',
    }));
  });

  it('registers global execution permission as its own settings page', () => {
    expect(settingsRegistry.pages.executionPermission).toMatchObject({
      title: '执行权限',
      component: ExecutionPermissionSettings,
    });
    expect(getSettingsNavItems()).toContainEqual(expect.objectContaining({
      id: 'executionPermission',
      label: '执行权限',
    }));
    render(<SettingsNav activePage="executionPermission" onNavigate={() => undefined} />);
    expect(screen.getByRole('button', { name: '执行权限' }).closest('.settings-nav-group')).toHaveTextContent('隐私与权限');
  });

  it('shows only the current category leaves and opens each category at its first page', () => {
    const navigate = vi.fn();
    const { rerender } = render(<SettingsNav activePage="appearance" onNavigate={navigate} />);
    expect(screen.getByRole('button', { name: '外观' })).toHaveAttribute('aria-current', 'page');
    expect(screen.queryByRole('button', { name: '文件访问' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '隐私与权限' }));
    expect(navigate).toHaveBeenLastCalledWith('executionPermission');
    rerender(<SettingsNav activePage="projectNetwork" onNavigate={navigate} />);
    expect(screen.getByRole('button', { name: '项目联网权限' })).toHaveAttribute('aria-current', 'page');
    expect(screen.queryByRole('button', { name: '外观' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '模型与连接' }));
    expect(navigate).toHaveBeenLastCalledWith('api');
  });
});

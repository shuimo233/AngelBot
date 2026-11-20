import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { ProjectNetworkApprovalsSettings } from './pages/ProjectNetworkApprovalsSettings';
import { ExecutionPermissionSettings } from './pages/ExecutionPermissionSettings';
import { SettingsNav } from './components/SettingsNav';
import { getSettingsNavItems, settingsRegistry } from './SettingsRouter';

describe('SettingsRouter project network permissions integration', () => {
  it('exposes channel settings in both navigation layouts', () => {
    expect(settingsRegistry.pages.channels.title).toBe('渠道与远程访问');
    render(<SettingsNav activePage="channels" onNavigate={() => undefined} />);
    expect(screen.getByRole('button', { name: '渠道与远程访问' })).toHaveAttribute('aria-current', 'page');
    expect(screen.getByRole('combobox', { name: '设置页面' })).toHaveValue('channels');
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

  it('keeps the same destinations available and switches directly to the requested page', () => {
    const navigate = vi.fn();
    const { rerender } = render(<SettingsNav activePage="appearance" onNavigate={navigate} />);
    expect(screen.getByRole('button', { name: '外观' })).toHaveAttribute('aria-current', 'page');
    fireEvent.click(screen.getByRole('button', { name: '文件访问' }));
    expect(navigate).toHaveBeenLastCalledWith('fileAccess');
    rerender(<SettingsNav activePage="projectNetwork" onNavigate={navigate} />);
    expect(screen.getByRole('button', { name: '项目联网权限' })).toHaveAttribute('aria-current', 'page');
    expect(screen.getByRole('button', { name: '外观' })).not.toHaveAttribute('aria-current');
    fireEvent.click(screen.getByRole('button', { name: '模型与 API' }));
    expect(navigate).toHaveBeenLastCalledWith('api');
  });
});

import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ProjectNetworkApprovalsSettings } from './pages/ProjectNetworkApprovalsSettings';
import { ExecutionPermissionSettings } from './pages/ExecutionPermissionSettings';
import { SettingsNav } from './components/SettingsNav';
import { getSettingsNavItems, settingsRegistry } from './SettingsRouter';

describe('SettingsRouter project network permissions integration', () => {
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
    expect(screen.getByRole('button', { name: '执行权限' }).closest('.settings-nav-group')).toHaveTextContent('权限与工具');
  });
});

import type { SettingsPage, SettingsRegistry } from './types';
import type { ReactNode } from 'react';
import { settingsNavItems } from './components/SettingsIcons';
import { PersonalitySettings } from './pages/PersonalitySettings';
import { MemoryGovernanceSettings } from './pages/MemoryGovernanceSettings';
import { ApiSettings } from './pages/ApiSettings';
import { AppearanceSettings } from './pages/AppearanceSettings';
import { McpServersSettings } from './pages/McpServersSettings';
import { NotificationsSettings } from './pages/NotificationsSettings';
import { DataSettings } from './pages/DataSettings';
import { FileAccessSettings } from './pages/FileAccessSettings';
import { ExecutionPermissionSettings } from './pages/ExecutionPermissionSettings';
import { DesktopControlSettings } from './pages/DesktopControlSettings';
import { DefaultWorkModeSettings } from './pages/DefaultWorkModeSettings';
import { WebSearchSettings } from './pages/WebSearchSettings';
import { ProjectNetworkApprovalsSettings } from './pages/ProjectNetworkApprovalsSettings';
import { ChannelsPage } from '../Channels/ChannelsPage';
import { SettingsNav } from './components/SettingsNav';

interface SettingsRouterProps {
  activePage: SettingsPage;
  onNavigate: (page: SettingsPage) => void;
  apiSaveAction?: ReactNode;
}

export const settingsRegistry: SettingsRegistry = {
  id: 'default',
  navItems: settingsNavItems,
  pages: {
    personality: { title: '性格', component: PersonalitySettings },
    memory: { title: '记忆', component: MemoryGovernanceSettings },
    api: { title: '模型与 API', component: ApiSettings },
    webSearch: { title: '联网搜索', component: WebSearchSettings },
    projectNetwork: { title: '项目联网权限', component: ProjectNetworkApprovalsSettings },
    workMode: { title: '默认工作方式', component: DefaultWorkModeSettings },
    appearance: { title: '外观', component: AppearanceSettings },
    mcp: { title: 'MCP 服务', component: McpServersSettings },
    notifications: { title: '通知', component: NotificationsSettings },
    data: { title: '数据', component: DataSettings },
    executionPermission: { title: '执行权限', component: ExecutionPermissionSettings },
    fileAccess: { title: '文件访问', component: FileAccessSettings },
    desktopControl: { title: '电脑操作', component: DesktopControlSettings },
    channels: { title: '渠道与远程访问', component: ChannelsPage },
  },
};

export function getPageComponent(pageId: SettingsPage) {
  return settingsRegistry.pages[pageId]?.component;
}

export function getSettingsNavItems() {
  return settingsRegistry.navItems;
}

export function registerSettingsPage(
  id: string,
  navItem: { label: string; icon: React.ReactNode },
  page: { title: string; component: React.FC }
) {
  settingsRegistry.navItems.push({ id, ...navItem });
  settingsRegistry.pages[id] = page;
}

export function SettingsRouter({ activePage, onNavigate, apiSaveAction }: SettingsRouterProps) {
  const page = settingsRegistry.pages[activePage];
  const PageComponent = page?.component;

  return (
    <div className="settings-router">
      <SettingsNav activePage={activePage} onNavigate={onNavigate} />
      <div className="settings-content" key={activePage}>
        {page && <div className="settings-page-header">
          <h2 className="settings-page-title settings-page-layout">{page.title}</h2>
        </div>}
        <div className="settings-page-body">
          <div className="settings-page-layout">
            {activePage === 'api' && PageComponent === ApiSettings ? <ApiSettings saveAction={apiSaveAction} />
              : PageComponent ? <PageComponent /> : <p className="settings-page-not-found">找不到此设置页面</p>}
          </div>
        </div>
      </div>
    </div>
  );
}

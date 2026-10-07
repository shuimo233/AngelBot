import type { SettingsPage, SettingsRegistry } from './types';
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
    channels: { title: 'Channels & Remote Access', component: ChannelsPage },
  },
};

const pageDescriptions: Partial<Record<SettingsPage, string>> = {
  personality: '塑造 AngelBot 的表达方式与长期互动风格。',
  memory: '查看、筛选和管理可审计的长期记忆。',
  api: '选择模型并配置连接与生成参数。',
  webSearch: '选择受信任的搜索服务，并控制 AngelBot 是否可以使用联网检索。',
  projectNetwork: '查看或撤销当前项目为探索子代理批准的联网访问范围。',
  workMode: '设置后续回复默认采用的表达与推理方式。',
  appearance: '调整界面显示与阅读体验。',
  mcp: '管理已连接的工具服务及其可用状态。',
  notifications: '设置提醒、主动消息与系统通知。',
  data: '导入、导出或清理本地数据。',
  executionPermission: '设置所有工作区共用的执行确认方式。',
  fileAccess: '限定当前会话能够读取和写入的本地目录。',
  desktopControl: '仅为受信任的应用授予经过确认的操作能力。',
  channels: '配置外部渠道与远程访问。',
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

export function SettingsRouter({ activePage, onNavigate }: SettingsRouterProps) {
  const PageComponent = getPageComponent(activePage);

  if (!PageComponent) {
    return (
      <div className="settings-router">
        <SettingsNav activePage={activePage} onNavigate={onNavigate} />
        <div className="settings-content">
          <div className="settings-page-content">
            <p className="settings-page-not-found">Page not found</p>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="settings-router">
      <SettingsNav activePage={activePage} onNavigate={onNavigate} />
      <div className="settings-content" key={activePage}>
        <div className="settings-page-header">
          <h2 className="settings-page-title">
            {settingsRegistry.pages[activePage]?.title}
          </h2>
          {pageDescriptions[activePage] && (
            <p className="settings-page-description">{pageDescriptions[activePage]}</p>
          )}
        </div>
        <div className="settings-page-body">
          <PageComponent />
        </div>
      </div>
    </div>
  );
}

import { settingsNavItems } from './SettingsIcons';
import type { SettingsPage } from '../types';

interface SettingsNavProps {
  activePage: SettingsPage;
  onNavigate: (page: SettingsPage) => void;
}

export function SettingsNav({ activePage, onNavigate }: SettingsNavProps) {
  const groups = [
    { label: '智能体', pages: ['personality', 'memory'] },
    { label: '模型与连接', pages: ['api', 'webSearch', 'mcp', 'channels'] },
    { label: '权限与工具', pages: ['executionPermission', 'desktopControl', 'fileAccess', 'projectNetwork'] },
    { label: '偏好与数据', pages: ['workMode', 'appearance', 'notifications', 'data'] },
  ];

  return (
    <nav className="settings-nav" aria-label="设置分类">
      {groups.map((group) => {
        const items = settingsNavItems.filter((item) => group.pages.includes(item.id));
        if (items.length === 0) return null;

        return (
          <div className="settings-nav-group" key={group.label}>
            <div className="settings-nav-group-label">{group.label}</div>
            {items.map((item) => (
              <button
                key={item.id}
                className={`settings-nav-item ${activePage === item.id ? 'active' : ''}`}
                onClick={() => onNavigate(item.id)}
                aria-current={activePage === item.id ? 'page' : undefined}
              >
                <span className="settings-nav-icon">{item.icon}</span>
                <span className="settings-nav-label">{item.label}</span>
              </button>
            ))}
          </div>
        );
      })}
    </nav>
  );
}

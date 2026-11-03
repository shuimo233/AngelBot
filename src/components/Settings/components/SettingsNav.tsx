import { settingsNavItems } from './SettingsIcons';
import type { SettingsPage } from '../types';

interface SettingsNavProps {
  activePage: SettingsPage;
  onNavigate: (page: SettingsPage) => void;
}

export function SettingsNav({ activePage, onNavigate }: SettingsNavProps) {
  const groups = [
    { label: '常规', pages: ['personality', 'appearance', 'workMode'] },
    { label: '模型与连接', pages: ['api'] },
    { label: '工具与应用', pages: ['webSearch', 'mcp', 'desktopControl', 'channels'] },
    { label: '隐私与权限', pages: ['executionPermission', 'fileAccess', 'projectNetwork'] },
    { label: '记忆与数据', pages: ['memory', 'notifications', 'data'] },
  ];

  return (
    <nav className="settings-nav" aria-label="设置分类">
      {groups.map((group) => {
        const items = group.pages.flatMap((id) => settingsNavItems.filter((item) => item.id === id));
        if (items.length === 0) return null;
        const active = group.pages.includes(activePage);

        return (
          <div className="settings-nav-group" key={group.label}>
            <button type="button" className="settings-nav-group-button"
              aria-current={active ? items.length === 1 ? 'page' : 'true' : undefined}
              onClick={() => { if (!active) onNavigate(items[0].id); }}>
              {group.label}
            </button>
            {active && items.length > 1 && <div className="settings-nav-pages">{items.map((item) => (
              <button
                key={item.id}
                className={`settings-nav-item ${activePage === item.id ? 'active' : ''}`}
                onClick={() => onNavigate(item.id)}
                aria-current={activePage === item.id ? 'page' : undefined}
              >
                <span className="settings-nav-icon">{item.icon}</span>
                <span className="settings-nav-label">{item.label}</span>
              </button>
            ))}</div>}
          </div>
        );
      })}
    </nav>
  );
}

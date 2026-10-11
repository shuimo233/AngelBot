import { settingsNavItems } from './SettingsIcons';
import type { SettingsPage } from '../types';
import { Select } from '../../../ui';

interface SettingsNavProps {
  activePage: SettingsPage;
  onNavigate: (page: SettingsPage) => void;
}

const groups = [
  { label: '智能体', pages: ['personality', 'memory'] },
  { label: '模型与连接', pages: ['api', 'webSearch', 'mcp', 'channels'] },
  { label: '权限与工具', pages: ['executionPermission', 'desktopControl', 'fileAccess', 'projectNetwork'] },
  { label: '偏好与数据', pages: ['workMode', 'appearance', 'notifications', 'data'] },
];

export function SettingsNav({ activePage, onNavigate }: SettingsNavProps) {
  return (
    <nav className="settings-nav" aria-label="设置分类">
      <label className="settings-nav-compact">
        <span>设置页面</span>
        <Select value={activePage} onChange={(event) => onNavigate(event.target.value as SettingsPage)}>
          {groups.map((group) => (
            <optgroup key={group.label} label={group.label}>
              {settingsNavItems.filter((item) => group.pages.includes(item.id)).map((item) => (
                <option key={item.id} value={item.id}>{item.label}</option>
              ))}
            </optgroup>
          ))}
        </Select>
      </label>
      <div className="settings-nav-list">
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
      </div>
    </nav>
  );
}

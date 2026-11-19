import { useEffect, useRef } from 'react';
import { settingsNavItems } from './SettingsIcons';
import type { SettingsPage } from '../types';
import { Select } from '../../../ui';

interface SettingsNavProps {
  activePage: SettingsPage;
  onNavigate: (page: SettingsPage) => void;
}

export function SettingsNav({ activePage, onNavigate }: SettingsNavProps) {
  const activeItem = useRef<HTMLButtonElement>(null);
  const groups = [
    { label: '常规', pages: ['personality', 'appearance', 'workMode'] },
    { label: '模型与连接', pages: ['api'] },
    { label: '工具与应用', pages: ['webSearch', 'mcp', 'desktopControl', 'channels'] },
    { label: '隐私与权限', pages: ['executionPermission', 'fileAccess', 'projectNetwork'] },
    { label: '记忆与数据', pages: ['memory', 'notifications', 'data'] },
  ];
  const sections = groups.map(group => ({ ...group,
    items: group.pages.flatMap(id => settingsNavItems.filter(item => item.id === id)),
  })).filter(group => group.items.length > 0);

  useEffect(() => {
    if (activeItem.current?.getClientRects().length) {
      activeItem.current.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    }
  }, [activePage]);

  return (
    <nav className="settings-nav" aria-label="设置分类">
      <label className="settings-nav-compact">
        <span>设置页面</span>
        <Select value={activePage} onChange={event => onNavigate(event.target.value as SettingsPage)}>
          {sections.map(group => <optgroup key={group.label} label={group.label}>
            {group.items.map(item => <option key={item.id} value={item.id}>{item.label}</option>)}
          </optgroup>)}
        </Select>
      </label>
      <div className="settings-nav-list">
        {sections.map(group => (
          <div className="settings-nav-group" role="group" aria-label={group.label} key={group.label}>
            {group.items.length > 1 && <p className="settings-nav-group-title" aria-hidden="true">{group.label}</p>}
            {group.items.map(item => (
              <button
                key={item.id}
                ref={activePage === item.id ? activeItem : undefined}
                type="button"
                className={`settings-nav-item ${activePage === item.id ? 'active' : ''}`}
                onClick={() => onNavigate(item.id)}
                aria-current={activePage === item.id ? 'page' : undefined}
              >
                <span className="settings-nav-icon" aria-hidden="true">{item.icon}</span>
                <span className="settings-nav-label">{item.label}</span>
              </button>
            ))}
          </div>
        ))}
      </div>
    </nav>
  );
}

import type { SettingsPage } from '../types';

// Inline SVG icons for Settings Navigation
// Using simple, consistent stroke-based icons

export const SettingsIcons = {
  personality: (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="8" r="4" />
      <path d="M4 20c0-4 4-6 8-6s8 2 8 6" />
    </svg>
  ),
  api: (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3" y="11" width="18" height="11" rx="2" />
      <path d="M7 11V7a5 5 0 0 1 10 0v4" />
    </svg>
  ),
  appearance: (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" />
    </svg>
  ),
  memory: (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M12 3a7 7 0 0 0-7 7v4a4 4 0 0 0 4 4h1v3h4v-3h1a4 4 0 0 0 4-4v-4a7 7 0 0 0-7-7z" />
      <path d="M9 10h.01M15 10h.01M9 14h6" />
    </svg>
  ),
  mcp: (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <rect x="2" y="3" width="20" height="14" rx="2" />
      <path d="M8 21h8M12 17v4" />
    </svg>
  ),
  notifications: (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9" />
      <path d="M13.73 21a2 2 0 0 1-3.46 0" />
    </svg>
  ),
  data: (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <ellipse cx="12" cy="5" rx="9" ry="3" />
      <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
      <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
    </svg>
  ),
  webSearch: (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="11" cy="11" r="7" />
      <path d="m20 20-4.2-4.2" />
      <path d="M8 11h6M11 8v6" />
    </svg>
  ),
  projectNetwork: (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M12 3 5 6v5c0 4.4 3 8.2 7 10 4-1.8 7-5.6 7-10V6l-7-3Z" />
      <path d="M8.5 12h7M12 8.5v7" />
    </svg>
  ),
  executionPermission: (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M12 3 4 7v5c0 4.5 3.2 8 8 9 4.8-1 8-4.5 8-9V7l-8-4Z" />
      <path d="m8.5 12 2.5 2.5 4.5-5" />
    </svg>
  ),
  desktopControl: (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3" y="4" width="18" height="13" rx="2" />
      <path d="M8 21h8M12 17v4M8 10h8" />
    </svg>
  ),
  workMode: (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M4 6h16M4 12h16M4 18h16" />
      <circle cx="9" cy="6" r="2" />
      <circle cx="15" cy="12" r="2" />
      <circle cx="11" cy="18" r="2" />
    </svg>
  ),
};

export interface SettingsNavItem {
  id: SettingsPage;
  label: string;
  icon: React.ReactNode;
}

export const settingsNavItems: SettingsNavItem[] = [
  { id: 'executionPermission', label: '执行权限', icon: SettingsIcons.executionPermission },
  { id: 'desktopControl', label: '电脑操作', icon: SettingsIcons.desktopControl },
  { id: 'fileAccess', label: '文件访问', icon: SettingsIcons.data },
  { id: 'personality', label: '性格', icon: SettingsIcons.personality },
  { id: 'memory', label: '记忆', icon: SettingsIcons.memory },
  { id: 'api', label: '模型 API', icon: SettingsIcons.api },
  { id: 'webSearch', label: '联网搜索', icon: SettingsIcons.webSearch },
  { id: 'projectNetwork', label: '项目联网权限', icon: SettingsIcons.projectNetwork },
  { id: 'workMode', label: '默认工作方式', icon: SettingsIcons.workMode },
  { id: 'appearance', label: '外观', icon: SettingsIcons.appearance },
  { id: 'mcp', label: 'MCP', icon: SettingsIcons.mcp },
  { id: 'notifications', label: '通知', icon: SettingsIcons.notifications },
  { id: 'data', label: '数据', icon: SettingsIcons.data },
];

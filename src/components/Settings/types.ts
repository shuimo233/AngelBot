import type { ReactNode } from 'react';

export interface SettingsNavItem {
  id: string;
  label: string;
  icon: ReactNode;
}

export type SettingsPage = string;

export interface SettingsRegistry {
  id: string;
  navItems: SettingsNavItem[];
  pages: Record<string, { title: string; component: React.FC }>;
}

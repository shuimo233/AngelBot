import { useCallback, useEffect, useState } from 'react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { useNavigationStore, type AppPage } from '$stores/navigation';
import { useSettingsStore } from '$stores/settings';
import { useWorkspacesStore } from '$stores/workspaces';
import './Sidebar.css';

interface SidebarProps { onToggleTheme?: () => void; dark?: boolean; onOpenSettings?: () => void; onOpenQuickSwitch?: () => void; open?: boolean; onClose?: () => void; }
const NAVIGATION: { id: AppPage; label: string }[] = [
  { id: 'chat', label: '对话' }, { id: 'tasks', label: '自动化' }, { id: 'knowledge', label: '资料库' },
];
const FolderIcon = () => <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" /></svg>;
const SunIcon = () => <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><circle cx="12" cy="12" r="4" /><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" /></svg>;
const MoonIcon = () => <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><path d="M21 12.8A9 9 0 1 1 11.2 3 7 7 0 0 0 21 12.8z" /></svg>;
const SettingsIcon = () => <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.7 1.7 0 0 0 .34 1.88l.06.06-2.12 2.12-.06-.06a1.7 1.7 0 0 0-1.88-.34 1.7 1.7 0 0 0-1.03 1.55V20.3h-3v-.09A1.7 1.7 0 0 0 10.68 18.66a1.7 1.7 0 0 0-1.88.34l-.06.06-2.12-2.12.06-.06A1.7 1.7 0 0 0 7.02 15 1.7 1.7 0 0 0 5.47 13.97h-.09v-3h.09A1.7 1.7 0 0 0 7.02 9.94a1.7 1.7 0 0 0-.34-1.88l-.06-.06 2.12-2.12.06.06a1.7 1.7 0 0 0 1.88.34 1.7 1.7 0 0 0 1.03-1.55v-.09h3v.09a1.7 1.7 0 0 0 1.03 1.55 1.7 1.7 0 0 0 1.88-.34l.06-.06L19.8 8l-.06.06a1.7 1.7 0 0 0-.34 1.88 1.7 1.7 0 0 0 1.55 1.03h.09v3h-.09A1.7 1.7 0 0 0 19.4 15z" /></svg>;
const SearchIcon = () => <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><circle cx="11" cy="11" r="7" /><path d="m20 20-4-4" /></svg>;

export function Sidebar({ onToggleTheme, dark, onOpenSettings, onOpenQuickSwitch, open, onClose }: SidebarProps) {
  const profile = useSettingsStore((state) => state.profile);
  const currentPage = useNavigationStore((state) => state.currentPage);
  const setPage = useNavigationStore((state) => state.setPage);
  const workspaces = useWorkspacesStore((state) => state.workspaces);
  const activeWorkspaceId = useWorkspacesStore((state) => state.activeWorkspaceId);
  const loadWorkspaces = useWorkspacesStore((state) => state.loadWorkspaces);
  const openWorkspace = useWorkspacesStore((state) => state.openWorkspace);
  const createProject = useWorkspacesStore((state) => state.createProject);
  const [projectError, setProjectError] = useState('');
  const [creating, setCreating] = useState(false);
  const [openingWorkspaceId, setOpeningWorkspaceId] = useState<string | null>(null);

  const createFromDirectory = useCallback(async () => {
    if (creating) return;
    setCreating(true);
    setProjectError('');
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        title: '选择项目文件夹',
      });
      if (typeof selected !== 'string') return;
      await createProject(selected);
      setPage('chat');
      onClose?.();
    } catch (reason) {
      setProjectError(reason instanceof Error ? reason.message : '创建项目失败');
    } finally {
      setCreating(false);
    }
  }, [createProject, creating, onClose, setPage]);

  useEffect(() => { void loadWorkspaces(); }, [loadWorkspaces]);
  useEffect(() => {
    const chooseProjectDirectory = () => { void createFromDirectory(); };
    window.addEventListener('new-project', chooseProjectDirectory);
    return () => window.removeEventListener('new-project', chooseProjectDirectory);
  }, [createFromDirectory]);
  const personal = workspaces.find((item) => item.kind === 'personal');
  const projects = workspaces.filter((item) => item.kind === 'project');
  const choose = useCallback(async (id: string) => {
    if (openingWorkspaceId) return;
    setOpeningWorkspaceId(id);
    setProjectError('');
    try {
      await openWorkspace(id);
      setPage('chat');
      onClose?.();
    } catch (reason) {
      setProjectError(reason instanceof Error ? reason.message : '无法打开此工作区');
    } finally {
      setOpeningWorkspaceId(null);
    }
  }, [onClose, openWorkspace, openingWorkspaceId, setPage]);
  return <aside className={`sidebar${open ? ' open' : ''}`}>
    <div className="sidebar-identity">
      <div className="sidebar-profile-mark">{(profile.name || 'A').slice(0, 1).toUpperCase()}</div>
      <span className="sidebar-profile-name">{profile.name || 'AngelBot'}</span><div className="sidebar-spacer" />
      <button type="button" className="icon-btn" title={dark ? '浅色模式' : '深色模式'} aria-label={dark ? '浅色模式' : '深色模式'} onClick={onToggleTheme}>{dark ? <SunIcon /> : <MoonIcon />}</button>
      <button type="button" className="icon-btn" title="设置" aria-label="设置" onClick={onOpenSettings}><SettingsIcon /></button>
    </div>
    <nav className="sidebar-primary-nav" aria-label="主导航">{NAVIGATION.map((item) => <button key={item.id} type="button" className={`sidebar-primary-nav-item ${currentPage === item.id ? 'active' : ''}`} onClick={() => setPage(item.id)}>{item.label}</button>)}</nav>
    <div className="sidebar-session-bar"><span className="sidebar-section-heading">工作区</span><div className="sidebar-spacer" />{onOpenQuickSwitch && <button type="button" className="sidebar-quick-switch" title="快速切换 (Ctrl+K)" aria-label="快速切换" onClick={onOpenQuickSwitch}><SearchIcon /></button>}<button type="button" className="sidebar-add-project" title={creating ? '正在创建项目' : '新建项目'} aria-label={creating ? '正在创建项目' : '新建项目'} disabled={creating} onClick={() => void createFromDirectory()}><span aria-hidden="true">+</span></button></div>
    <div className="sidebar-section">
      {projectError && <p className="sidebar-project-error" role="alert">{projectError}</p>}
      {personal && <button type="button" className={`session-item-select ${activeWorkspaceId === personal.id ? 'active' : ''}`} onClick={() => void choose(personal.id)} disabled={Boolean(openingWorkspaceId)} aria-busy={openingWorkspaceId === personal.id || undefined}><span className="session-item-title">AngelBot 日常</span><span className="session-item-time">陪伴与日常事务</span></button>}
      <div className="sidebar-section-label">项目</div>
      {projects.map((workspace) => <button key={workspace.id} type="button" className={`session-item-select ${activeWorkspaceId === workspace.id ? 'active' : ''}`} onClick={() => void choose(workspace.id)} title={workspace.rootPath} disabled={Boolean(openingWorkspaceId)} aria-busy={openingWorkspaceId === workspace.id || undefined}><FolderIcon /><span className="session-item-title">{workspace.name}</span></button>)}
      {projects.length === 0 && <div className="session-empty"><div className="session-empty-hint">创建项目后，AngelBot 会为它保留唯一的主对话。</div></div>}
    </div>
  </aside>;
}

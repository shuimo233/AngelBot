import { useCallback, useEffect, useRef, useState } from 'react';
import { openProjectFolder } from '$lib/project-folder';
import { useNavigationStore, type AppPage } from '$stores/navigation';
import { useSettingsStore } from '$stores/settings';
import { useThemeStore } from '$stores/theme';
import { useWorkspacesStore } from '$stores/workspaces';
import { SidebarToggleIcon } from './SidebarToggleIcon';
import './Sidebar.css';

interface SidebarProps { onToggleTheme?: () => void; dark?: boolean; onOpenSettings?: () => void; onOpenQuickSwitch?: () => void; open?: boolean; onClose?: () => void; }
const NAVIGATION: { id: AppPage; label: string }[] = [
  { id: 'chat', label: '日常' }, { id: 'projects', label: '项目' }, { id: 'tasks', label: '自动化' }, { id: 'knowledge', label: '资料库' },
];
const FolderIcon = () => <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" /></svg>;
const SunIcon = () => <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><circle cx="12" cy="12" r="4" /><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" /></svg>;
const MoonIcon = () => <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><path d="M21 12.8A9 9 0 1 1 11.2 3 7 7 0 0 0 21 12.8z" /></svg>;
const SettingsIcon = () => <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.7 1.7 0 0 0 .34 1.88l.06.06-2.12 2.12-.06-.06a1.7 1.7 0 0 0-1.88-.34 1.7 1.7 0 0 0-1.03 1.55V20.3h-3v-.09A1.7 1.7 0 0 0 10.68 18.66a1.7 1.7 0 0 0-1.88.34l-.06.06-2.12-2.12.06-.06A1.7 1.7 0 0 0 7.02 15 1.7 1.7 0 0 0 5.47 13.97h-.09v-3h.09A1.7 1.7 0 0 0 7.02 9.94a1.7 1.7 0 0 0-.34-1.88l-.06-.06 2.12-2.12.06.06a1.7 1.7 0 0 0 1.88.34 1.7 1.7 0 0 0 1.03-1.55v-.09h3v.09a1.7 1.7 0 0 0 1.03 1.55 1.7 1.7 0 0 0 1.88-.34l.06-.06L19.8 8l-.06.06a1.7 1.7 0 0 0-.34 1.88 1.7 1.7 0 0 0 1.55 1.03h.09v3h-.09A1.7 1.7 0 0 0 19.4 15z" /></svg>;
const SearchIcon = () => <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><circle cx="11" cy="11" r="7" /><path d="m20 20-4-4" /></svg>;
const NavIcon = ({ page }: { page: AppPage }) => <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
  {page === 'chat' ? <path d="M5 4h14v12H9l-4 4V4zM9 8h6M9 12h4" />
    : page === 'tasks' ? <><rect x="4" y="5" width="16" height="15" rx="2" /><path d="M8 3v4M16 3v4M4 10h16m-10 5 2 2 4-4" /></>
    : <><path d="M4 5h6l2 2h8v13H4zM8 11h8M8 15h5" /></>}
</svg>;

export function Sidebar({ onToggleTheme, dark, onOpenSettings, onOpenQuickSwitch, open, onClose }: SidebarProps) {
  const layout = useThemeStore((state) => state.layout);
  const setLayout = useThemeStore((state) => state.setLayout);
  const profile = useSettingsStore((state) => state.profile);
  const currentPage = useNavigationStore((state) => state.currentPage);
  const setPage = useNavigationStore((state) => state.setPage);
  const workspaces = useWorkspacesStore((state) => state.workspaces);
  const activeWorkspaceId = useWorkspacesStore((state) => state.activeWorkspaceId);
  const loadWorkspaces = useWorkspacesStore((state) => state.loadWorkspaces);
  const loadingWorkspaces = useWorkspacesStore((state) => state.loading);
  const workspaceError = useWorkspacesStore((state) => state.lastError);
  const openWorkspace = useWorkspacesStore((state) => state.openWorkspace);
  const [projectError, setProjectError] = useState('');
  const [creating, setCreating] = useState(false);
  const creationInFlight = useRef(false);
  const [openingWorkspaceId, setOpeningWorkspaceId] = useState<string | null>(null);

  const createFromDirectory = useCallback(async () => {
    if (creationInFlight.current) return;
    creationInFlight.current = true;
    setCreating(true);
    setProjectError('');
    try {
      if (await openProjectFolder()) onClose?.();
    } catch (reason) {
      setProjectError(reason instanceof Error ? reason.message : '创建项目失败');
    } finally {
      creationInFlight.current = false;
      setCreating(false);
    }
  }, [onClose]);

  useEffect(() => { void loadWorkspaces(); }, [loadWorkspaces]);
  useEffect(() => {
    const chooseProjectDirectory = () => { void createFromDirectory(); };
    window.addEventListener('new-project', chooseProjectDirectory);
    return () => window.removeEventListener('new-project', chooseProjectDirectory);
  }, [createFromDirectory]);
  const personal = workspaces.find((item) => item.kind === 'personal');
  const projects = workspaces.filter((item) => item.kind === 'project');
  const activeWorkspace = workspaces.find(item => item.id === activeWorkspaceId);
  const showProjects = currentPage === 'projects' || (currentPage === 'chat' && activeWorkspace?.kind === 'project');
  const currentNav = currentPage === 'chat' && activeWorkspace?.kind === 'project' ? 'projects' : currentPage;
  const choose = useCallback(async (id: string) => {
    if (openingWorkspaceId) return;
    setOpeningWorkspaceId(id);
    setProjectError('');
    const navigation = useNavigationStore.getState();
    try {
      const accepted = await openWorkspace(id);
      if (!accepted || navigation !== useNavigationStore.getState()) return;
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
      <button type="button" className="sidebar-layout-toggle" aria-label="切换界面布局" aria-pressed={layout === 'B'} title={layout === 'A' ? '收起文字侧栏，切换到 B 生活视图' : '展开文字侧栏，切换到 A 安静工作区'} onClick={() => setLayout(layout === 'A' ? 'B' : 'A')}><SidebarToggleIcon /></button>
      <span className="sidebar-profile-name">{profile.name || 'AngelBot'}</span>
    </div>
    {onOpenQuickSwitch && <button type="button" className="sidebar-quick-switch" title="快速切换 (Ctrl+K)" aria-label="快速切换" onClick={onOpenQuickSwitch}><SearchIcon /><span>查找与切换</span><kbd>Ctrl K</kbd></button>}
    <nav className="sidebar-primary-nav" aria-label="主导航">{NAVIGATION.map((item) => <button key={item.id} type="button" disabled={item.id === 'chat' && (!personal || Boolean(openingWorkspaceId))} aria-current={currentNav === item.id ? 'page' : undefined} aria-label={item.label} title={item.label} className={`sidebar-primary-nav-item ${currentNav === item.id ? 'active' : ''}`} onClick={() => {
      if (item.id === 'chat' && personal) { void choose(personal.id); return; }
      setPage(item.id); onClose?.();
    }}><NavIcon page={item.id} /><span>{item.label}</span></button>)}</nav>
    <div className="sidebar-feedback">
      {projectError && <p className="sidebar-project-error" role="alert">{projectError}</p>}
      {workspaceError && !projectError && <div className="sidebar-project-error" role="alert">{workspaceError}<button type="button" disabled={loadingWorkspaces} onClick={() => void loadWorkspaces()}>重新加载</button></div>}
      {loadingWorkspaces && !workspaces.length && <p className="sidebar-loading" role="status">正在读取工作区…</p>}
    </div>
    <div className="sidebar-section">
      {showProjects && <><div className="sidebar-project-heading"><span className="sidebar-section-label">项目</span><button type="button" className="sidebar-add-project" title={creating ? '正在创建项目' : '新建项目'} aria-label={creating ? '正在创建项目' : '新建项目'} disabled={creating} onClick={() => void createFromDirectory()}><span aria-hidden="true">+</span><span>添加</span></button></div>
      {projects.some((workspace) => workspace.directoryConfirmed === false) && <button type="button" className="sidebar-restore-directories" onClick={() => window.dispatchEvent(new CustomEvent('open-settings', { detail: 'data' }))}>确认恢复目录（{projects.filter((workspace) => workspace.directoryConfirmed === false).length}）</button>}
      {projects.map((workspace) => <button key={workspace.id} type="button" className={`session-item-select ${activeWorkspaceId === workspace.id && currentPage === 'chat' ? 'active' : ''}`} onClick={() => void choose(workspace.id)} title={workspace.rootPath} disabled={Boolean(openingWorkspaceId)} aria-busy={openingWorkspaceId === workspace.id || undefined}><FolderIcon /><span className="session-item-title">{workspace.name}</span></button>)}
      {projects.length === 0 && !loadingWorkspaces && !workspaceError && <div className="session-empty"><div className="session-empty-hint">添加常用文件夹，在独立项目里接着上次的事继续。</div></div>}</>}
    </div>
    <footer className="sidebar-footer">
      <button type="button" className="sidebar-settings" aria-label="设置" title="设置 (Ctrl+,)" onClick={onOpenSettings}><SettingsIcon /><span>设置</span></button>
      <button type="button" className="icon-btn" title={dark ? '浅色模式' : '深色模式'} aria-label={dark ? '浅色模式' : '深色模式'} onClick={onToggleTheme}>{dark ? <SunIcon /> : <MoonIcon />}</button>
    </footer>
  </aside>;
}

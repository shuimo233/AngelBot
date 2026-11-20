import { useCallback, useEffect, useState, type DragEvent } from 'react';
import { listen } from '@tauri-apps/api/event';
import { DESKTOP_PET_OPEN_CHAT } from '$lib/commands/desktop-pet';
import { isTauriRuntime } from '$lib/invoke';
import { Sidebar } from './components/Sidebar';
import { SidebarToggleIcon } from './components/SidebarToggleIcon';
import { ChatArea } from './components/ChatArea';
import { RightPanel, type WorkspaceView } from './components/RightPanel';
import { AutomationPage } from './components/Automation/AutomationPage';
import { LibraryPage } from './components/Library/LibraryPage';
import { ProjectsPage } from './components/ProjectsPage';
import { RuntimeHealthNotice } from './components/RuntimeHealthNotice';
import { SettingsModal } from './components/Settings';
import { CommandPalette } from './components/CommandPalette';
import type { SettingsPage } from './components/Settings/types';
import { useNavigationStore } from './stores/navigation';
import { useSettingsStore } from './stores/settings';
import { useWorkspacesStore } from './stores/workspaces';
import { initializeTheme, useThemeStore } from './stores/theme';
import { initializeAppearance } from './stores/appearance';
import './styles/settings.css';
import './styles/layouts.css';

interface OpenSettingsEvent extends Event {
  detail?: SettingsPage;
}

interface LocateFileEvent extends Event {
  detail?: { path?: unknown };
}

// HTML file drops belong to an explicit child target, never WebView navigation.
// Do not stop propagation: the composer's bounded loader still handles its drop.
function preventFileNavigation(event: DragEvent<HTMLDivElement>) {
  if (event.dataTransfer.files.length || event.dataTransfer.types.includes('Files')) event.preventDefault();
}

function visibleCompactMenu() {
  const menu = document.querySelector<HTMLButtonElement>('.mobile-menu-btn');
  return menu && menu.getBoundingClientRect().width > 0 ? menu : null;
}

export default function App() {
  const apiConfigLoaded = useSettingsStore((s) => s.apiConfigLoaded);
  const loadApiConfig = useSettingsStore((s) => s.loadApiConfig);
  const currentPage = useNavigationStore((s) => s.currentPage);
  const workspaces = useWorkspacesStore((s) => s.workspaces);
  const activeWorkspaceId = useWorkspacesStore((s) => s.activeWorkspaceId);
  const dark = useThemeStore((s) => s.resolvedTheme === 'dark');
  const toggleTheme = useThemeStore((s) => s.toggleTheme);
  const layout = useThemeStore((s) => s.layout);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsPage, setSettingsPage] = useState<SettingsPage>('personality');
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [workbenchOpen, setWorkbenchOpen] = useState(false);
  const [workbenchRequest, setWorkbenchRequest] = useState<{ view: WorkspaceView; revision: number } | null>(null);
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);
  const [fileLocator, setFileLocator] = useState<{
    path: string;
    revision: number;
    workspaceId: string;
    sessionId: string;
    rootPath?: string;
    directoryConfirmed?: boolean;
  } | null>(null);
  const activeWorkspace = workspaces.find((workspace) => workspace.id === activeWorkspaceId);
  const workbenchAvailable = activeWorkspace?.kind === 'project' || activeWorkspace?.kind === 'personal';
  // Effects run after children can consume props: reject an old scope during
  // render, before RightPanel can dispatch a read against the new Main.
  const scopedFileLocator = fileLocator
    && fileLocator.workspaceId === activeWorkspaceId
    && fileLocator.sessionId === activeWorkspace?.activeSessionId
    && fileLocator.rootPath === activeWorkspace?.rootPath
    && fileLocator.directoryConfirmed === activeWorkspace?.directoryConfirmed
    ? fileLocator : null;
  const closeWorkbench = useCallback(() => {
    setWorkbenchOpen(false);
    setWorkbenchRequest(null);
    setFileLocator(null);
  }, []);
  const focusWorkbenchToggle = useCallback(() => {
    window.requestAnimationFrame(() =>
      document.querySelector<HTMLButtonElement>('button[aria-controls="conversation-workbench"]')?.focus());
  }, []);
  const toggleWorkbench = useCallback(() => {
    if (workbenchOpen) {
      closeWorkbench();
      focusWorkbenchToggle();
    } else setWorkbenchOpen(true);
  }, [closeWorkbench, focusWorkbenchToggle, workbenchOpen]);
  const openWorkbenchView = useCallback((view: WorkspaceView) => {
    setFileLocator(null);
    setWorkbenchRequest((current) => ({ view, revision: (current?.revision ?? 0) + 1 }));
    setWorkbenchOpen(true);
  }, []);

  // Discard the finished locator state when its owner changes; the synchronous
  // scope check above already prevents consumption before this effect runs.
  useEffect(() => {
    setFileLocator(null);
  }, [activeWorkspaceId, activeWorkspace?.activeSessionId, activeWorkspace?.rootPath, activeWorkspace?.directoryConfirmed]);

  useEffect(() => {
    if (!apiConfigLoaded) {
      void loadApiConfig();
    }
  }, [apiConfigLoaded, loadApiConfig]);

  useEffect(() => initializeTheme(), []);
  useEffect(() => initializeAppearance(), []);
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen(DESKTOP_PET_OPEN_CHAT, () => {
      if (!disposed) useNavigationStore.getState().setPage('chat');
    }).then(stop => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch(() => {
      // The desktop window can still bring AngelBot to the foreground.
    });
    return () => { disposed = true; unlisten?.(); };
  }, []);

  // B's rail is always visible. Returning to compact A must not strand
  // keyboard focus in its now off-screen sidebar.
  useEffect(() => {
    if (layout !== 'A' || sidebarOpen || !document.activeElement?.closest('.sidebar')) return;
    visibleCompactMenu()?.focus();
  }, [layout, sidebarOpen]);

  useEffect(() => {
    const handleLocateFile = (event: Event) => {
      if (!workbenchAvailable || !activeWorkspace) return;
      const path = (event as LocateFileEvent).detail?.path;
      if (typeof path !== 'string' || !path.trim()) return;
      setFileLocator((current) => ({
        path: path.trim(), revision: (current?.revision ?? 0) + 1,
        workspaceId: activeWorkspace.id,
        sessionId: activeWorkspace.activeSessionId,
        rootPath: activeWorkspace.rootPath,
        directoryConfirmed: activeWorkspace.directoryConfirmed,
      }));
      setWorkbenchOpen(true);
    };
    window.addEventListener('angelbot:locate-file', handleLocateFile);
    return () => window.removeEventListener('angelbot:locate-file', handleLocateFile);
  }, [workbenchAvailable, activeWorkspace]);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      const modalOpen = Boolean(document.querySelector('[aria-modal="true"]'));
      if (e.key === 'Escape' && !e.defaultPrevented && !modalOpen) {
        const menu = layout === 'A' && sidebarOpen ? visibleCompactMenu() : null;
        if (menu) {
          e.preventDefault();
          setSidebarOpen(false);
          menu.focus();
          return;
        }
        if (workbenchOpen && currentPage === 'chat') {
          e.preventDefault();
          closeWorkbench();
          focusWorkbenchToggle();
          return;
        }
      }
      if ((e.metaKey || e.ctrlKey) && e.key.toLocaleLowerCase() === 'k') {
        e.preventDefault();
        setCommandPaletteOpen((open) => !open);
        return;
      }
      if ((e.metaKey || e.ctrlKey) && e.key === ',') {
        e.preventDefault();
        setSettingsPage('personality');
        setSettingsOpen(true);
      }
      if ((e.metaKey || e.ctrlKey) && e.key === 'n') {
        e.preventDefault();
        window.dispatchEvent(new CustomEvent('new-project'));
      }
      if (!(e.metaKey || e.ctrlKey) || e.defaultPrevented || e.repeat || e.isComposing || modalOpen
        || currentPage !== 'chat' || !workbenchAvailable) return;
      if (e.key === '.') {
        e.preventDefault();
        toggleWorkbench();
        return;
      }
      if (e.altKey || e.shiftKey || (e.target instanceof HTMLElement
        && e.target.closest('input, textarea, select, [contenteditable="true"]'))) return;
      const key = e.key.toLowerCase();
      const view: WorkspaceView | null = key === 'p' ? 'files' : key === 'j' ? 'activity' : key === 't' ? 'browser' : null;
      if (!view) return;
      e.preventDefault();
      openWorkbenchView(view);
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [workbenchAvailable, workbenchOpen, currentPage, layout, sidebarOpen, closeWorkbench, focusWorkbenchToggle, toggleWorkbench, openWorkbenchView]);

  useEffect(() => {
    const handleOpenSettings = (event: Event) => {
      const { detail } = event as OpenSettingsEvent;
      if (typeof detail === 'string' && detail.trim()) {
        setSettingsPage(detail);
      }
      setSettingsOpen(true);
    };

    window.addEventListener('open-settings', handleOpenSettings);
    return () => window.removeEventListener('open-settings', handleOpenSettings);
  }, []);

  return (
    <div className={`app app-shell${currentPage === 'chat' ? ' app-shell--chat' : ''}${workbenchOpen && workbenchAvailable && currentPage === 'chat' ? ' app-shell--workbench' : ''}`}
      data-layout={layout} onDragOver={preventFileNavigation} onDrop={preventFileNavigation}>
      <header className="layout-context-header" aria-label="当前工作区">
        <span>{activeWorkspace?.name || 'AngelBot'}</span>
      </header>
      {/* 窄窗口（<960px）显示的移动端顶栏，内含侧栏菜单按钮 */}
      <div className="mobile-topbar">
        <button
          type="button"
          className="mobile-menu-btn"
          aria-label="打开菜单"
          title="展开侧栏"
          onClick={() => setSidebarOpen(true)}
        >
          <SidebarToggleIcon />
        </button>
      </div>
      <Sidebar
        dark={dark}
        onToggleTheme={toggleTheme}
        onOpenSettings={() => {
          setSettingsPage('personality');
          setSettingsOpen(true);
        }}
        onOpenQuickSwitch={() => setCommandPaletteOpen(true)}
        open={sidebarOpen}
        onClose={() => setSidebarOpen(false)}
      />
      {sidebarOpen && (
        <button type="button" className="sidebar-scrim" aria-label="关闭菜单" onClick={() => setSidebarOpen(false)} />
      )}
      {currentPage === 'chat' ? (
        <ChatArea workbenchOpen={workbenchOpen} onToggleWorkbench={workbenchAvailable ? toggleWorkbench : undefined}
          onOpenWorkbenchView={workbenchAvailable ? openWorkbenchView : undefined} />
      ) : (
        <div className="main-with-nav">
          <RuntimeHealthNotice />
          {currentPage === 'tasks' && <AutomationPage />}
          {currentPage === 'knowledge' && <LibraryPage />}
          {currentPage === 'projects' && <ProjectsPage />}
        </div>
      )}
      {workbenchOpen && workbenchAvailable && currentPage === 'chat' && (
        <RightPanel
          onClose={closeWorkbench}
          locateRequest={scopedFileLocator}
          workspaceView={workbenchRequest?.view}
          workspaceViewRevision={workbenchRequest?.revision}
        />
      )}
      <SettingsModal
        isOpen={settingsOpen}
        initialPage={settingsPage}
        onClose={() => setSettingsOpen(false)}
      />
      <CommandPalette
        isOpen={commandPaletteOpen}
        onClose={() => setCommandPaletteOpen(false)}
      />
    </div>
  );
}

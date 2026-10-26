import { useEffect, useState, type DragEvent } from 'react';
import { Sidebar } from './components/Sidebar';
import { ChatArea } from './components/ChatArea';
import { RightPanel } from './components/RightPanel';
import { AutomationPage } from './components/Automation/AutomationPage';
import { LibraryPage } from './components/Library/LibraryPage';
import { SettingsModal } from './components/Settings';
import { CommandPalette } from './components/CommandPalette';
import type { SettingsPage } from './components/Settings/types';
import { useNavigationStore } from './stores/navigation';
import { useSettingsStore } from './stores/settings';
import { useWorkspacesStore } from './stores/workspaces';
import { initializeTheme, useThemeStore } from './stores/theme';
import './styles/settings.css';

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

export default function App() {
  const apiConfigLoaded = useSettingsStore((s) => s.apiConfigLoaded);
  const loadApiConfig = useSettingsStore((s) => s.loadApiConfig);
  const currentPage = useNavigationStore((s) => s.currentPage);
  const workspaces = useWorkspacesStore((s) => s.workspaces);
  const activeWorkspaceId = useWorkspacesStore((s) => s.activeWorkspaceId);
  const dark = useThemeStore((s) => s.resolvedTheme === 'dark');
  const toggleTheme = useThemeStore((s) => s.toggleTheme);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsPage, setSettingsPage] = useState<SettingsPage>('personality');
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [workbenchOpen, setWorkbenchOpen] = useState(false);
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
      if ((e.metaKey || e.ctrlKey) && e.key === '.') {
        e.preventDefault();
        if (workbenchAvailable) setWorkbenchOpen((open) => !open);
      }
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [workbenchAvailable]);

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
    <div className={`app app-shell${workbenchOpen ? ' app-shell--workbench' : ''}`}
      onDragOver={preventFileNavigation} onDrop={preventFileNavigation}>
      {/* 窄窗口（<960px）显示的移动端顶栏，内含侧栏菜单按钮 */}
      <div className="mobile-topbar">
        <button
          type="button"
          className="mobile-menu-btn"
          aria-label="打开菜单"
          onClick={() => setSidebarOpen(true)}
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
            <line x1="3" y1="6" x2="21" y2="6" /><line x1="3" y1="12" x2="21" y2="12" /><line x1="3" y1="18" x2="21" y2="18" />
          </svg>
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
        <ChatArea onToggleWorkbench={workbenchAvailable
          ? () => setWorkbenchOpen((open) => !open)
          : undefined} />
      ) : (
        <main className="main-with-nav">
          {currentPage === 'tasks' && <AutomationPage />}
          {currentPage === 'knowledge' && <LibraryPage />}
        </main>
      )}
      {workbenchOpen && workbenchAvailable && (
        <RightPanel
          onClose={() => setWorkbenchOpen(false)}
          locateRequest={scopedFileLocator}
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

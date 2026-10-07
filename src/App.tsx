import { useEffect, useState } from 'react';
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
import { runDueAutomations } from './lib/commands/automation';
import { isTauriRuntime } from './lib/invoke';
import { AUTOMATIONS_UPDATED_EVENT } from './components/WorkspaceReminders';
import './styles/settings.css';

interface NotificationEventPayload {
  title?: unknown;
  body?: unknown;
}

interface OpenSettingsEvent extends Event {
  detail?: SettingsPage;
}

interface LocateFileEvent extends Event {
  detail?: { path?: unknown };
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
  const [fileLocator, setFileLocator] = useState<{ path: string; revision: number } | null>(null);
  const activeWorkspace = workspaces.find((workspace) => workspace.id === activeWorkspaceId);

  // A locator is scoped to the workspace in which it was emitted.  Clearing it
  // prevents a delayed render from attempting the same relative path in a
  // different project's worktree.
  useEffect(() => {
    setFileLocator(null);
  }, [activeWorkspaceId]);

  useEffect(() => {
    if (!apiConfigLoaded) {
      void loadApiConfig();
    }
  }, [apiConfigLoaded, loadApiConfig]);

  useEffect(() => initializeTheme(), []);

  useEffect(() => {
    const handleLocateFile = (event: Event) => {
      if (activeWorkspace?.kind !== 'project') return;
      const path = (event as LocateFileEvent).detail?.path;
      if (typeof path !== 'string' || !path.trim()) return;
      setFileLocator((current) => ({ path: path.trim(), revision: (current?.revision ?? 0) + 1 }));
      setWorkbenchOpen(true);
    };
    window.addEventListener('angelbot:locate-file', handleLocateFile);
    return () => window.removeEventListener('angelbot:locate-file', handleLocateFile);
  }, [activeWorkspace?.kind]);

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
        if (activeWorkspace?.kind === 'project') setWorkbenchOpen((open) => !open);
      }
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [activeWorkspace?.kind]);

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

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let heartbeat: number | undefined;
    let cancelled = false;

    const disposeListener = () => {
      const listener = unlisten;
      unlisten = undefined;
      if (!listener) return;
      try {
        // Tauri's runtime cleanup may return a Promise despite the void type.
        void Promise.resolve(listener()).catch(() => undefined);
      } catch {
        // Listener teardown must not prevent the heartbeat from being cleared.
      }
    };

    async function setupDesktopRuntime() {
      if (isTauriRuntime()) {
        try {
          const [{ listen }, { sendNotification }] = await Promise.all([
            import('@tauri-apps/api/event'),
            import('@tauri-apps/plugin-notification'),
          ]);

          if (cancelled) return;

          unlisten = await listen('notification-requested', async (event) => {
            if (cancelled) return;
            const payload = event.payload as NotificationEventPayload;
            if (
              typeof payload?.title !== 'string' ||
              typeof payload?.body !== 'string' ||
              !payload.title.trim() ||
              !payload.body.trim()
            ) {
              return;
            }

            try {
              await sendNotification({
                title: payload.title,
                body: payload.body,
              });
            } catch {
              // A notification attempt is not proof of delivery. A denied or
              // unavailable notification must not interrupt ordinary scheduling.
            }
          });
        } catch {
          // Keep other scheduled work alive even if desktop notifications are unavailable.
        }
      }

      if (cancelled) {
        disposeListener();
        return;
      }

      // Register the notification listener before the first due-work scan so
      // reminders recovered during startup cannot be emitted into a race.
      const runDue = async () => {
        if (cancelled) return;
        try {
          await runDueAutomations();
          // Refresh durable reminder/run state even when notifications are off.
          // This signals a completed scan, not operating-system delivery.
          if (!cancelled) window.dispatchEvent(new Event(AUTOMATIONS_UPDATED_EVENT));
        } catch {
          // Failed scans are not completion. The next heartbeat can retry.
        }
      };
      void runDue();
      heartbeat = window.setInterval(runDue, 60_000);
    }

    setupDesktopRuntime();

    return () => {
      cancelled = true;
      disposeListener();
      if (heartbeat !== undefined) window.clearInterval(heartbeat);
    };
  }, []);

  return (
    <div className={`app app-shell${workbenchOpen ? ' app-shell--workbench' : ''}`}>
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
        <ChatArea onToggleWorkbench={activeWorkspace?.kind === 'project'
          ? () => setWorkbenchOpen((open) => !open)
          : undefined} />
      ) : (
        <main className="main-with-nav">
          {currentPage === 'tasks' && <AutomationPage />}
          {currentPage === 'knowledge' && <LibraryPage />}
        </main>
      )}
      {workbenchOpen && activeWorkspace?.kind === 'project' && (
        <RightPanel
          onClose={() => setWorkbenchOpen(false)}
          locateRequest={fileLocator}
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

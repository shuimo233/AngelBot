import { useEffect, useMemo, useRef, useState } from 'react';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspacesStore } from '$stores/workspaces';
import './CommandPalette.css';

type CommandCategory = 'workspace' | 'navigate' | 'action' | 'settings';

interface Command {
  id: string;
  label: string;
  description: string;
  category: CommandCategory;
  action: () => void | Promise<void>;
}

interface CommandPaletteProps {
  isOpen: boolean;
  onClose: () => void;
}

const CATEGORY_ORDER: CommandCategory[] = ['workspace', 'navigate', 'action', 'settings'];
const CATEGORY_LABEL: Record<CommandCategory, string> = {
  workspace: '工作区',
  navigate: '功能',
  action: '操作',
  settings: '设置',
};

const SearchIcon = () => (
  <svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
    <circle cx="11" cy="11" r="8" />
    <line x1="21" y1="21" x2="16.65" y2="16.65" />
  </svg>
);

function normalized(value: string): string {
  return value.trim().toLocaleLowerCase();
}

export function CommandPalette({ isOpen, onClose }: CommandPaletteProps) {
  const [query, setQuery] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [runningCommandId, setRunningCommandId] = useState<string | null>(null);
  const [actionError, setActionError] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);
  const workspaces = useWorkspacesStore((state) => state.workspaces);
  const activeWorkspaceId = useWorkspacesStore((state) => state.activeWorkspaceId);
  const openWorkspace = useWorkspacesStore((state) => state.openWorkspace);
  const setPage = useNavigationStore((state) => state.setPage);

  const commands: Command[] = useMemo(() => [
    ...workspaces.map((workspace) => ({
      id: `workspace-${workspace.id}`,
      label: workspace.kind === 'personal' ? 'AngelBot 日常' : workspace.name,
      description: workspace.kind === 'personal'
        ? '个人事务、提醒与跨项目入口'
        : workspace.rootPath ?? '项目主对话',
      category: 'workspace' as const,
      action: async () => {
        if (workspace.id !== activeWorkspaceId) await openWorkspace(workspace.id);
        setPage('chat');
      },
    })),
    {
      id: 'navigate-automations',
      label: '自动化',
      description: '查看提醒、定时任务与运行记录',
      category: 'navigate',
      action: () => setPage('tasks'),
    },
    {
      id: 'navigate-knowledge',
      label: '资料库',
      description: '管理 AngelBot 可引用的个人资料',
      category: 'navigate',
      action: () => setPage('knowledge'),
    },
    {
      id: 'action-new-project',
      label: '打开项目文件夹',
      description: '从本机文件夹创建或重新打开项目',
      category: 'action',
      action: () => { window.dispatchEvent(new CustomEvent('new-project')); },
    },
    {
      id: 'settings-api',
      label: '模型与 API',
      description: '配置模型服务、凭据和生成参数',
      category: 'settings',
      action: () => { window.dispatchEvent(new CustomEvent('open-settings', { detail: 'api' })); },
    },
    {
      id: 'settings-desktop',
      label: '电脑操作',
      description: '管理 Windows 应用和桌面操作能力',
      category: 'settings',
      action: () => { window.dispatchEvent(new CustomEvent('open-settings', { detail: 'desktop-control' })); },
    },
    {
      id: 'settings-mcp',
      label: 'MCP 服务',
      description: '管理外部工具及工作区准入范围',
      category: 'settings',
      action: () => { window.dispatchEvent(new CustomEvent('open-settings', { detail: 'mcp' })); },
    },
  ], [activeWorkspaceId, openWorkspace, setPage, workspaces]);

  const filteredCommands = useMemo(() => {
    const search = normalized(query);
    if (!search) return commands;
    return commands.filter((command) => normalized(`${command.label} ${command.description}`).includes(search));
  }, [commands, query]);

  const groupedCommands = useMemo(() => CATEGORY_ORDER.map((category) => ({
    category,
    commands: filteredCommands.filter((command) => command.category === category),
  })).filter((group) => group.commands.length > 0), [filteredCommands]);

  useEffect(() => {
    if (!isOpen) return undefined;
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setQuery('');
    setSelectedIndex(0);
    setActionError('');
    const timer = window.setTimeout(() => inputRef.current?.focus(), 0);
    return () => {
      window.clearTimeout(timer);
      previousFocus?.focus();
    };
  }, [isOpen]);

  useEffect(() => {
    if (selectedIndex >= filteredCommands.length) setSelectedIndex(Math.max(filteredCommands.length - 1, 0));
  }, [filteredCommands.length, selectedIndex]);

  const execute = async (command: Command) => {
    if (runningCommandId) return;
    setRunningCommandId(command.id);
    setActionError('');
    try {
      await command.action();
      onClose();
    } catch (reason) {
      setActionError(reason instanceof Error ? reason.message : `无法执行“${command.label}”`);
    } finally {
      setRunningCommandId(null);
    }
  };

  const handleKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      onClose();
      return;
    }
    if (filteredCommands.length === 0) return;
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      setSelectedIndex((index) => (index + 1) % filteredCommands.length);
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      setSelectedIndex((index) => (index - 1 + filteredCommands.length) % filteredCommands.length);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      void execute(filteredCommands[selectedIndex]);
    }
  };

  if (!isOpen) return null;

  let optionIndex = -1;
  return (
    <div className="command-palette-overlay" onMouseDown={(event) => {
      if (event.target === event.currentTarget) onClose();
    }}>
      <section className="command-palette" role="dialog" aria-modal="true" aria-labelledby="command-palette-title">
        <div className="command-palette-input-wrapper">
          <SearchIcon />
          <label id="command-palette-title" className="sr-only" htmlFor="command-palette-input">快速切换</label>
          <input
            ref={inputRef}
            id="command-palette-input"
            type="text"
            className="command-palette-input"
            placeholder="搜索项目、功能或设置"
            autoComplete="off"
            aria-controls="command-palette-results"
            aria-activedescendant={filteredCommands[selectedIndex] ? `command-${filteredCommands[selectedIndex].id}` : undefined}
            value={query}
            onChange={(event) => { setQuery(event.target.value); setSelectedIndex(0); setActionError(''); }}
            onKeyDown={handleKeyDown}
          />
          <kbd className="command-palette-hint">Esc</kbd>
        </div>

        {actionError && <div className="command-palette-error" role="alert">{actionError}<span>请重试，或从左侧工作区列表打开。</span></div>}

        <div id="command-palette-results" className="command-palette-results" role="listbox" aria-label="快速切换结果">
          {filteredCommands.length === 0 ? (
            <div className="command-palette-empty">
              <strong>没有找到“{query.trim()}”</strong>
              <span>试试项目名称、“自动化”、“模型”或“MCP”。</span>
            </div>
          ) : groupedCommands.map((group) => (
            <div key={group.category} className="command-palette-group" role="group" aria-label={CATEGORY_LABEL[group.category]}>
              <div className="command-palette-group-label">{CATEGORY_LABEL[group.category]}</div>
              {group.commands.map((command) => {
                optionIndex += 1;
                const currentIndex = optionIndex;
                const running = runningCommandId === command.id;
                return (
                  <button
                    key={command.id}
                    id={`command-${command.id}`}
                    type="button"
                    className={`command-item${currentIndex === selectedIndex ? ' selected' : ''}`}
                    disabled={Boolean(runningCommandId)}
                    onClick={() => void execute(command)}
                    onMouseEnter={() => setSelectedIndex(currentIndex)}
                    role="option"
                    aria-selected={currentIndex === selectedIndex}
                  >
                    <span className="command-item-label">{running ? '正在打开…' : command.label}</span>
                    <span className="command-item-description">{command.description}</span>
                    {command.id === `workspace-${activeWorkspaceId}` && <span className="command-item-state">当前</span>}
                  </button>
                );
              })}
            </div>
          ))}
        </div>

        <footer className="command-palette-footer">
          <span><kbd>↑↓</kbd> 选择</span>
          <span><kbd>Enter</kbd> 打开</span>
          <span className="command-palette-footer-note">Ctrl+K 随时呼出</span>
        </footer>
      </section>
    </div>
  );
}

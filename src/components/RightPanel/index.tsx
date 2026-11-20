import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { useSessionsStore } from '$stores/sessions';
import { useWorkspacesStore } from '$stores/workspaces';
import { listWorkDir, openDailyResultsFolder, readSessionFile, saveSessionFile, type WorkDirEntry } from '$lib/commands/file';
import { WorkspaceActivityCapsule } from '../WorkspaceActivityCapsule';
import { WorkspaceReminders } from '../WorkspaceReminders';
import { MessageMarkdown } from '../MessageMarkdown';
import { openProjectFolder } from '$lib/project-folder';
import { IconButton, InlineNotice, Input } from '../../ui';
import { WorkbenchFrame, useCompactWorkbench } from './WorkbenchFrame';
import { SourcePreview } from './SourcePreview';
import { FileTreePage, matchingLoadedFiles, pathForChildLevel } from './FileTreeBrowse';
import './RightPanel.css';

export type WorkspaceView = 'activity' | 'browser' | 'files';
type FileReveal = { scope: string; path: string; revision: number };
type FileOperationNotice = { message: string; tone: 'success' | 'danger' };

const PREVIEW_LIMIT = 500_000;
function filePreview(scope: string, path: string, content: string) {
  return { scope, path, content: content.slice(0, PREVIEW_LIMIT), truncated: content.length > PREVIEW_LIMIT, showSource: false };
}

const BrowserIcon = () => <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/></svg>;
const FileIcon = () => <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>;
const OpenFolderIcon = () => <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3 18V6a2 2 0 0 1 2-2h5l2 2h6a2 2 0 0 1 2 2v2M3 18l3-7h16l-3 7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>;
const ActivityIcon = () => <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 4h14v16H5z"/><path d="M8 8h8M8 12h5M8 16h7"/></svg>;
const TreeCaret = () => <svg className="file-tree-caret" viewBox="0 0 12 12" aria-hidden="true"><path d="m4.5 2.5 3.5 3.5-3.5 3.5"/></svg>;
const SaveAsIcon = () => <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 2v8m-3-3 3 3 3-3M3 10v3h10v-3"/></svg>;
const RefreshIcon = () => <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M20 5v5h-5M4.8 10a7.5 7.5 0 0 1 12.8-3.8L20 10m0 4a7.5 7.5 0 0 1-12.8 3.8L4 14m0 5v-5h5"/></svg>;

const fileGlyphs = {
  code: 'm6 10-2 2 2 2m4-4 2 2-2 2',
  config: 'M5 10H4v4h1m6-4h1v4h-1M8 10v4',
  markdown: 'M4 14v-4l2 2 2-2v4m3-4v4m-1.5-1.5L11 14l1.5-1.5',
  image: 'm4 13 3-3 2 2 1-1 2 2m-2-4h.1',
  document: 'M4.5 9.5h7m-7 3h7',
};
const fileKinds: Record<string, keyof typeof fileGlyphs> = {
  ts: 'code', tsx: 'code', js: 'code', jsx: 'code', py: 'code', rs: 'code', go: 'code',
  html: 'code', css: 'code', sh: 'code', ps1: 'code', bat: 'code', sql: 'code',
  json: 'config', yaml: 'config', yml: 'config', toml: 'config', xml: 'config',
  md: 'markdown', markdown: 'markdown', png: 'image', jpg: 'image', jpeg: 'image', svg: 'image', webp: 'image', gif: 'image',
};

function FileTypeIcon({ entry, expanded }: { entry: WorkDirEntry; expanded: boolean }) {
  if (entry.isDirectory) return <span className="file-type-icon folder">{expanded ? <OpenFolderIcon /> : <FileIcon />}</span>;
  const extension = /\.([a-z\d]+)$/i.exec(entry.name)?.[1].toLowerCase() ?? '';
  const kind = Object.prototype.hasOwnProperty.call(fileKinds, extension) ? fileKinds[extension] : 'document';
  return <span className={`file-type-icon ${kind}`}><svg viewBox="0 0 16 16" aria-hidden="true">
    <path d="M3 1.5h6L13 5v9.5H3zm6 0V5h4"/>
    <path d={fileGlyphs[kind]}/>
  </svg></span>;
}

function FileTreeNode({ entry, depth = 0, sessionId, onOpenFile, onSaveFile, savingFile, onError,
  selectedPath, reveal, treeVisible, onDirectoryLoaded, showParentPath = false }: {
  entry: WorkDirEntry;
  depth?: number;
  sessionId: string;
  onOpenFile: (entry: WorkDirEntry, trigger: HTMLButtonElement) => void;
  onSaveFile: (path: string) => void;
  savingFile: boolean;
  onError: (message: string) => void;
  selectedPath?: string;
  reveal: FileReveal | null;
  treeVisible: boolean;
  onDirectoryLoaded?: (path: string, entries: WorkDirEntry[]) => void;
  showParentPath?: boolean;
}) {
  const shouldReveal = entry.isDirectory && Boolean(reveal?.path.startsWith(`${entry.path}/`));
  const revealRevision = shouldReveal ? reveal?.revision : undefined;
  const selected = !entry.isDirectory && entry.path === selectedPath;
  const selectedRevision = reveal?.path === entry.path ? reveal.revision : undefined;
  const button = useRef<HTMLButtonElement>(null);
  const [expanded, setExpanded] = useState(shouldReveal);
  const [children, setChildren] = useState<WorkDirEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const parentPath = showParentPath ? entry.path.slice(0, Math.max(0, entry.path.lastIndexOf('/'))) : '';

  useEffect(() => {
    if (revealRevision !== undefined) setExpanded(true);
  }, [revealRevision]);

  useEffect(() => {
    if (selected && treeVisible) button.current?.scrollIntoView?.({ block: 'nearest' });
  }, [selected, selectedRevision, treeVisible]);

  useEffect(() => {
    if (!entry.isDirectory || !expanded) return;
    let cancelled = false;
    setLoading(true);
    listWorkDir(sessionId, entry.path)
      .then((items) => {
        if (cancelled) return;
        setChildren(items);
        onDirectoryLoaded?.(entry.path, items);
      })
      .catch((error) => { if (!cancelled) onError(error instanceof Error ? error.message : String(error)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [entry.isDirectory, entry.path, expanded, onDirectoryLoaded, onError, sessionId]);

  return (
    <li className="file-tree-node" style={{ '--file-depth': depth } as CSSProperties}>
      <div className={`file-tree-row${selected ? ' selected' : ''}`}>
        <button type="button" className="file-item" ref={button}
          aria-label={showParentPath ? undefined : `${entry.isDirectory ? (expanded ? '收起' : '展开') : '打开'} ${entry.name}`}
          onClick={(event) => entry.isDirectory ? setExpanded((value) => !value) : onOpenFile(entry, event.currentTarget)}
          aria-expanded={entry.isDirectory ? expanded : undefined}
          aria-current={selected ? 'true' : undefined}
          title={entry.temporary ? `${entry.path} · 浏览器临时文件，不随备份；需要保留请另存或让 AngelBot 复制到结果区。` : entry.path}>
          {entry.isDirectory ? <TreeCaret /> : <span className="file-tree-caret" aria-hidden="true" />}
          <FileTypeIcon entry={entry} expanded={expanded} />
          <span className="file-search-label">
            <span className="file-name">{entry.temporary && entry.isDirectory && !entry.path.includes('/') ? '浏览器临时文件（不随备份）' : entry.name}</span>
            {parentPath && <small>{parentPath}</small>}
          </span>
        </button>
        {!entry.isDirectory && <button type="button" className="file-save-as" disabled={savingFile} aria-label={`另存 ${entry.name}`}
          title={`另存 ${entry.name}`} onClick={() => onSaveFile(entry.path)}><SaveAsIcon /></button>}
      </div>
      {expanded && (
        <ul className="file-tree-children">
          {loading ? <li className="context-loading">加载中…</li> : <FileTreePage entries={children}
            requiredEntryPath={pathForChildLevel(selectedPath, entry.path)} renderEntry={(child) => (
            <FileTreeNode entry={child} depth={depth + 1} sessionId={sessionId} onOpenFile={onOpenFile} onSaveFile={onSaveFile} savingFile={savingFile} onError={onError}
              selectedPath={selectedPath} reveal={reveal} treeVisible={treeVisible} onDirectoryLoaded={onDirectoryLoaded} />
          )} />}
        </ul>
      )}
    </li>
  );
}

export function RightPanel({
  onClose,
  workspaceView = null,
  workspaceViewRevision,
  locateRequest = null,
}: {
  onClose?: () => void;
  workspaceView?: WorkspaceView | null;
  workspaceViewRevision?: number;
  locateRequest?: { path: string; revision: number; sessionId: string } | null;
}) {
  const session = useSessionsStore((state) => state.activeSession);
  const workspaces = useWorkspacesStore((state) => state.workspaces);
  const activeWorkspaceId = useWorkspacesStore((state) => state.activeWorkspaceId);
  const compact = useCompactWorkbench();
  const workspace = workspaces.find((item) => item.id === activeWorkspaceId);
  const fileScope = JSON.stringify([session?.id, workspace?.rootPath, workspace?.directoryConfirmed]);
  const [view, setView] = useState<WorkspaceView>(workspaceView ?? 'activity');
  const [rootSnapshot, setRootSnapshot] = useState<{ scope: string; entries: WorkDirEntry[] } | null>(null);
  const rootEntries = rootSnapshot?.scope === fileScope ? rootSnapshot.entries : null;
  const [rootExpanded, setRootExpanded] = useState(true);
  const folderTree = useRef<HTMLDivElement>(null);
  const panelHeader = useRef<HTMLDivElement>(null);
  const filePreviewHeading = useRef<HTMLHeadingElement>(null);
  const openedFrom = useRef<HTMLButtonElement | null>(null);
  const returnToTree = useRef(false);
  const [openingFolder, setOpeningFolder] = useState(false);
  const folderRequest = useRef(false);
  const rootLabel = workspace?.kind === 'personal' ? '日常结果'
    : workspace?.rootPath?.split(/[\\/]/).filter(Boolean).pop() || workspace?.name || '项目目录';
  const [loadingFiles, setLoadingFiles] = useState(false);
  const [fileRevision, setFileRevision] = useState(0);
  const [fileFilter, setFileFilter] = useState('');
  const [loadedDirectories, setLoadedDirectories] = useState(() => new Map<string, WorkDirEntry[]>());
  const onDirectoryLoaded = useCallback((path: string, entries: WorkDirEntry[]) => {
    setLoadedDirectories((previous) => new Map(previous).set(path, entries));
  }, []);
  const filtering = Boolean(fileFilter.trim());
  const fileMatches = useMemo(() => rootEntries ? matchingLoadedFiles(rootEntries, loadedDirectories, fileFilter) : [],
    [rootEntries, loadedDirectories, fileFilter]);
  const [loadingPreview, setLoadingPreview] = useState(false);
  const [fileError, setFileError] = useState<string | null>(null);
  const [previewSnapshot, setPreview] = useState<ReturnType<typeof filePreview> | null>(null);
  const preview = previewSnapshot?.scope === fileScope ? previewSnapshot : null;
  const [revealedFile, setRevealedFile] = useState<FileReveal | null>(null);
  const reveal = revealedFile?.scope === fileScope ? revealedFile : null;
  const [treeSelection, setTreeSelection] = useState<{ scope: string; path: string } | null>(null);
  const selectedPath = treeSelection?.scope === fileScope ? treeSelection.path : reveal?.path;
  const treeVisible = !compact || !preview;
  const isMarkdownPreview = preview && /\.(md|markdown)$/i.test(preview.path);
  const [savingFile, setSavingFile] = useState(false);
  const [saveNotice, setSaveNotice] = useState<(FileOperationNotice & { fromPreview: boolean }) | null>(null);
  const [copyNotice, setCopyNotice] = useState<FileOperationNotice | null>(null);
  const previewRequest = useRef(0);

  useEffect(() => {
    if (compact && view === 'files' && preview?.path) {
      filePreviewHeading.current?.focus({ preventScroll: true });
    }
  }, [compact, preview?.path, view]);

  useEffect(() => {
    if (!compact || preview || !returnToTree.current) return;
    returnToTree.current = false;
    const trigger = openedFrom.current;
    if (trigger?.isConnected) trigger.focus({ preventScroll: true });
    else {
      const tree = folderTree.current;
      const target = tree?.querySelector<HTMLButtonElement>('.workbench-tree-content:not([hidden]) [aria-current="true"]')
        ?? tree?.querySelector<HTMLButtonElement>('.workbench-root-folder');
      target?.focus({ preventScroll: true });
    }
  }, [compact, preview]);

  useEffect(() => {
    if (compact && !panelHeader.current?.closest('aside')?.contains(document.activeElement)) {
      panelHeader.current?.querySelector<HTMLButtonElement>('button')?.focus();
    }
  }, [compact]);

  useEffect(() => {
    previewRequest.current += 1;
    openedFrom.current = null;
    returnToTree.current = false;
    setPreview(null);
    setRevealedFile(null);
    setTreeSelection(null);
    setLoadingPreview(false);
    setSaveNotice(null);
    setCopyNotice(null);
    setRootExpanded(true);
    setFileFilter('');
    setLoadedDirectories(new Map());
  }, [fileScope]);
  const [browserUrl, setBrowserUrl] = useState('');
  const [browserPage, setBrowserPage] = useState<string | null>(null);
  const [browserMessage, setBrowserMessage] = useState<string | null>(null);
  const [browserHistory, setBrowserHistory] = useState<string[]>([]);
  const [browserIndex, setBrowserIndex] = useState(-1);
  const [browserRevision, setBrowserRevision] = useState(0);

  useEffect(() => {
    if (workspaceView) setView(workspaceView);
  }, [workspaceView, workspaceViewRevision]);

  useEffect(() => {
    if (!locateRequest || !session || locateRequest.sessionId !== session.id) return;
    const revision = ++previewRequest.current;
    let cancelled = false;
    openedFrom.current = null;
    setCopyNotice(null);
    setSaveNotice(null);
    setView('files');
    setLoadingPreview(true);
    setFileError(null);
    setPreview(null);
    setTreeSelection(null);
    setRevealedFile({ scope: fileScope, path: locateRequest.path, revision: locateRequest.revision });
    setRootExpanded(true);
    setFileFilter('');
    readSessionFile(session.id, locateRequest.path)
      .then((result) => {
        if (cancelled || previewRequest.current !== revision) return;
        if (result.success) {
          setPreview(filePreview(fileScope, locateRequest.path, result.content ?? ''));
        } else {
          setFileError(result.error ?? '无法定位该项目文件');
        }
      })
      .catch((error) => {
        if (!cancelled && previewRequest.current === revision) setFileError(error instanceof Error ? error.message : String(error));
      })
      .finally(() => { if (!cancelled && previewRequest.current === revision) setLoadingPreview(false); });
    return () => { cancelled = true; };
  }, [locateRequest?.path, locateRequest?.revision, locateRequest?.sessionId, fileScope]);

  useEffect(() => {
    if (view !== 'files' || !session) return;
    let cancelled = false;
    setLoadingFiles(true);
    setRootSnapshot(null);
    setLoadedDirectories(new Map());
    setFileError(null);
    listWorkDir(session.id)
      .then((items) => { if (!cancelled) setRootSnapshot({ scope: fileScope, entries: items }); })
      .catch((error) => {
        if (cancelled) return;
        previewRequest.current += 1;
        setPreview(null);
        setTreeSelection(null);
        setLoadingPreview(false);
        setFileError(error instanceof Error ? error.message : String(error));
      })
      .finally(() => { if (!cancelled) setLoadingFiles(false); });
    return () => { cancelled = true; };
  }, [fileScope, view, fileRevision]);

  const openFolder = async () => {
    if (folderRequest.current) return;
    folderRequest.current = true;
    setOpeningFolder(true);
    setFileError(null);
    try {
      if (workspace?.kind === 'personal') await openDailyResultsFolder();
      else await openProjectFolder();
    } catch (error) {
      setFileError(error instanceof Error ? error.message : typeof error === 'string' ? error : '无法打开文件夹，请重试。');
    } finally {
      folderRequest.current = false;
      setOpeningFolder(false);
    }
  };

  const returnToConversation = (focusSelector = 'button[aria-controls="conversation-workbench"]') => {
    if (onClose) {
      onClose();
      window.requestAnimationFrame(() => document.querySelector<HTMLElement>(focusSelector)?.focus());
    }
  };

  const openEntry = async (entry: WorkDirEntry, trigger: HTMLButtonElement) => {
    if (!session) return;
    const revision = ++previewRequest.current;
    openedFrom.current = trigger;
    setCopyNotice(null);
    setSaveNotice(null);
    setLoadingPreview(true);
    setFileError(null);
    setTreeSelection({ scope: fileScope, path: entry.path });
    setPreview(null);
    setRevealedFile(null);
    try {
      const result = await readSessionFile(session.id, entry.path);
      if (previewRequest.current !== revision || useSessionsStore.getState().activeSessionId !== session.id) return;
      if (result.success) {
        setPreview(filePreview(fileScope, entry.path, result.content ?? ''));
      } else {
        setFileError(result.error ?? '无法读取该文件');
      }
    } catch (error) {
      if (previewRequest.current === revision && useSessionsStore.getState().activeSessionId === session.id) setFileError(error instanceof Error ? error.message : String(error));
    } finally {
      if (previewRequest.current === revision && useSessionsStore.getState().activeSessionId === session.id) setLoadingPreview(false);
    }
  };

  const referencePreviewedFile = () => {
    if (!preview) return;
    window.dispatchEvent(new CustomEvent('angelbot:reference-file', {
      detail: workspace ? { path: preview.path, workspaceId: workspace.id } : { path: preview.path },
    }));
    if (compact && onClose) returnToConversation('.composer-textarea');
  };

  const closePreview = () => {
    previewRequest.current += 1;
    returnToTree.current = compact;
    setCopyNotice(null);
    setPreview(null);
  };

  const copyPreview = async () => {
    if (!preview) return;
    const revision = previewRequest.current;
    const showNotice = (message: string, tone: FileOperationNotice['tone']) => {
      if (previewRequest.current === revision) setCopyNotice({ message, tone });
    };
    try {
      await navigator.clipboard.writeText(preview.content);
      showNotice(preview.truncated ? '已复制预览片段' : '已复制原文', 'success');
    } catch {
      showNotice('无法复制，可使用另存为。', 'danger');
    }
  };

  const saveFile = async (path: string, fromPreview = false) => {
    if (!session || savingFile) return;
    const sessionId = session.id;
    setSavingFile(true);
    setSaveNotice(null);
    try {
      const result = await saveSessionFile(sessionId, path);
      if (useSessionsStore.getState().activeSessionId === sessionId && result.status === 'saved') {
        setSaveNotice({ message: `已另存 ${result.name}（${result.bytes} 字节），原文件保留。`, tone: 'success', fromPreview });
      }
    } catch (error) {
      if (useSessionsStore.getState().activeSessionId === sessionId) {
        setSaveNotice({ message: error instanceof Error ? error.message : String(error), tone: 'danger', fromPreview });
      }
    } finally {
      setSavingFile(false);
    }
  };

  const popOutBrowser = () => {
    if (!browserPage) return;
    // An isolated popup does not expose a handle even when it opens successfully.
    window.open(browserPage, '_blank', 'noopener,noreferrer');
  };

  const fileFeedback = <>
    {fileError && <InlineNotice tone="danger">{fileError}</InlineNotice>}
    {loadingPreview && <InlineNotice>正在读取文件…</InlineNotice>}
    {saveNotice && (!saveNotice.fromPreview || !preview) && <InlineNotice tone={saveNotice.tone}>{saveNotice.message}</InlineNotice>}
  </>;

  // The panel itself is the workbench. It augments the active Main-Agent
  // session instead of switching into a second conversation surface.
  return (
    <WorkbenchFrame compact={compact}>
      <div className="right-panel-header" ref={panelHeader}>
        {compact && view === 'files' && preview ? (
          <button type="button" className="right-panel-back" onClick={closePreview}>← 返回文件列表</button>
        ) : (
          <div className="workbench-heading">
            <h2 className="right-panel-title">工作台</h2>
            {workspace && <span className="workbench-context" title={workspace.name}>{workspace.name}</span>}
          </div>
        )}
        {onClose && (
          <button type="button" className="icon-btn" onClick={() => returnToConversation()} title="关闭 (Esc 或 Ctrl+.)" aria-label="关闭工作侧栏">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
          </button>
        )}
      </div>

      <nav className="workbench-tabs" aria-label="工作台视图">
        <button type="button" aria-pressed={view === 'activity'} aria-keyshortcuts="Control+J" title="任务 (Ctrl+J)" onClick={() => setView('activity')}><ActivityIcon /><span>任务</span></button>
        <button type="button" aria-pressed={view === 'files'} aria-keyshortcuts="Control+P" title="文件 (Ctrl+P)" onClick={() => setView('files')}><FileIcon /><span>{workspace?.kind === 'personal' ? '结果文件' : '文件'}</span></button>
        <button type="button" aria-pressed={view === 'browser'} aria-keyshortcuts="Control+T" title="浏览器 (Ctrl+T)" onClick={() => setView('browser')}><BrowserIcon /><span>浏览器</span></button>
      </nav>

      <div className={`right-panel-content${view === 'files' ? ' right-panel-content--files' : view === 'browser' ? ' right-panel-content--browser' : ''}`}>
        {view === 'activity' && (
          <>
            <WorkspaceActivityCapsule mode="panel" onReturnToConversation={onClose ? () => returnToConversation() : undefined}
              onReturnToResultReview={(messageId) => {
                onClose?.();
                window.dispatchEvent(new CustomEvent('angelbot:review-result-in-chat', { detail: messageId }));
              }} />
            <WorkspaceReminders workspaceId={workspace?.id} />
          </>
        )}

        {view === 'browser' && (
          <section className="workbench-browser" aria-labelledby="workbench-browser-title">
            <div className="workbench-toolbar">
              <h2 id="workbench-browser-title" className="workbench-section-title">浏览器</h2>
              <button type="button" className="workbench-mode-button" disabled={!browserPage} onClick={popOutBrowser}>弹出窗口</button>
            </div>
            <form className="browser-address-form" onSubmit={(event) => {
              event.preventDefault();
              const candidate = browserUrl.trim();
              if (!candidate) return setBrowserMessage('请输入网址。');
              const url = /^[a-z][a-z\d+.-]*:/i.test(candidate) ? candidate : `https://${candidate}`;
              try {
                const parsed = new URL(url);
                if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') throw new Error('仅支持 http:// 或 https:// 网站。');
                const nextUrl = parsed.toString();
                setBrowserHistory((history) => {
                  const next = [...history.slice(0, browserIndex + 1), nextUrl];
                  setBrowserIndex(next.length - 1);
                  return next;
                });
                setBrowserUrl(nextUrl);
                setBrowserPage(nextUrl);
                setBrowserMessage(null);
              } catch (error) {
                setBrowserMessage(error instanceof Error ? error.message : String(error));
              }
            }}>
              <label htmlFor="browser-address">网址</label>
              <div className="browser-address-row">
                <div className="browser-navigation">
                  <button type="button" className="browser-nav-button" aria-label="后退" disabled={browserIndex <= 0} onClick={() => { const index = browserIndex - 1; const page = browserHistory[index]; setBrowserIndex(index); setBrowserUrl(page); setBrowserPage(page); }}>←</button>
                  <button type="button" className="browser-nav-button" aria-label="前进" disabled={browserIndex >= browserHistory.length - 1} onClick={() => { const index = browserIndex + 1; const page = browserHistory[index]; setBrowserIndex(index); setBrowserUrl(page); setBrowserPage(page); }}>→</button>
                  <button type="button" className="browser-nav-button" aria-label="刷新" disabled={!browserPage} onClick={() => setBrowserRevision((value) => value + 1)}>↻</button>
                </div>
                <div className="browser-address-entry">
                  <input id="browser-address" value={browserUrl} onChange={(event) => setBrowserUrl(event.target.value)} placeholder="输入网址" autoComplete="url" aria-label="网址" />
                  <button className="browser-go-button" type="submit" aria-label="打开网址">打开</button>
                </div>
              </div>
            </form>
            {browserMessage && <InlineNotice tone="warning">{browserMessage}</InlineNotice>}
            {browserPage && <iframe key={browserRevision} className="workbench-browser-frame" title="AngelBot 浏览器" src={browserPage} />}
          </section>
        )}

        {view === 'files' && (
          <section className="workbench-files workbench-files--workspace" aria-labelledby={compact && preview ? 'workbench-preview-title' : 'workbench-files-title'}>
            <h2 id="workbench-files-title" className="workbench-files-title">资源管理器</h2>
            <div className={`workbench-file-layout${preview ? ' has-preview' : ''}`}>
              <div className="workbench-file-browser" hidden={!treeVisible}>
                <div className="workbench-file-toolbar">
                  <Input type="search" className="workbench-file-filter" disabled={!rootEntries || loadingFiles}
                    aria-label="筛选已读取的文件" placeholder="筛选已读取的文件" value={fileFilter}
                    onChange={(event) => setFileFilter(event.target.value)} />
                  <IconButton size="small" label={workspace?.kind === 'personal' ? '打开日常结果目录' : '打开文件夹'}
                    busy={openingFolder} onClick={() => void openFolder()}><OpenFolderIcon /></IconButton>
                  <IconButton size="small" label="刷新目录" disabled={loadingFiles || !session}
                    onClick={() => setFileRevision(value => value + 1)}><RefreshIcon /></IconButton>
                </div>
                {!session && <p className="workbench-hint">打开文件夹以查看文件。</p>}
                {treeVisible && fileFeedback}
                {loadingFiles && treeVisible && <InlineNotice>正在读取目录…</InlineNotice>}
                {session && !loadingFiles && rootEntries && (
                  <div className="workbench-folder-tree" ref={folderTree}>
                    <div className="workbench-tree-content" hidden={filtering}>
                      <button type="button" className="workbench-root-folder" aria-expanded={rootExpanded} aria-label={`${rootExpanded ? '收起' : '展开'} ${rootLabel}`} title={workspace?.rootPath || session.workDir || 'AngelBot 自有日常结果目录'} onClick={() => setRootExpanded(value => !value)}>
                        <TreeCaret /><span className="file-type-icon folder">{rootExpanded ? <OpenFolderIcon /> : <FileIcon />}</span><span className="file-name">{rootLabel}</span>
                      </button>
                      {rootExpanded && <ul className="file-tree" aria-label={`${rootLabel} 的文件`}>
                        {rootEntries.length === 0 ? !fileError && <li className="context-empty-hint">目录为空</li> : <FileTreePage
                          key={`${session.id}:${fileRevision}`} entries={rootEntries} requiredEntryPath={pathForChildLevel(selectedPath)} renderEntry={(entry) => (
                            <FileTreeNode entry={entry} sessionId={session.id} onOpenFile={openEntry} onSaveFile={saveFile} savingFile={savingFile} onError={setFileError}
                              selectedPath={selectedPath} reveal={reveal} treeVisible={treeVisible && !filtering} onDirectoryLoaded={onDirectoryLoaded} />
                          )} />}
                      </ul>}
                    </div>
                    {filtering && (fileMatches.length ? <ul className="file-tree" aria-label="筛选结果">
                      <FileTreePage key={fileFilter.trim()} entries={fileMatches} requiredEntryPath={selectedPath} renderEntry={(entry) => (
                        <FileTreeNode entry={entry} sessionId={session.id} onOpenFile={openEntry} onSaveFile={saveFile}
                          savingFile={savingFile} onError={setFileError} selectedPath={selectedPath} reveal={null} treeVisible={treeVisible} showParentPath />
                      )} />
                    </ul> : <InlineNotice>已读取的目录中没有匹配文件。</InlineNotice>)}
                  </div>
                )}
              </div>
              {preview && (
                <div className="file-preview">
                    <div className="file-preview-header">
                      <h3 id="workbench-preview-title" className="file-preview-path" aria-label={preview.path} title={preview.path}
                        tabIndex={-1} ref={filePreviewHeading}>{preview.path.split(/[\\/]/).at(-1) || preview.path}</h3>
                      <div className="file-preview-actions">
                        <span className="workbench-read-only">只读预览</span>
                        {isMarkdownPreview && <button type="button" onClick={() => setPreview({ ...preview, showSource: !preview.showSource })}>{preview.showSource ? '阅读预览' : '查看源码'}</button>}
                        {!compact && <button type="button" onClick={closePreview}>关闭</button>}
                        <button type="button" className="file-reference-btn" onClick={referencePreviewedFile}>引用文件</button>
                        <button type="button" onClick={() => void copyPreview()}>{preview.truncated ? '复制片段' : '复制原文'}</button>
                        <button type="button" disabled={savingFile} onClick={() => void saveFile(preview.path, true)}>{savingFile ? '另存中…' : '另存为…'}</button>
                      </div>
                      {!treeVisible && fileFeedback}
                      {saveNotice?.fromPreview && <InlineNotice tone={saveNotice.tone}>{saveNotice.message}</InlineNotice>}
                      {copyNotice && <InlineNotice tone={copyNotice.tone}>{copyNotice.message}</InlineNotice>}
                      {preview.truncated && <InlineNotice tone="warning">仅显示开头部分，可另存完整文件。</InlineNotice>}
                    </div>
                    {isMarkdownPreview && !preview.showSource ? (
                      <div className="file-preview-content file-preview-markdown" aria-label="Markdown 阅读预览">
                        <MessageMarkdown content={preview.content} filePreview />
                      </div>
                    ) : <SourcePreview path={preview.path} content={preview.content} />}
                </div>
              )}
            </div>
          </section>
        )}

      </div>
    </WorkbenchFrame>
  );
}

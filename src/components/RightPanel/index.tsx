import { useEffect, useState } from 'react';
import { useSessionsStore } from '$stores/sessions';
import { useWorkspacesStore } from '$stores/workspaces';
import { listWorkDir, readSessionFile, type WorkDirEntry } from '$lib/commands/file';
import { WorkspaceActivityCapsule } from '../WorkspaceActivityCapsule';
import './RightPanel.css';

type PanelView = 'home' | 'activity' | 'browser' | 'files';
type WorkspaceView = Exclude<PanelView, 'home'>;

const BrowserIcon = () => <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/></svg>;
const FileIcon = () => <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>;
const ActivityIcon = () => <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 4h14v16H5z"/><path d="M8 8h8M8 12h5M8 16h7"/></svg>;

function FileTypeIcon({ entry }: { entry: WorkDirEntry }) {
  if (entry.isDirectory) return <span className="file-type-icon folder" aria-label="文件夹">▣</span>;
  const extension = entry.name.split('.').pop()?.toLowerCase() ?? '';
  const labels: Record<string, string> = { ts: 'TS', tsx: 'TS', js: 'JS', jsx: 'JS', py: 'PY', rs: 'RS', html: 'HT', css: 'CSS', json: '{}', md: 'MD', yml: 'Y', yaml: 'Y', bat: 'BAT', ps1: 'PS', vbs: 'VB', txt: 'TXT', sql: 'SQL' };
  return <span className={`file-type-icon ext-${extension || 'plain'}`} aria-label={`${extension || '文本'} 文件`}>{labels[extension] ?? '·'}</span>;
}

function FileTreeNode({ entry, sessionId, onOpenFile, onError, selectedPath }: {
  entry: WorkDirEntry;
  sessionId: string;
  onOpenFile: (entry: WorkDirEntry) => void;
  onError: (message: string) => void;
  selectedPath?: string;
}) {
  const [expanded, setExpanded] = useState(false);
  const [children, setChildren] = useState<WorkDirEntry[]>([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!entry.isDirectory || !expanded) return;
    let cancelled = false;
    setLoading(true);
    listWorkDir(sessionId, entry.path)
      .then((items) => { if (!cancelled) setChildren(items); })
      .catch((error) => { if (!cancelled) onError(error instanceof Error ? error.message : String(error)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [entry.isDirectory, entry.path, expanded, onError, sessionId]);

  return (
    <li className="file-tree-node">
      <button
        className={`file-item${entry.path === selectedPath ? ' selected' : ''}`}
        onClick={() => entry.isDirectory ? setExpanded((value) => !value) : onOpenFile(entry)}
        aria-expanded={entry.isDirectory ? expanded : undefined}
        aria-current={!entry.isDirectory && entry.path === selectedPath ? 'true' : undefined}
      >
        <span className="file-tree-caret">{entry.isDirectory ? (expanded ? '⌄' : '›') : ''}</span>
        <FileTypeIcon entry={entry} />
        <span className="file-name">{entry.name}</span>
      </button>
      {expanded && (
        <ul className="file-tree-children">
          {loading ? <li className="context-loading">加载中…</li> : children.map((child) => (
            <FileTreeNode key={child.path} entry={child} sessionId={sessionId} onOpenFile={onOpenFile} onError={onError} selectedPath={selectedPath} />
          ))}
        </ul>
      )}
    </li>
  );
}

export function RightPanel({
  onClose,
  workspaceView = null,
  locateRequest = null,
}: {
  onClose?: () => void;
  workspaceView?: WorkspaceView | null;
  locateRequest?: { path: string; revision: number } | null;
}) {
  const session = useSessionsStore((state) => state.activeSession);
  const workspaces = useWorkspacesStore((state) => state.workspaces);
  const activeWorkspaceId = useWorkspacesStore((state) => state.activeWorkspaceId);
  const workspace = workspaces.find((item) => item.id === activeWorkspaceId);
  const [view, setView] = useState<PanelView>('home');
  const [rootEntries, setRootEntries] = useState<WorkDirEntry[]>([]);
  const [loadingFiles, setLoadingFiles] = useState(false);
  const [loadingPreview, setLoadingPreview] = useState(false);
  const [fileError, setFileError] = useState<string | null>(null);
  const [preview, setPreview] = useState<{ path: string; content: string } | null>(null);
  const [browserUrl, setBrowserUrl] = useState('');
  const [browserPage, setBrowserPage] = useState<string | null>(null);
  const [browserMessage, setBrowserMessage] = useState<string | null>(null);
  const [browserHistory, setBrowserHistory] = useState<string[]>([]);
  const [browserIndex, setBrowserIndex] = useState(-1);
  const [browserRevision, setBrowserRevision] = useState(0);

  useEffect(() => {
    if (workspaceView) setView(workspaceView);
  }, [workspaceView]);

  useEffect(() => {
    if (!locateRequest || !session) return;
    let cancelled = false;
    setView('files');
    setLoadingPreview(true);
    setFileError(null);
    setPreview(null);
    readSessionFile(session.id, locateRequest.path)
      .then((result) => {
        if (cancelled) return;
        if (result.success) {
          setPreview({ path: locateRequest.path, content: (result.content ?? '').slice(0, 500_000) });
        } else {
          setFileError(result.error ?? '无法定位该项目文件');
        }
      })
      .catch((error) => {
        if (!cancelled) setFileError(error instanceof Error ? error.message : String(error));
      })
      .finally(() => { if (!cancelled) setLoadingPreview(false); });
    return () => { cancelled = true; };
  }, [locateRequest?.path, locateRequest?.revision, session?.id]);

  useEffect(() => {
    if (view !== 'files' || !session) return;
    let cancelled = false;
    setLoadingFiles(true);
    setFileError(null);
    setPreview(null);
    listWorkDir(session.id)
      .then((items) => { if (!cancelled) setRootEntries(items); })
      .catch((error) => { if (!cancelled) setFileError(error instanceof Error ? error.message : String(error)); })
      .finally(() => { if (!cancelled) setLoadingFiles(false); });
    return () => { cancelled = true; };
  }, [session?.id, view]);

  const openView = (next: Exclude<PanelView, 'home'>) => {
    setView(next);
  };

  const returnToConversation = () => {
    if (onClose) {
      onClose();
      return;
    }
    setView('home');
  };

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey)) return;
      const key = event.key.toLowerCase();
      const next = key === 't'
        ? 'browser'
        : key === 'p'
          ? 'files'
          : key === 'j'
            ? 'activity'
          : null;
      if (!next) return;
      event.preventDefault();
      openView(next);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  });

  const openEntry = async (entry: WorkDirEntry) => {
    if (!session) return;
    setLoadingPreview(true);
    setFileError(null);
    setPreview(null);
    try {
      const result = await readSessionFile(session.id, entry.path);
      if (result.success) {
        const content = (result.content ?? '').slice(0, 500_000);
        setPreview({ path: entry.path, content });
      } else {
        setFileError(result.error ?? '无法读取该文件');
      }
    } catch (error) {
      setFileError(error instanceof Error ? error.message : String(error));
    } finally {
      setLoadingPreview(false);
    }
  };

  const referencePreviewedFile = () => {
    if (!preview) return;
    window.dispatchEvent(new CustomEvent('angelbot:reference-file', {
      detail: workspace ? { path: preview.path, workspaceId: workspace.id } : { path: preview.path },
    }));
  };

  const popOutBrowser = () => {
    if (!browserPage) return;
    const browserWindow = window.open(browserPage, '_blank', 'noopener,noreferrer');
    if (!browserWindow) setBrowserMessage('浏览器阻止了弹出窗口，请允许 AngelBot 打开窗口后重试。');
  };

  // The panel itself is the workbench. It augments the active Main-Agent
  // session instead of switching into a second conversation surface.
  return (
    <aside className="right-panel" aria-label="工作侧栏">
      <div className="right-panel-header">
        {view === 'home' ? (
          <span className="right-panel-title">工作侧栏</span>
        ) : (
          <button className="right-panel-back" onClick={() => setView('home')}>← 返回</button>
        )}
        {onClose && (
          <button className="icon-btn" onClick={onClose} title="关闭 (Esc 或 Ctrl+.)" aria-label="关闭工作侧栏">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
          </button>
        )}
      </div>

      <div className={`right-panel-content${view === 'files' ? ' right-panel-content--files' : view === 'browser' ? ' right-panel-content--browser' : ''}`}>
        {view === 'home' && (
          <div className="workbench-menu">
            <button className="workbench-menu-item" onClick={() => openView('activity')}>
              <ActivityIcon /><span>任务与委派</span><kbd>Ctrl+J</kbd>
            </button>
            <button className="workbench-menu-item" onClick={() => openView('browser')}>
              <BrowserIcon /><span>浏览器</span><kbd>Ctrl+T</kbd>
            </button>
            <button className="workbench-menu-item" onClick={() => openView('files')}>
              <FileIcon /><span>文件</span><kbd>Ctrl+P</kbd>
            </button>
          </div>
        )}

        {view === 'activity' && (
          <WorkspaceActivityCapsule mode="panel" onReturnToConversation={returnToConversation} />
        )}

        {view === 'browser' && (
          <section className="workbench-browser">
            <div className="workbench-toolbar">
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
              <div className="browser-address-row">
                <button type="button" className="browser-nav-button" aria-label="后退" disabled={browserIndex <= 0} onClick={() => { const index = browserIndex - 1; const page = browserHistory[index]; setBrowserIndex(index); setBrowserUrl(page); setBrowserPage(page); }}>←</button>
                <button type="button" className="browser-nav-button" aria-label="前进" disabled={browserIndex >= browserHistory.length - 1} onClick={() => { const index = browserIndex + 1; const page = browserHistory[index]; setBrowserIndex(index); setBrowserUrl(page); setBrowserPage(page); }}>→</button>
                <button type="button" className="browser-nav-button" aria-label="刷新" disabled={!browserPage} onClick={() => setBrowserRevision((value) => value + 1)}>↻</button>
                <input id="browser-address" value={browserUrl} onChange={(event) => setBrowserUrl(event.target.value)} placeholder="输入网址" autoComplete="url" aria-label="网址" />
                <button className="browser-go-button" type="submit" aria-label="打开网址">↗</button>
              </div>
            </form>
            {browserMessage && <p className="workbench-hint browser-address-message">{browserMessage}</p>}
            {browserPage ? <iframe key={browserRevision} className="workbench-browser-frame" title="AngelBot 浏览器" src={browserPage} /> : <div className="browser-empty"><BrowserIcon /><strong>开始浏览</strong><span>输入 URL 以打开网页</span></div>}
          </section>
        )}

        {view === 'files' && (
          <section className={`workbench-files workbench-files--workspace${preview ? ' workbench-files--has-preview' : ''}`}>
            <div className="workbench-section-title">项目文件</div>
            {!session ? (
              <div className="workbench-empty"><FileIcon /><p>先选择一个项目工作区，再浏览其文件。</p></div>
            ) : (
              <>
                <p className="workbench-hint">{workspace?.rootPath || session.workDir || '项目目录'} · 定位、预览或引用文件给主 Agent</p>
                {loadingFiles && <div className="context-loading">加载中…</div>}
                {loadingPreview && <div className="context-loading">正在读取文件…</div>}
                {fileError && <div className="workbench-error">{fileError}</div>}
                {!loadingFiles && !fileError && (
                  <ul className="file-tree">
                    {rootEntries.length === 0 ? <li className="context-empty-hint">目录为空</li> : rootEntries.map((entry) => (
                      <FileTreeNode key={entry.path} entry={entry} sessionId={session.id} onOpenFile={(file) => void openEntry(file)} onError={setFileError} selectedPath={preview?.path} />
                    ))}
                  </ul>
                )}
                {preview && (
                  <div className="file-preview">
                    <div className="file-preview-header">
                      <span className="file-preview-path" title={preview.path}>{preview.path}</span>
                      <div className="file-preview-actions">
                        <span className="workbench-read-only">只读预览</span>
                        <button type="button" onClick={() => setPreview(null)}>关闭</button>
                        <button type="button" className="file-reference-btn" onClick={referencePreviewedFile}>引用文件</button>
                      </div>
                    </div>
                    <pre className="file-preview-content"><code>{preview.content}</code></pre>
                  </div>
                )}
              </>
            )}
          </section>
        )}

      </div>
    </aside>
  );
}

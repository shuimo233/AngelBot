import { useEffect, useRef, useState } from 'react';
import { useSessionsStore } from '$stores/sessions';
import { useWorkspacesStore } from '$stores/workspaces';
import { listWorkDir, readSessionFile, saveSessionFile, type WorkDirEntry } from '$lib/commands/file';
import { WorkspaceActivityCapsule } from '../WorkspaceActivityCapsule';
import { WorkspaceReminders } from '../WorkspaceReminders';
import { openProjectFolder } from '$lib/project-folder';
import './RightPanel.css';

type PanelView = 'home' | 'activity' | 'browser' | 'files';
type WorkspaceView = Exclude<PanelView, 'home'>;

const BrowserIcon = () => <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/></svg>;
const FileIcon = () => <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>;
const ActivityIcon = () => <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 4h14v16H5z"/><path d="M8 8h8M8 12h5M8 16h7"/></svg>;

function FileTypeIcon({ entry }: { entry: WorkDirEntry }) {
  if (entry.isDirectory) return <span className="file-type-icon folder" aria-label="文件夹"><FileIcon /></span>;
  const extension = entry.name.split('.').pop()?.toLowerCase() ?? '';
  const labels: Record<string, string> = { ts: 'TS', tsx: 'TS', js: 'JS', jsx: 'JS', py: 'PY', rs: 'RS', html: 'HT', css: 'CSS', json: '{}', md: 'MD', yml: 'Y', yaml: 'Y', bat: 'BAT', ps1: 'PS', vbs: 'VB', txt: 'TXT', sql: 'SQL' };
  return <span className={`file-type-icon ext-${extension || 'plain'}`} aria-label={`${extension || '文本'} 文件`}>{labels[extension] ?? '·'}</span>;
}

function FileTreeNode({ entry, sessionId, onOpenFile, onSaveFile, savingFile, onError, selectedPath }: {
  entry: WorkDirEntry;
  sessionId: string;
  onOpenFile: (entry: WorkDirEntry) => void;
  onSaveFile: (path: string) => void;
  savingFile: boolean;
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
        aria-label={`${entry.isDirectory ? (expanded ? '收起' : '展开') : '打开'} ${entry.name}`}
        onClick={() => entry.isDirectory ? setExpanded((value) => !value) : onOpenFile(entry)}
        aria-expanded={entry.isDirectory ? expanded : undefined}
        aria-current={!entry.isDirectory && entry.path === selectedPath ? 'true' : undefined}
        title={entry.temporary ? `${entry.path} · 浏览器临时文件，不随备份；需要保留请另存或让 AngelBot 复制到结果区。` : entry.path}
      >
        <span className="file-tree-caret">{entry.isDirectory ? (expanded ? '⌄' : '›') : ''}</span>
        <FileTypeIcon entry={entry} />
        <span className="file-name">{entry.temporary && entry.isDirectory && !entry.path.includes('/') ? '浏览器临时文件（不随备份）' : entry.name}</span>
      </button>
      {!entry.isDirectory && <button type="button" className="file-save-as" disabled={savingFile} aria-label={`另存 ${entry.name}`} onClick={() => onSaveFile(entry.path)}>另存</button>}
      {expanded && (
        <ul className="file-tree-children">
          {loading ? <li className="context-loading">加载中…</li> : children.map((child) => (
            <FileTreeNode key={child.path} entry={child} sessionId={sessionId} onOpenFile={onOpenFile} onSaveFile={onSaveFile} savingFile={savingFile} onError={onError} selectedPath={selectedPath} />
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
  locateRequest?: { path: string; revision: number; sessionId: string } | null;
}) {
  const session = useSessionsStore((state) => state.activeSession);
  const workspaces = useWorkspacesStore((state) => state.workspaces);
  const activeWorkspaceId = useWorkspacesStore((state) => state.activeWorkspaceId);
  const workspace = workspaces.find((item) => item.id === activeWorkspaceId);
  const fileScope = JSON.stringify([session?.id, workspace?.rootPath, workspace?.directoryConfirmed]);
  const [view, setView] = useState<PanelView>('home');
  const [rootSnapshot, setRootSnapshot] = useState<{ scope: string; entries: WorkDirEntry[] } | null>(null);
  const rootEntries = rootSnapshot?.scope === fileScope ? rootSnapshot.entries : [];
  const [rootExpanded, setRootExpanded] = useState(true);
  const [openingFolder, setOpeningFolder] = useState(false);
  const folderRequest = useRef(false);
  const rootLabel = workspace?.kind === 'personal' ? '日常结果'
    : workspace?.rootPath?.split(/[\\/]/).filter(Boolean).pop() || workspace?.name || '项目目录';
  const [loadingFiles, setLoadingFiles] = useState(false);
  const [fileRevision, setFileRevision] = useState(0);
  const [loadingPreview, setLoadingPreview] = useState(false);
  const [fileError, setFileError] = useState<string | null>(null);
  const [previewSnapshot, setPreview] = useState<{ scope: string; path: string; content: string } | null>(null);
  const preview = previewSnapshot?.scope === fileScope ? previewSnapshot : null;
  const [savingFile, setSavingFile] = useState(false);
  const [saveNotice, setSaveNotice] = useState<string | null>(null);
  const previewRequest = useRef(0);

  useEffect(() => { previewRequest.current += 1; setPreview(null); setLoadingPreview(false); setSaveNotice(null); setRootExpanded(true); }, [fileScope]);
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
    if (!locateRequest || !session || locateRequest.sessionId !== session.id) return;
    const revision = ++previewRequest.current;
    let cancelled = false;
    setView('files');
    setLoadingPreview(true);
    setFileError(null);
    setPreview(null);
    readSessionFile(session.id, locateRequest.path)
      .then((result) => {
        if (cancelled || previewRequest.current !== revision) return;
        if (result.success) {
          setPreview({ scope: fileScope, path: locateRequest.path, content: (result.content ?? '').slice(0, 500_000) });
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
    setFileError(null);
    listWorkDir(session.id)
      .then((items) => { if (!cancelled) setRootSnapshot({ scope: fileScope, entries: items }); })
      .catch((error) => { if (!cancelled) setFileError(error instanceof Error ? error.message : String(error)); })
      .finally(() => { if (!cancelled) setLoadingFiles(false); });
    return () => { cancelled = true; };
  }, [fileScope, view, fileRevision]);

  const openFolder = async () => {
    if (folderRequest.current) return;
    folderRequest.current = true;
    setOpeningFolder(true);
    setFileError(null);
    try {
      await openProjectFolder();
    } catch (error) {
      setFileError(error instanceof Error ? error.message : '无法打开文件夹，请重试。');
    } finally {
      folderRequest.current = false;
      setOpeningFolder(false);
    }
  };

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
    const revision = ++previewRequest.current;
    setLoadingPreview(true);
    setFileError(null);
    setPreview(null);
    try {
      const result = await readSessionFile(session.id, entry.path);
      if (previewRequest.current !== revision || useSessionsStore.getState().activeSessionId !== session.id) return;
      if (result.success) {
        const content = (result.content ?? '').slice(0, 500_000);
        setPreview({ scope: fileScope, path: entry.path, content });
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
  };

  const saveFile = async (path: string) => {
    if (!session || savingFile) return;
    const sessionId = session.id;
    setSavingFile(true);
    setSaveNotice(null);
    try {
      const result = await saveSessionFile(sessionId, path);
      if (useSessionsStore.getState().activeSessionId === sessionId && result.status === 'saved') {
        setSaveNotice(`已另存 ${result.name}（${result.bytes} 字节），原文件保留。`);
      }
    } catch (error) {
      if (useSessionsStore.getState().activeSessionId === sessionId) setSaveNotice(String(error));
    } finally {
      setSavingFile(false);
    }
  };

  const popOutBrowser = () => {
    if (!browserPage) return;
    const browserWindow = window.open(browserPage, '_blank', 'noopener,noreferrer');
    if (!browserWindow) setBrowserMessage('浏览器阻止了弹出窗口，请允许 AngelBot 打开窗口后重试。');
  };

  // The panel itself is the workbench. It augments the active Main-Agent
  // session instead of switching into a second conversation surface.
  return (
    <aside id="conversation-workbench" className="right-panel" aria-label="工作侧栏">
      <div className="right-panel-header">
        {view === 'home' ? (
          <span className="right-panel-title">工作台</span>
        ) : (
          <button className="right-panel-back" onClick={() => setView('home')}>← 返回</button>
        )}
        {onClose && (
          <button className="icon-btn" onClick={onClose} title="关闭 (Esc 或 Ctrl+.)" aria-label="关闭工作侧栏">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
          </button>
        )}
      </div>

      <nav className="workbench-tabs" aria-label="工作台视图">
        <button type="button" aria-pressed={view === 'activity'} onClick={() => openView('activity')}><ActivityIcon /><span>任务</span></button>
        <button type="button" aria-pressed={view === 'files'} onClick={() => openView('files')}><FileIcon /><span>{workspace?.kind === 'personal' ? '结果文件' : '文件'}</span></button>
        <button type="button" aria-pressed={view === 'browser'} onClick={() => openView('browser')}><BrowserIcon /><span>浏览器</span></button>
      </nav>

      <div className={`right-panel-content${view === 'files' ? ' right-panel-content--files' : view === 'browser' ? ' right-panel-content--browser' : ''}`}>
        {view === 'home' && (
          <div className="workbench-overview">
            <span className="ui-eyebrow">{workspace?.kind === 'personal' ? '个人空间' : '当前项目'}</span>
            <h2>{workspace?.kind === 'personal' ? '日常事务，也有条理' : workspace?.name || '与你的对话一起工作'}</h2>
            <p>查看正在处理的任务、打开文件，或浏览参考页面。对话始终留在左侧。</p>
            <dl>
              <div><dt>任务</dt><dd>查看进展与需要你决定的事项</dd></div>
              <div><dt>{workspace?.kind === 'personal' ? '结果文件' : '文件'}</dt><dd>{workspace?.kind === 'personal' ? '预览和另存 AngelBot 交付的文件' : '预览项目文件，引用到当前对话'}</dd></div>
              <div><dt>浏览器</dt><dd>打开参考页面；不会因此授予操作权限</dd></div>
            </dl>
            {workspace?.rootPath && <p className="workbench-scope" title={workspace.rootPath}>{workspace.rootPath}</p>}
          </div>
        )}

        {view === 'activity' && (
          <>
            <WorkspaceActivityCapsule mode="panel" onReturnToConversation={returnToConversation} />
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
                  <button className="browser-go-button" type="submit" aria-label="打开网址">↗</button>
                </div>
              </div>
            </form>
            {browserMessage && <p className="workbench-hint browser-address-message">{browserMessage}</p>}
            {browserPage ? <iframe key={browserRevision} className="workbench-browser-frame" title="AngelBot 浏览器" src={browserPage} /> : <div className="browser-empty"><BrowserIcon /><strong>开始浏览</strong><span>输入 URL 以打开网页</span></div>}
          </section>
        )}

        {view === 'files' && (
          <section className={`workbench-files workbench-files--workspace${preview ? ' workbench-files--has-preview' : ''}`} aria-labelledby="workbench-files-title">
            <div className="workbench-files-header"><h2 id="workbench-files-title" className="workbench-section-title">资源管理器</h2><div className="workbench-folder-actions">
              <button type="button" disabled={openingFolder} onClick={() => void openFolder()} title="选择文件夹，打开或建立对应项目及其主对话">{openingFolder ? '正在打开…' : '打开文件夹'}</button>
              <button type="button" disabled={loadingFiles || !session} onClick={() => setFileRevision((value) => value + 1)}>刷新目录</button>
            </div></div>
            {fileError && <div className="workbench-error" role="alert">{fileError}</div>}
            {!session ? (
              <div className="workbench-empty"><FileIcon /><p>打开一个文件夹，在独立项目中浏览文件并与 AngelBot 继续工作。</p></div>
            ) : (
              <>
                <p className="workbench-hint">{workspace?.kind === 'personal'
                  ? 'AngelBot 生成的文件默认保留在这里，不会访问其它目录。可预览、引用和另存；数据备份包含这些文件。'
                  : <><span className="workbench-directory-path">{workspace?.rootPath || session.workDir || '项目目录'}</span><span>定位、预览或引用文件给主 Agent</span></>}</p>
                {loadingFiles && <div className="context-loading">加载中…</div>}
                {loadingPreview && <div className="context-loading">正在读取文件…</div>}
                {saveNotice && <p className="workbench-hint" role="status">{saveNotice}</p>}
                {!loadingFiles && (
                  <div className="workbench-folder-tree">
                  <button type="button" className="workbench-root-folder" aria-expanded={rootExpanded} aria-label={`${rootExpanded ? '收起' : '展开'} ${rootLabel}`} title={workspace?.rootPath || 'AngelBot 自有日常结果目录'} onClick={() => setRootExpanded(value => !value)}>
                    <span aria-hidden="true">{rootExpanded ? '⌄' : '›'}</span><FileIcon /><span>{rootLabel}</span>
                  </button>
                  {rootExpanded && <ul className="file-tree" aria-label={`${rootLabel} 的文件`}>
                    {rootEntries.length === 0 ? !fileError && <li className="context-empty-hint">目录为空</li> : rootEntries.map((entry) => (
                      <FileTreeNode key={`${session.id}:${fileRevision}:${entry.path}`} entry={entry} sessionId={session.id} onOpenFile={(file) => void openEntry(file)} onSaveFile={(path) => void saveFile(path)} savingFile={savingFile} onError={setFileError} selectedPath={preview?.path} />
                    ))}
                  </ul>}
                  </div>
                )}
                {preview && (
                  <div className="file-preview">
                    <div className="file-preview-header">
                      <span className="file-preview-path" title={preview.path}>{preview.path}</span>
                      <div className="file-preview-actions">
                        <span className="workbench-read-only">只读预览</span>
                        <button type="button" onClick={() => setPreview(null)}>关闭</button>
                        <button type="button" className="file-reference-btn" onClick={referencePreviewedFile}>引用文件</button>
                        <button type="button" disabled={savingFile} onClick={() => void saveFile(preview.path)}>{savingFile ? '另存中…' : '另存为…'}</button>
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

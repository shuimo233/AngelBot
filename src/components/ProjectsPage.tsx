import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import { normalizeProjectSearch } from '$lib/project-search';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspacesStore } from '$stores/workspaces';
import { Input } from '../ui';
import './ProjectsPage.css';

const PROJECT_BATCH_SIZE = 20;
const PROJECT_SEARCH_THRESHOLD = 5;

/** Project selection is navigation, never a file grant or a new conversation. */
export function ProjectsPage() {
  const workspaces = useWorkspacesStore(state => state.workspaces);
  const loading = useWorkspacesStore(state => state.loading);
  const lastError = useWorkspacesStore(state => state.lastError);
  const load = useWorkspacesStore(state => state.loadWorkspaces);
  const [opening, setOpening] = useState<string | null>(null);
  const [error, setError] = useState('');
  const [query, setQuery] = useState('');
  const [visibleCount, setVisibleCount] = useState(PROJECT_BATCH_SIZE);
  const inFlight = useRef(false);
  const firstAddedProject = useRef<number | null>(null);
  const projectList = useRef<HTMLElement>(null);
  const projects = useMemo(() => workspaces.filter(workspace => workspace.kind === 'project'), [workspaces]);
  const search = normalizeProjectSearch(query);
  const filtered = useMemo(() => search ? projects.filter(project =>
    normalizeProjectSearch(project.name).includes(search)
    || normalizeProjectSearch(project.rootPath ?? '').includes(search),
  ) : projects, [projects, search]);
  const remaining = filtered.length - visibleCount;

  useLayoutEffect(() => {
    const index = firstAddedProject.current;
    if (index === null) return;
    firstAddedProject.current = null;
    projectList.current?.querySelectorAll<HTMLButtonElement>('.project-entry')[index]?.focus();
  }, [visibleCount]);

  const open = async (id: string) => {
    if (inFlight.current) return;
    inFlight.current = true; setOpening(id); setError('');
    const navigation = useNavigationStore.getState();
    try {
      const accepted = await useWorkspacesStore.getState().openWorkspace(id);
      if (accepted && navigation === useNavigationStore.getState()) navigation.setPage('chat');
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : '暂时无法打开项目，请重试。');
    } finally { inFlight.current = false; setOpening(null); }
  };

  return <main className="projects-page" aria-label="项目">
    <header className="projects-header">
      <h1>项目</h1>
      <button type="button" className="projects-add" onClick={() => window.dispatchEvent(new CustomEvent('new-project'))}>添加项目</button>
    </header>
    {(error || lastError) && <div className="projects-error" role="alert"><p>{error || lastError}</p>{lastError && <button type="button" disabled={loading} onClick={() => void load()}>重新加载</button>}</div>}
    {(projects.length > PROJECT_SEARCH_THRESHOLD || query) && <div className="projects-search">
      <Input type="search" aria-label="搜索项目" placeholder="项目名或路径" value={query} onChange={event => {
        setQuery(event.target.value);
        setVisibleCount(PROJECT_BATCH_SIZE);
      }} />
      {search && <span role="status">{filtered.length ? `找到 ${filtered.length} 个项目` : '没有匹配的项目'}</span>}
    </div>}
    {loading && !projects.length && <p role="status" className="projects-empty">正在读取项目…</p>}
    {!loading && !projects.length && !lastError && <p className="projects-empty">暂无项目</p>}
    {filtered.length > 0 && <section className="projects-list" aria-label="已有项目" ref={projectList}>
      {filtered.slice(0, visibleCount).map(project => <button type="button" key={project.id} className="project-entry" disabled={opening !== null} aria-busy={opening === project.id || undefined} onClick={() => void open(project.id)}>
        <svg width="21" height="21" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="M3 6h6l2 2h10v12H3V6zM3 6V4h7l2 2" /></svg>
        <span className="project-entry-copy"><strong>{project.name}</strong><span title={project.rootPath}>{project.rootPath || '项目主对话'}</span></span>
        <span className="project-entry-action">{opening === project.id ? '正在进入…' : project.directoryConfirmed === false ? '目录待确认' : '进入项目'}<span aria-hidden="true"> →</span></span>
      </button>)}
      {remaining > 0 && <button type="button" className="projects-more" disabled={opening !== null} onClick={() => {
        firstAddedProject.current = visibleCount;
        setVisibleCount(count => count + PROJECT_BATCH_SIZE);
      }}>显示更多（还有 {remaining} 个）</button>}
    </section>}
  </main>;
}

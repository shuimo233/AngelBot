import { useRef, useState } from 'react';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspacesStore } from '$stores/workspaces';
import './ProjectsPage.css';

/** Project selection is navigation, never a file grant or a new conversation. */
export function ProjectsPage() {
  const workspaces = useWorkspacesStore(state => state.workspaces);
  const loading = useWorkspacesStore(state => state.loading);
  const lastError = useWorkspacesStore(state => state.lastError);
  const load = useWorkspacesStore(state => state.loadWorkspaces);
  const [opening, setOpening] = useState<string | null>(null);
  const [error, setError] = useState('');
  const inFlight = useRef(false);
  const projects = workspaces.filter(workspace => workspace.kind === 'project');

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
      <div><h1>项目</h1><p>每个项目保留独立的资料与主对话，日常事务不必创建项目。</p></div>
      <button type="button" className="projects-add" onClick={() => window.dispatchEvent(new CustomEvent('new-project'))}>添加项目</button>
    </header>
    {(error || lastError) && <div className="projects-error" role="alert"><p>{error || lastError}</p>{lastError && <button type="button" disabled={loading} onClick={() => void load()}>重新加载</button>}</div>}
    {loading && !projects.length ? <p role="status" className="projects-empty">正在读取项目…</p>
      : !projects.length && !lastError ? <section className="projects-empty"><h2>给持续推进的事留一个空间</h2><p>旅行计划、长期写作或软件开发，都可以成为项目。一次性的事务无需建项目。</p><button type="button" onClick={() => window.dispatchEvent(new CustomEvent('new-project'))}>选择项目文件夹</button></section>
      : <section className="projects-list" aria-label="已有项目">{projects.map(project => <button type="button" key={project.id} className="project-entry" disabled={opening !== null} aria-busy={opening === project.id || undefined} onClick={() => void open(project.id)}>
        <svg width="21" height="21" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="M3 6h6l2 2h10v12H3V6zM3 6V4h7l2 2" /></svg>
        <span className="project-entry-copy"><strong>{project.name}</strong><span title={project.rootPath}>{project.rootPath || '项目主对话'}</span></span>
        <span className="project-entry-action">{opening === project.id ? '正在进入…' : project.directoryConfirmed === false ? '目录待确认' : '进入项目'}<span aria-hidden="true"> →</span></span>
      </button>)}</section>}
  </main>;
}

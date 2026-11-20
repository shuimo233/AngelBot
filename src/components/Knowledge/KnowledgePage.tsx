import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { useKnowledgeStore, type KnowledgeEntry } from '$stores/knowledge';
import { deleteMemory, saveMemory, updateMemory } from '$lib/commands/memory';
import { loadTextAttachments, TEXT_ATTACHMENT_ACCEPT } from '$lib/text-attachments';
import './KnowledgePage.css';

function titleOf(content: string) {
  return content.split('\n').find(line => line.trim())?.replace(/^\s{0,3}#{1,6}\s+/, '').trim().slice(0, 100) || '未命名资料';
}
const formatDate = (ts: number) => new Date(ts * 1000).toLocaleDateString();
// A Markdown heading already appears in the detail header; avoid repeating it.
const bodyOf = (content: string) => content.replace(/^\s{0,3}#{1,6}\s+[^\n]*(?:\n|$)/, '').trim();
const compactWidth = 760;

function rememberListPosition(row: HTMLElement) {
  const offsets: Array<{ element: HTMLElement; top: number }> = [];
  for (let element = row.parentElement; element; element = element.parentElement) {
    offsets.push({ element, top: element.scrollTop });
  }
  return { row, offsets };
}

export function KnowledgePage() {
  const { entries, isLoading, error, query, setQuery, loadKnowledge } = useKnowledgeStore();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [sort, setSort] = useState('updated');
  const [source, setSource] = useState('all');
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [visibleCount, setVisibleCount] = useState(30);
  const [showAddForm, setShowAddForm] = useState(false);
  const [draft, setDraft] = useState('');
  const [keep, setKeep] = useState(true);
  const [editDraft, setEditDraft] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState('');
  const [notice, setNotice] = useState('');
  const [operationEntry, setOperationEntry] = useState<KnowledgeEntry | null>(null);
  const [compact, setCompact] = useState(false);
  const [reading, setReading] = useState(false);
  const pageRef = useRef<HTMLDivElement>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const detailRef = useRef<HTMLElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const addButtonRef = useRef<HTMLButtonElement>(null);
  const focusTarget = useRef<'detail' | 'list' | null>(null);
  const listPosition = useRef<ReturnType<typeof rememberListPosition> | null>(null);
  const locked = busy || editDraft !== null || confirmDelete;

  useEffect(() => { void loadKnowledge(); }, [loadKnowledge]);
  const search = query.trim().toLowerCase();
  useEffect(() => { setVisibleCount(30); }, [search, source, sort]);
  const filtered = entries.filter(entry => (!search || entry.content.toLowerCase().includes(search))
    && (source === 'all' || (source === 'user' ? entry.source === 'user' : entry.source !== 'user')))
    .sort((a, b) => sort === 'name' ? titleOf(a.content).localeCompare(titleOf(b.content)) : b.updatedAt - a.updatedAt || a.id.localeCompare(b.id));
  const matchingEntry = filtered.find(entry => entry.id === selectedId);
  const selected = locked && operationEntry ? operationEntry : matchingEntry ?? (compact && reading ? undefined : filtered[0]);
  const detailOnly = compact && (reading || Boolean(operationEntry)) && Boolean(selected);

  useLayoutEffect(() => {
    const page = pageRef.current;
    if (!page) return;
    const update = (width: number) => { if (width > 0) setCompact(width <= compactWidth); };
    update(page.getBoundingClientRect().width);
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(([entry]) => update(entry.contentRect.width));
    observer.observe(page);
    return () => observer.disconnect();
  }, []);

  useLayoutEffect(() => {
    if (compact && reading && !selected && !locked) {
      focusTarget.current = 'list';
      setReading(false);
    }
    if (focusTarget.current === 'detail' && detailOnly) {
      focusTarget.current = null;
      detailRef.current?.focus({ preventScroll: true });
      detailRef.current?.scrollIntoView?.({ block: 'start' });
    } else if (focusTarget.current === 'list' && !detailOnly && !locked) {
      focusTarget.current = null;
      const saved = listPosition.current;
      listPosition.current = null;
      const target = saved?.row.isConnected ? saved.row : searchRef.current ?? addButtonRef.current;
      target?.focus({ preventScroll: true });
      for (const { element, top } of saved?.offsets ?? []) {
        if (element.isConnected && element.scrollTop !== top) element.scrollTop = top;
      }
    }
  }, [compact, reading, selected, detailOnly, locked]);

  const returnToList = () => {
    focusTarget.current = 'list';
    setReading(false);
  };

  const add = async () => {
    if (!draft.trim() || locked) return;
    setBusy(true); setActionError(''); setNotice('');
    try {
      const id = crypto.randomUUID();
      const savedId = await saveMemory({ id, scope: 'global', category: 'knowledge', content: draft.trim(), importance: 3, source: 'user', isPermanent: keep, userConfirmed: true });
      setDraft(''); setShowAddForm(false); setSelectedId(savedId); setQuery(''); setSource('all');
      setNotice(savedId === id ? '资料已保存到共享资料，可在相关对话中检索。' : '已打开相同的共享资料。');
      await loadKnowledge();
      if (compact) { focusTarget.current = 'detail'; setReading(true); }
    } catch (cause) { setActionError(cause instanceof Error ? cause.message : typeof cause === 'string' ? cause : '保存失败，请重试。'); }
    finally { setBusy(false); }
  };
  const edit = async () => {
    if (!selected || !editDraft?.trim() || busy) return;
    setBusy(true); setActionError(''); setNotice('');
    try {
      await updateMemory({ id: selected.id, content: editDraft.trim(), scope: selected.scope, category: selected.category, importance: selected.importance, isPermanent: selected.isPermanent });
      setEditDraft(null); setOperationEntry(null); setNotice('资料已更新，原有范围与保留设置未改变。');
      await loadKnowledge();
    } catch (cause) { setActionError(cause instanceof Error ? cause.message : '更新失败，修改内容仍保留。'); }
    finally { setBusy(false); }
  };
  const remove = async () => {
    if (!selected || busy) return;
    setBusy(true); setActionError(''); setNotice('');
    try {
      await deleteMemory(selected.id);
      if (detailOnly) returnToList();
      setConfirmDelete(false); setOperationEntry(null); setSelectedId(null); setNotice('资料已删除。'); await loadKnowledge();
    } catch (cause) { setActionError(cause instanceof Error ? cause.message : '删除失败，请核对后重试。'); }
    finally { setBusy(false); }
  };
  const importFile = async (file?: File) => {
    if (!file || locked || draft.trim()) return;
    setBusy(true); setActionError(''); setNotice('');
    try {
      const [loaded] = await loadTextAttachments([file]);
      setDraft('# ' + loaded.name + '\n\n' + loaded.text); setShowAddForm(true);
      setNotice('已读取文字快照，请核对后保存。不会保管或同步原文件。');
    } catch (cause) { setActionError(cause instanceof Error ? cause.message : '无法读取文件，未添加资料。'); }
    finally { setBusy(false); }
  };

  return <div ref={pageRef} className={`knowledge-page${compact ? ' knowledge-page--compact' : ''}`}>
    <div hidden={detailOnly}>
    <div className="knowledge-toolbar" role="group" aria-label="参考资料工具栏">
      {entries.length > 0 && <div className="knowledge-search-tools">
        <input ref={searchRef} className="knowledge-search-input" type="search" aria-label="搜索资料" placeholder="搜索标题或正文…" disabled={locked} value={query} onChange={event => setQuery(event.target.value)} />
        <button type="button" disabled={locked} aria-expanded={filtersOpen} onClick={() => setFiltersOpen(open => !open)}>筛选与排序{source !== 'all' || sort !== 'updated' ? ' · 已设置' : ''}</button>
      </div>}
      <div className="knowledge-actions">
        <button ref={addButtonRef} type="button" className="knowledge-add-button knowledge-primary-action" disabled={locked} aria-expanded={showAddForm} onClick={() => { setShowAddForm(value => !value); setActionError(''); }}>{showAddForm ? '收起添加' : '添加资料'}</button>
        <input ref={fileInput} type="file" hidden aria-label="导入资料文件" accept={TEXT_ATTACHMENT_ACCEPT} onChange={event => { const file = event.target.files?.[0]; event.target.value = ''; void importFile(file); }} />
      </div>
    </div>
    {entries.length > 0 && filtersOpen && <div className="knowledge-filters"><label>来源<select aria-label="资料来源" disabled={locked} value={source} onChange={event => setSource(event.target.value)}><option value="all">全部来源</option><option value="user">手动添加</option><option value="other">其他来源</option></select></label><label>排序<select aria-label="资料排序" disabled={locked} value={sort} onChange={event => setSort(event.target.value)}><option value="updated">最近更新</option><option value="name">按标题</option></select></label></div>}
    {showAddForm && <section className="knowledge-add-form" aria-label="添加资料">
      <div className="knowledge-form-heading"><button type="button" aria-describedby="knowledge-import-hint" title={draft.trim() ? '先保存或清空草稿，避免覆盖内容' : undefined} disabled={locked || Boolean(draft.trim())} onClick={() => fileInput.current?.click()}>从文件读取</button><span id="knowledge-import-hint" className="knowledge-add-hint">文本、DOCX、XLSX、PDF 文字层</span></div>
      <label className="knowledge-draft-label" htmlFor="knowledge-draft">资料内容</label>
      <textarea id="knowledge-draft" autoFocus value={draft} disabled={locked} onChange={event => setDraft(event.target.value)} rows={6} placeholder="第一行写标题，下面写正文。支持 Markdown。" />
      <div className="knowledge-add-form-footer">
        <label className="knowledge-retention"><input type="checkbox" checked={keep} disabled={locked} onChange={event => setKeep(event.target.checked)} />长期保留</label>
        <div className="knowledge-actions"><span className="knowledge-add-hint">日常与项目均可检索</span><button type="button" disabled={!draft.trim() || locked} onClick={() => void add()}>{busy ? '处理中…' : '保存资料'}</button></div>
      </div>
      <details className="knowledge-help"><summary>保留规则与导入限制</summary><p>长期保留的资料不随时间衰减。</p><p>最多提取 16 KiB 文字。DOCX、XLSX、PDF 原文件最多 4 MiB，PDF 最多 40 页。扫描件需先 OCR。</p></details>
    </section>}
    </div>
    {actionError && <p role="alert" className="knowledge-error">{actionError}</p>}
    {notice && <p role="status" className="knowledge-add-hint">{notice}</p>}
    {error && <div className="knowledge-error" role="alert"><span>{error} {entries.length > 0 && '当前展示上次读取的资料。'}</span><button type="button" disabled={isLoading || locked} onClick={() => void loadKnowledge()}>重新加载</button></div>}
    {isLoading && <p role="status" className="knowledge-add-hint">正在读取资料…</p>}
    {!isLoading && !filtered.length && !error && (entries.length > 0
      ? <div className="knowledge-empty"><h3>没有匹配资料</h3></div>
      : !showAddForm && <div className="knowledge-empty"><h3>暂无参考资料</h3></div>)}
    {filtered.length > 0 && <div className="knowledge-browser">
      <div className="knowledge-list" aria-label="资料列表" hidden={detailOnly}>{filtered.slice(0, visibleCount).map(entry => <button type="button" key={entry.id} className="knowledge-entry" disabled={locked} aria-pressed={selected?.id === entry.id} onClick={event => {
        setSelectedId(entry.id); setActionError(''); setNotice('');
        if (compact) {
          listPosition.current = rememberListPosition(event.currentTarget);
          focusTarget.current = 'detail'; setReading(true);
        }
      }}>
        <strong>{titleOf(entry.content)}</strong><span className="knowledge-snippet">{bodyOf(entry.content).replace(/^\s{0,3}#{1,6}\s+/gm, '').slice(0, 240) || '仅标题'}</span><span className="knowledge-entry-meta">{formatDate(entry.updatedAt)}</span>
      </button>)}{filtered.length > visibleCount && <button type="button" disabled={locked} onClick={() => setVisibleCount(count => count + 30)}>显示更多（还有 {filtered.length - visibleCount} 条）</button>}</div>
      {selected && <article ref={detailRef} tabIndex={-1} className="knowledge-detail" aria-label="资料详情" hidden={compact && !detailOnly} onKeyDown={event => {
        if (event.key !== 'Escape' || event.defaultPrevented || !detailOnly || locked) return;
        event.preventDefault(); event.stopPropagation(); returnToList();
      }}>
        {detailOnly && <button className="knowledge-back" type="button" disabled={locked} onClick={returnToList}>返回列表</button>}
        <header><h3>{titleOf(selected.content)}</h3>
          {editDraft === null && <div className="knowledge-actions"><button type="button" disabled={locked} onClick={() => { setSelectedId(selected.id); setOperationEntry(selected); setEditDraft(selected.content); setActionError(''); }}>编辑</button><details className="knowledge-more"><summary>更多操作</summary><button type="button" disabled={locked} onClick={() => { setOperationEntry(selected); setConfirmDelete(true); setActionError(''); }}>删除资料</button></details></div>}
        </header>
        {confirmDelete && <div className="knowledge-delete-confirm" role="group" aria-label="确认删除资料"><p>删除“{titleOf(selected.content)}”？此操作无法在资料页撤销。</p><button type="button" disabled={busy} onClick={() => void remove()}>确认删除</button><button type="button" disabled={busy} onClick={() => { setConfirmDelete(false); setOperationEntry(null); }}>保留资料</button></div>}
        {editDraft !== null ? <div className="knowledge-editor"><label htmlFor="knowledge-edit">编辑资料内容</label><textarea id="knowledge-edit" rows={12} autoFocus value={editDraft} disabled={busy} onChange={event => setEditDraft(event.target.value)} /><div className="knowledge-actions"><button type="button" disabled={busy} onClick={() => { setEditDraft(null); setOperationEntry(null); setActionError(''); }}>取消修改</button><button type="button" className="knowledge-primary-action" disabled={busy || !editDraft.trim()} onClick={() => void edit()}>保存修改</button></div></div>
          : <div className="knowledge-markdown"><ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: ({ children, href }) => <span>{children}{href ? ` (${href})` : ''}</span>, img: ({ alt }) => <span>[图片：{alt || '不加载外部图片'}]</span> }}>{bodyOf(selected.content) || '这份资料只有标题。'}</ReactMarkdown></div>}
        <footer className="knowledge-entry-meta"><span>更新于 {formatDate(selected.updatedAt)}</span><details className="knowledge-help"><summary>资料信息</summary><p>{selected.scope === 'global' ? '共享资料 · 所有对话' : '限定范围资料'} · {selected.forgetStage === 'active' ? '活跃' : selected.forgetStage === 'archived' ? '已归档' : selected.forgetStage === 'summarized' ? '已整理' : '状态待核对'}{selected.isPermanent ? ' · 长期保留' : ''}</p><p>创建于 {formatDate(selected.createdAt)} · {selected.source === 'user' ? '手动添加' : '其他来源'}</p></details></footer>
      </article>}
    </div>}
  </div>;
}

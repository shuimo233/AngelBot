import { useId, useMemo, useRef, useState } from 'react';
import { useSettingsStore } from '$stores/settings';
import { appendTavernNotes, PERSONA_NOTES_LIMIT, readTavernBook, type TavernBookEntry } from '$lib/tavern-card';

const PAGE_SIZE = 20;

function WorldbookExcerpt({ entry, notes, canAdd, onAdd }: {
  entry: TavernBookEntry;
  notes: string;
  canAdd: boolean;
  onAdd: (excerpt: string) => void;
}) {
  const sourceRef = useRef<HTMLDivElement>(null);
  const editButton = useRef<HTMLButtonElement>(null);
  const selectedBeforeClick = useRef('');
  const excerptId = useId();
  const budgetId = useId();
  const [excerpt, setExcerpt] = useState<string | null>(null);
  const excerptText = excerpt?.trim() ?? '';
  const used = notes.trim().length;
  const remaining = PERSONA_NOTES_LIMIT - used - (used ? 2 : 0) - `${entry.label}：\n`.length;

  const selectedSourceText = () => {
    const selection = window.getSelection();
    const source = sourceRef.current;
    return source?.contains(selection?.anchorNode ?? null) && source?.contains(selection?.focusNode ?? null)
      ? selection?.toString().trim() ?? '' : '';
  };
  const closeExcerpt = () => { setExcerpt(null); editButton.current?.focus(); };

  return <>
    <div ref={sourceRef} className="library-book-content">{entry.content}</div>
    {canAdd && <>
      <button type="button" ref={editButton} aria-expanded={excerpt !== null} aria-controls={excerpt !== null ? excerptId : undefined}
        onPointerDown={() => { selectedBeforeClick.current = selectedSourceText(); }}
        onClick={() => {
          if (excerpt !== null) closeExcerpt();
          else setExcerpt(selectedBeforeClick.current || selectedSourceText() || entry.content);
          selectedBeforeClick.current = '';
        }}>编辑摘录</button>
      {excerpt !== null && <div className="library-excerpt" id={excerptId}>
        <label>摘录<textarea rows={4} autoFocus value={excerpt} onChange={event => setExcerpt(event.target.value)} aria-describedby={budgetId} /></label>
        <p id={budgetId} className="library-catalog-note">{remaining >= excerptText.length
          ? `加入后还剩 ${remaining - excerptText.length} 字。`
          : `超出人设补充上限 ${excerptText.length - remaining} 字，请缩短摘录。`}</p>
        <div className="library-excerpt-actions">
          <button type="button" disabled={!excerptText || excerptText.length > remaining} onClick={() => { onAdd(excerptText); closeExcerpt(); }}>加入人设草稿</button>
          <button type="button" onClick={closeExcerpt}>取消摘录</button>
        </div>
      </div>}
    </>}
  </>;
}

export function PersonaLibraryPanel() {
  const profile = useSettingsStore(state => state.activeProfile);
  const draftProfile = useSettingsStore(state => state.profile);
  const updateProfile = useSettingsStore(state => state.updateProfile);
  const notes = draftProfile.personaNotes ?? '';
  const notesDirty = notes !== (profile.personaNotes ?? '');
  const notesStatusId = useId();
  const canAdd = draftProfile.characterCard?.rawData?.character_book === profile.characterCard?.rawData?.character_book;
  const displayName = profile.name.trim() || 'AngelBot';
  const book = useMemo(() => readTavernBook(profile.characterCard), [profile.characterCard]);
  const [query, setQuery] = useState('');
  const [page, setPage] = useState(0);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const detailRef = useRef<HTMLElement>(null);
  const filtered = useMemo(() => {
    const search = query.trim().toLowerCase();
    return (book?.entries ?? []).filter(entry => !search || [entry.label, entry.content].some(value => value.toLowerCase().includes(search)));
  }, [book, query]);
  const pages = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  const currentPage = Math.min(page, pages - 1);
  const visible = filtered.slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE);
  const selected = visible.find(entry => entry.sourceId === selectedId) ?? visible[0];
  const manage = () => window.dispatchEvent(new CustomEvent('open-settings', { detail: 'personality' }));
  const addExcerpt = (entry: TavernBookEntry, content: string) => {
    updateProfile({ personaNotes: appendTavernNotes(notes, [{ ...entry, content }]) });
  };
  const fields = [
    ['名字', displayName],
    ['相处方式', profile.bio || '未单独设置'],
    ...(profile.characterCard ? [['角色性格', profile.characterCard.personality || '未提供'], ['场景', profile.characterCard.scenario || '未提供']] : []),
    ['开场风格', profile.greeting || '未单独设置'],
  ];
  return <div className="library-persona">
    <div className="library-section-heading">
      <h2>人物卡与世界书</h2>
      <button type="button" className="library-primary-action" onClick={manage}>管理人物卡与人设</button>
    </div>
    <h3 className="library-persona-name">{displayName}</h3>
    <p className="library-catalog-note">{profile.characterCard ? '已导入人物卡' : '日常人设 · 未导入人物卡'} · {profile.bio.slice(0, 240) || '未单独设置相处方式'}{profile.bio.length > 240 && '…'}</p>
    <details className="library-persona-overview"><summary>性格、场景与开场风格</summary><dl className="library-persona-facts">{fields.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value.slice(0, 1200)}{value.length > 1200 && '…（完整设定见性格设置）'}</dd></div>)}</dl></details>
    <section className="library-book" aria-label="随卡世界书">
      <div className="library-section-heading"><h3>人设补充</h3>{notesDirty && <button type="button" onClick={manage}>前往性格设置保存</button>}</div>
      <textarea className="library-persona-notes" rows={4} aria-label="人设补充草稿" aria-describedby={notesStatusId}
        placeholder="添加角色背景、喜好或相处细节" value={notes} maxLength={PERSONA_NOTES_LIMIT}
        onChange={event => updateProfile({ personaNotes: event.target.value })} />
      <p id={notesStatusId} className="library-catalog-note" role="status">{notesDirty ? '未保存' : '当前生效'} · 剩余 {Math.max(0, PERSONA_NOTES_LIMIT - notes.length)} 字</p>
      <h3>随卡世界书原文</h3>
      {book ? <>
        <p className="library-catalog-note">{canAdd ? '可摘录条目中的文字，保存为对话背景。' : '人物卡有未保存的更换，请先在性格设置中保存或放弃。'}</p>
        <p className="library-catalog-note"><strong>{book.name}</strong><span>原卡 {book.totalEntries} 条 · 可预览 {book.entries.length} 条 · 另 {book.omittedEntries} 条仅随原卡保留</span></p>
        {book.entries.length > 0 ? <>
          <div className="library-catalog-toolbar"><input type="search" aria-label="搜索世界书条目" placeholder="搜索标题或内容…" value={query} onChange={event => { setQuery(event.target.value); setPage(0); setSelectedId(null); }} /><span className="library-catalog-count">{filtered.length} 条匹配</span></div>
          {selected ? <div className="library-book-browser">
            <div className="library-book-list" aria-label="世界书条目列表">{visible.map(entry => <button type="button" key={entry.sourceId} className="library-book-entry" aria-pressed={entry.sourceId === selected.sourceId} onClick={event => {
              setSelectedId(entry.sourceId);
              const detail = detailRef.current;
              const list = event.currentTarget.parentElement?.getBoundingClientRect();
              if (detail && list && list.width > 0 && detail.getBoundingClientRect().top >= list.bottom) { detail.scrollIntoView({ block: 'start' }); detail.focus({ preventScroll: true }); }
            }}><strong>{entry.label}</strong><span>{entry.content.slice(0, 80)}</span></button>)}</div>
            <article ref={detailRef} tabIndex={-1} className="library-book-detail" aria-label="世界书条目详情">
              <h4>{selected.label}</h4>
              <WorldbookExcerpt key={`${selected.sourceId}:${selected.content}`} entry={selected} notes={notes} canAdd={canAdd} onAdd={excerpt => addExcerpt(selected, excerpt)} />
              <p className="library-catalog-note">{book.name} · {selected.sourceId}</p>
            </article>
          </div> : <p className="library-empty-skills">没有匹配条目，试试其他关键词。</p>}
          {pages > 1 && <div className="library-book-pagination"><button type="button" disabled={currentPage === 0} onClick={() => { setPage(currentPage - 1); setSelectedId(null); }}>上一页</button><span>第 {currentPage + 1} / {pages} 页，每页最多 {PAGE_SIZE} 条</span><button type="button" disabled={currentPage + 1 >= pages} onClick={() => { setPage(currentPage + 1); setSelectedId(null); }}>下一页</button></div>}
        </> : <p className="library-empty-skills">没有当前版本可读取的条目；原始数据仍随卡保留。</p>}
      </> : <p className="library-empty-skills">暂无随卡世界书，可导入含世界书的人物卡。</p>}
    </section>
  </div>;
}

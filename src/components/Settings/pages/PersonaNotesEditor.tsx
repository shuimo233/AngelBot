import { useEffect, useMemo, useState } from 'react';
import { appendTavernNotes, PERSONA_NOTES_LIMIT, readTavernBook } from '$lib/tavern-card';
import type { TavernCharacterCard } from '$types';

const PAGE_SIZE = 20;

export function PersonaNotesEditor({ card, value, onChange, preview = false }: {
  card?: TavernCharacterCard; value: string; onChange: (value: string) => void; preview?: boolean;
}) {
  const book = useMemo(() => readTavernBook(card), [card]);
  const [query, setQuery] = useState('');
  const [page, setPage] = useState(0);
  const [selected, setSelected] = useState<string[]>([]);
  const [error, setError] = useState('');
  useEffect(() => { setSelected([]); setQuery(''); setPage(0); setError(''); }, [book]);
  const entries = book?.entries ?? [];
  const filtered = entries.filter(entry => `${entry.label}\n${entry.content}`.toLowerCase().includes(query.trim().toLowerCase()));
  const pages = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  const currentPage = Math.min(page, pages - 1);
  const label = preview ? '预览人设补充' : '人设补充';
  return <div className="persona-notes-editor">
    <label className="persona-notes-label">{label}<textarea rows={5} value={value} maxLength={PERSONA_NOTES_LIMIT}
      placeholder="补充希望保留的角色经历、喜好或相处细节；也可以从下面的世界书中挑选文字。"
      onChange={event => { onChange(event.target.value); setError(''); }} /></label>
    <p className="personality-note">剩余 {Math.max(0, PERSONA_NOTES_LIMIT - value.length)} 字 · 保存后作为对话背景{book && '，不执行世界书规则或写入个人记忆'}</p>
    {book && <details className="persona-notes-source"><summary>从随卡世界书挑选内容（{entries.length} 条可预览）</summary>
      <p className="personality-note">{book.name} · 原卡 {book.totalEntries} 条{book.omittedEntries > 0 && `（${book.omittedEntries} 条仅随原卡保留）`}</p>
      <input type="search" aria-label="筛选可选世界书条目" placeholder="搜索标题或内容…" value={query} onChange={event => { setQuery(event.target.value); setPage(0); }} />
      <div className="persona-notes-options" key={currentPage}>{filtered.slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE).map(entry => <div key={entry.sourceId}>
        <label><input type="checkbox" checked={selected.includes(entry.sourceId)} onChange={event => setSelected(previous => event.target.checked ? [...previous, entry.sourceId] : previous.filter(id => id !== entry.sourceId))} />{entry.label}</label>
        <details><summary>查看原文</summary><pre>{entry.content}</pre></details>
      </div>)}</div>
      <p className="personality-note">匹配 {filtered.length} 条 · 已选 {selected.length} 条</p>
      {pages > 1 && <div className="persona-notes-pagination">
        <button type="button" className="btn btn-secondary" disabled={currentPage === 0} onClick={() => setPage(currentPage - 1)}>上一页</button>
        <span>第 {currentPage + 1} / {pages} 页</span>
        <button type="button" className="btn btn-secondary" disabled={currentPage + 1 === pages} onClick={() => setPage(currentPage + 1)}>下一页</button>
      </div>}
      <button type="button" className="btn btn-secondary" disabled={selected.length === 0} onClick={() => {
        try { onChange(appendTavernNotes(value, entries.filter(entry => selected.includes(entry.sourceId)))); setSelected([]); setError(''); }
        catch (cause) { setError(cause instanceof Error ? cause.message : '无法加入所选内容'); }
      }}>将所选文字加入人设补充</button>
      {error && <p role="alert" className="personality-note">{error}</p>}
    </details>}
  </div>;
}

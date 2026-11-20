import { useCallback, useEffect, useId, useMemo, useState } from 'react';
import {
  deleteMemory,
  findRelatedMemories,
  getMemories,
  getMemoryHistory,
  replaceMemory,
  restoreReplacedMemory,
  setMemoryPaused,
  setMemoryPermanent,
  updateMemory,
  type MemoryReviewCandidate,
  type MemoryHistoryEntry,
  type RustMemory,
} from '$lib/commands/memory';
import { memoryCategoryLabel, memoryCategoryLabels, memoryScopeLabel } from '../../Memory/memoryPresentation';
import { SettingsDisclosureSummary } from '../components/SettingsSection';

type MemoryDraft = {
  scope: string;
  category: string;
  content: string;
  importance: number;
  isPermanent: boolean;
};

const stageOptions = ['all', 'active', 'paused', 'summarized', 'archived', 'replaced'];
const stageLabels: Record<string, string> = { all: '未被替代', active: '活跃', paused: '已暂停召回', summarized: '已摘要', archived: '已归档', replaced: '已替代' };
const sourceLabels = new Map([['user', '用户'], ['agent', '助手']]);
const sourceLabel = (source: string) => sourceLabels.get(source) ?? source;
const historyLabels = new Map([['pause', '暂停召回'], ['resume', '恢复召回'], ['supersede', '替代记忆'], ['restore_replaced', '撤销替代'], ['replacement_deleted', '替代记录已删除'], ['forget', '标记遗忘']]);

function toDraft(memory: RustMemory): MemoryDraft {
  return {
    scope: memory.scope,
    category: memory.category,
    content: memory.content,
    importance: memory.importance,
    isPermanent: memory.is_permanent,
  };
}

function formatDate(timestamp: number | null) {
  if (!timestamp) return '暂无';
  return new Date(timestamp * 1000).toLocaleString();
}

export function MemoryGovernanceSettings({ focusedMemoryId }: { focusedMemoryId?: string } = {}) {
  const instanceId = useId();
  const [memories, setMemories] = useState<RustMemory[]>([]);
  const [drafts, setDrafts] = useState<Record<string, MemoryDraft>>({});
  const [query, setQuery] = useState('');
  const [categoryFilter, setCategoryFilter] = useState('all');
  const [stageFilter, setStageFilter] = useState('all');
  const [selectedMemoryId, setSelectedMemoryId] = useState<string | null>(null);
  const [history, setHistory] = useState<Record<string, MemoryHistoryEntry[]>>({});
  const [candidates, setCandidates] = useState<Record<string, MemoryReviewCandidate[]>>({});
  const [loading, setLoading] = useState(false);
  const [restoringId, setRestoringId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const loadMemories = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const loaded = await getMemories(true);
      setMemories(loaded);
      setCandidates({});
      setHistory({});
      setSelectedMemoryId(null);
      setDrafts((prev) => {
        const next = { ...prev };
        for (const memory of loaded) {
          if (!memory.superseded_by) next[memory.id] = next[memory.id] ?? toDraft(memory);
        }
        return next;
      });
      return true;
    } catch (err) {
      setError(typeof err === 'string' ? err : err instanceof Error ? err.message : '无法读取记忆');
      return false;
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadMemories();
  }, [loadMemories]);

  const categories = useMemo(() => {
    return Array.from(new Set(memories.map((m) => m.category))).sort();
  }, [memories]);

  const filteredMemories = useMemo(() => {
    const q = query.trim().toLowerCase();
    return memories.filter((memory) => {
      if (focusedMemoryId) return memory.id === focusedMemoryId;
      if (categoryFilter !== 'all' && memory.category !== categoryFilter) return false;
      const stage = memory.superseded_by ? 'replaced' : memory.forget_stage;
      if (stageFilter === 'all' ? stage === 'replaced' : stage !== stageFilter) return false;
      if (!q) return true;
      return [memory.content, memory.category, memoryCategoryLabel(memory.category), memory.scope, memoryScopeLabel(memory.scope), memory.source]
        .some((value) => value.toLowerCase().includes(q));
    });
  }, [categoryFilter, focusedMemoryId, memories, query, stageFilter]);

  const updateDraft = useCallback((id: string, patch: Partial<MemoryDraft>) => {
    setDrafts((prev) => ({
      ...prev,
      [id]: { ...(prev[id] ?? { scope: 'global', category: 'fact', content: '', importance: 5, isPermanent: false }), ...patch },
    }));
  }, []);

  const saveDraft = useCallback(async (memory: RustMemory) => {
    const draft = drafts[memory.id] ?? toDraft(memory);
    setError(null);
    await updateMemory({
      id: memory.id,
      scope: draft.scope,
      category: draft.category,
      content: draft.content,
      importance: draft.importance,
      isPermanent: draft.isPermanent,
    });
    await loadMemories();
  }, [drafts, loadMemories]);

  const togglePermanent = useCallback(async (memory: RustMemory) => {
    setError(null);
    await setMemoryPermanent(memory.id, !memory.is_permanent);
    setDrafts((prev) => ({
      ...prev,
      [memory.id]: { ...(prev[memory.id] ?? toDraft(memory)), isPermanent: !memory.is_permanent },
    }));
    await loadMemories();
  }, [loadMemories]);

  const removeMemory = useCallback(async (id: string) => {
    setError(null);
    await deleteMemory(id);
    await loadMemories();
  }, [loadMemories]);

  const loadHistory = useCallback(async (id: string) => {
    setSelectedMemoryId((current) => current === id ? null : id);
    if (history[id]) return;
    setError(null);
    const entries = await getMemoryHistory(id);
    setHistory((prev) => ({ ...prev, [id]: entries }));
  }, [history]);

  const checkRelated = async (memory: RustMemory) => {
    setError(null);
    const found = await findRelatedMemories(memory.id);
    setCandidates((prev) => ({ ...prev, [memory.id]: found }));
  };

  const replaceWithCurrent = async (source: MemoryReviewCandidate, target: RustMemory) => {
    setError(null);
    await replaceMemory({
      sourceId: source.memory_id,
      sourceContent: source.content,
      targetId: target.id,
      targetContent: target.content,
    });
    await loadMemories();
  };

  const togglePause = async (memory: RustMemory) => {
    setError(null);
    await setMemoryPaused(memory.id, memory.forget_stage === 'active');
    await loadMemories();
  };

  const restoreReplacement = async (id: string, replacementId: string) => {
    setError(null);
    setRestoringId(id);
    try {
      await restoreReplacedMemory(id, replacementId);
      if (await loadMemories()) {
        setStageFilter((stage) => stage === 'replaced' ? 'paused' : stage);
      }
    } finally {
      setRestoringId(null);
    }
  };

  const runAction = (action: Promise<void>) => {
    void action.catch(cause => {
      const message = typeof cause === 'string' ? cause : cause instanceof Error ? cause.message : '';
      setError(message.trim() ? message : '操作未完成，请核对后重试。');
    });
  };

  return (
    <div className="memory-governance">
      {!focusedMemoryId && <div className="memory-summary-row">
        <span role="status">{filteredMemories.length} 条显示中 · {memories.filter((m) => m.is_permanent && !m.superseded_by).length} 条长期保留</span>
        <button type="button" className="ui-button ui-button--ghost" onClick={loadMemories} disabled={loading}>
          {loading ? '加载中' : '刷新'}
        </button>
      </div>}
      {!focusedMemoryId && <div className="memory-toolbar">
        <input
          className="memory-search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="搜索记忆"
          aria-label="搜索记忆"
        />
        <select value={categoryFilter} onChange={(e) => setCategoryFilter(e.target.value)} aria-label="按类别筛选">
          <option value="all">全部分类</option>
          {categories.map((category) => (
            <option key={category} value={category}>{memoryCategoryLabel(category)}</option>
          ))}
        </select>
        <select value={stageFilter} onChange={(e) => setStageFilter(e.target.value)} aria-label="按阶段筛选">
          {stageOptions.map((stage) => (
            <option key={stage} value={stage}>{stageLabels[stage]}</option>
          ))}
        </select>
      </div>}
      {focusedMemoryId && loading && <p role="status">正在读取记忆…</p>}

      {error && <p className="memory-error" role="alert">{error}</p>}

      <div className="memory-list">
        {filteredMemories.map((memory) => {
          const recordId = instanceId + '-' + memory.id;
          const replacementId = memory.superseded_by;
          if (replacementId) {
            const replacement = memories.find((record) => record.id === replacementId);
            return (
              <section className="memory-item" key={memory.id}>
                <div className="memory-item-header">
                  <div className="memory-badges">
                    <span className="memory-badge">{memoryCategoryLabel(memory.category)}</span>
                    <span className="memory-badge muted">{memoryScopeLabel(memory.scope)}</span>
                    <span className="memory-badge muted">已替代</span>
                  </div>
                  <div className="memory-meta">来源 {sourceLabel(memory.source)}</div>
                </div>
                <label className="memory-content-label" htmlFor={recordId + '-original'}>原记录</label>
                <textarea id={recordId + '-original'} className="memory-content-input" value={memory.content} readOnly rows={3} />
                {replacement && (
                  <details className="memory-record-tools">
                    <summary>查看替代记录</summary>
                    <label className="memory-content-label" htmlFor={recordId + '-replacement'}>替代记录的当前内容</label>
                    <textarea id={recordId + '-replacement'} className="memory-content-input" value={replacement.content} readOnly rows={3} />
                  </details>
                )}
                <div className="memory-actions">
                  <button className="btn btn-secondary btn-sm" disabled={restoringId !== null || loading} onClick={() => runAction(restoreReplacement(memory.id, replacementId))}>
                    {restoringId === memory.id ? '正在撤销替代' : '撤销替代'}
                  </button>
                  <span>原记录会移到「已暂停召回」。需要再次使用时，可手动恢复召回。</span>
                </div>
              </section>
            );
          }
          const draft = drafts[memory.id] ?? toDraft(memory);
          const memoryHistory = history[memory.id] ?? [];
          const related = candidates[memory.id];
          const active = memory.forget_stage === 'active';
          const paused = memory.forget_stage === 'paused';
          const dirty = draft.content !== memory.content || draft.scope !== memory.scope
            || draft.category !== memory.category || draft.importance !== memory.importance || draft.isPermanent !== memory.is_permanent;
          return (
            <section className="memory-item" key={memory.id}>
              <div className="memory-item-header">
                <div className="memory-badges">
                  <span className="memory-badge">{memoryCategoryLabel(memory.category)}</span>
                  <span className="memory-badge muted">{memoryScopeLabel(memory.scope)}</span>
                  <span className="memory-badge muted">{stageLabels[memory.forget_stage] ?? memory.forget_stage}</span>
                </div>
                <div className="memory-meta">
                  <span>来源 {sourceLabel(memory.source)}</span>
                  <span>已引用 {memory.frequency} 次</span>
                  <span>更新于 {formatDate(memory.updated_at)}</span>
                </div>
              </div>

              <label className="memory-content-label" htmlFor={recordId + '-content'}>要记住的内容</label>
              <textarea
                id={recordId + '-content'}
                className="memory-content-input"
                value={draft.content}
                onChange={(e) => updateDraft(memory.id, { content: e.target.value })}
                rows={3}
              />

              <div className="memory-record-footer">
                <label className="memory-check">
                  <input type="checkbox" aria-label="长期保留" aria-describedby={recordId + '-retention'} checked={draft.isPermanent} onChange={(e) => updateDraft(memory.id, { isPermanent: e.target.checked })} />
                  <span>长期保留<small id={recordId + '-retention'}>不参与自动衰减，仍按问题需要检索。</small></span>
                </label>
                <div className="memory-save-area">
                  <span className="memory-save-state" aria-live="polite">{dirty ? '有未保存的修改' : '已保存'}</span>
                  {focusedMemoryId && (active || paused) && (
                    <button className="btn btn-secondary btn-sm" disabled={loading} onClick={() => runAction(togglePause(memory))}>
                      {paused ? '恢复使用这条记忆' : '暂时不用这条记忆'}
                    </button>
                  )}
                  <button className="btn btn-primary btn-sm" onClick={() => runAction(saveDraft(memory))}>保存</button>
                </div>
              </div>

              <details className="settings-disclosure memory-record-tools">
                <SettingsDisclosureSummary title="更多设置与操作" />
                <div className="memory-edit-grid">
                  <label>
                    适用范围标识
                    <input aria-label="适用范围标识" aria-describedby={recordId + '-scope'} value={draft.scope} onChange={(e) => updateDraft(memory.id, { scope: e.target.value })} />
                    <small id={recordId + '-scope'}>global 表示所有对话；其他值是限定范围的标识。修改此处不会改变文件或操作权限。</small>
                  </label>
                  <label>
                    分类标识
                    <input aria-label="分类标识" aria-describedby={recordId + '-category'} list={recordId + '-categories'} value={draft.category} onChange={(e) => updateDraft(memory.id, { category: e.target.value })} />
                    <datalist id={recordId + '-categories'}>{Object.entries(memoryCategoryLabels).map(([value, label]) => <option key={value} value={value} label={label} />)}</datalist>
                    <small id={recordId + '-category'}>当前为“{memoryCategoryLabel(draft.category)}”。可保留或输入自定义分类，不会自动转换原值。</small>
                  </label>
                  <label>
                    重要度
                    <input
                      aria-label="重要度"
                      aria-describedby={recordId + '-importance'}
                      type="number"
                      min={1}
                      max={10}
                      value={draft.importance}
                      onChange={(e) => updateDraft(memory.id, { importance: Number(e.target.value) })}
                    />
                    <small id={recordId + '-importance'}>1–10，用于记忆排序，越大权重越高；不是准确率，也不保证每轮引用。</small>
                  </label>
                </div>
                <p>此处操作立即生效，编辑内容需另行保存。</p>
                <div className="memory-actions">
                  <button className="btn btn-secondary btn-sm" onClick={() => runAction(togglePermanent(memory))}>
                    {memory.is_permanent ? '立即取消长期保留' : '立即设为长期保留'}
                  </button>
                  <button className="btn btn-secondary btn-sm" onClick={() => runAction(loadHistory(memory.id))}>历史记录</button>
                  {!focusedMemoryId && (active || paused) && (
                    <button className="btn btn-secondary btn-sm" onClick={() => runAction(togglePause(memory))}>
                      {paused ? '恢复召回' : '暂停召回'}
                    </button>
                  )}
                  {active && <button className="btn btn-secondary btn-sm" disabled={dirty} onClick={() => runAction(checkRelated(memory))}>查找相关记忆</button>}
                </div>
                <details className="settings-disclosure memory-delete">
                  <SettingsDisclosureSummary title="删除这条记忆" description="删除后无法在记忆页撤销。" tone="danger" />
                  <button className="btn btn-danger btn-sm" onClick={() => runAction(removeMemory(memory.id))}>确认删除</button>
                </details>

                {selectedMemoryId === memory.id && (
                  <div className="memory-history">
                    {memoryHistory.length === 0 ? (
                      <p>暂无历史记录。</p>
                    ) : memoryHistory.map((entry) => (
                      <div className="memory-history-entry" key={entry.id}>
                        <strong>{historyLabels.get(entry.operation) ?? entry.operation}</strong>
                        <span>{formatDate(entry.createdAt)}</span>
                        {entry.details && <p>{entry.details}</p>}
                      </div>
                    ))}
                  </div>
                )}

                {active && !dirty && related && (
                  <div className="memory-conflicts">
                    <p>近期同范围、同分类的文字匹配结果。请核对含义。</p>
                    {related.length === 0 && <p>未找到相关候选。</p>}
                    {related.map((candidate) => (
                      <div className="memory-conflict" key={candidate.memory_id}>
                        <div>
                          <strong>来源 {sourceLabel(candidate.source)} · {formatDate(candidate.updated_at)}</strong>
                          <p>{candidate.content}</p>
                        </div>
                        <details>
                          <summary>用当前记忆替代此条</summary>
                          <p>以下当前记录会继续参与召回。候选记录的原文将保留在数据库中，并退出召回。</p>
                          <blockquote>{memory.content}</blockquote>
                          <button className="btn btn-secondary btn-sm" onClick={() => runAction(replaceWithCurrent(candidate, memory))}>确认替代</button>
                        </details>
                      </div>
                    ))}
                  </div>
                )}
              </details>
            </section>
          );
        })}
      </div>

      {!loading && !error && filteredMemories.length === 0 && (
        <div className="memory-empty">{focusedMemoryId ? '这条记忆已不在列表中。' : '没有符合当前筛选条件的记忆。'}</div>
      )}
    </div>
  );
}

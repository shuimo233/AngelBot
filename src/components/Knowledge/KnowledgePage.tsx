import { useEffect, useState } from 'react';
import { useKnowledgeStore } from '$stores/knowledge';
import { saveMemory } from '$lib/commands/memory';
import './KnowledgePage.css';

function formatDate(ts: number) {
  return new Date(ts * 1000).toLocaleDateString();
}

export function KnowledgePage() {
  const { entries, isLoading, error, query, setQuery, loadKnowledge } = useKnowledgeStore();
  const [showAddForm, setShowAddForm] = useState(false);
  const [draft, setDraft] = useState('');
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  useEffect(() => { loadKnowledge(); }, [loadKnowledge]);

  const filtered = query.trim()
    ? entries.filter((e) => e.content.toLowerCase().includes(query.toLowerCase()))
    : entries;

  const handleAdd = async () => {
    const content = draft.trim();
    if (!content || saving) return;
    setSaving(true);
    setSaveError(null);
    try {
      await saveMemory({
        id: crypto.randomUUID(),
        scope: 'global',
        category: 'knowledge',
        content,
        importance: 3,
        source: 'user',
        userConfirmed: true,
      });
      setDraft('');
      setShowAddForm(false);
      await loadKnowledge();
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : '保存失败，请重试。');
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="knowledge-page">
      <div className="knowledge-header">
        <h2 className="knowledge-page-title">资料</h2>
        <div className="knowledge-header-actions">
          <div className="knowledge-search-bar">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" className="knowledge-search-icon">
              <circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/>
            </svg>
            <input
              className="knowledge-search-input"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="搜索资料…"
            />
          </div>
          <button
            type="button"
            className="knowledge-add-button"
            onClick={() => { setShowAddForm((v) => !v); setSaveError(null); }}
            aria-expanded={showAddForm}
          >
            {showAddForm ? '取消' : '添加资料'}
          </button>
        </div>
      </div>

      {showAddForm && (
        <div className="knowledge-add-form">
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder="把希望 AngelBot 长期引用的内容写在这里，例如项目约定、常用资料摘要…"
            rows={4}
            autoFocus
          />
          <div className="knowledge-add-form-footer">
            <span className="knowledge-add-hint">资料会保存在本地，并在相关对话中被引用。</span>
            <button type="button" onClick={handleAdd} disabled={!draft.trim() || saving}>
              {saving ? '保存中…' : '保存资料'}
            </button>
          </div>
          {saveError && <p className="knowledge-add-error" role="alert">{saveError}</p>}
        </div>
      )}

      {error && <div className="knowledge-error">{error}</div>}

      <div className="knowledge-content">
        {isLoading ? (
          <div className="knowledge-empty knowledge-loading">加载中…</div>
        ) : filtered.length === 0 ? (
          <div className="knowledge-empty">
            {query
              ? '没有找到匹配的资料'
              : '还没有资料。点击右上角「添加资料」，把希望 AngelBot 长期引用的内容放进来。'}
          </div>
        ) : (
          <div className="knowledge-list">
            {filtered.map((entry) => (
              <div key={entry.id} className="knowledge-entry">
                <div className="knowledge-entry-header">
                  <span className="knowledge-entry-category">{entry.category}</span>
                  <span className="knowledge-entry-date">{formatDate(entry.updatedAt)}</span>
                </div>
                <div className="knowledge-entry-content">{entry.content}</div>
                <div className="knowledge-entry-footer">
                  <span className="knowledge-entry-source">{entry.source === 'user' ? '手动添加' : entry.source}</span>
                  <span className="knowledge-entry-importance">重要度 {Math.min(entry.importance, 5)}/5</span>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

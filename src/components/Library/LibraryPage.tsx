import { useId, useState } from 'react';
import { KnowledgePage } from '../Knowledge/KnowledgePage';
import { MemoryPage } from '../Memory/MemoryPage';
import { ActivityPage } from './ActivityPage';
import { ExtensionsPanel } from './ExtensionsPanel';
import { PersonaLibraryPanel } from './PersonaLibraryPanel';
import './LibraryPage.css';

type LibraryTab = 'knowledge' | 'persona' | 'memory' | 'skills' | 'connections' | 'activity';
const tabs: Array<{ id: LibraryTab; label: string }> = [
  { id: 'knowledge', label: '参考资料' },
  { id: 'persona', label: '人物卡与世界书' },
  { id: 'memory', label: '关于我的记忆' },
  { id: 'skills', label: '技能' },
  { id: 'connections', label: '应用连接' },
  { id: 'activity', label: '使用记录' },
];

export function LibraryPage() {
  const [tab, setTab] = useState<LibraryTab>('knowledge');
  const tabId = useId();
  return <main className="library-page" aria-label="资料库">
    <div className="library-layout">
      <header className="library-header">
        <h1>资料库</h1>
      </header>
      <div className="library-navigation" role="tablist" aria-orientation="horizontal" aria-label="资料库分类">
        {tabs.map(({ id, label }) => <div key={id} className="library-navigation-entry">
          <button id={tabId + '-' + id} type="button" role="tab" tabIndex={tab === id ? 0 : -1} aria-selected={tab === id} aria-controls={tabId + '-panel'} onClick={() => setTab(id)} onKeyDown={event => {
            const index = tabs.findIndex(item => item.id === id);
            const next = ['ArrowDown', 'ArrowRight'].includes(event.key) ? (index + 1) % tabs.length
              : ['ArrowUp', 'ArrowLeft'].includes(event.key) ? (index + tabs.length - 1) % tabs.length
              : event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : null;
            if (next === null) return;
            event.preventDefault(); setTab(tabs[next].id);
            document.getElementById(tabId + '-' + tabs[next].id)?.focus();
          }}><span>{label}</span></button>
        </div>)}
      </div>
      <section className="library-content" role="tabpanel" id={tabId + '-panel'} aria-labelledby={tabId + '-' + tab}>
        {/* Retain unsaved reference drafts; all other views load only on entry. */}
        <div hidden={tab !== 'knowledge'}><KnowledgePage /></div>
        {tab === 'persona' && <PersonaLibraryPanel />}
        {tab === 'memory' && <MemoryPage />}
        {tab === 'activity' && <div className="library-records"><div className="library-section-heading"><h2>使用记录</h2></div><ActivityPage /></div>}
        {(tab === 'skills' || tab === 'connections') && <ExtensionsPanel key={tab} kind={tab === 'skills' ? 'skills' : 'connections'} />}
      </section>
    </div>
  </main>;
}

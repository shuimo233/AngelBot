import { useId, useState } from 'react';
import { KnowledgePage } from '../Knowledge/KnowledgePage';
import { MemoryPage } from '../Memory/MemoryPage';
import { ActivityPage } from './ActivityPage';
import { ExtensionsPanel } from './ExtensionsPanel';
import './LibraryPage.css';

type LibraryTab = 'knowledge' | 'memory' | 'skills' | 'connections' | 'activity';
const tabs: Array<{ id: LibraryTab; label: string; hint: string; group?: string }> = [
  { id: 'knowledge', label: '参考资料', hint: '我主动保存的内容', group: '内容与记忆' },
  { id: 'memory', label: '关于我的记忆', hint: '交流中保留的偏好和事实' },
  { id: 'skills', label: '技能', hint: '教助手如何做事', group: '助手能力' },
  { id: 'connections', label: '应用连接', hint: '让助手使用外部工具' },
  { id: 'activity', label: '使用记录', hint: '回看任务结果与记忆变更', group: '记录' },
];

export function LibraryPage() {
  const [tab, setTab] = useState<LibraryTab>('knowledge');
  const tabId = useId();
  return <main className="library-page" aria-label="资料库">
    <header className="library-header">
      <div className="library-heading"><h1>资料库</h1><p>保存常用内容，了解助手记住了什么、能使用哪些能力。</p></div>
    </header>
    <div className="library-layout">
      <div className="library-navigation" role="tablist" aria-orientation="vertical" aria-label="资料库分类">
        {tabs.map(({ id, label, hint, group }) => <div key={id} className="library-navigation-entry">
          {group && <p className="library-navigation-group" aria-hidden="true">{group}</p>}
          <button id={tabId + '-' + id} type="button" role="tab" aria-label={label} aria-describedby={tabId + '-' + id + '-hint'} tabIndex={tab === id ? 0 : -1} aria-selected={tab === id} aria-controls={tabId + '-panel'} onClick={() => setTab(id)} onKeyDown={event => {
            const index = tabs.findIndex(item => item.id === id);
            const next = ['ArrowDown', 'ArrowRight'].includes(event.key) ? (index + 1) % tabs.length
              : ['ArrowUp', 'ArrowLeft'].includes(event.key) ? (index + tabs.length - 1) % tabs.length
              : event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : null;
            if (next === null) return;
            event.preventDefault(); setTab(tabs[next].id);
            document.getElementById(tabId + '-' + tabs[next].id)?.focus();
          }}><span>{label}</span><small id={tabId + '-' + id + '-hint'}>{hint}</small></button>
        </div>)}
      </div>
      <section className="library-content" role="tabpanel" id={tabId + '-panel'} aria-labelledby={tabId + '-' + tab}>
        {/* Retain unsaved reference drafts; all other views load only on entry. */}
        <div hidden={tab !== 'knowledge'}><KnowledgePage /></div>
        {tab === 'memory' && <MemoryPage />}
        {tab === 'activity' && <div className="library-records"><div className="library-section-heading"><div><h2>使用记录</h2><p>回看任务的完成、失败结果，以及记忆的保存与修改。</p></div></div><ActivityPage /></div>}
        {(tab === 'skills' || tab === 'connections') && <ExtensionsPanel key={tab} kind={tab === 'skills' ? 'skills' : 'connections'} />}
      </section>
    </div>
  </main>;
}

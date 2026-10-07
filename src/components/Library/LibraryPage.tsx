import { useCallback, useEffect, useState } from 'react';
import { KnowledgePage } from '../Knowledge/KnowledgePage';
import { MemoryPage } from '../Memory/MemoryPage';
import { ActivityPage } from './ActivityPage';
import { getSkills, type SkillManifest } from '$lib/commands/skill';
import './LibraryPage.css';

type LibraryTab = 'knowledge' | 'memory' | 'activity' | 'skills';

const tabs: Array<{ id: LibraryTab; label: string }> = [
  { id: 'knowledge', label: '资料' },
  { id: 'memory', label: '记忆' },
  { id: 'activity', label: '活动' },
  { id: 'skills', label: '技能' },
];

export function LibraryPage() {
  const [tab, setTab] = useState<LibraryTab>('knowledge');
  const [skills, setSkills] = useState<SkillManifest[]>([]);
  const [skillsError, setSkillsError] = useState<string | null>(null);

  const refreshSkills = useCallback(() => {
    setSkillsError(null);
    getSkills().then(setSkills).catch(() => setSkillsError('无法加载已添加的技能。请稍后重试。'));
  }, []);

  useEffect(() => {
    if (tab === 'skills') refreshSkills();
  }, [tab, refreshSkills]);

  return (
    <main className="library-page" aria-label="资料库">
      <header className="library-header">
        <div className="library-heading"><h1>资料库</h1><p>资料、记忆和已添加的技能。</p></div>
        <div className="library-tabs" role="tablist" aria-label="资料库分类">
          {tabs.map(({ id, label }) => (
            <button key={id} type="button" role="tab" aria-selected={tab === id} onClick={() => setTab(id)}>{label}</button>
          ))}
        </div>
      </header>

      {tab === 'skills' && (
        <div className="library-section-heading">
          <h2>已添加的技能</h2>
          <div className="library-section-actions">
            <span className="library-section-count">{skills.length} 项</span>
          </div>
        </div>
      )}

      <section className="library-content" role="tabpanel">
        {tab === 'knowledge' && <KnowledgePage />}
        {tab === 'memory' && <MemoryPage />}
        {tab === 'activity' && <ActivityPage />}
        {tab === 'skills' && (
          <div className="library-skills">
            {skillsError && <p className="library-skills-error" role="alert">{skillsError}</p>}
            {skills.length === 0 && !skillsError ? (
              <section className="library-empty-skills">
                <strong>还没有已添加的技能</strong>
                <p>在 Agent Mode 中发送公开 GitHub 仓库链接即可安装；按名称安装将在 Web Search 配置完成后可用。安装完成后会显示在这里。</p>
              </section>
            ) : (
              <div className="library-skill-grid">
                {skills.map((skill) => (
                  <article key={skill.id} className="library-skill-card">
                    <div className="library-skill-card-title"><strong>{skill.name}</strong><span>v{skill.version}</span></div>
                    <p>{skill.description || '未提供说明'}</p>
                    <footer><span>{skill.actions.length} 个动作</span><span>{skill.permissions.length ? `${skill.permissions.length} 项权限声明` : '无权限声明'}</span></footer>
                  </article>
                ))}
              </div>
            )}
          </div>
        )}
      </section>
    </main>
  );
}

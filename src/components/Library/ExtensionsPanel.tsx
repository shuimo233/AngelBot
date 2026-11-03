import { useCallback, useEffect, useRef, useState } from 'react';
import { getSkills, type SkillManifest } from '$lib/commands/skill';
import { getMcpServers } from '$lib/commands/mcp';
import { useWorkspacesStore } from '$stores/workspaces';
import type { McpServer } from '$types';

/** Browsing one catalogue never starts an external process or loads the other. */
export function ExtensionsPanel({ kind }: { kind: 'skills' | 'connections' }) {
  const [skills, setSkills] = useState<SkillManifest[]>([]);
  const [servers, setServers] = useState<McpServer[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [query, setQuery] = useState('');
  const [showAddHelp, setShowAddHelp] = useState(false);
  const generation = useRef(0);
  const workspaces = useWorkspacesStore(state => state.workspaces);
  const activeId = useWorkspacesStore(state => state.activeWorkspaceId);
  const active = workspaces.find(workspace => workspace.id === activeId);
  const isSkills = kind === 'skills';
  const refresh = useCallback(() => {
    const current = ++generation.current;
    setLoading(true); setError('');
    const request = kind === 'skills'
      ? getSkills().then(items => { if (current === generation.current) setSkills(items); })
      : getMcpServers().then(items => { if (current === generation.current) setServers(items); });
    void request.catch(() => { if (current === generation.current) setError(kind === 'skills' ? '无法加载已添加的技能。请稍后重试。' : '无法读取应用连接，请重新加载。'); })
      .finally(() => { if (current === generation.current) setLoading(false); });
  }, [kind]);
  useEffect(() => { refresh(); return () => { generation.current += 1; }; }, [refresh]);
  const search = query.trim().toLowerCase();
  const visibleSkills = skills.filter(skill => [skill.name, skill.description, ...skill.dependencies, ...skill.actions.map(action => action.name)].join('\n').toLowerCase().includes(search));
  const visibleServers = servers.filter(server => server.name.toLowerCase().includes(search));
  const manage = () => window.dispatchEvent(new CustomEvent('open-settings', { detail: 'mcp' }));
  const status = (server: McpServer) => {
    if (!server.enabled) return '已停用';
    if (server.envUnavailable) return '凭据需要处理';
    if (activeId && !server.enabledWorkspaceIds.includes(activeId)) return '当前空间未启用';
    if (!server.enabledWorkspaceIds.length) return '尚未选择使用范围';
    if (server.catalogError) return '需要重新检查配置';
    return server.catalogReady ? '已设置 · 按需连接' : '等待检查配置';
  };
  const count = isSkills ? visibleSkills.length : visibleServers.length;

  return <div className="library-extensions">
    <div className="library-section-heading">
      <div><h2>{isSkills ? '技能' : '应用连接'}</h2><p>{isSkills ? '让助手学会一种做事方法，比如整理资料或核对文档。' : '让助手使用浏览器、笔记等外部工具。'}</p></div>
      {isSkills ? <button type="button" className="library-primary-action" aria-expanded={showAddHelp} onClick={() => setShowAddHelp(open => !open)}>如何添加技能</button> : <button type="button" className="library-primary-action" onClick={manage}>管理连接</button>}
    </div>
    {showAddHelp && isSkills && <section className="library-inline-guide" aria-label="添加技能说明"><h3>在对话中添加，不必手动配置文件</h3><ol><li>找到想使用的技能，复制公开 GitHub 仓库链接。</li><li>在主对话中说：“请先检查这个技能是否适合，再帮我添加。”并附上链接。</li><li>核对来源和用途，确认添加后回到这里刷新列表。</li></ol><p>也可以直接描述想补充的能力；按名称查找需要先配置联网搜索。</p></section>}
    <div className="library-catalog-toolbar">
      <input type="search" aria-label={isSkills ? '搜索技能' : '搜索应用连接'} placeholder={isSkills ? '搜索技能名称或用途…' : '搜索连接名称…'} value={query} onChange={event => setQuery(event.target.value)} />
      <span className="library-catalog-count">{loading ? '正在读取…' : error ? '读取失败' : count + (isSkills ? ' 个技能' : ' 个连接')}</span>
      <button type="button" disabled={loading} onClick={refresh}>刷新列表</button>
    </div>
    {!isSkills && active && <p className="library-catalog-note"><span>当前空间</span><strong>{active.name}</strong></p>}
    {error && <div className="library-skills-error" role="alert"><p>{error}</p><button type="button" onClick={refresh}>重新加载</button></div>}
    {loading && <p role="status">{isSkills ? '正在读取已添加的技能…' : '正在读取连接配置…'}</p>}
    {!loading && !error && isSkills && (!skills.length ? <section className="library-empty-skills"><h3>还没有已添加的技能</h3><p>点击“如何添加技能”，从常做的一件事开始。</p></section>
      : !visibleSkills.length ? <p className="library-empty-skills">没有匹配的技能，试试其他关键词。</p> : <div className="library-extension-list">{visibleSkills.map(skill => <details key={skill.id} className="library-extension-row">
        <summary><span className="library-extension-copy"><strong>{skill.name}</strong><span>{skill.description || '暂未提供用途说明'}</span></span><span className="library-row-affordance">查看详情</span></summary>
        <div className="library-extension-detail">
          <dl><dt>版本</dt><dd>{skill.version}</dd><dt>标识</dt><dd>{skill.id}</dd><dt>作者</dt><dd>{skill.author || '未声明'}</dd><dt>依赖声明</dt><dd>{skill.dependencies.join('、') || '无'}</dd><dt>权限声明</dt><dd>{skill.permissions.join('、') || '无'}</dd></dl>
          <p>声明不是授权或兼容证明。不会自动安装依赖；实际调用仍受已有工具与空间权限限制。</p>
          {skill.actions.map(action => <div key={action.name} className="library-action-description"><strong>{action.name}</strong><p>{action.description || '未提供动作说明'}</p><span>处理器声明：{action.handler || '无'}</span></div>)}
          {skill.prompts?.map((prompt, index) => <details key={prompt.name + '-' + index}><summary>模板：{prompt.name || '未命名'}</summary><pre>{prompt.content}</pre></details>)}
          {!skill.prompts?.length && <p>此技能未提供可预览的结构化模板；原文件可能仍有其他正文。</p>}
        </div>
      </details>)}</div>)}
    {!loading && !error && !isSkills && (!servers.length ? <section className="library-empty-skills"><h3>还没有应用连接</h3><p>在“管理连接”中添加工具，再选择使用空间。</p></section>
      : !visibleServers.length ? <p className="library-empty-skills">没有匹配的连接，试试其他关键词。</p> : <div className="library-extension-list">{visibleServers.map(server => <details key={server.id} className="library-extension-row">
        <summary><span className="library-extension-copy"><strong>{server.name}</strong><span>{status(server)}</span></span><span className="library-row-affordance">使用范围</span></summary>
        <div className="library-extension-detail"><p>启用范围：{server.enabledWorkspaceIds.map(id => workspaces.find(workspace => workspace.id === id)?.name || '未加载的空间').join('、') || '未选择'}</p><p>这里显示已保存的配置与工具目录，不代表服务正在运行。查看本页不会连接、启动或授权服务。</p><button type="button" onClick={manage}>打开连接设置</button></div>
      </details>)}</div>)}
    <details className="library-compatibility">
      <summary>{isSkills ? '技能说明' : '连接说明'}</summary>
      {isSkills ? <dl className="library-help-facts">
        <dt>技能用途</dt><dd>提供做事步骤和提示，不是可独立运行的应用。</dd>
        <dt>兼容格式</dt><dd>可读取公开 GitHub 仓库中的 SKILL.md，以及部分 AGENTS.md、CLAUDE.md、.cursorrules 文字；不保证完整适配所有格式。</dd>
        <dt>工具与权限</dt><dd>导入文字不会带入原助手的工具、脚本执行器或权限。需要外部工具时，在“应用连接”中单独配置。</dd>
      </dl> : <dl className="library-help-facts">
        <dt>接入方式</dt><dd>通过 MCP 接入浏览器、笔记等外部工具。</dd>
        <dt>运行状态</dt><dd>保存配置不代表服务正在运行，查看本页不会连接或启动服务。</dd>
        <dt>操作权限</dt><dd>实际调用时会复核可用性、使用空间与操作权限；已有配置不代表已授权所有操作。</dd>
      </dl>}
    </details>
  </div>;
}

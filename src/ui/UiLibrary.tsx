import { useEffect, useState, type ReactNode } from 'react';
import { initializeTheme, useThemeStore, type Theme } from '../stores/theme';
import { Button, IconButton, Dialog, TextField, SelectField, Switch } from './index';

function Glyph({ kind }: { kind: 'folder' | 'plus' | 'arrow' | 'close' | 'check' | 'layout' | 'control' | 'type' }) {
  const paths: Record<typeof kind, ReactNode> = {
    folder: <path d="M3 7h6l2 2h10v11H3V7Zm0 0V4h6l2 3" />,
    plus: <path d="M12 5v14M5 12h14" />,
    arrow: <path d="M5 12h14m-6-6 6 6-6 6" />,
    close: <path d="m6 6 12 12M6 18 18 6" />,
    check: <path d="m5 12 4 4L19 6" />,
    layout: <><rect x="3" y="4" width="18" height="16" rx="2" /><path d="M9 4v16M9 9h12" /></>,
    control: <><path d="M4 7h16M4 17h16" /><circle cx="9" cy="7" r="2" /><circle cx="15" cy="17" r="2" /></>,
    type: <path d="M4 5h16M12 5v15M8 20h8" />,
  };
  return <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{paths[kind]}</svg>;
}

const sections = [
  { id: 'overview', label: '整体界面', icon: 'layout' },
  { id: 'controls', label: '操作控件', icon: 'control' },
  { id: 'fields', label: '表单与反馈', icon: 'type' },
  { id: 'language', label: '基础样式', icon: 'folder' },
] as const;

function Section({ id, title, description, children }: { id: string; title: string; description: string; children: ReactNode }) {
  return <section id={id} className="gallery-section" aria-labelledby={`${id}-title`}>
    <header className="gallery-section-heading"><h2 id={`${id}-title`}>{title}</h2><p>{description}</p></header>
    {children}
  </section>;
}

export function UiLibrary() {
  const theme = useThemeStore((s) => s.theme);
  const setTheme = useThemeStore((s) => s.setTheme);
  const [active, setActive] = useState('overview');
  const [dialogOpen, setDialogOpen] = useState(false);
  const [notice, setNotice] = useState('');
  const [notifications, setNotifications] = useState(true);
  const [project, setProject] = useState('我的资料');
  const [keyValue, setKeyValue] = useState('');
  const [checked, setChecked] = useState(false);
  const [running, setRunning] = useState(false);
  useEffect(() => initializeTheme(), []);

  function navigate(id: string) {
    setActive(id);
    document.getElementById(id)?.scrollIntoView({ behavior: 'auto', block: 'start' });
  }

  return <div className="gallery-shell">
    <a className="gallery-skip" href="#gallery-main">跳到内容</a>
    <aside className="gallery-sidebar">
      <div className="gallery-brand"><span className="gallery-brand-mark" aria-hidden="true">A</span><div><strong>AngelBot</strong><span>界面与组件</span></div></div>
      <nav aria-label="组件分类">{sections.map((section) => <button key={section.id} className={`gallery-nav-item ${active === section.id ? 'is-current' : ''}`} aria-current={active === section.id ? 'location' : undefined} onClick={() => navigate(section.id)}><Glyph kind={section.icon} />{section.label}</button>)}</nav>
      <div className="gallery-sidebar-note"><p>组件预览<br />不会调用模型或读写文件。</p></div>
    </aside>

    <main id="gallery-main" className="gallery-main" tabIndex={-1}>
      <header className="gallery-topbar"><span>组件预览</span><SelectField label="外观" aria-label="外观" value={theme} onChange={(e) => setTheme(e.target.value as Theme)} options={[{ value: 'light', label: '浅色' }, { value: 'dark', label: '深色' }, { value: 'system', label: '跟随系统' }]} wrapperClassName="gallery-theme" /></header>
      <div className="gallery-content">
        <div className="gallery-intro"><h1>AngelBot 组件</h1><p>按钮、表单、消息与弹窗的交互样本。<br />所有操作仅用于预览，不会调用模型或读写文件。</p></div>

        <Section id="overview" title="布局示例" description="侧栏、对话与文件工作台。">
          <div className="gallery-workspace">
            <aside className="gallery-projects" aria-label="项目示例"><span className="ui-eyebrow">工作区</span><button className="gallery-project is-current"><Glyph kind="layout" />AngelBot 日常</button><span className="ui-eyebrow gallery-project-label">项目</span><button className="gallery-project"><Glyph kind="folder" />我的资料</button><button className="gallery-project"><Glyph kind="folder" />旅行计划</button><p>界面示例，不会读取文件或执行实际操作。</p></aside>
            <div className="gallery-conversation">
              <div className="gallery-conversation-header"><div><span className="ui-eyebrow">个人空间</span><h3>日常对话</h3></div><span className="ui-tag"><span className="ui-status-dot" />状态示例：就绪</span></div>
              <div className="gallery-chat-content"><div className="gallery-user-message">帮我整理一下下载目录，先让我看看计划。</div><div className="gallery-answer"><span className="gallery-agent-label">AngelBot · 回复示例</span><p>可以。我会先按文件类型分组，并保留所有原文件。整理前，会把需要移动的文件列给你确认。</p><div className="gallery-result"><Glyph kind="folder" /><div><strong>下载目录整理</strong><span>整理计划示例 · 未读取文件</span></div><Button size="small" onClick={() => setDialogOpen(true)}>查看计划</Button></div></div></div>
              <div className="gallery-composer"><TextField label="给 AngelBot 发消息" aria-label="示例消息" placeholder="继续说说你的想法…" wrapperClassName="gallery-composer-field" /><IconButton variant="primary" label="发送示例消息" onClick={() => setNotice('这是界面样本，不会向模型发送消息。')}><Glyph kind="arrow" /></IconButton></div>
            </div>
            <aside className="gallery-workbench" aria-label="文件工作台示例"><header><span>工作台</span><IconButton size="small" label="添加示例文件" onClick={() => setNotice('新增文件入口已触发（样本）。')}><Glyph kind="plus" /></IconButton></header><div className="gallery-workbench-tabs"><span className="is-current">文件</span><span>浏览器</span><span>任务</span></div><div className="gallery-file"><Glyph kind="folder" /><span>Downloads</span></div><div className="gallery-file-child">整理计划.md</div><div className="gallery-file-child">原文件清单.txt</div><div className="gallery-workbench-note">示例文件列表，<br />不会读取本地目录。</div></aside>
          </div>
        </Section>

        <Section id="controls" title="按钮" description="主要、次要、图标、禁用与执行状态。">
          <div className="gallery-control-grid">
            <div className="gallery-specimen"><h3>操作与状态</h3><div className="ui-inline"><Button variant="primary" onClick={() => setNotice('主要操作已触发（样本），没有执行实际操作。')}>开始整理</Button><Button onClick={() => setDialogOpen(true)}>查看计划</Button><Button variant="ghost" onClick={() => setNotice('暂时保留现状（样本）。')}>稍后再说</Button></div><div className="ui-inline"><Button disabled>暂不可用</Button><Button busy={running} onClick={() => setRunning(true)}>{running ? '正在处理' : '试用执行状态'}</Button>{running && <Button variant="ghost" onClick={() => setRunning(false)}>停止示例</Button>}</div></div>
            <div className="gallery-specimen"><h3>小型操作</h3><div className="ui-inline"><Button size="small">打开文件</Button><IconButton label="新增项目" onClick={() => setDialogOpen(true)}><Glyph kind="plus" /></IconButton><Button variant="danger" size="small" onClick={() => setDialogOpen(true)}>删除示例</Button></div><p className="gallery-caption">图标按钮有可访问名称；删除操作同时使用文字与颜色标识。</p></div>
          </div>
        </Section>

        <Section id="fields" title="表单与反馈" description="文本输入、选择、开关、校验与状态提示。">
          <div className="gallery-form-grid">
            <div className="gallery-specimen"><h3>项目设置</h3><TextField label="项目名称" value={project} onChange={(e) => setProject(e.target.value)} hint="项目名称用于侧栏显示。" /><SelectField label="操作权限" defaultValue="confirm" options={[{ value: 'confirm', label: '重要操作前确认' }, { value: 'readonly', label: '仅查看与分析' }]} hint="此处只展示权限选择控件。" /><Switch label="任务完成时提醒我" hint="开关样本，不会发送通知。" checked={notifications} onChange={(e) => setNotifications(e.target.checked)} /></div>
            <div className="gallery-specimen"><h3>连接与反馈</h3><TextField label="API Key（仅示例）" type="password" value={keyValue} onChange={(e) => { setKeyValue(e.target.value); setChecked(false); }} error={checked && !keyValue ? '请先填写 Key，再测试连接。' : undefined} hint="样本不会保存或发送这里的内容。" /><Button onClick={() => { setChecked(true); if (keyValue) setNotice('已填写示例 Key；未发起真实连接。'); }}>测试示例校验</Button><div className="ui-notice ui-notice--success"><Glyph kind="check" /><span>成功提示示例：设置已保存。</span></div><div className="ui-notice ui-notice--warning">风险提示示例：操作将移动文件，执行前需确认。</div></div>
          </div>
        </Section>

        <Section id="language" title="基础样式" description="颜色、字号、间距与圆角。">
          <div className="gallery-language"><div><h3>颜色</h3><div className="gallery-swatches">{[['canvas', '画布'], ['surface', '内容'], ['subtle', '辅助'], ['accent', '操作']].map(([token, name]) => <div key={token}><span style={{ background: `var(--ui-${token})` }} /><span>{name}</span></div>)}</div></div><div><h3>文字</h3><p className="gallery-type-title">标题示例</p><p>正文 14 · 阅读 16 · 标题 20</p><p className="gallery-caption">Windows 本地字体，无需联网加载。</p></div><div><h3>间距与圆角</h3><p>基础间距 4 / 8</p><p>控件 6 · 面板 10 · 弹窗 12</p><p className="gallery-caption">动画遵循系统减少动态效果设置。</p></div></div>
        </Section>
        <footer className="gallery-footer">AngelBot UI · 本地组件预览</footer>
      </div>
    </main>
    {notice && <div className="gallery-toast ui-notice" role="status"><span>{notice}</span><IconButton size="small" label="关闭提示" onClick={() => setNotice('')}><Glyph kind="close" /></IconButton></div>}
    <Dialog open={dialogOpen} title="执行前，先确认计划" onClose={() => setDialogOpen(false)} initialFocusSelector="[data-cancel]">
      <p className="gallery-dialog-copy">这是确认弹窗样本，不会读取文件、调用模型或执行操作。</p><div className="ui-notice ui-notice--warning">此计划仅作展示，没有实际文件变更。</div><div className="gallery-dialog-actions"><Button data-cancel onClick={() => setDialogOpen(false)}>先不执行</Button><Button variant="primary" onClick={() => { setDialogOpen(false); setNotice('示例计划已确认，没有执行实际文件操作。'); }}>确认示例计划</Button></div>
    </Dialog>
  </div>;
}

import { useCallback, useEffect, useState } from 'react';
import { createAutomation, deleteAutomation, getAutomationRuns, getAutomations, runAutomationNow, setAutomationEnabled, type Automation, type AutomationRun } from '$lib/commands/automation';
import { getWorkspaces, type Workspace } from '$lib/commands/workspace';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspacesStore } from '$stores/workspaces';
import { normalizeDailySchedule, parseAutomationIntent, titleFromPrompt, type AutomationIntent } from './parseAutomationIntent';
import './AutomationPage.css';

const formatTime = (value?: number | null) => value ? new Date(value * 1000).toLocaleString() : '等待设置';

const FREQUENCY_OPTIONS = ['每天'];

type AutomationStatusPresentation = {
  label: string;
  scheduleLabel: string;
  scheduleValue: string;
  completed?: boolean;
  awaitingConfirmation?: boolean;
};

function statusPresentation(item: Automation, latestRun?: AutomationRun): AutomationStatusPresentation {
  if (item.executorKind === 'agent' && item.triggerKind === 'once' && latestRun) {
    switch (latestRun.status) {
      case 'queued':
        return { label: '排队中', scheduleLabel: '本次状态', scheduleValue: '等待主 Agent 处理' };
      case 'running':
        return { label: '执行中', scheduleLabel: '本次状态', scheduleValue: '主 Agent 正在处理' };
      case 'awaiting_confirmation':
        return { label: '等待确认', scheduleLabel: '本次状态', scheduleValue: '等待你的确认', awaitingConfirmation: true };
      case 'needs_attention':
        return { label: '需要处理', scheduleLabel: '本次状态', scheduleValue: '请在归属工作区继续处理' };
      case 'completed':
        return { label: '已完成', scheduleLabel: '本次状态', scheduleValue: '已完成', completed: true };
      default:
        break;
    }
  }

  const isReminder = item.executorKind === 'notification';
  const reminderCompleted = isReminder && !item.enabled && item.lastRunAt !== undefined;
  const reminderCancelled = isReminder && !item.enabled && item.lastRunAt === undefined;
  return {
    label: item.enabled ? (isReminder ? '待提醒' : '已启用') : reminderCompleted ? '已提醒' : reminderCancelled ? '已取消' : '已暂停',
    scheduleLabel: isReminder ? '提醒时间' : '下次执行',
    scheduleValue: item.enabled ? formatTime(item.nextRunAt) : reminderCompleted ? '已完成' : reminderCancelled ? '已取消' : '已暂停',
    completed: item.enabled || reminderCompleted,
  };
}

export function AutomationPage() {
  const [items, setItems] = useState<Automation[]>([]);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [title, setTitle] = useState('');
  const [triggerValue, setTriggerValue] = useState('每天 09:00');
  const [scriptPath, setScriptPath] = useState('');
  const [scriptArgs, setScriptArgs] = useState('');
  const [workingDir, setWorkingDir] = useState('');
  const [timeoutSeconds, setTimeoutSeconds] = useState('300');
  const [runs, setRuns] = useState<Record<string, AutomationRun[]>>({});
  const [review, setReview] = useState(false);
  const [saving, setSaving] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [nlText, setNlText] = useState('');
  const [nlDraft, setNlDraft] = useState<AutomationIntent | null>(null);
  const [nlPrompt, setNlPrompt] = useState('');
  const [nlFrequency, setNlFrequency] = useState('每天');
  const [nlTime, setNlTime] = useState('21:00');
  const [nlExecutor, setNlExecutor] = useState<'agent' | 'script'>('agent');
  const [nlWorkspaceId, setNlWorkspaceId] = useState('');
  const [nlScriptPath, setNlScriptPath] = useState('');
  const [nlSaving, setNlSaving] = useState(false);
  const [nlError, setNlError] = useState<string | null>(null);
  const [nlSuccess, setNlSuccess] = useState<string | null>(null);
  const setPage = useNavigationStore((state) => state.setPage);
  const openWorkspace = useWorkspacesStore((state) => state.openWorkspace);

  const load = useCallback(async () => {
    try {
      setError(null);
      const [automations, availableWorkspaces] = await Promise.all([getAutomations(), getWorkspaces()]);
      setItems(automations);
      setWorkspaces(availableWorkspaces);
      setNlWorkspaceId(current => current || availableWorkspaces.find(workspace => workspace.kind === 'personal')?.id || availableWorkspaces[0]?.id || '');
      setRuns(Object.fromEntries(await Promise.all(automations.map(async item => [item.id, await getAutomationRuns(item.id)] as const))));
    } catch {
      setError('无法加载自动化任务。请稍后重试。');
    }
  }, []);

  useEffect(() => { void load(); }, [load]);

  const hasActiveAgentRun = Object.values(runs)
    .some(runList => runList?.some(run => run.status === 'queued' || run.status === 'running'));
  useEffect(() => {
    if (!hasActiveAgentRun) return undefined;
    const timer = window.setInterval(() => { void load(); }, 5_000);
    return () => window.clearInterval(timer);
  }, [hasActiveAgentRun, load]);

  const create = async () => {
    if (!title.trim() || !scriptPath.trim()) return;
    const dailyTriggerValue = normalizeDailySchedule(triggerValue);
    if (!dailyTriggerValue) {
      setError('重复自动化目前只支持每天，请填写「每天 HH:MM」，例如「每天 09:00」。一次性提醒请在对话中创建。');
      return;
    }
    const args = scriptArgs.trim() ? scriptArgs.split(/\s+/) : [];
    const timeout = Number(timeoutSeconds);
    if (!Number.isInteger(timeout) || timeout < 10 || timeout > 3600) {
      setError('超时时间必须是 10 到 3600 秒之间的整数。');
      return;
    }
    setSaving(true);
    try {
      await createAutomation({
        title: title.trim(), prompt: '', triggerKind: 'schedule', triggerValue: dailyTriggerValue,
        permissionSummary: '仅执行已配置的本地脚本；输出和退出码会保存在运行记录中。',
        executorKind: 'script', scriptPath: scriptPath.trim(), scriptArgs: args,
        workingDir: workingDir.trim() || undefined, timeoutSeconds: timeout,
      });
      setTitle(''); setScriptPath(''); setScriptArgs(''); setWorkingDir(''); setReview(false);
      await load();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : '无法创建自动化任务。');
    } finally {
      setSaving(false);
    }
  };

  const perform = async (id: string, action: () => Promise<void>) => {
    setBusyId(id);
    try {
      await action();
      await load();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : '自动化操作失败，请重试。');
    } finally {
      setBusyId(null);
    }
  };

  const openOwningWorkspace = async (workspaceId: string) => {
    try {
      await openWorkspace(workspaceId);
      setPage('chat');
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : '无法打开归属工作区。');
    }
  };

  const parseNl = () => {
    setNlSuccess(null);
    const result = parseAutomationIntent(nlText);
    if (!result.ok) {
      setNlError(result.error);
      setNlDraft(null);
      return;
    }
    setNlError(null);
    setNlDraft(result.intent);
    setNlPrompt(result.intent.prompt);
    setNlFrequency(result.intent.frequency);
    setNlTime(result.intent.time);
    setNlExecutor(result.intent.executorKind);
    setNlScriptPath(result.intent.scriptPath ?? '');
  };

  const createFromNl = async () => {
    const prompt = nlPrompt.trim();
    if (!prompt) {
      setNlError('请填写要做什么。');
      return;
    }
    if (nlFrequency !== '每天') {
      setNlError('重复自动化目前只支持每天，不会将其他频率改成每日任务。');
      return;
    }
    const dailyTriggerValue = normalizeDailySchedule(nlTime);
    if (!dailyTriggerValue) {
      setNlError('时间格式需要是 HH:MM，例如 21:00。');
      return;
    }
    if (nlExecutor === 'script' && !nlScriptPath.trim()) {
      setNlError('请填写脚本路径。');
      return;
    }
    setNlSaving(true);
    setNlError(null);
    try {
      const title = titleFromPrompt(prompt);
      await createAutomation({
        title, prompt, triggerKind: 'schedule',
        triggerValue: dailyTriggerValue,
        permissionSummary: '每次运行前询问',
        executorKind: nlExecutor,
        workspaceId: nlExecutor === 'agent' ? nlWorkspaceId : undefined,
        scriptPath: nlExecutor === 'script' ? nlScriptPath.trim() : undefined,
        scriptArgs: [], workingDir: undefined, timeoutSeconds: 300,
      });
      setNlText(''); setNlDraft(null); setNlScriptPath('');
      setNlSuccess(`已创建自动化任务「${title}」。`);
      await load();
    } catch (cause) {
      setNlError(cause instanceof Error ? cause.message : '无法创建自动化任务。');
    } finally {
      setNlSaving(false);
    }
  };

  return (
    <main className="automation-page" aria-label="自动化">
      <header className="automation-header">
        <div><h1>自动化</h1><p>AngelBot 运行时，固定脚本独立执行；需判断的任务进入所选工作区的主 Agent 队列。</p></div>
        <span className="automation-task-count">{items.length} 个任务</span>
      </header>

      <section className="automation-create-card" aria-labelledby="automation-nl-title">
        <div className="automation-section-head"><div><h2 id="automation-nl-title">用一句话创建</h2><p>每日重复任务，例如：每天晚上九点提醒我整理当天笔记。一次性提醒请在对话中创建。</p></div><small>本地解析，不会调用 AI</small></div>
        <div className="automation-form automation-nl-form">
          <label>一句话描述<input
            value={nlText}
            onChange={e => { setNlText(e.target.value); setNlDraft(null); setNlError(null); setNlSuccess(null); }}
            onKeyDown={e => { if (e.key === 'Enter' && nlText.trim()) parseNl(); }}
            placeholder="每天晚上九点提醒我整理当天笔记"
          /></label>
          <button type="button" disabled={!nlText.trim()} onClick={parseNl}>解析</button>
        </div>
        {nlError && <p className="automation-error automation-nl-error" role="alert">{nlError}</p>}
        {nlDraft && (
          <div className="automation-form automation-nl-confirm">
            <label className="wide">要做什么<input value={nlPrompt} onChange={e => setNlPrompt(e.target.value)} placeholder="整理当天笔记" /></label>
            <label>频率<select value={nlFrequency} onChange={e => setNlFrequency(e.target.value)}>
              {FREQUENCY_OPTIONS.map(option => <option key={option} value={option}>{option}</option>)}
            </select><small>目前仅支持每天；每周和工作日暂不支持。</small></label>
            <label>时间<input value={nlTime} onChange={e => setNlTime(e.target.value)} placeholder="21:00" /></label>
            <label>执行方式<select value={nlExecutor} onChange={e => setNlExecutor(e.target.value as 'agent' | 'script')}>
              <option value="agent">由 AngelBot 处理</option>
              <option value="script">运行本地脚本</option>
            </select></label>
            {nlExecutor === 'agent' && (
              <label className="wide">归属工作区<select value={nlWorkspaceId} onChange={event => setNlWorkspaceId(event.target.value)}>
                {workspaces.map(workspace => <option key={workspace.id} value={workspace.id}>{workspace.kind === 'personal' ? '个人空间' : workspace.name}</option>)}
              </select><small>任务会携带此工作区的上下文与权限进入主 Agent 队列。</small></label>
            )}
            {nlExecutor === 'script' && (
              <label className="wide">脚本路径<input value={nlScriptPath} onChange={e => setNlScriptPath(e.target.value)} placeholder="工作目录\scripts\daily.py" /></label>
            )}
            <div className="automation-nl-actions">
              <button type="button" className="secondary" onClick={() => { setNlDraft(null); setNlError(null); }}>取消</button>
              <button type="button" disabled={nlSaving || !nlPrompt.trim() || (nlExecutor === 'agent' && !nlWorkspaceId)} onClick={() => void createFromNl()}>{nlSaving ? '创建中…' : '创建任务'}</button>
            </div>
          </div>
        )}
        {nlSuccess && <p className="automation-success" role="status">{nlSuccess}</p>}
      </section>

      <section className="automation-create-card" aria-labelledby="automation-create-title">
        <div className="automation-section-head"><div><h2 id="automation-create-title">新建任务</h2><p>脚本与工作目录必须位于当前工作目录内。</p></div><small>不会调用 AI</small></div>
        <div className="automation-form automation-script-form">
          <label>任务名称<input value={title} onChange={e => setTitle(e.target.value)} placeholder="例如：整理下载目录" /></label>
          <label>触发时间<input value={triggerValue} onChange={e => setTriggerValue(e.target.value)} placeholder="每天 09:00" /><small>仅支持每日重复，按此电脑的系统时区执行。</small></label>
          <label className="wide">脚本路径<input value={scriptPath} onChange={e => setScriptPath(e.target.value)} placeholder="工作目录\scripts\daily.py" /></label>
          <label>参数（以空格分隔）<input value={scriptArgs} onChange={e => setScriptArgs(e.target.value)} placeholder="--dry-run today" /></label>
          <label>脚本工作目录（可选）<input value={workingDir} onChange={e => setWorkingDir(e.target.value)} placeholder="默认使用脚本所在目录" /></label>
          <label>超时（秒）<input inputMode="numeric" value={timeoutSeconds} onChange={e => setTimeoutSeconds(e.target.value)} /></label>
          <button type="button" disabled={!title.trim() || !scriptPath.trim()} onClick={() => {
            const dailyTriggerValue = normalizeDailySchedule(triggerValue);
            if (!dailyTriggerValue) {
              setError('重复自动化目前只支持每天，请填写「每天 HH:MM」，例如「每天 09:00」。一次性提醒请在对话中创建。');
              return;
            }
            setError(null); setTriggerValue(dailyTriggerValue); setReview(true);
          }}>预览规则</button>
        </div>
      </section>

      {review && <section className="automation-review" role="status">
        <div><strong>确认创建任务</strong><p>{title} · {triggerValue}</p><small>超时后停止进程并保留日志。</small></div>
        <div><button type="button" className="secondary" onClick={() => setReview(false)}>返回编辑</button><button type="button" disabled={saving} onClick={() => void create()}>{saving ? '创建中…' : '创建任务'}</button></div>
      </section>}
      {error && <p className="automation-error" role="alert">{error}</p>}

      <section className="automation-list-section" aria-labelledby="automation-list-title">
        <div className="automation-list-title"><div><h2 id="automation-list-title">任务</h2><p>{items.length ? `${items.length} 条自动化任务` : '还没有已保存的任务'}</p></div><button className="automation-refresh" type="button" onClick={() => void load()}>刷新</button></div>
        {items.length === 0 ? <div className="automation-empty"><strong>还没有自动化任务</strong><p>固定脚本可独立运行；Agent 任务需要先选择归属工作区。</p></div> : <div className="automation-grid">{items.map(item => {
          const latestRun = runs[item.id]?.[0];
          const workspace = workspaces.find(candidate => candidate.id === item.workspaceId);
          const pausedAgent = item.executorKind === 'agent' && !item.enabled;
          const isReminder = item.executorKind === 'notification';
          const reminderCompleted = isReminder && !item.enabled && item.lastRunAt !== undefined;
          const reminderCancelled = isReminder && !item.enabled && item.lastRunAt === undefined;
          const presentation = statusPresentation(item, latestRun);
          const sourceLabel = item.executorKind === 'agent'
            ? `AngelBot · ${workspace?.name || '已删除工作区'}`
            : isReminder
              ? `本地通知 · ${workspace?.name || 'AngelBot'}`
              : item.scriptPath || '未配置脚本';
          return <article key={item.id} className="automation-card">
            <div className="automation-card-head"><span className={`automation-status ${presentation.completed ? 'enabled' : ''}`}><i />{presentation.label}</span><button className="automation-delete" type="button" aria-label={`删除 ${item.title}`} onClick={() => void perform(item.id, () => deleteAutomation(item.id))}>×</button></div>
            <h3>{item.title}</h3><p className="automation-trigger">{item.triggerValue}</p><p className="automation-path" title={item.scriptPath}>{sourceLabel}</p>
            <dl><div><dt>{presentation.scheduleLabel}</dt><dd>{presentation.scheduleValue}</dd></div><div><dt>最近结果</dt><dd>{latestRun?.summary || '尚无运行记录'}</dd></div></dl>
            {latestRun?.output && <details className="automation-output"><summary>{latestRun.exitCode === undefined ? '查看输出' : `退出码 ${latestRun.exitCode}`}</summary><pre>{latestRun.output}</pre></details>}
            <div className="automation-actions">{presentation.awaitingConfirmation && workspace && item.workspaceId && <button type="button" className="secondary" onClick={() => void openOwningWorkspace(item.workspaceId!)}>前往 {workspace.kind === 'personal' ? 'AngelBot 日常' : workspace.name} 确认</button>}<button type="button" disabled={busyId === item.id || pausedAgent} onClick={() => void perform(item.id, () => runAutomationNow(item.id))}>{busyId === item.id ? '处理中…' : pausedAgent ? '先启用再执行' : item.executorKind === 'agent' ? '立即执行' : isReminder ? '立即提醒' : '立即运行'}</button><button type="button" className="secondary" disabled={busyId === item.id || reminderCompleted} onClick={() => void perform(item.id, () => setAutomationEnabled(item.id, !item.enabled))}>{reminderCompleted ? '已完成' : item.enabled ? '暂停' : reminderCancelled ? '恢复' : '启用'}</button></div>
          </article>;
        })}</div>}
      </section>
    </main>
  );
}

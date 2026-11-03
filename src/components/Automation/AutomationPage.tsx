import { useCallback, useEffect, useRef, useState } from 'react';
import { createAutomation, deleteAutomation, getAutomationDashboard, runAutomationNow, setAutomationEnabled, stopAutomationRun, type Automation, type AutomationDashboardItem, type AutomationRunSummary } from '$lib/commands/automation';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { getAutostart, setAutostart } from '$lib/commands/settings';
import { getWorkspaces, type Workspace } from '$lib/commands/workspace';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspacesStore } from '$stores/workspaces';
import { subscribeAutomationUpdates } from '$lib/automation-events';
import { normalizeDailySchedule } from './dailySchedule';
import { AutomationHistory } from './AutomationHistory';
import { AutomationTimePicker } from './AutomationTimePicker';
import './AutomationPage.css';

const formatTime = (value?: number | null) => value ? new Date(value * 1000).toLocaleString() : '等待设置';
const nextTrigger = (item: Automation) => !item.enabled ? '已暂停' : item.triggerKind === 'startup' ? '下次 Windows 登录' : formatTime(item.nextRunAt);

const SCRIPT_PERMISSION_SUMMARY = '仅执行已配置的本地脚本；输出和退出码会保存在运行记录中。';
const SCRIPT_LIFECYCLE_NOTICE = '脚本结束或超时会回收其自有进程树并保留日志，不影响原先打开的软件。GUI 辅助工具需要明确的任务控制接口；仅启动软件后退出的脚本不代表其中任务已执行。';
const errorMessage = (cause: unknown, fallback: string) =>
  cause instanceof Error ? cause.message : typeof cause === 'string' ? cause : fallback;

type AutomationStatusPresentation = {
  label: string;
  scheduleLabel: string;
  scheduleValue: string;
  completed?: boolean;
  workspaceAction?: '确认' | '继续处理' | '查看结果' | '查看进度';
  notice?: string;
  attention?: boolean;
};

function statusPresentation(item: Automation, latestRun?: AutomationRunSummary | null): AutomationStatusPresentation {
  const isMaa = item.executorKind === 'maa';
  if ((item.executorKind === 'script' || isMaa) && latestRun) {
    if (latestRun.status === 'running') {
      return {
        label: latestRun.stopControl === 'requested' ? '正在停止' : '已启动，等待结果',
        scheduleLabel: '本次回执', scheduleValue: '尚无结束结果',
        notice: latestRun.stopControl === 'requested'
          ? '正在清理本次受控运行并等待结束回执。停止针对本次执行，已发生的操作不会撤回；需人工处理的异常会暂停后续计划。'
          : latestRun.stopControl === 'available'
            ? '停止仅针对 AngelBot 持有的本次运行，已发生的操作不会撤回。暂停只影响后续调度；需人工处理的异常会暂停后续计划。'
            : `已登记本次启动回执，尚未收到结束结果；重启后不能仅凭此回执确认${isMaa ? 'MAA 队列' : '脚本'}仍在运行。暂停只影响后续调度，不会停止这次${isMaa ? '队列' : '脚本'}。`,
      };
    }
    if (latestRun.status === 'attention_required') {
      return {
        label: '需要处理', scheduleLabel: '本次回执', scheduleValue: '结果未知', attention: true,
        notice: '这次运行结果未知，自动调度已暂停，不会自动重试。请先核对脚本或软件的实际状态，确认上次执行已结束且不会重复产生副作用后手动重试；得到明确结果后才能重新启用定时。',
      };
    }
    if (latestRun.status === 'cancelled') {
      return {
        label: '已停止', scheduleLabel: '下次执行',
        scheduleValue: nextTrigger(item),
        notice: '本次受控运行已停止，已发生的操作不会撤回。后续计划请以下次执行状态为准。',
      };
    }
    if (latestRun.status === 'failed' || latestRun.status === 'timed_out') {
      return {
        label: latestRun.status === 'failed' ? (isMaa ? '存在失败' : '执行失败') : '已超时', attention: true,
        scheduleLabel: '下次执行', scheduleValue: nextTrigger(item),
      };
    }
    if (latestRun.status === 'completed') {
      return {
        label: isMaa ? '队列已结束' : '脚本已结束', scheduleLabel: '下次执行',
        scheduleValue: nextTrigger(item), completed: true,
        notice: isMaa ? '记录的是 MAA 队列执行结果，不代表游戏业务目标全部完成。' : '记录的是脚本结束结果，不代表外部应用的业务目标全部完成。',
      };
    }
  }
  if (item.executorKind === 'agent' && latestRun) {
    const schedule = (currentState: string) => item.triggerKind === 'once'
      ? { scheduleLabel: '本次状态', scheduleValue: currentState }
      : { scheduleLabel: '下次执行', scheduleValue: nextTrigger(item) };
    switch (latestRun.status) {
      case 'queued':
        return { label: '排队中', ...schedule('等待主 Agent 处理') };
      case 'running':
        return { label: '执行中', ...schedule('主 Agent 正在处理'), workspaceAction: '查看进度' };
      case 'awaiting_confirmation':
        return { label: '等待确认', ...schedule('等待你的确认'), workspaceAction: '确认' };
      case 'needs_attention':
        return { label: '需要处理', ...schedule('请在归属工作区继续处理'), workspaceAction: '继续处理', attention: true };
      case 'failed':
        return { label: '执行失败', ...schedule('请在归属工作区继续处理'), workspaceAction: '继续处理', attention: true };
      case 'interrupted':
        return { label: '已中断', ...schedule('请在归属工作区继续处理'), workspaceAction: '继续处理', attention: true };
      case 'completed':
        return { label: '已完成', ...schedule('已完成'), workspaceAction: '查看结果', completed: true };
      default:
        break;
    }
  }

  const isReminder = item.executorKind === 'notification';
  if (isReminder && latestRun?.status === 'skipped') {
    return {
      label: '本次未发送', scheduleLabel: '本次状态', scheduleValue: '未发送',
      notice: '本次触发已记录，通知关闭时不会发送，也不会自动补发。需要时可检查通知设置后选择立即提醒。',
    };
  }
  if (isReminder && latestRun?.status === 'needs_attention') {
    return {
      label: '需要处理', scheduleLabel: '本次状态', scheduleValue: '通知未确认', attention: true,
      notice: '本次提醒未取得正常结束回执，不会自动补发。请检查最近结果及 Windows 通知设置；如仍需要提醒，可明确选择立即提醒。',
    };
  }
  const reminderCompleted = isReminder && !item.enabled && item.lastRunAt !== undefined;
  const reminderCancelled = isReminder && !item.enabled && item.lastRunAt === undefined;
  return {
    label: item.enabled ? (isReminder ? '待提醒' : '已启用') : reminderCompleted ? '提醒已触发' : reminderCancelled ? '已取消' : '已暂停',
    scheduleLabel: isReminder ? '提醒时间' : '下次执行',
    scheduleValue: item.enabled ? nextTrigger(item) : reminderCompleted ? '已完成' : reminderCancelled ? '已取消' : '已暂停',
    completed: item.enabled || reminderCompleted,
    notice: reminderCompleted ? '已处理本次提醒触发，不代表 Windows 通知已送达或已被查看。' : undefined,
  };
}

export function AutomationPage() {
  const [items, setItems] = useState<AutomationDashboardItem[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [title, setTitle] = useState('');
  const [triggerValue, setTriggerValue] = useState('09:00');
  const [triggerKind, setTriggerKind] = useState<'schedule' | 'startup'>('schedule');
  const [executorKind, setExecutorKind] = useState<'script' | 'agent'>('script');
  const [prompt, setPrompt] = useState('');
  const [workspaceId, setWorkspaceId] = useState(useWorkspacesStore.getState().activeWorkspaceId || '');
  const [autostart, setAutostartReady] = useState<boolean | null>(null);
  const [changingAutostart, setChangingAutostart] = useState(false);
  const autostartInFlight = useRef(false);
  const [scriptPath, setScriptPath] = useState('');
  const [scriptArgs, setScriptArgs] = useState('');
  const [workingDir, setWorkingDir] = useState('');
  const [timeoutSeconds, setTimeoutSeconds] = useState('300');
  const [review, setReview] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [startupRevision, setStartupRevision] = useState(0);
  const [pickingPath, setPickingPath] = useState(false);
  const pickerInFlight = useRef(false);
  const createInFlight = useRef(false);
  const [saving, setSaving] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  // Only requests owned by this page; persisted receipts remain authoritative.
  const [pendingRunIds, setPendingRunIds] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const setPage = useNavigationStore((state) => state.setPage);
  const target = useNavigationStore((state) => state.automationTarget);
  const openWorkspace = useWorkspacesStore((state) => state.openWorkspace);
  const loadGeneration = useRef(0);
  const targetCard = useRef<HTMLElement>(null);
  const focusedTarget = useRef<typeof target>(null);
  const targetItem = target && items.find(item => item.id === target.automationId && item.workspaceId === target.workspaceId);

  useEffect(() => {
    if (!target || !targetItem || focusedTarget.current === target) return;
    const details = targetCard.current?.querySelector('details');
    if (details) details.open = true;
    targetCard.current?.scrollIntoView?.({ block: 'nearest', behavior: 'auto' });
    targetCard.current?.focus({ preventScroll: true });
    focusedTarget.current = target;
  }, [target, targetItem]);

  const load = useCallback(async () => {
    const generation = ++loadGeneration.current;
    try {
      setError(null);
      const [automations, availableWorkspaces] = await Promise.all([
        getAutomationDashboard(), getWorkspaces(),
      ]);
      if (generation !== loadGeneration.current) return;
      setItems(automations.items);
      setLoaded(true);
      setWorkspaces(availableWorkspaces);
    } catch {
      if (generation !== loadGeneration.current) return;
      setError('无法加载自动化任务。请稍后重试。');
    }
  }, []);

  useEffect(() => {
    const refresh = () => { void load(); };
    refresh();
    const unsubscribe = subscribeAutomationUpdates(refresh);
    return () => {
      ++loadGeneration.current;
      unsubscribe();
    };
  }, [load]);

  const hasActiveRun = pendingRunIds.length > 0 || items
    .some(item => item.latestRun?.status === 'queued' || item.latestRun?.status === 'running');
  useEffect(() => {
    if (!hasActiveRun) return undefined;
    const timer = window.setInterval(() => { void load(); }, 5_000);
    return () => window.clearInterval(timer);
  }, [hasActiveRun, load]);

  const invalidateReview = () => { setReview(false); };
  const triggerLabel = triggerKind === 'startup' ? '开机自启（登录 Windows 后）' : normalizeDailySchedule(triggerValue) || '请选择时间';
  const agentWorkspace = workspaces.find(workspace => workspace.id === workspaceId);
  const canPreview = !!title.trim() && (triggerKind !== 'startup' || autostart === true)
    && (executorKind === 'script' ? !!scriptPath.trim() : !!prompt.trim() && !!agentWorkspace);

  useEffect(() => {
    if (!createOpen || triggerKind !== 'startup') return;
    let active = true;
    setAutostartReady(null);
    void getAutostart().then(value => { if (active) setAutostartReady(value); })
      .catch(cause => { if (active) setError(errorMessage(cause, '无法读取开机启动状态，请重试。')); });
    return () => { active = false; };
  }, [createOpen, triggerKind, startupRevision]);

  const changeAutostart = async (enabled: boolean) => {
    if (autostartInFlight.current) return;
    autostartInFlight.current = true;
    setChangingAutostart(true);
    setError(null);
    invalidateReview();
    try {
      await setAutostart(enabled);
      setAutostartReady(enabled);
    } catch (cause) {
      setError(errorMessage(cause, '无法更改开机启动设置。'));
    } finally { autostartInFlight.current = false; setChangingAutostart(false); }
  };

  const choosePath = async (directory: boolean) => {
    if (pickerInFlight.current) return;
    pickerInFlight.current = true;
    setPickingPath(true);
    setError(null);
    try {
      const selected = await openDialog({
        directory, multiple: false, defaultPath: (directory ? workingDir.trim() : scriptPath.trim()) || undefined,
        title: directory ? '选择脚本工作目录' : '选择自动化脚本',
        ...(directory ? {} : { filters: [{ name: '本地脚本', extensions: ['py', 'js', 'mjs', 'cjs', 'ps1', 'bat', 'cmd'] }] }),
      });
      if (typeof selected !== 'string') return;
      if (directory) setWorkingDir(selected);
      else setScriptPath(selected);
      invalidateReview();
    } catch (cause) {
      setError(errorMessage(cause, '无法选择路径，请重试。'));
    } finally {
      pickerInFlight.current = false;
      setPickingPath(false);
    }
  };

  const create = async () => {
    if (!canPreview || !review || createInFlight.current) return;
    const dailyTriggerValue = triggerKind === 'startup' ? '登录 Windows 后' : normalizeDailySchedule(triggerValue);
    if (!dailyTriggerValue) {
      setError('请选择有效的每日执行时间。');
      return;
    }
    const args = scriptArgs.split(/\r\n|\r|\n/).filter(arg => arg.length > 0);
    const timeout = executorKind === 'agent' ? 300 : Number(timeoutSeconds);
    if (!Number.isInteger(timeout) || timeout < 10 || timeout > 3600) {
      setError('超时时间必须是 10 到 3600 秒之间的整数。');
      return;
    }
    createInFlight.current = true;
    setSaving(true);
    try {
      await createAutomation({
        title: title.trim(), prompt: executorKind === 'agent' ? prompt.trim() : '', triggerKind, triggerValue: dailyTriggerValue,
        permissionSummary: executorKind === 'script' ? SCRIPT_PERMISSION_SUMMARY : '由归属空间的主 Agent 处理，沿用该空间现有权限和风险确认。',
        executorKind, workspaceId: executorKind === 'agent' ? workspaceId : undefined,
        scriptPath: executorKind === 'script' ? scriptPath.trim() : undefined, scriptArgs: executorKind === 'script' ? args : [],
        workingDir: executorKind === 'script' ? workingDir.trim() || undefined : undefined, timeoutSeconds: timeout,
      });
      setScriptPath(''); setScriptArgs(''); setWorkingDir('');
      setCreateOpen(false);
      setTitle(''); setPrompt(''); invalidateReview();
      await load();
    } catch (cause) {
      setError(errorMessage(cause, '无法创建自动化任务。'));
    } finally {
      createInFlight.current = false;
      setSaving(false);
    }
  };

  const perform = async (id: string, action: () => Promise<void>, isRun = false) => {
    if (isRun) setPendingRunIds(current => [...current, id]);
    else setBusyId(id);
    try {
      await action();
      await load();
    } catch (cause) {
      setError(errorMessage(cause, '自动化操作失败，请重试。'));
    } finally {
      if (isRun) setPendingRunIds(current => current.filter(candidate => candidate !== id));
      else setBusyId(current => current === id ? null : current);
    }
  };

  const openOwningWorkspace = async (workspaceId: string) => {
    const navigation = useNavigationStore.getState();
    try {
      const accepted = await openWorkspace(workspaceId);
      if (!accepted || navigation !== useNavigationStore.getState()) return;
      setPage('chat');
    } catch (cause) {
      setError(errorMessage(cause, '无法打开归属工作区。'));
    }
  };

  return (
    <main className="automation-page" aria-label="自动化">
      <header className="automation-header">
        <div><h1>自动化</h1><p>定时任务需保持 AngelBot 运行。</p></div>
        <button type="button" className="automation-create-toggle" disabled={saving || pickingPath || changingAutostart} aria-expanded={createOpen} onClick={() => setCreateOpen(open => !open)}>{createOpen ? '收起配置' : '添加自动化'}</button>
      </header>
      <p className="automation-conversation-guide">可以直接与 AngelBot 讨论并配置自动化。<button type="button" onClick={() => setPage('chat')}>去对话</button></p>
      {createOpen && <section className="automation-create-card" aria-labelledby="automation-create-title">
        <div className="automation-section-head"><h2 id="automation-create-title">添加自动化任务</h2></div>
        <fieldset className="automation-form automation-script-form" aria-label="自动化配置" disabled={saving || pickingPath || changingAutostart}>
          <section className="automation-config-section" aria-labelledby="automation-basics-title">
            <h3 id="automation-basics-title">基本信息</h3>
            <div className="automation-config-fields automation-config-fields--stacked">
              <label>任务名称<input value={title} onChange={e => { setTitle(e.target.value); invalidateReview(); }} placeholder="例如：整理下载目录" /></label>
            </div>
          </section>
          <section className="automation-config-section" aria-labelledby="automation-trigger-title">
            <h3 id="automation-trigger-title">触发方式</h3>
            <div className="automation-config-fields">
              <label>触发方式<select value={triggerKind} onChange={e => { setTriggerKind(e.target.value as 'schedule' | 'startup'); invalidateReview(); }}><option value="schedule">定时触发</option><option value="startup">开机自启触发</option></select>{triggerKind === 'startup' && <small>登录 Windows 后触发。</small>}</label>
              {triggerKind === 'schedule'
                ? <AutomationTimePicker value={triggerValue} onChange={value => { setTriggerValue(value); invalidateReview(); }} />
                : <div className="automation-startup-setting"><strong>AngelBot 开机启动</strong><p>不预先运行任务；暂停任务不关闭自启。</p><button type="button" className="secondary" disabled={autostart === null || changingAutostart} onClick={() => void changeAutostart(!autostart)}>{changingAutostart ? '正在设置…' : autostart ? '关闭 AngelBot 开机启动' : autostart === null ? '正在读取…' : '启用 AngelBot 开机启动'}</button>{autostart === null && <button type="button" className="secondary" onClick={() => setStartupRevision(value => value + 1)}>重新读取启动状态</button>}<small>{autostart ? '下次登录触发，手动重开不触发。' : '启用后即可保存开机任务。'}</small></div>}
            </div>
          </section>
          <section className="automation-config-section" aria-labelledby="automation-executor-title">
            <h3 id="automation-executor-title">执行方式</h3>
            <label>是否使用模型<select value={executorKind} onChange={e => { setExecutorKind(e.target.value as 'script' | 'agent'); invalidateReview(); }}><option value="script">不使用模型 · 运行本地脚本</option><option value="agent">使用模型 · 由 AngelBot 处理</option></select><small>{executorKind === 'script' ? '不消耗模型额度。' : '使用当前模型，会消耗额度。'}</small></label>
          </section>
          {executorKind === 'script' ? <>
          <section className="automation-config-section" aria-labelledby="automation-script-title">
            <h3 id="automation-script-title">脚本与参数</h3>
            <div className="automation-config-fields automation-config-fields--stacked">
              <div className="automation-path-field"><label htmlFor="automation-script-path">脚本路径</label><div className="automation-path-input"><input id="automation-script-path" value={scriptPath} onChange={e => { setScriptPath(e.target.value); invalidateReview(); }} placeholder="选择脚本或输入完整路径" /><button type="button" disabled={pickingPath || saving} onClick={() => void choosePath(false)}>选择脚本</button></div>
                <small>保存副本运行，原文件不变。</small>
              </div>
              <label>参数（每行一个）<textarea value={scriptArgs} onChange={e => { setScriptArgs(e.target.value); invalidateReview(); }} rows={3} placeholder={'--profile\nC:\\Tools\\daily.json'} /></label>
            </div>
          </section>
          <details className="automation-config-options">
            <summary><span>运行选项</span><small>{workingDir.trim() ? '自定义工作目录' : '副本目录'} · {timeoutSeconds || '—'} 秒超时</small></summary>
            <div className="automation-config-fields">
              <div className="automation-path-field"><label htmlFor="automation-working-dir">脚本工作目录（可选）</label><div className="automation-path-input"><input id="automation-working-dir" value={workingDir} onChange={e => { setWorkingDir(e.target.value); invalidateReview(); }} placeholder="默认使用副本目录" /><button type="button" disabled={pickingPath || saving} onClick={() => void choosePath(true)}>选择目录</button></div></div>
              <label>超时（秒）<input inputMode="numeric" value={timeoutSeconds} onChange={e => { setTimeoutSeconds(e.target.value); invalidateReview(); }} /><small>10–3600 秒。</small></label>
            </div>
          </details>
          </> : <section className="automation-config-section" aria-labelledby="automation-agent-title">
            <h3 id="automation-agent-title">交给 AngelBot 的任务</h3>
            <div className="automation-config-fields automation-config-fields--stacked">
              <label>归属空间<select value={workspaceId} onChange={e => { setWorkspaceId(e.target.value); invalidateReview(); }}><option value="">选择日常或项目</option>{workspaces.map(workspace => <option key={workspace.id} value={workspace.id}>{workspace.name}</option>)}</select><small>结果回到所选空间的主对话。</small></label>
              <label>任务说明<textarea rows={4} value={prompt} onChange={e => { setPrompt(e.target.value); invalidateReview(); }} placeholder="描述要完成的事项、使用的资料和预期结果" /></label>
            </div>
          </section>}
          <div className="automation-form-actions automation-config-footer"><button type="button" disabled={!canPreview || saving || pickingPath} onClick={() => {
            if (triggerKind === 'schedule' && !normalizeDailySchedule(triggerValue)) return setError('请选择有效的每日执行时间。');
            const timeout = executorKind === 'agent' ? 300 : Number(timeoutSeconds);
            if (!Number.isInteger(timeout) || timeout < 10 || timeout > 3600) return setError('超时时间必须是 10 到 3600 秒之间的整数。');
            setError(null); setReview(true);
          }}>预览规则</button></div>
        </fieldset>
        {executorKind === 'script' && <details className="automation-script-notes"><summary>脚本使用说明</summary>
          <p className="automation-script-guidance">支持 Python、Node.js、PowerShell 和批处理。只导入可信脚本。单个脚本保存在 AngelBot 的 automation-scripts 目录；不包含配置和模块，源文件更新不会自动同步。选择启动目录不会复制该目录的内容。</p>
          <p className="automation-script-guidance">参数每行一个，空行忽略，无需额外加引号。批处理（.bat/.cmd）不接受引号或 shell 元字符。</p>
          <p className="automation-script-guidance">脚本按计划运行，不逐次确认，也不继承模型密钥。退出码不保证外部业务完成。暂停仅影响后续调度，不会停止已启动的脚本。{SCRIPT_LIFECYCLE_NOTICE}</p>
        </details>}
      </section>}
      {review && createOpen && <section className="automation-review" role="status">
        <div className="automation-review-content"><h3>确认创建任务</h3><p>{title} · {triggerLabel}</p>
          <dl className="automation-script-review"><dt>使用模型</dt><dd>{executorKind === 'agent' ? '是 · 当前已保存的模型' : '否 · 仅运行脚本'}</dd>{executorKind === 'script' ? <><dt>源脚本</dt><dd>{scriptPath.trim()}</dd><dt>执行副本</dt><dd>AngelBot 托管目录（原文件不变）</dd><dt>参数</dt><dd><pre>{scriptArgs.split(/\r\n|\r|\n/).filter(arg => arg.length > 0).join('\n') || '无'}</pre></dd><dt>工作目录</dt><dd>{workingDir.trim() || '副本目录'}</dd><dt>超时</dt><dd>{timeoutSeconds} 秒</dd></> : <><dt>归属空间</dt><dd>{agentWorkspace?.name}</dd><dt>任务说明</dt><dd><pre>{prompt.trim()}</pre></dd></>}</dl>
          <small>{executorKind === 'script' ? '只运行可信脚本；仅复制单个文件，不含依赖，按 Windows 用户权限运行。' : '沿用所选空间的模型配置与权限。'}暂停不会撤销已发生的操作。{triggerKind === 'startup' && '从下次 Windows 登录开始触发。'}</small>
        </div>
        <div className="automation-form-actions"><button type="button" className="secondary" disabled={saving} onClick={invalidateReview}>返回编辑</button><button type="button" disabled={saving || !canPreview || pickingPath || changingAutostart} onClick={() => void create()}>{saving ? '创建中…' : '创建任务'}</button></div>
      </section>}
      {error && <p className="automation-error" role="alert">{error}</p>}
      {target && loaded && !targetItem && <p className="automation-error" role="status">无法定位该事项；它可能已删除或归属工作区已改变。下方仍可查看其他任务。</p>}

      <section className="automation-list-section" aria-labelledby="automation-list-title">
        <div className="automation-list-title"><div><h2 id="automation-list-title">任务</h2><p>{items.length ? `${items.length} 条自动化任务` : '还没有已保存的任务'}</p></div><button className="automation-refresh" type="button" onClick={() => void load()}>刷新</button></div>
        {!loaded ? <p role="status">{error ? '任务尚未加载，请刷新重试。' : '正在读取自动化任务…'}</p> : items.length === 0 ? <div className="automation-empty"><strong>让重复的事，按时发生</strong><p>把重复事务交给 AngelBot，也可以手动添加通用脚本任务。</p><button type="button" className="secondary" onClick={() => setCreateOpen(true)}>添加自动化任务</button></div> : <div className="automation-grid">{items.map(item => {
          const latestRun = item.latestRun;
          const localAutomation = item.executorKind === 'script' || item.executorKind === 'maa';
          const activeLocalRun = localAutomation && latestRun?.status === 'running' ? latestRun : undefined;
          const activeAgentReceipt = item.executorKind === 'agent' && latestRun && ['running', 'queued', 'awaiting_confirmation'].includes(latestRun.status) ? latestRun : undefined;
          const pendingRun = pendingRunIds.includes(item.id);
          const localBusy = localAutomation && (!!activeLocalRun || pendingRun);
          const activeAgentRun = !!activeAgentReceipt;
          const unresolvedLocalRun = localAutomation && latestRun?.status === 'attention_required';
          const workspace = workspaces.find(candidate => candidate.id === item.workspaceId);
          const pausedAgent = item.executorKind === 'agent' && !item.enabled;
          const isReminder = item.executorKind === 'notification';
          const reminderCompleted = isReminder && !item.enabled && item.lastRunAt !== undefined;
          const reminderCancelled = isReminder && !item.enabled && item.lastRunAt === undefined;
          const reminderNeedsAttention = isReminder && latestRun?.status === 'needs_attention';
          const reminderSuppressed = isReminder && latestRun?.status === 'skipped';
          const presentation = statusPresentation(item, latestRun);
          const sourceLabel = item.executorKind === 'agent'
            ? workspace?.name || '已删除工作区'
            : isReminder
              ? `本地通知 · ${workspace?.name || 'AngelBot'}`
              : item.executorKind === 'maa'
                ? `MAA 当前任务队列 · ${workspace?.name || '已删除工作区'}`
                : item.scriptPath || '未配置脚本';
          const targeted = targetItem?.id === item.id;
          return <article key={item.id} ref={targeted ? targetCard : undefined} tabIndex={targeted ? -1 : undefined} aria-label={item.title} className={`automation-card${targeted ? ' automation-card-target' : ''}`}>
            <details className="automation-card-details">
              <summary className="automation-card-summary" aria-label={`${item.title} 详情`}>
                <h3 className="automation-card-title">{item.title}</h3>
                <div className="automation-card-meta">
                  <p className="automation-trigger">{item.triggerValue}</p>
                  <p className={`automation-path${item.executorKind === 'script' ? ' is-file' : ''}`} title={item.scriptPath}>{sourceLabel}</p>
                </div>
                <span className="automation-card-status">
                  <span className={`automation-status ${presentation.completed ? 'enabled' : presentation.attention ? 'attention' : ''}`}><i />{presentation.label}</span>
                  <span className="automation-card-chevron" aria-hidden="true">›</span>
                </span>
              </summary>
              <div className="automation-card-body">
                <dl><div><dt>{presentation.scheduleLabel}</dt><dd>{presentation.scheduleValue}</dd></div><div><dt>{activeAgentReceipt ? '当前任务' : '最近结果'}</dt><dd>{latestRun?.summary || '尚无运行记录'}</dd></div></dl>
                {presentation.notice && <p className="automation-state-note">{presentation.notice}</p>}
                {item.lastSkippedAt != null && <p className="automation-state-note">本次未启动 · {formatTime(item.lastSkippedAt)}；展开运行记录查看原因。</p>}
                <AutomationHistory automationId={item.id} revision={`${latestRun?.id}:${latestRun?.status}:${item.lastSkippedAt}`} />
                <div className="automation-actions">
                  <button className="secondary automation-delete" type="button" disabled={busyId === item.id || localBusy} aria-label={`删除 ${item.title}`} onClick={() => void perform(item.id, () => deleteAutomation(item.id))}>删除任务</button>
                  {presentation.workspaceAction && workspace && item.workspaceId && <button type="button" className="secondary automation-open-result" aria-label={`前往 ${workspace.kind === 'personal' ? 'AngelBot 日常' : workspace.name} ${presentation.workspaceAction}`} onClick={() => void openOwningWorkspace(item.workspaceId!)}>{presentation.workspaceAction}</button>}
                  {(activeLocalRun?.stopControl === 'available' || activeLocalRun?.stopControl === 'requested') && <button type="button" className="secondary" disabled={busyId === item.id || activeLocalRun.stopControl === 'requested'} onClick={() => void perform(item.id, () => stopAutomationRun(item.id, activeLocalRun.id))}>{activeLocalRun.stopControl === 'requested' ? '正在停止' : '停止本次执行'}</button>}
                  <button type="button" className="secondary" disabled={busyId === item.id || pendingRun || !!activeLocalRun || !!activeAgentRun || pausedAgent} onClick={() => void perform(item.id, () => runAutomationNow(item.id), true)}>{activeAgentRun ? presentation.label : activeLocalRun ? '等待结果' : pendingRun || busyId === item.id ? '处理中…' : pausedAgent ? '先启用再执行' : unresolvedLocalRun ? '核对后重新运行' : item.executorKind === 'agent' ? '立即执行' : isReminder ? '立即提醒' : '立即运行'}</button>
                  <button type="button" className="secondary" disabled={busyId === item.id || reminderCompleted || reminderNeedsAttention || reminderSuppressed || (!item.enabled && unresolvedLocalRun)} onClick={() => void perform(item.id, () => setAutomationEnabled(item.id, !item.enabled))}>{reminderNeedsAttention ? '需检查通知' : reminderSuppressed ? '本次未发送' : reminderCompleted ? '已完成' : item.enabled ? '暂停' : reminderCancelled ? '恢复' : '启用'}</button>
                </div>
              </div>
            </details>
          </article>;
        })}</div>}
      </section>
    </main>
  );
}

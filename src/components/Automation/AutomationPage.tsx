import { useCallback, useEffect, useRef, useState } from 'react';
import { createAutomation, createMaaAutomation, deleteAutomation, getAutomationDashboard, runAutomationNow, setAutomationEnabled, stopAutomationRun, type Automation, type AutomationDashboardItem, type AutomationRunSummary } from '$lib/commands/automation';
import { getDesktopTrustedApps, type TrustedDesktopApp } from '$lib/commands/desktop';
import { getWorkspaces, type Workspace } from '$lib/commands/workspace';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspacesStore } from '$stores/workspaces';
import { subscribeAutomationUpdates } from '$lib/automation-events';
import { normalizeDailySchedule, parseAutomationIntent, titleFromPrompt, type AutomationIntent } from './parseAutomationIntent';
import { AutomationHistory } from './AutomationHistory';
import './AutomationPage.css';

const formatTime = (value?: number | null) => value ? new Date(value * 1000).toLocaleString() : '等待设置';

const FREQUENCY_OPTIONS = ['每天'];
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
        scheduleValue: item.enabled ? formatTime(item.nextRunAt) : '已暂停',
        notice: '本次受控运行已停止，已发生的操作不会撤回。后续计划请以下次执行状态为准。',
      };
    }
    if (latestRun.status === 'failed' || latestRun.status === 'timed_out') {
      return {
        label: latestRun.status === 'failed' ? (isMaa ? '存在失败' : '执行失败') : '已超时', attention: true,
        scheduleLabel: '下次执行', scheduleValue: item.enabled ? formatTime(item.nextRunAt) : '已暂停',
      };
    }
    if (latestRun.status === 'completed') {
      return {
        label: isMaa ? '队列已结束' : '脚本已结束', scheduleLabel: '下次执行',
        scheduleValue: item.enabled ? formatTime(item.nextRunAt) : '已暂停', completed: true,
        notice: isMaa ? '记录的是 MAA 队列执行结果，不代表游戏业务目标全部完成。' : '记录的是脚本结束结果，不代表外部应用的业务目标全部完成。',
      };
    }
  }
  if (item.executorKind === 'agent' && latestRun) {
    const schedule = (currentState: string) => item.triggerKind === 'once'
      ? { scheduleLabel: '本次状态', scheduleValue: currentState }
      : { scheduleLabel: '下次执行', scheduleValue: item.enabled ? formatTime(item.nextRunAt) : '已暂停' };
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
    scheduleValue: item.enabled ? formatTime(item.nextRunAt) : reminderCompleted ? '已完成' : reminderCancelled ? '已取消' : '已暂停',
    completed: item.enabled || reminderCompleted,
    notice: reminderCompleted ? '已处理本次提醒触发，不代表 Windows 通知已送达或已被查看。' : undefined,
  };
}

export function AutomationPage() {
  const [items, setItems] = useState<AutomationDashboardItem[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [localExecutor, setLocalExecutor] = useState<'script' | 'maa'>('script');
  const [maaApps, setMaaApps] = useState<TrustedDesktopApp[]>([]);
  const [maaAppsError, setMaaAppsError] = useState<string | null>(null);
  const [maaAppId, setMaaAppId] = useState('');
  const [maaWorkspaceId, setMaaWorkspaceId] = useState('');
  const [maaLaunchEmulator, setMaaLaunchEmulator] = useState(false);
  const [maaConfirmed, setMaaConfirmed] = useState(false);
  const [title, setTitle] = useState('');
  const [triggerValue, setTriggerValue] = useState('每天 09:00');
  const [scriptPath, setScriptPath] = useState('');
  const [scriptArgs, setScriptArgs] = useState('');
  const [workingDir, setWorkingDir] = useState('');
  const [timeoutSeconds, setTimeoutSeconds] = useState('300');
  const [review, setReview] = useState(false);
  const [saving, setSaving] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  // Only requests owned by this page; persisted receipts remain authoritative.
  const [pendingRunIds, setPendingRunIds] = useState<string[]>([]);
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
  const target = useNavigationStore((state) => state.automationTarget);
  const openWorkspace = useWorkspacesStore((state) => state.openWorkspace);
  const loadGeneration = useRef(0);
  const targetCard = useRef<HTMLElement>(null);
  const focusedTarget = useRef<typeof target>(null);
  const targetItem = target && items.find(item => item.id === target.automationId && item.workspaceId === target.workspaceId);

  useEffect(() => {
    if (!target || !targetItem || focusedTarget.current === target) return;
    targetCard.current?.scrollIntoView?.({ block: 'nearest', behavior: 'auto' });
    targetCard.current?.focus({ preventScroll: true });
    focusedTarget.current = target;
  }, [target, targetItem]);

  const load = useCallback(async () => {
    const generation = ++loadGeneration.current;
    try {
      setError(null);
      const [automations, availableWorkspaces, appDiscovery] = await Promise.all([
        getAutomationDashboard(), getWorkspaces(),
        getDesktopTrustedApps().then(apps => ({ apps, error: null }))
          .catch(cause => ({ apps: [], error: errorMessage(cause, '无法读取受信任应用，请在设置中检查后刷新。') })),
      ]);
      if (generation !== loadGeneration.current) return;
      setItems(automations.items);
      setLoaded(true);
      setWorkspaces(availableWorkspaces);
      setNlWorkspaceId(current => current || availableWorkspaces.find(workspace => workspace.kind === 'personal')?.id || availableWorkspaces[0]?.id || '');
      setMaaWorkspaceId(current => availableWorkspaces.some(workspace => workspace.id === current) ? current : availableWorkspaces.find(workspace => workspace.kind === 'personal')?.id || availableWorkspaces[0]?.id || '');
      const compatibleApps = appDiscovery.apps.filter(app => app.enabled && app.capabilities.includes('launch') && app.executablePath.split(/[\\/]/).pop()?.toLowerCase() === 'maa.exe');
      setMaaApps(compatibleApps);
      setMaaAppsError(appDiscovery.error);
      setMaaAppId(current => compatibleApps.some(app => app.id === current) ? current : compatibleApps[0]?.id || '');
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

  useEffect(() => { setMaaConfirmed(false); }, [maaAppId, maaWorkspaceId]);

  const hasActiveRun = pendingRunIds.length > 0 || items
    .some(item => item.latestRun?.status === 'queued' || item.latestRun?.status === 'running');
  useEffect(() => {
    if (!hasActiveRun) return undefined;
    const timer = window.setInterval(() => { void load(); }, 5_000);
    return () => window.clearInterval(timer);
  }, [hasActiveRun, load]);

  const invalidateReview = () => { setReview(false); setMaaConfirmed(false); };
  const selectedMaaApp = maaApps.find(app => app.id === maaAppId);
  const canPreview = !!title.trim() && (localExecutor === 'maa' ? !!selectedMaaApp && !!maaWorkspaceId : !!scriptPath.trim());

  const create = async () => {
    if (!canPreview || (localExecutor === 'maa' && !maaConfirmed)) return;
    const dailyTriggerValue = normalizeDailySchedule(triggerValue);
    if (!dailyTriggerValue) {
      setError('重复自动化目前只支持每天，请填写「每天 HH:MM」，例如「每天 09:00」。一次性提醒请在对话中创建。');
      return;
    }
    const args = scriptArgs.split(/\r\n|\r|\n/).filter(arg => arg.length > 0);
    const timeout = Number(timeoutSeconds);
    if (!Number.isInteger(timeout) || timeout < 10 || timeout > 3600) {
      setError('超时时间必须是 10 到 3600 秒之间的整数。');
      return;
    }
    setSaving(true);
    try {
      if (localExecutor === 'maa') {
        await createMaaAutomation({
          title: title.trim(), triggerValue: dailyTriggerValue, workspaceId: maaWorkspaceId,
          appId: maaAppId, launchEmulator: maaLaunchEmulator, timeoutSeconds: timeout,
        });
        setMaaLaunchEmulator(false);
      } else {
        await createAutomation({
          title: title.trim(), prompt: '', triggerKind: 'schedule', triggerValue: dailyTriggerValue,
          permissionSummary: SCRIPT_PERMISSION_SUMMARY,
          executorKind: 'script', scriptPath: scriptPath.trim(), scriptArgs: args,
          workingDir: workingDir.trim() || undefined, timeoutSeconds: timeout,
        });
        setScriptPath(''); setScriptArgs(''); setWorkingDir('');
      }
      setTitle(''); invalidateReview();
      await load();
    } catch (cause) {
      setError(errorMessage(cause, '无法创建自动化任务。'));
    } finally {
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
        permissionSummary: nlExecutor === 'script' ? SCRIPT_PERMISSION_SUMMARY : '按归属工作区的 Agent 操作权限执行',
        executorKind: nlExecutor,
        workspaceId: nlExecutor === 'agent' ? nlWorkspaceId : undefined,
        scriptPath: nlExecutor === 'script' ? nlScriptPath.trim() : undefined,
        scriptArgs: [], workingDir: undefined, timeoutSeconds: 300,
      });
      setNlText(''); setNlDraft(null); setNlScriptPath('');
      setNlSuccess(`已创建自动化任务「${title}」。`);
      await load();
    } catch (cause) {
      setNlError(errorMessage(cause, '无法创建自动化任务。'));
    } finally {
      setNlSaving(false);
    }
  };

  return (
    <main className="automation-page" aria-label="自动化">
      <header className="automation-header">
        <div><h1>自动化</h1><p>每日任务仅在 AngelBot 运行时检查执行；固定脚本与 MAA 当前队列不会调用 AI，需判断的任务进入所选工作区的主 Agent 队列。</p></div>
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
              <label className="wide">脚本路径<input value={nlScriptPath} onChange={e => setNlScriptPath(e.target.value)} placeholder="工作目录\scripts\daily.py" /><small>按已保存的配置执行，不会在每次运行前询问；如需参数、工作目录或超时设置，请使用下方脚本表单。</small><small>{SCRIPT_LIFECYCLE_NOTICE}</small></label>
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
        <div className="automation-section-head"><div><h2 id="automation-create-title">新建任务</h2><p>{localExecutor === 'maa' ? '按需启动可信 MAA，运行其当前保存且已勾选的任务队列。' : '脚本与工作目录必须位于当前工作目录内。'}</p></div><small>不会调用 AI</small></div>
        <p className="automation-script-guidance">{localExecutor === 'maa'
          ? '按需启动 MAA 6.18 的当前任务队列，不修改任务或消耗设置。需关闭 MAA 原生定时、启动即运行及自动重启模拟器；不允许关闭既有模拟器、关机或休眠等结束动作。队列结束不代表游戏目标全部完成，暂停只影响后续定时。'
          : '运行结果依据脚本退出码，不表示软件打开或日常任务已完成。GUI 辅助工具需要支持按需启动与明确的任务控制接口；本页只执行等待自身任务结束的脚本，仅启动程序不能验证其任务结果。暂停仅影响后续定时执行，不会停止已启动的脚本。脚本不会继承 AngelBot 的模型密钥等环境变量。'}</p>
        <div className="automation-form automation-script-form">
          <label>任务类型<select value={localExecutor} onChange={e => {
            const kind = e.target.value === 'maa' ? 'maa' : 'script';
            setLocalExecutor(kind); setTimeoutSeconds(kind === 'maa' ? '3600' : '300');
            setTriggerValue(kind === 'maa' ? '每天 18:00' : '每天 09:00'); invalidateReview();
          }}><option value="script">运行本地脚本</option><option value="maa">MAA 当前任务队列</option></select></label>
          <label>任务名称<input value={title} onChange={e => { setTitle(e.target.value); invalidateReview(); }} placeholder={localExecutor === 'maa' ? '例如：MAA 每日队列' : '例如：整理下载目录'} /></label>
          <label>触发时间<input value={triggerValue} onChange={e => { setTriggerValue(e.target.value); invalidateReview(); }} placeholder="每天 09:00" /><small>仅支持每日重复，按此电脑的系统时区执行。</small></label>
          {localExecutor === 'maa' ? <>
            <label className="wide">受信任的 MAA 应用<select value={maaAppId} onChange={e => { setMaaAppId(e.target.value); invalidateReview(); }} disabled={!maaApps.length}>
              {!maaApps.length && <option value="">没有可用的 MAA 应用</option>}
              {maaApps.map(app => <option key={app.id} value={app.id}>{app.displayName}</option>)}
            </select><small>仅显示已启用、允许启动且可执行文件名为 MAA.exe 的受信任应用。</small></label>
            {(!maaApps.length || maaAppsError) && <p className="automation-state-note">{maaAppsError || '请在 设置 → 电脑操作 → 受信任应用 中添加 MAA.exe 并允许启动，然后刷新此页。'}</p>}
            <label className="wide">归属工作区<select value={maaWorkspaceId} onChange={e => { setMaaWorkspaceId(e.target.value); invalidateReview(); }}>
              {workspaces.map(workspace => <option key={workspace.id} value={workspace.id}>{workspace.kind === 'personal' ? '个人空间' : workspace.name}</option>)}
            </select><small>此任务及运行记录归属于所选工作区。</small></label>
            <label className="wide automation-checkbox"><input type="checkbox" checked={maaLaunchEmulator} onChange={e => { setMaaLaunchEmulator(e.target.checked); invalidateReview(); }} /><span>按 MAA 当前配置启动模拟器<small>默认不启动；开启时使用 MAA 已保存的模拟器启动配置，关闭时需已有可连接的模拟器。</small></span></label>
          </> : <>
            <label className="wide">脚本路径<input value={scriptPath} onChange={e => { setScriptPath(e.target.value); invalidateReview(); }} placeholder="工作目录\scripts\daily.py" /></label>
            <label className="automation-args">参数（每行一个）<textarea value={scriptArgs} onChange={e => { setScriptArgs(e.target.value); invalidateReview(); }} rows={3} placeholder={'--profile\nC:\\Game Tools\\daily.json'} /><small>每行作为一个完整参数，空行忽略，不需要额外加引号。批处理（.bat/.cmd）不接受引号或 shell 元字符。</small></label>
            <label>脚本工作目录（可选）<input value={workingDir} onChange={e => { setWorkingDir(e.target.value); invalidateReview(); }} placeholder="默认使用脚本所在目录" /></label>
          </>}
          <label>超时（秒）<input inputMode="numeric" value={timeoutSeconds} onChange={e => { setTimeoutSeconds(e.target.value); invalidateReview(); }} /></label>
          <button type="button" disabled={!canPreview} onClick={() => {
            const dailyTriggerValue = normalizeDailySchedule(triggerValue);
            if (!dailyTriggerValue) {
              setError('重复自动化目前只支持每天，请填写「每天 HH:MM」，例如「每天 09:00」。一次性提醒请在对话中创建。');
              return;
            }
            setError(null); setTriggerValue(dailyTriggerValue); setMaaConfirmed(false); setReview(true);
          }}>预览规则</button>
        </div>
      </section>

      {review && <section className="automation-review" role="status">
        <div><strong>确认创建任务</strong><p>{title} · {triggerValue}</p>{localExecutor === 'maa' ? <>
          <small>{selectedMaaApp?.displayName} · {workspaces.find(workspace => workspace.id === maaWorkspaceId)?.name} · {maaLaunchEmulator ? '按 MAA 配置启动模拟器' : '不启动模拟器'} · 超时 {timeoutSeconds} 秒</small>
          <label className="automation-checkbox"><input type="checkbox" checked={maaConfirmed} onChange={e => setMaaConfirmed(e.target.checked)} /><span>我确认无人值守运行 MAA 当前保存且已勾选的任务队列，并允许结束或超时后清理本次启动的 MAA 和模拟器进程树；不接管原先打开的 MAA 或关闭原先运行的模拟器进程。游戏操作仍由 MAA 当前队列执行。<small>创建后按计划运行，不会每次再询问。AngelBot 必须保持运行。</small></span></label>
        </> : <small>{SCRIPT_LIFECYCLE_NOTICE}</small>}</div>
        <div><button type="button" className="secondary" onClick={invalidateReview}>返回编辑</button><button type="button" disabled={saving || !canPreview || (localExecutor === 'maa' && !maaConfirmed)} onClick={() => void create()}>{saving ? '创建中…' : localExecutor === 'maa' ? '确认并创建 MAA 任务' : '创建任务'}</button></div>
      </section>}
      {error && <p className="automation-error" role="alert">{error}</p>}
      {target && loaded && !targetItem && <p className="automation-error" role="status">无法定位该事项；它可能已删除或归属工作区已改变。下方仍可查看其他任务。</p>}

      <section className="automation-list-section" aria-labelledby="automation-list-title">
        <div className="automation-list-title"><div><h2 id="automation-list-title">任务</h2><p>{items.length ? `${items.length} 条自动化任务` : '还没有已保存的任务'}</p></div><button className="automation-refresh" type="button" onClick={() => void load()}>刷新</button></div>
        {!loaded ? <p role="status">{error ? '任务尚未加载，请刷新重试。' : '正在读取自动化任务…'}</p> : items.length === 0 ? <div className="automation-empty"><strong>还没有自动化任务</strong><p>固定脚本可独立运行；Agent 任务需要先选择归属工作区。</p></div> : <div className="automation-grid">{items.map(item => {
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
            ? `AngelBot · ${workspace?.name || '已删除工作区'}`
            : isReminder
              ? `本地通知 · ${workspace?.name || 'AngelBot'}`
              : item.executorKind === 'maa'
                ? `MAA 当前任务队列 · ${workspace?.name || '已删除工作区'}`
                : item.scriptPath || '未配置脚本';
          const targeted = targetItem?.id === item.id;
          return <article key={item.id} ref={targeted ? targetCard : undefined} tabIndex={targeted ? -1 : undefined} aria-label={item.title} className={`automation-card${targeted ? ' automation-card-target' : ''}`}>
            <div className="automation-card-head"><span className={`automation-status ${presentation.completed ? 'enabled' : presentation.attention ? 'attention' : ''}`}><i />{presentation.label}</span><button className="automation-delete" type="button" disabled={busyId === item.id || localBusy} aria-label={`删除 ${item.title}`} onClick={() => void perform(item.id, () => deleteAutomation(item.id))}>×</button></div>
            <h3>{item.title}</h3><p className="automation-trigger">{item.triggerValue}</p><p className="automation-path" title={item.scriptPath}>{sourceLabel}</p>
            <dl><div><dt>{presentation.scheduleLabel}</dt><dd>{presentation.scheduleValue}</dd></div><div><dt>{activeAgentReceipt ? '当前任务' : '最近结果'}</dt><dd>{latestRun?.summary || '尚无运行记录'}</dd></div></dl>
            {presentation.notice && <p className="automation-state-note">{presentation.notice}</p>}
            {item.lastSkippedAt != null && <p className="automation-state-note">本次未启动 · {formatTime(item.lastSkippedAt)}；展开运行记录查看原因。</p>}
            <AutomationHistory automationId={item.id} revision={`${latestRun?.id}:${latestRun?.status}:${item.lastSkippedAt}`} />
            <div className="automation-actions">
              {presentation.workspaceAction && workspace && item.workspaceId && <button type="button" className="secondary" onClick={() => void openOwningWorkspace(item.workspaceId!)}>前往 {workspace.kind === 'personal' ? 'AngelBot 日常' : workspace.name} {presentation.workspaceAction}</button>}
              {(activeLocalRun?.stopControl === 'available' || activeLocalRun?.stopControl === 'requested') && <button type="button" className="secondary" disabled={busyId === item.id || activeLocalRun.stopControl === 'requested'} onClick={() => void perform(item.id, () => stopAutomationRun(item.id, activeLocalRun.id))}>{activeLocalRun.stopControl === 'requested' ? '正在停止' : '停止本次执行'}</button>}
              <button type="button" disabled={busyId === item.id || pendingRun || !!activeLocalRun || !!activeAgentRun || pausedAgent} onClick={() => void perform(item.id, () => runAutomationNow(item.id), true)}>{activeAgentRun ? presentation.label : activeLocalRun ? '等待结果' : pendingRun || busyId === item.id ? '处理中…' : pausedAgent ? '先启用再执行' : unresolvedLocalRun ? '核对后重新运行' : item.executorKind === 'agent' ? '立即执行' : isReminder ? '立即提醒' : '立即运行'}</button>
              <button type="button" className="secondary" disabled={busyId === item.id || reminderCompleted || reminderNeedsAttention || reminderSuppressed || (!item.enabled && unresolvedLocalRun)} onClick={() => void perform(item.id, () => setAutomationEnabled(item.id, !item.enabled))}>{reminderNeedsAttention ? '需检查通知' : reminderSuppressed ? '本次未发送' : reminderCompleted ? '已完成' : item.enabled ? '暂停' : reminderCancelled ? '恢复' : '启用'}</button>
            </div>
          </article>;
        })}</div>}
      </section>
    </main>
  );
}

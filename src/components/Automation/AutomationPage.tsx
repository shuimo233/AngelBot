import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { createAutomation, createMaaAutomation, deleteAutomation, getAutomationDashboard, runAutomationNow, setAutomationEnabled, stopAutomationRun, type Automation, type AutomationDashboardItem, type AutomationRunRequest, type AutomationRunSummary } from '$lib/commands/automation';
import { getDesktopTrustedApps, type TrustedDesktopApp } from '$lib/commands/desktop';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { getAutostart, setAutostart } from '$lib/commands/settings';
import { getWorkspaces, type Workspace } from '$lib/commands/workspace';
import { useNavigationStore } from '$stores/navigation';
import { useMessagesStore } from '$stores/messages';
import { useWorkspacesStore } from '$stores/workspaces';
import { subscribeAutomationUpdates } from '$lib/automation-events';
import { createProjectionRefresh } from '$lib/projection-refresh';
import { normalizeDailySchedule, previewNextDailyRun } from './dailySchedule';
import { AutomationHistory } from './AutomationHistory';
import { AutomationTimePicker } from './AutomationTimePicker';
import { Button, Dialog, RadioChoices } from '../../ui';
import './AutomationPage.css';

const formatTime = (value?: number | null) => value ? new Date(value * 1000).toLocaleString() : '等待设置';
const nextTrigger = (item: Automation) => !item.enabled ? '已暂停' : item.triggerKind === 'startup' ? '下次 Windows 登录' : formatTime(item.nextRunAt);

const SCRIPT_PERMISSION_SUMMARY = '仅执行已配置的本地脚本；输出和退出码会保存在运行记录中。';
const SCRIPT_LIFECYCLE_NOTICE = '脚本结束或超时会回收其自有进程树并保留日志，不影响原先打开的软件。GUI 辅助工具需要明确的任务控制接口；仅启动软件后退出的脚本不代表其中任务已执行。';
const executorOptions = [{ value: 'agent', label: '交给 AngelBot' }, { value: 'script', label: '运行本地脚本' }, { value: 'maa', label: '明日方舟日常（MAA）' }] as const;
const triggerOptions = [{ value: 'schedule', label: '每天定时' }, { value: 'startup', label: '登录 Windows 时' }] as const;
const maaDefaultTitle = '明日方舟日常';
const canLaunchMaa = (app: TrustedDesktopApp) => app.enabled && app.capabilities.includes('launch');
const errorMessage = (cause: unknown, fallback: string) =>
  cause instanceof Error ? cause.message : typeof cause === 'string' ? cause : fallback;

const pendingRunsKey = 'angelbot.automation.pending-runs';
const pendingRunsChanged = 'angelbot:automation-pending-runs-changed';
const maxPendingRuns = 100;
const sameRequest = (a: AutomationRunRequest, b: AutomationRunRequest) =>
  a.automationId === b.automationId && a.runId === b.runId;
let pendingRunsValue = '';
let pendingRuns: AutomationRunRequest[] = [];
let pendingRunsStorageUnavailable = false;

function getPendingRuns() {
  if (pendingRunsStorageUnavailable) return pendingRuns;
  try {
    const value = sessionStorage.getItem(pendingRunsKey) ?? '';
    if (value !== pendingRunsValue) {
      const parsed: unknown = value ? JSON.parse(value) : [];
      pendingRuns = Array.isArray(parsed) ? parsed.filter((item): item is AutomationRunRequest =>
        typeof item?.automationId === 'string' && typeof item?.runId === 'string').slice(0, maxPendingRuns) : [];
      pendingRunsValue = value;
    }
  } catch { pendingRunsStorageUnavailable = true; }
  return pendingRuns;
}

function updatePendingRuns(requests: AutomationRunRequest[]) {
  pendingRuns = requests;
  pendingRunsValue = JSON.stringify(requests);
  try { sessionStorage.setItem(pendingRunsKey, pendingRunsValue); } catch { pendingRunsStorageUnavailable = true; }
  window.dispatchEvent(new Event(pendingRunsChanged));
}

function subscribePendingRuns(listener: () => void) {
  window.addEventListener(pendingRunsChanged, listener);
  return () => window.removeEventListener(pendingRunsChanged, listener);
}

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
        label: '结果未知', scheduleLabel: '后续定时', scheduleValue: '已暂停，不会自动重试', attention: true,
        notice: `先核对${isMaa ? ' MAA ' : '脚本'}的实际状态，确认本次已结束且不会重复操作后再运行。`,
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
        label: latestRun.status === 'failed' ? (isMaa ? '队列有失败' : '执行失败') : '已超时', attention: true,
        scheduleLabel: '下次执行', scheduleValue: nextTrigger(item),
        notice: isMaa ? '先查看运行记录，再到 MAA 核对失败项。' : undefined,
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
  const [executorKind, setExecutorKind] = useState<(typeof executorOptions)[number]['value']>('agent');
  const [prompt, setPrompt] = useState('');
  const [workspaceId, setWorkspaceId] = useState(useWorkspacesStore.getState().activeWorkspaceId || '');
  const [autostart, setAutostartReady] = useState<boolean | null>(null);
  const [changingAutostart, setChangingAutostart] = useState(false);
  const autostartInFlight = useRef(false);
  const [scriptPath, setScriptPath] = useState('');
  const [scriptArgs, setScriptArgs] = useState('');
  const [workingDir, setWorkingDir] = useState('');
  const [maaApps, setMaaApps] = useState<TrustedDesktopApp[] | 'error' | null>(null);
  const [maaAppId, setMaaAppId] = useState('');
  const [launchEmulator, setLaunchEmulator] = useState(false);
  const [maaRevision, setMaaRevision] = useState(0);
  const [timeoutSeconds, setTimeoutSeconds] = useState('300');
  const [review, setReview] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [scriptDialog, setScriptDialog] = useState<'options' | 'instructions' | null>(null);
  const [startupRevision, setStartupRevision] = useState(0);
  const [pickingPath, setPickingPath] = useState(false);
  const pickerInFlight = useRef(false);
  const createInFlight = useRef(false);
  const [saving, setSaving] = useState(false);
  const actionRequestsInFlight = useRef(new Set<string>());
  const [busyIds, setBusyIds] = useState<ReadonlySet<string>>(() => new Set());
  // Only requests owned by this page; persisted receipts remain authoritative.
  const [pendingRunIds, setPendingRunIds] = useState<string[]>([]);
  const runRequestsInFlight = useRef(new Set<string>());
  const unconfirmedRequests = useSyncExternalStore(subscribePendingRuns, getPendingRuns);
  const [actionError, setError] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const error = actionError ?? loadError;
  const setPage = useNavigationStore((state) => state.setPage);
  const target = useNavigationStore((state) => state.automationTarget);
  const openWorkspace = useWorkspacesStore((state) => state.openWorkspace);
  const targetCard = useRef<HTMLElement>(null);
  const focusedTarget = useRef<typeof target>(null);
  const pageTop = useRef<HTMLElement>(null);
  const stepTitle = useRef<HTMLHeadingElement>(null);
  const targetItem = target && items.find(item => item.id === target.automationId && item.workspaceId === target.workspaceId);

  useEffect(() => {
    if (!createOpen) {
      setScriptDialog(null);
      return;
    }
    pageTop.current?.scrollIntoView?.({ block: 'start' });
    stepTitle.current?.focus({ preventScroll: true });
  }, [createOpen, review]);

  useEffect(() => {
    if (!target || !targetItem || focusedTarget.current === target) return;
    if (createOpen) {
      if (!saving && !pickingPath && !changingAutostart) { setCreateOpen(false); setReview(false); }
      return;
    }
    if (!targetCard.current) return;
    const details = targetCard.current?.querySelector('details');
    if (details) details.open = true;
    targetCard.current?.scrollIntoView?.({ block: 'nearest', behavior: 'auto' });
    targetCard.current?.focus({ preventScroll: true });
    focusedTarget.current = target;
  }, [target, targetItem, createOpen, saving, pickingPath, changingAutostart]);

  const [reads] = useState(() => createProjectionRefresh({
    read: async (_: null) => {
      // Wait for both branches even on failure, so the trailing refresh cannot
      // start a second copy of an IPC read that is still in flight.
      const [dashboard, workspaces] = await Promise.allSettled([
        getAutomationDashboard(getPendingRuns()), getWorkspaces(),
      ]);
      if (dashboard.status === 'rejected' || workspaces.status === 'rejected') throw new Error('dashboard unavailable');
      return [dashboard.value, workspaces.value] as const;
    },
    onSuccess: (_, [automations, availableWorkspaces]) => {
      setLoadError(null);
      setItems(automations.items);
      const pending = getPendingRuns();
      const unresolved = pending.filter(request => !automations.acceptedRequests.some(receipt => sameRequest(request, receipt)));
      if (pending.length !== unresolved.length) updatePendingRuns(unresolved);
      setLoaded(true);
      setWorkspaces(availableWorkspaces);
    },
    onError: () => setLoadError('无法加载自动化任务。请稍后重试。'),
  }));
  const load = () => reads.request(null);

  useEffect(() => {
    const refresh = () => { void reads.request(null); };
    const refreshVisible = () => { if (document.visibilityState !== 'hidden') refresh(); };
    refresh();
    const unsubscribe = subscribeAutomationUpdates(refresh);
    window.addEventListener('focus', refreshVisible);
    document.addEventListener('visibilitychange', refreshVisible);
    return () => {
      reads.invalidate();
      unsubscribe();
      window.removeEventListener('focus', refreshVisible);
      document.removeEventListener('visibilitychange', refreshVisible);
    };
  }, [reads]);

  const hasActiveRun = pendingRunIds.length > 0 || items
    .some(item => item.latestRun?.status === 'queued' || item.latestRun?.status === 'running');
  useEffect(() => {
    if (!hasActiveRun) return undefined;
    const timer = window.setInterval(() => {
      if (document.visibilityState !== 'hidden' && !reads.isPending()) void reads.request(null);
    }, 5_000);
    return () => window.clearInterval(timer);
  }, [hasActiveRun, reads]);

  const invalidateReview = () => { setReview(false); };
  const startInConversation = () => {
    const { activeWorkspaceId, workspaces: availableWorkspaces } = useWorkspacesStore.getState();
    const sessionId = availableWorkspaces.find(workspace => workspace.id === activeWorkspaceId)?.activeSessionId;
    if (sessionId) {
      const messages = useMessagesStore.getState();
      messages.activateInput(sessionId);
      if (!useMessagesStore.getState().input.trim()) messages.setInput('帮我设一个提醒：');
    }
    setPage('chat');
  };
  const isMaa = executorKind === 'maa';
  const triggerLabel = triggerKind === 'startup' ? '下次登录 Windows 时触发' : normalizeDailySchedule(triggerValue) || '请选择时间';
  const nextDailyRun = review && triggerKind === 'schedule' ? previewNextDailyRun(triggerValue) : null;
  const agentWorkspace = workspaces.find(workspace => workspace.id === workspaceId);
  const canPreview = !!title.trim() && (triggerKind !== 'startup' || autostart === true)
    && (executorKind === 'script' ? !!scriptPath.trim() : !!prompt.trim() && !!agentWorkspace);
  const maaCandidates = Array.isArray(maaApps) ? maaApps : [];
  const canSaveMaa = !!agentWorkspace && maaCandidates.some(app => app.id === maaAppId && canLaunchMaa(app));

  useEffect(() => {
    if (!createOpen || !isMaa) return;
    let active = true;
    setMaaApps(null);
    void getDesktopTrustedApps().then(apps => {
      if (active) setMaaApps(apps.filter(app => app.executablePath.split(/[\\/]/).pop()?.toLowerCase() === 'maa.exe'));
    }).catch(() => { if (active) setMaaApps('error'); });
    return () => { active = false; };
  }, [createOpen, isMaa, maaRevision]);

  useEffect(() => {
    if (!createOpen || isMaa || triggerKind !== 'startup') return;
    let active = true;
    setAutostartReady(null);
    void getAutostart().then(value => { if (active) setAutostartReady(value); })
      .catch(cause => { if (active) setError(errorMessage(cause, '无法读取开机启动状态，请重试。')); });
    return () => { active = false; };
  }, [createOpen, isMaa, triggerKind, startupRevision]);

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
    if (createInFlight.current || (isMaa ? !canSaveMaa : !canPreview || !review)) return;
    const dailyTriggerValue = !isMaa && triggerKind === 'startup' ? '登录 Windows 后' : normalizeDailySchedule(triggerValue);
    if (!dailyTriggerValue) {
      setError('请选择有效的每日执行时间。');
      return;
    }
    const args = scriptArgs.split(/\r\n|\r|\n/).filter(arg => arg.length > 0);
    const timeout = executorKind === 'agent' ? 300 : Number(timeoutSeconds);
    if (!isMaa && (!Number.isInteger(timeout) || timeout < 10 || timeout > 3600)) {
      setError('超时时间必须是 10 到 3600 秒之间的整数。');
      return;
    }
    createInFlight.current = true;
    setSaving(true);
    setError(null);
    try {
      if (isMaa) {
        const saved = await createMaaAutomation({ title: title.trim() || maaDefaultTitle,
          triggerValue: dailyTriggerValue, workspaceId, appId: maaAppId, launchEmulator });
        setItems(previous => previous.some(item => item.id === saved.id) ? previous
          : [...previous, { ...saved, latestRun: null, lastSkippedAt: null }]);
        setLoaded(true);
        setMaaAppId(''); setLaunchEmulator(false);
      } else await createAutomation({
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

  const perform = async (id: string, action: () => Promise<void>) => {
    const active = actionRequestsInFlight.current;
    if (active.has(id)) return;
    active.add(id);
    setBusyIds(new Set(active));
    setError(null);
    try {
      await action();
      await load();
    } catch (cause) {
      setError(errorMessage(cause, '自动化操作失败，请重试。'));
    } finally {
      active.delete(id);
      setBusyIds(new Set(active));
    }
  };

  const runNow = async (id: string) => {
    const pending = getPendingRuns();
    if (actionRequestsInFlight.current.has(id) || runRequestsInFlight.current.has(id)
      || pending.some(request => request.automationId === id)) return;
    if (pending.length >= maxPendingRuns) {
      setError('待核对的自动化运行过多，请先刷新运行记录。');
      return;
    }
    setError(null);
    const request = { automationId: id, runId: crypto.randomUUID() };
    runRequestsInFlight.current.add(id);
    setPendingRunIds(current => [...current, id]);
    updatePendingRuns([...pending, request]);
    try {
      const result = await runAutomationNow(id, request.runId);
      const remaining = getPendingRuns().filter(candidate => !sameRequest(request, candidate));
      updatePendingRuns(remaining);
      await load();
      if (result.status === 'rejected') setError(result.error);
    } catch (cause) {
      await load();
      if (getPendingRuns().some(candidate => sameRequest(request, candidate))) {
        setError(errorMessage(cause, '执行请求未收到确认，请刷新查看运行记录。'));
      }
    } finally {
      runRequestsInFlight.current.delete(id);
      setPendingRunIds(current => current.filter(candidate => candidate !== id));
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
    <main ref={pageTop} className={`automation-page${createOpen ? ' automation-page--editing' : ''}`} aria-label="自动化">
      <header className="automation-header">
        <h1 id="automation-page-title" ref={createOpen && !review ? stepTitle : undefined} tabIndex={createOpen && !review ? -1 : undefined}>{createOpen ? '添加自动化' : '自动化'}</h1>
        <div className="automation-header-actions">
          {!createOpen && <button type="button" className="secondary" onClick={startInConversation}>在对话里交代</button>}
          <button type="button" className={`automation-create-toggle${createOpen ? ' secondary' : ''}`} disabled={saving || pickingPath || changingAutostart} aria-expanded={createOpen} onClick={() => { setCreateOpen(open => !open); setError(null); invalidateReview(); }}>{createOpen ? '返回任务' : '添加自动化'}</button>
        </div>
      </header>
      {createOpen && !review && <section className="automation-create-card" aria-labelledby="automation-page-title">
        <fieldset className="automation-form automation-script-form" aria-label="自动化配置" disabled={saving || pickingPath || changingAutostart}>
          <section className="automation-config-section" aria-labelledby="automation-executor-title">
            <h2 id="automation-executor-title">要做什么</h2>
            <div className="automation-config-fields">
              {executorKind === 'agent' && <label className="automation-config-full">任务说明<textarea rows={2} value={prompt} onChange={e => { setPrompt(e.target.value); invalidateReview(); }} placeholder="例如：整理明天的安排，给我一份简短清单" /></label>}
              <label>{isMaa ? '任务名称（选填）' : '任务名称'}<input value={title} onChange={e => { setTitle(e.target.value); invalidateReview(); }} placeholder={isMaa ? maaDefaultTitle : executorKind === 'agent' ? '例如：明日安排' : '例如：整理下载目录'} /></label>
              <div className="automation-choice-field"><span>执行方式</span>
                <RadioChoices label="执行方式" value={executorKind} options={executorOptions}
                  onChange={value => { setExecutorKind(value); setError(null); invalidateReview(); }} />
              </div>
            </div>
          </section>
          <section className="automation-config-section" aria-labelledby="automation-trigger-title">
            <h2 id="automation-trigger-title">什么时候</h2>
            <div className="automation-config-fields">
              {!isMaa && <div className="automation-choice-field"><span>触发方式</span>
                <RadioChoices label="触发方式" value={triggerKind} options={triggerOptions}
                  onChange={value => { setTriggerKind(value); invalidateReview(); }} />
              </div>}
              {isMaa || triggerKind === 'schedule'
                ? <AutomationTimePicker value={triggerValue} onChange={value => { setTriggerValue(value); invalidateReview(); }} />
                : <div className="automation-startup-setting"><strong>AngelBot 开机启动</strong><p>不预先运行任务；暂停任务不关闭自启。</p><button type="button" className="secondary" disabled={autostart === null || changingAutostart} onClick={() => void changeAutostart(!autostart)}>{changingAutostart ? '正在设置…' : autostart ? '关闭 AngelBot 开机启动' : autostart === null ? '正在读取…' : '启用 AngelBot 开机启动'}</button>{autostart === null && <button type="button" className="secondary" onClick={() => setStartupRevision(value => value + 1)}>重新读取启动状态</button>}<small>{autostart ? '下次登录触发，手动重开不触发。' : '启用后即可保存开机任务。'}</small></div>}
            </div>
          </section>
          {isMaa && <section className="automation-config-section" aria-labelledby="automation-maa-title">
            <h2 id="automation-maa-title">MAA 队列</h2>
            <div className="automation-config-fields">
              <label>可信 MAA<select value={maaAppId} disabled={!maaCandidates.some(canLaunchMaa)} onChange={event => { setMaaAppId(event.target.value); setError(null); }}>
                <option value="">选择可信应用</option>
                {maaCandidates.map(app => <option key={app.id} value={app.id} disabled={!canLaunchMaa(app)}>
                  {app.displayName}{!app.enabled ? '（已停用）' : !canLaunchMaa(app) ? '（未授予打开应用权限）' : ''}
                </option>)}
              </select></label>
              <label>归属空间<select value={workspaceId} onChange={event => { setWorkspaceId(event.target.value); setError(null); }}>
                <option value="">选择日常或项目</option>
                {workspaces.map(workspace => <option key={workspace.id} value={workspace.id}>{workspace.name}</option>)}
              </select></label>
              {maaApps === null ? <p className="automation-config-full automation-maa-note" role="status">正在读取可信应用…</p>
                : !maaCandidates.some(canLaunchMaa) && <div className="automation-config-full automation-maa-unavailable">
                  <p className="automation-maa-note" role="status">{maaApps === 'error' ? '无法读取可信应用，请重新读取。'
                    : maaCandidates.length ? '可信列表中的 MAA 已停用或未授予打开应用权限。' : '尚未添加可信的 MAA.exe。'}</p>
                  {maaApps !== 'error' && <button type="button" className="secondary" onClick={() => window.dispatchEvent(new CustomEvent('open-settings', { detail: 'desktopControl' }))}>管理可信应用</button>}
                  <button type="button" className="secondary" onClick={() => setMaaRevision(value => value + 1)}>重新读取可信应用</button>
                </div>}
              <label className="automation-config-full automation-maa-checkbox"><input type="checkbox" checked={launchEmulator} onChange={event => setLaunchEmulator(event.target.checked)} />按需启动 MAA 已配置的模拟器</label>
              <p className="automation-config-full automation-maa-note">到点时，AngelBot 需保持运行。日程运行 MAA 当时勾选的任务队列，不改动勾选设置，也不需要连接模型。保存不会立即启动 MAA 或游戏。</p>
            </div>
          </section>}
          {executorKind === 'script' ? <>
          <section className="automation-config-section" aria-labelledby="automation-script-title">
            <h2 id="automation-script-title">脚本与参数</h2>
            <div className="automation-config-fields automation-config-fields--stacked">
              <div className="automation-path-field"><label htmlFor="automation-script-path">脚本路径</label><div className="automation-path-input"><input id="automation-script-path" value={scriptPath} onChange={e => { setScriptPath(e.target.value); invalidateReview(); }} placeholder="选择脚本或输入完整路径" /><button type="button" disabled={pickingPath || saving} onClick={() => void choosePath(false)}>选择脚本</button></div>
                <small>保存副本运行，原文件不变。</small>
              </div>
            </div>
          </section>
          <div className="automation-config-options">
            <small>{scriptArgs.trim() ? '已填写参数' : '参数可选'} · {workingDir.trim() ? '自定义工作目录' : '副本目录'} · {timeoutSeconds || '—'} 秒超时</small>
            <button type="button" className="secondary" aria-haspopup="dialog" onClick={() => setScriptDialog('options')}>运行选项</button>
            <button type="button" className="secondary" aria-haspopup="dialog" onClick={() => setScriptDialog('instructions')}>脚本使用说明</button>
          </div>
          </> : null}
          <div className="automation-form-actions automation-config-footer">
            {executorKind === 'agent' && <div className="automation-destination">
              <label htmlFor="automation-destination">结果发到</label>
              <select id="automation-destination" value={workspaceId} onChange={e => { setWorkspaceId(e.target.value); invalidateReview(); }}>
                <option value="">选择日常或项目</option>
                {workspaces.map(workspace => <option key={workspace.id} value={workspace.id}>{workspace.name}</option>)}
              </select>
            </div>}
            {isMaa ? <button type="button" disabled={!canSaveMaa || saving} onClick={() => void create()}>{saving ? '保存中…' : '保存 MAA 日程'}</button>
            : <button type="button" disabled={!canPreview || saving || pickingPath} onClick={() => {
              if (triggerKind === 'schedule' && !normalizeDailySchedule(triggerValue)) return setError('请选择有效的每日执行时间。');
              const timeout = executorKind === 'agent' ? 300 : Number(timeoutSeconds);
              if (!Number.isInteger(timeout) || timeout < 10 || timeout > 3600) return setError('超时时间必须是 10 到 3600 秒之间的整数。');
              setError(null); setReview(true);
            }}>预览规则</button>}
          </div>
        </fieldset>
        <Dialog open={scriptDialog !== null} title={scriptDialog === 'options' ? '运行选项' : '脚本使用说明'}
          onClose={() => { if (!pickingPath) setScriptDialog(null); }}>
          {scriptDialog === 'options' ? (
            <fieldset className="automation-form automation-script-form automation-script-options-form" aria-label="脚本运行选项" disabled={pickingPath || saving}>
              <label>参数（每行一个）<textarea value={scriptArgs} onChange={e => { setScriptArgs(e.target.value); invalidateReview(); }} rows={3} placeholder={'--profile\nC:\\Tools\\daily.json'} /></label>
              <div className="automation-path-field">
                <label htmlFor="automation-working-dir">脚本工作目录（可选）</label>
                <div className="automation-path-input">
                  <input id="automation-working-dir" value={workingDir} onChange={e => { setWorkingDir(e.target.value); invalidateReview(); }} placeholder="默认使用副本目录" />
                  <Button onClick={() => void choosePath(true)}>选择目录</Button>
                </div>
              </div>
              <label>超时（秒）<input inputMode="numeric" value={timeoutSeconds} onChange={e => { setTimeoutSeconds(e.target.value); invalidateReview(); }} /><small>10–3600 秒。</small></label>
              {error && <p className="automation-error" role="alert">{error}</p>}
            </fieldset>
          ) : (
            <>
              <p className="automation-script-guidance">支持 Python、Node.js、PowerShell 和批处理。只导入可信脚本。单个脚本保存在 AngelBot 的 automation-scripts 目录；不包含配置和模块，源文件更新不会自动同步。选择启动目录不会复制该目录的内容。</p>
              <p className="automation-script-guidance">参数每行一个，空行忽略，无需额外加引号。批处理（.bat/.cmd）不接受引号或 shell 元字符。</p>
              <p className="automation-script-guidance">脚本按计划运行，不逐次确认，也不继承模型密钥。退出码不保证外部业务完成。暂停仅影响后续调度，不会停止已启动的脚本。{SCRIPT_LIFECYCLE_NOTICE}</p>
            </>
          )}
          <div className="automation-form-actions"><Button variant="primary" disabled={pickingPath} onClick={() => setScriptDialog(null)}>返回配置</Button></div>
        </Dialog>
      </section>}
      {review && createOpen && <section className="automation-review" role="status">
        <div className="automation-review-content"><h2 ref={stepTitle} tabIndex={-1}>确认创建任务</h2><p>{title} · {triggerLabel}</p>
          <dl className="automation-script-review">
            <dt>预计首次运行</dt><dd>{triggerKind === 'startup' ? '下次登录 Windows' : nextDailyRun?.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }) ?? '以任务列表为准'}</dd>
            <dt>查看结果</dt><dd>{executorKind === 'agent' ? `${agentWorkspace?.name}的对话` : '自动化 → 此任务 → 运行记录'}</dd>
            <dt>使用模型</dt><dd>{executorKind === 'agent' ? '是 · 当前已保存的模型' : '否 · 仅运行脚本'}</dd>
            {executorKind === 'script' ? <><dt>源脚本</dt><dd>{scriptPath.trim()}</dd><dt>执行副本</dt><dd>AngelBot 托管目录（原文件不变）</dd><dt>参数</dt><dd><pre>{scriptArgs.split(/\r\n|\r|\n/).filter(arg => arg.length > 0).join('\n') || '无'}</pre></dd><dt>工作目录</dt><dd>{workingDir.trim() || '副本目录'}</dd><dt>超时</dt><dd>{timeoutSeconds} 秒</dd></> : <><dt>任务说明</dt><dd><pre>{prompt.trim()}</pre></dd></>}
          </dl>
          {triggerKind === 'schedule' && <p>到点时 AngelBot 需保持运行。</p>}
          <small>{executorKind === 'script' ? '只运行可信脚本。脚本副本不含依赖，按 Windows 用户权限运行。' : '使用归属空间的模型额度与权限。'}</small>
        </div>
        <div className="automation-form-actions"><button type="button" className="secondary" disabled={saving} onClick={invalidateReview}>返回编辑</button><button type="button" disabled={saving || !canPreview || pickingPath || changingAutostart} onClick={() => void create()}>{saving ? '创建中…' : '创建任务'}</button></div>
      </section>}
      {error && scriptDialog !== 'options' && <p className="automation-error" role="alert">{error}</p>}
      {target && loaded && !targetItem && <p className="automation-error" role="status">无法定位该事项；它可能已删除或归属工作区已改变。下方仍可查看其他任务。</p>}

      {!createOpen && <section className="automation-list-section" aria-label="自动化任务">
        <div className="automation-list-title">
          {items.some(item => item.enabled && item.triggerKind === 'schedule') && <p className="automation-schedule-note">定时任务需保持 AngelBot 运行。</p>}
          <button className="automation-refresh" type="button" onClick={() => { setError(null); void load(); }}>刷新</button>
        </div>
        {!loaded ? <p role="status">{loadError ? '任务尚未加载，请刷新重试。' : '正在读取自动化任务…'}</p> : items.length === 0 ? <p className="automation-empty">暂无自动化任务</p> : <div className="automation-grid">{items.map(item => {
          const isBusy = busyIds.has(item.id);
          const latestRun = item.latestRun;
          const localAutomation = item.executorKind === 'script' || item.executorKind === 'maa';
          const activeLocalRun = localAutomation && latestRun?.status === 'running' ? latestRun : undefined;
          const activeAgentReceipt = item.executorKind === 'agent' && latestRun && ['running', 'queued', 'awaiting_confirmation'].includes(latestRun.status) ? latestRun : undefined;
          const pendingRun = pendingRunIds.includes(item.id);
          const unconfirmedRun = !pendingRun && unconfirmedRequests.some(request => request.automationId === item.id);
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
          const presentation: AutomationStatusPresentation = unconfirmedRun ? {
            ...statusPresentation(item, latestRun), completed: false,
            label: '结果未确认', attention: true, scheduleLabel: '手动执行请求', scheduleValue: '尚未确认是否执行',
            notice: '运行记录不能证明这次手动执行的结果。请核对实际状态；不会自动重试。',
          } : statusPresentation(item, latestRun);
          const localIssue = localAutomation && !unconfirmedRun && presentation.attention;
          const sourceLabel = item.executorKind === 'agent'
            ? workspace?.name || '已删除工作区'
            : isReminder
              ? `本地通知 · ${workspace?.name || 'AngelBot'}`
              : item.executorKind === 'maa'
                ? `MAA 当前任务队列 · ${workspace?.name || '已删除工作区'}`
                : item.scriptPath?.split(/[\\/]/).pop() || '未配置脚本';
          const targeted = targetItem?.id === item.id;
          const summaryFactsId = `automation-summary-${item.id}`;
          const resultLabel = unconfirmedRun ? '运行记录（未关联本次请求）' : activeAgentReceipt ? '当前任务' : localIssue ? '本次记录' : '最近结果';
          const resultText = latestRun?.summary || '尚无运行记录';
          return <article key={item.id} ref={targeted ? targetCard : undefined} tabIndex={targeted ? -1 : undefined} aria-label={item.title} className={`automation-card${targeted ? ' automation-card-target' : ''}`}>
            <details className="automation-card-details" onToggle={localIssue ? event => {
              const card = event.currentTarget;
              if (event.target !== card || !card.open) return;
              const history = card.querySelector<HTMLDetailsElement>('.automation-output');
              if (history) history.open = true;
            } : undefined}>
              <summary className="automation-card-summary" aria-label={`${item.title} ${localIssue ? '查看运行记录' : '详情'}`} aria-describedby={localIssue ? `${summaryFactsId} ${summaryFactsId}-guidance` : summaryFactsId}>
                <h3 className="automation-card-title">{item.title}</h3>
                <div className="automation-card-meta">
                  <p className="automation-trigger">{item.triggerValue}</p>
                  <p className={`automation-path${item.executorKind === 'script' ? ' is-file' : ''}`} title={item.scriptPath}>{sourceLabel}</p>
                </div>
                <span className="automation-card-status">
                  <span className={`automation-status ${presentation.completed ? 'enabled' : presentation.attention ? 'attention' : ''}`}><i />{presentation.label}</span>
                  <span className="automation-card-chevron" aria-hidden="true">›</span>
                </span>
                <dl className="automation-summary-facts" id={summaryFactsId}>
                  <div><dt>{presentation.scheduleLabel}</dt><dd>{presentation.scheduleValue}</dd></div>
                  <div><dt>{resultLabel}</dt><dd>{resultText}</dd></div>
                </dl>
                {localIssue && <span className="automation-issue-guidance" id={`${summaryFactsId}-guidance`}>
                  {presentation.notice && <span>{presentation.notice}</span>}
                  <span className="automation-issue-action" aria-hidden="true">查看运行记录</span>
                </span>}
              </summary>
              <div className="automation-card-body">
                {item.executorKind === 'script' && item.scriptPath && <dl><div className="automation-execution-path"><dt>脚本副本</dt><dd><code>{item.scriptPath}</code></dd></div></dl>}
                {!localIssue && presentation.notice && <p className="automation-state-note">{presentation.notice}</p>}
                {item.lastSkippedAt != null && <p className="automation-state-note">本次未启动 · {formatTime(item.lastSkippedAt)}；展开运行记录查看原因。</p>}
                <AutomationHistory automationId={item.id} revision={`${latestRun?.id}:${latestRun?.status}:${item.lastSkippedAt}:${unconfirmedRun}`} />
                <div className="automation-actions">
                  <button className="secondary automation-delete" type="button" disabled={isBusy || localBusy || pendingRun || unconfirmedRun} aria-label={`删除 ${item.title}`} onClick={() => void perform(item.id, () => deleteAutomation(item.id))}>删除任务</button>
                  {presentation.workspaceAction && workspace && item.workspaceId && <button type="button" className="secondary automation-open-result" aria-label={`前往 ${workspace.kind === 'personal' ? 'AngelBot 日常' : workspace.name} ${presentation.workspaceAction}`} onClick={() => void openOwningWorkspace(item.workspaceId!)}>{presentation.workspaceAction}</button>}
                  {(activeLocalRun?.stopControl === 'available' || activeLocalRun?.stopControl === 'requested') && <button type="button" className="secondary" disabled={isBusy || activeLocalRun.stopControl === 'requested'} onClick={() => void perform(item.id, () => stopAutomationRun(item.id, activeLocalRun.id))}>{activeLocalRun.stopControl === 'requested' ? '正在停止' : '停止本次执行'}</button>}
                  <button type="button" className="secondary" disabled={isBusy || reminderCompleted || reminderNeedsAttention || reminderSuppressed || (!item.enabled && (unresolvedLocalRun || unconfirmedRun))} onClick={() => void perform(item.id, () => setAutomationEnabled(item.id, !item.enabled))}>{unconfirmedRun && !item.enabled ? '暂不能启用' : reminderNeedsAttention ? '需检查通知' : reminderSuppressed ? '本次未发送' : reminderCompleted ? '已完成' : item.enabled ? '暂停' : reminderCancelled ? '恢复' : '启用'}</button>
                  <button type="button" className={localIssue ? 'secondary' : 'automation-run-button'} disabled={isBusy || pendingRun || unconfirmedRun || !!activeLocalRun || !!activeAgentRun || pausedAgent} onClick={() => void runNow(item.id)}>{unconfirmedRun ? '结果未确认' : activeAgentRun ? presentation.label : activeLocalRun ? '等待结果' : pendingRun || isBusy ? '处理中…' : pausedAgent ? '先启用再执行' : unresolvedLocalRun ? '核对后重新运行' : item.executorKind === 'agent' ? '立即执行' : isReminder ? '立即提醒' : '立即运行'}</button>
                </div>
              </div>
            </details>
          </article>;
        })}</div>}
      </section>}
    </main>
  );
}

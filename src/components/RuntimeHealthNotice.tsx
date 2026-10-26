import { useEffect, useRef, useState } from 'react';
import { relaunch } from '@tauri-apps/plugin-process';
import { getRuntimeHealth, type RuntimeHealth } from '$lib/commands/runtime-health';
import { ConfirmDialog } from './common/ConfirmDialog';

export function RuntimeHealthPanel({
  health,
  onRestart,
}: {
  health: RuntimeHealth;
  onRestart: () => void | Promise<void>;
}) {
  const [confirmRestart, setConfirmRestart] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const [restartError, setRestartError] = useState<string | null>(null);
  const restartInFlight = useRef(false);
  const hasCriticalIssue = health.issues.some((issue) => issue.severity === 'critical');
  const needsRestart = health.issues.some((issue) => issue.recoveryAction === 'restart');
  const ephemeralStorage = health.issues.some((issue) => issue.code === 'ephemeral_storage');
  const primaryIssue = health.issues[0];

  if (!primaryIssue) return null;

  const restart = async () => {
    if (restartInFlight.current) return;
    restartInFlight.current = true;
    setRestarting(true);
    setRestartError(null);
    try {
      await onRestart();
      // A successful dispatch is not evidence that the new process is ready.
      // Keep the action locked until this window exits rather than retrying it.
    } catch {
      restartInFlight.current = false;
      setRestarting(false);
      setRestartError('重新启动未能完成。当前界面仍可使用，请保存重要内容后再试。');
    }
  };

  return (
    <details
      className={`runtime-health-notice ${health.status === 'degraded' ? 'is-warning' : 'is-notice'}`}
      open={hasCriticalIssue || undefined}
    >
      <summary className="runtime-health-summary">
        <span className="runtime-health-indicator" aria-hidden="true" />
        <strong>{health.status === 'degraded' ? '部分能力受限' : '运行提示'}</strong>
        <span>{primaryIssue.title}{health.issues.length > 1 ? `，另有 ${health.issues.length - 1} 项` : ''}</span>
      </summary>
      <div className="runtime-health-details">
        <ul>
          {health.issues.map((issue) => (
            <li key={issue.code}>
              <strong>{issue.title}</strong>
              <span>{issue.detail}</span>
            </li>
          ))}
        </ul>
        {needsRestart && (
          <button type="button" className="runtime-health-action" disabled={restarting} onClick={() => setConfirmRestart(true)}>
            重新启动 AngelBot
          </button>
        )}
      </div>
      <ConfirmDialog open={confirmRestart} title="重新启动 AngelBot？"
        body={`正在运行的任务和自动化可能中断，未发送文字及未保存配置可能丢失。请先完成或停止这些操作，并保存重要内容；重启不会撤回已经执行的外部操作。${ephemeralStorage ? '当前存储未连接，本次会话的新内容未持久保存，重启后将丢失，请先复制保存。' : ''}`}
        confirmLabel="确认重启" cancelLabel="暂不重启" loading={restarting} error={restartError}
        onConfirm={() => { void restart(); }}
        onCancel={() => { if (!restartInFlight.current) setConfirmRestart(false); }} />
    </details>
  );
}

export function RuntimeHealthNotice() {
  const [health, setHealth] = useState<RuntimeHealth | null>(null);
  const [loadError, setLoadError] = useState(false);
  const [revision, setRevision] = useState(0);

  useEffect(() => {
    // ChatArea tests intentionally own the exact IPC call sequence. Runtime
    // health is covered at its command boundary and by the desktop E2E flow.
    if (import.meta.env.MODE === 'test') return undefined;
    let active = true;
    setLoadError(false);
    void getRuntimeHealth()
      .then((next) => { if (active) setHealth(next); })
      .catch(() => { if (active) setLoadError(true); });
    return () => { active = false; };
  }, [revision]);

  if (loadError) {
    return (
      <aside className="runtime-health-notice is-warning" role="alert">
        <div className="runtime-health-summary">
          <span className="runtime-health-indicator" aria-hidden="true" />
          <strong>无法读取运行状态</strong>
          <span>主界面仍可使用，但在状态恢复前请谨慎执行重要操作。</span>
        </div>
        <button type="button" className="runtime-health-action" onClick={() => setRevision((value) => value + 1)}>
          重新检查
        </button>
      </aside>
    );
  }

  if (!health || health.issues.length === 0) return null;
  return <RuntimeHealthPanel health={health} onRestart={() => relaunch()} />;
}

import { useEffect, useState } from 'react';
import { relaunch } from '@tauri-apps/plugin-process';
import { getRuntimeHealth, type RuntimeHealth } from '$lib/commands/runtime-health';

export function RuntimeHealthPanel({
  health,
  onRestart,
}: {
  health: RuntimeHealth;
  onRestart: () => void;
}) {
  const hasCriticalIssue = health.issues.some((issue) => issue.severity === 'critical');
  const needsRestart = health.issues.some((issue) => issue.recoveryAction === 'restart');
  const primaryIssue = health.issues[0];

  if (!primaryIssue) return null;

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
          <button type="button" className="runtime-health-action" onClick={onRestart}>
            重新启动 AngelBot
          </button>
        )}
      </div>
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
  return <RuntimeHealthPanel health={health} onRestart={() => { void relaunch(); }} />;
}

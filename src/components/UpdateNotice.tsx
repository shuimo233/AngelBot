import { useEffect, useState } from 'react';
import { relaunch } from '@tauri-apps/plugin-process';
import { checkForAppUpdate, type AvailableAppUpdate } from '$lib/updates';

type InstallState = 'available' | 'installing' | 'failed';

export function UpdateNoticePanel({
  update,
  state,
  onInstall,
  onDismiss,
}: {
  update: Pick<AvailableAppUpdate, 'version' | 'body'>;
  state: InstallState;
  onInstall: () => void;
  onDismiss: () => void;
}) {
  const status = state === 'installing'
    ? '正在安全下载并安装，完成后将重新启动'
    : state === 'failed'
      ? '更新未能完成，当前版本仍可继续使用'
      : `版本 ${update.version} 已可用`;

  return (
    <aside className={`app-update-notice is-${state}`} aria-live="polite">
      <span className="app-update-marker" aria-hidden="true" />
      <div className="app-update-copy">
        <strong>{state === 'failed' ? '更新失败' : 'AngelBot 更新'}</strong>
        <span>{status}</span>
      </div>
      <div className="app-update-actions">
        {state !== 'installing' && (
          <button type="button" className="app-update-primary" onClick={onInstall}>
            {state === 'failed' ? '重新尝试' : '更新并重启'}
          </button>
        )}
        {state === 'available' && (
          <button type="button" className="app-update-dismiss" onClick={onDismiss} aria-label="暂时忽略此更新">
            稍后
          </button>
        )}
      </div>
    </aside>
  );
}

export function UpdateNotice() {
  const [update, setUpdate] = useState<AvailableAppUpdate | null>(null);
  const [state, setState] = useState<InstallState>('available');

  useEffect(() => {
    if (import.meta.env.MODE === 'test' || import.meta.env.VITE_ANGELBOT_DESKTOP_E2E === '1') {
      return undefined;
    }
    let active = true;
    const timer = window.setTimeout(() => {
      void checkForAppUpdate().then((available) => {
        if (active) setUpdate(available);
      });
    }, 2_000);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, []);

  useEffect(() => () => {
    if (update) void update.close();
  }, [update]);

  if (!update) return null;

  const install = async () => {
    setState('installing');
    try {
      await update.install();
      // Windows exits during installation. On platforms where it does not,
      // relaunch explicitly so the newly installed version becomes active.
      await relaunch();
    } catch {
      setState('failed');
    }
  };

  const dismiss = () => {
    setUpdate(null);
  };

  return (
    <UpdateNoticePanel
      update={update}
      state={state}
      onInstall={() => { void install(); }}
      onDismiss={dismiss}
    />
  );
}

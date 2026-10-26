import { useEffect, useRef, useState } from 'react';
import { relaunch } from '@tauri-apps/plugin-process';
import { checkForAppUpdate, type AvailableAppUpdate } from '$lib/updates';
import { ConfirmDialog } from './common/ConfirmDialog';

type InstallState = 'available' | 'installing' | 'failed' | 'restarting' | 'restart_required';

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
  const [confirmInstall, setConfirmInstall] = useState(false);
  const confirmationOpen = useRef(false);
  const busy = state === 'installing' || state === 'restarting';
  const restartOnly = state === 'restart_required';
  const status = state === 'installing'
    ? '正在安全下载并安装，完成后将重新启动'
    : state === 'restarting' ? '更新已安装，正在重新启动'
      : restartOnly ? '更新已安装，尚未完成重新启动'
        : state === 'failed' ? '更新未能完成，当前版本仍可继续使用'
          : `版本 ${update.version} 已可用`;

  return (
    <aside className={`app-update-notice is-${state}`} aria-live="polite">
      <span className="app-update-marker" aria-hidden="true" />
      <div className="app-update-copy">
        <strong>{state === 'failed' ? '更新失败' : restartOnly ? '等待重启' : 'AngelBot 更新'}</strong>
        <span>{status}</span>
      </div>
      <div className="app-update-actions">
        {!busy && (
          <button type="button" className="app-update-primary" onClick={() => { confirmationOpen.current = true; setConfirmInstall(true); }}>
            {restartOnly ? '重新启动' : state === 'failed' ? '重新尝试' : '更新并重启'}
          </button>
        )}
        {state === 'available' && (
          <button type="button" className="app-update-dismiss" onClick={onDismiss} aria-label="暂时忽略此更新">
            稍后
          </button>
        )}
      </div>
      <ConfirmDialog open={confirmInstall} title={restartOnly ? '重新启动 AngelBot？' : '更新并重新启动 AngelBot？'}
        body={`${restartOnly ? '更新已安装，本次只重新启动，不重复下载或安装。' : `将更新到版本 ${update.version}，完成后关闭并重新启动 AngelBot。`}正在运行的任务和自动化可能中断，未发送文字及未保存配置可能丢失。请先完成或停止这些操作，并保存重要内容；重启不会撤回已经执行的外部操作。`}
        confirmLabel={restartOnly ? '确认重启' : '确认更新并重启'} cancelLabel={restartOnly ? '暂不重启' : '暂不更新'}
        onConfirm={() => {
          if (!confirmationOpen.current || busy) return;
          confirmationOpen.current = false;
          setConfirmInstall(false);
          onInstall();
        }}
        onCancel={() => { confirmationOpen.current = false; setConfirmInstall(false); }} />
    </aside>
  );
}

export function UpdateNotice() {
  const [update, setUpdate] = useState<AvailableAppUpdate | null>(null);
  const [state, setState] = useState<InstallState>('available');
  const actionInFlight = useRef(false);
  const installCompleted = useRef(false);

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
    if (actionInFlight.current) return;
    actionInFlight.current = true;
    if (!installCompleted.current) {
      setState('installing');
      try {
        await update.install();
        installCompleted.current = true;
      } catch {
        actionInFlight.current = false;
        setState('failed');
        return;
      }
    }
    // Windows normally exits during installation. If this process remains,
    // a failed relaunch must never cause another installation in this owner.
    setState('restarting');
    try {
      await relaunch();
    } catch {
      actionInFlight.current = false;
      setState('restart_required');
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

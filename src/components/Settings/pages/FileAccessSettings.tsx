import { useEffect, useMemo, useState } from 'react';
import { useSessionsStore } from '$stores/sessions';
import { useWorkspacesStore } from '$stores/workspaces';
import { getSessionFileAccess, type SessionFileAccess } from '$lib/commands/file';
import { Button } from '../../../ui';
import { SettingsRow, SettingsSection } from '../components/SettingsSection';
import './PermissionSettings.css';

/** Workspace-owned file policy. Internal sessions never choose their own root. */
export function FileAccessSettings() {
  const activeSessionId = useSessionsStore((state) => state.activeSessionId);
  const workspaces = useWorkspacesStore((state) => state.workspaces);
  const activeWorkspaceId = useWorkspacesStore((state) => state.activeWorkspaceId);
  const activeWorkspace = useMemo(
    () => workspaces.find((workspace) => workspace.id === activeWorkspaceId) ?? null,
    [workspaces, activeWorkspaceId],
  );
  const [access, setAccess] = useState<SessionFileAccess | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setAccess(null);
    setError(null);
    if (!activeSessionId || !activeWorkspace) return;
    getSessionFileAccess(activeSessionId)
      .then((value) => { if (!cancelled) setAccess(value); })
      .catch((reason) => { if (!cancelled) setError(String(reason)); });
    return () => { cancelled = true; };
  }, [activeSessionId, activeWorkspace?.kind, activeWorkspace?.directoryConfirmed]);

  if (!activeSessionId || !activeWorkspace) {
    return <div className="settings-page-content"><p className="permission-copy">打开工作区后可查看文件范围。</p></div>;
  }

  const isProject = activeWorkspace.kind === 'project';
  const rootPath = access?.workDir ?? (isProject ? activeWorkspace.rootPath : undefined);
  const needsConfirmation = isProject && activeWorkspace.directoryConfirmed === false;

  return (
    <div className="settings-page-content">
      <SettingsSection title={activeWorkspace.name}
        description={isProject ? '项目写入限于根目录。' : '处理其他文件时，可在对话中附加文件或打开项目。'}>
        <div className="settings-rows permission-file-scope">
          <SettingsRow title={isProject ? '项目根目录' : '日常结果区'} description={rootPath || (error ? '未能读取目录' : '正在读取…')}>
            {needsConfirmation ? <>
              <span className="permission-status" data-state="pending">尚未授权</span>
              <Button size="small" onClick={() => window.dispatchEvent(new CustomEvent('open-settings', { detail: 'data' }))}>确认恢复目录</Button>
            </> : rootPath && <span className="permission-status">{isProject ? '可读写' : '仅此目录可读写'}</span>}
          </SettingsRow>
        </div>
        {error && <p className="permission-error" role="alert">{error}</p>}
      </SettingsSection>
    </div>
  );
}

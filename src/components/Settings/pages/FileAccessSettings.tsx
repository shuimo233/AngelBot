import { useEffect, useMemo, useState } from 'react';
import { useSessionsStore } from '$stores/sessions';
import { useWorkspacesStore } from '$stores/workspaces';
import { getSessionFileAccess, type SessionFileAccess } from '$lib/commands/file';

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
    return <div className="settings-page-content">请先打开一个工作区以查看文件访问范围。</div>;
  }

  const isProject = activeWorkspace.kind === 'project';
  const rootPath = access?.workDir ?? activeWorkspace.rootPath;

  return (
    <div className="settings-page-content file-access-settings">
      <section className="file-access-card">
        <p className="file-access-eyebrow">WORKSPACE FILE ACCESS</p>
        <h3>{isProject ? `${activeWorkspace.name} 的文件范围` : 'AngelBot 日常不连接项目文件'}</h3>
        {isProject ? (
          <>
            <p>项目目录在创建工作区时确定。主 Agent 与其内部子代理只能在此项目范围内写入，不能由会话重新定向。</p>
            <div className="file-access-root"><span>项目根目录</span><code>{rootPath ?? '正在读取…'}</code><b>{activeWorkspace.directoryConfirmed === false ? '待确认，尚未授权' : '可读写'}</b></div>
            {activeWorkspace.directoryConfirmed === false && <button type="button" onClick={() => window.dispatchEvent(new CustomEvent('open-settings', { detail: 'data' }))}>批量确认恢复目录</button>}
          </>
        ) : (
          <>
            <p>日常对话可将生成的文件保存在 AngelBot 自有结果区，默认保留，可预览、另存与备份；不会读取或写入项目目录，也不授予命令执行权限。处理已有文件可显式附加资料，需要项目范围时再打开项目。</p>
            <div className="file-access-root"><span>日常结果区</span><code>{access?.workDir ?? '正在读取…'}</code><b>仅此目录可读写</b></div>
          </>
        )}
      </section>
      {error && <p className="file-access-error" role="alert">{error}</p>}
    </div>
  );
}

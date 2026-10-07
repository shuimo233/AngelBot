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
    if (!activeSessionId || activeWorkspace?.kind !== 'project') {
      setAccess(null);
      return;
    }
    getSessionFileAccess(activeSessionId)
      .then(setAccess)
      .catch((reason) => setError(String(reason)));
  }, [activeSessionId, activeWorkspace?.kind]);

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
            <div className="file-access-root"><span>项目根目录</span><code>{rootPath ?? '正在读取…'}</code><b>可读写</b></div>
          </>
        ) : (
          <p>日常对话只使用全局偏好与长期记忆，不会读取或写入任何项目目录。需要处理文件时，请创建或打开对应项目工作区。</p>
        )}
      </section>
      {error && <p className="file-access-error" role="alert">{error}</p>}
    </div>
  );
}

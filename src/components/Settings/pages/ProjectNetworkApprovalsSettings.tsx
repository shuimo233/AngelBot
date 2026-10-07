import { useEffect, useMemo, useRef, useState } from 'react';
import {
  getProjectNetworkApprovals,
  revokeProjectNetworkApproval,
  type ProjectNetworkApproval,
} from '$lib/commands/project-network-approvals';
import { useWorkspacesStore } from '$stores/workspaces';

function formatGrantedAt(unixSeconds: number): string {
  return new Intl.DateTimeFormat('zh-CN', {
    dateStyle: 'medium',
    timeStyle: 'short',
  }).format(new Date(unixSeconds * 1000));
}

function formatResponseLimit(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(bytes % (1024 * 1024) === 0 ? 0 : 1)} MB`;
}

function formatActions(actions: ProjectNetworkApproval['actions']): string {
  const labels = actions.map((action) => action === 'search' ? '搜索' : '读取');
  return labels.length > 0 ? labels.join('、') : '无';
}

/**
 * Project-scoped approvals are intentionally separate from global search-provider
 * configuration. This page reads the active workspace, never accepts a path,
 * and renders only the sanitized projection returned by the backend.
 */
export function ProjectNetworkApprovalsSettings() {
  const workspaces = useWorkspacesStore((state) => state.workspaces);
  const activeWorkspaceId = useWorkspacesStore((state) => state.activeWorkspaceId);
  const activeWorkspace = useMemo(
    () => workspaces.find((workspace) => workspace.id === activeWorkspaceId) ?? null,
    [activeWorkspaceId, workspaces],
  );
  const currentWorkspaceId = activeWorkspace?.id ?? null;
  const currentWorkspaceIdRef = useRef(currentWorkspaceId);
  currentWorkspaceIdRef.current = currentWorkspaceId;

  const [approvals, setApprovals] = useState<ProjectNetworkApproval[]>([]);
  const [loading, setLoading] = useState(false);
  const [revokingRef, setRevokingRef] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const isProject = activeWorkspace?.kind === 'project';

  useEffect(() => {
    let current = true;

    if (!isProject || !currentWorkspaceId) {
      setApprovals([]);
      setLoading(false);
      setRevokingRef(null);
      setError(null);
      return () => { current = false; };
    }

    setLoading(true);
    setError(null);
    setApprovals([]);
    getProjectNetworkApprovals(currentWorkspaceId)
      .then((result) => {
        if (current) setApprovals(result.approvals);
      })
      .catch(() => {
        if (current) setError('无法读取此项目的联网授权。请稍后重试。');
      })
      .finally(() => {
        if (current) setLoading(false);
      });

    return () => { current = false; };
  }, [currentWorkspaceId, isProject]);

  const revoke = async (approval: ProjectNetworkApproval) => {
    if (!isProject || !currentWorkspaceId || revokingRef) return;

    const workspaceId = currentWorkspaceId;
    setRevokingRef(approval.approvalRef);
    setError(null);
    try {
      await revokeProjectNetworkApproval(workspaceId, approval.approvalRef);
      if (currentWorkspaceIdRef.current === workspaceId) {
        setApprovals((current) => current.filter((item) => item.approvalRef !== approval.approvalRef));
      }
    } catch {
      if (currentWorkspaceIdRef.current === workspaceId) {
        setError('无法撤销此联网授权。请稍后重试。');
      }
    } finally {
      if (currentWorkspaceIdRef.current === workspaceId) setRevokingRef(null);
    }
  };

  if (!activeWorkspace) {
    return (
      <div className="settings-page-content project-network-approvals-settings">
        <section className="project-network-approvals-section">
          <h3>请先打开一个项目</h3>
          <p>项目联网权限只在项目工作区中可用。</p>
        </section>
      </div>
    );
  }

  if (!isProject) {
    return (
      <div className="settings-page-content project-network-approvals-settings">
        <section className="project-network-approvals-section" aria-labelledby="project-network-personal-title">
          <p className="project-network-approvals-eyebrow">PROJECT NETWORK PERMISSIONS</p>
          <h3 id="project-network-personal-title">个人空间没有项目联网授权</h3>
          <p>联网委派的授权只属于具体项目，不会保存到 AngelBot 日常。打开一个项目后，可以在这里查看或撤销它的授权。</p>
        </section>
      </div>
    );
  }

  return (
    <div className="settings-page-content project-network-approvals-settings">
      <section className="project-network-approvals-section project-network-approvals-intro" aria-labelledby="project-network-overview-title">
        <p className="project-network-approvals-eyebrow">PROJECT NETWORK PERMISSIONS</p>
        <h3 id="project-network-overview-title">{activeWorkspace.name} 的联网授权</h3>
        <p>这些授权仅用于主 Agent 派发的探索子代理。撤销后，后续联网操作会被阻止；已在传输中的结果也会在安全校验中被丢弃。</p>
      </section>

      <section className="project-network-approvals-section" aria-labelledby="project-network-list-title" aria-live="polite">
        <header className="project-network-approvals-section-header">
          <h3 id="project-network-list-title">已批准的访问范围</h3>
          {!loading && <span>{approvals.length} 项</span>}
        </header>

        {loading && <p className="project-network-approvals-muted">正在读取此项目的授权…</p>}
        {!loading && !error && approvals.length === 0 && (
          <p className="project-network-approvals-muted">尚无联网授权。需要联网探索时，AngelBot 会先向你说明要访问的范围。</p>
        )}

        {!loading && approvals.map((approval) => {
          const primaryHost = approval.hosts[0] ?? '此范围';
          const hostLabel = approval.hosts.length > 0 ? approval.hosts.join('、') : '无可访问主机';

          return (
            <article className="project-network-approval" key={approval.approvalRef} aria-label={`${hostLabel} 的联网授权`}>
              <div className="project-network-approval-main">
                <p className="project-network-approval-label">允许访问</p>
                <div className="project-network-approval-hosts">
                  {approval.hosts.map((host) => <code key={host} title={host}>{host}</code>)}
                </div>
                <dl className="project-network-approval-facts">
                  <div>
                    <dt>批准时间</dt>
                    <dd>{formatGrantedAt(approval.grantedAt)}</dd>
                  </div>
                  <div>
                    <dt>允许操作</dt>
                    <dd>{formatActions(approval.actions)}</dd>
                  </div>
                  <div>
                    <dt>响应上限</dt>
                    <dd>{formatResponseLimit(approval.maxResponseBytes)}</dd>
                  </div>
                  <div>
                    <dt>重定向上限</dt>
                    <dd>{approval.maxRedirects} 次</dd>
                  </div>
                </dl>
              </div>
              <button
                type="button"
                className="project-network-approval-revoke"
                disabled={revokingRef !== null}
                onClick={() => { void revoke(approval); }}
                aria-label={`撤销 ${primaryHost} 的联网授权`}
              >
                {revokingRef === approval.approvalRef ? '正在撤销…' : '撤销授权'}
              </button>
            </article>
          );
        })}
      </section>

      {error && <p className="project-network-approvals-error" role="alert">{error}</p>}
    </div>
  );
}

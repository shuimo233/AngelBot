import { useEffect, useId, useMemo, useRef, useState } from 'react';
import {
  getProjectNetworkApprovals,
  revokeProjectNetworkApproval,
  type ProjectNetworkApproval,
} from '$lib/commands/project-network-approvals';
import { useWorkspacesStore } from '$stores/workspaces';
import { SettingsSection } from '../components/SettingsSection';

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
  const revokeNoticeId = useId();
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

  if (!isProject || !activeWorkspace) {
    return (
      <div className="settings-page-content project-network-approvals-settings">
        <p className="project-network-approvals-muted">打开项目即可查看和撤销联网授权。</p>
      </div>
    );
  }

  return (
    <div className="settings-page-content project-network-approvals-settings">
      <SettingsSection title={activeWorkspace.name} description="以下授权仅供此项目的探索子代理使用。">
        {loading && <p className="project-network-approvals-muted" role="status">正在读取授权…</p>}
        {!loading && !error && approvals.length === 0 && (
          <p className="project-network-approvals-muted" role="status">尚无联网授权。需要时会先确认访问范围。</p>
        )}
        {!loading && approvals.length > 0 && <div className="settings-rows">
          <p id={revokeNoticeId} className="project-network-approvals-muted settings-rows-note">撤销后会阻止新请求，并丢弃传输中的结果。</p>
          {approvals.map((approval) => {
            const primaryHost = approval.hosts[0] ?? '此范围';
            const hostLabel = approval.hosts.length > 0 ? approval.hosts.join('、') : '无可访问主机';

            return (
              <article className="project-network-approval" key={approval.approvalRef} aria-label={`${hostLabel} 的联网授权`}>
                <div className="project-network-approval-main">
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
                  aria-describedby={revokeNoticeId}
                >
                  {revokingRef === approval.approvalRef ? '正在撤销…' : '撤销授权'}
                </button>
              </article>
            );
          })}
        </div>}
        {error && <p className="project-network-approvals-error" role="alert">{error}</p>}
      </SettingsSection>
    </div>
  );
}

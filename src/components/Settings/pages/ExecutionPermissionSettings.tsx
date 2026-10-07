import { useEffect } from 'react';
import type { AgentExecutionPermission } from '$lib/commands/settings';
import { useExecutionPermissionStore } from '$stores/executionPermission';

const PERMISSION_OPTIONS: Array<{
  value: AgentExecutionPermission;
  label: string;
  description: string;
}> = [
  { value: 'ask', label: '请求批准', description: '需要确认的写入或外部操作会逐次请求你的批准。' },
  { value: 'workspace_auto', label: '工作区自动', description: '自动新建或覆盖项目文件，并使用当前工作区已启用的 MCP 工具；编辑、移动等其他操作仍按规则确认。' },
  { value: 'full_access', label: '完全访问', description: '自动执行已准入的文件、桌面、MCP 与外部服务工具；保护操作仍会询问。' },
];

/** Global execution preference, independent of the currently selected workspace. */
export function ExecutionPermissionSettings() {
  const permission = useExecutionPermissionStore((state) => state.permission);
  const loading = useExecutionPermissionStore((state) => state.loading);
  const saving = useExecutionPermissionStore((state) => state.saving);
  const error = useExecutionPermissionStore((state) => state.error);
  const loadPermission = useExecutionPermissionStore((state) => state.load);
  const savePermission = useExecutionPermissionStore((state) => state.save);

  useEffect(() => {
    void loadPermission();
  }, [loadPermission]);

  const activePermission = PERMISSION_OPTIONS.find((option) => option.value === permission);

  return (
    <div className="settings-page-content execution-permission-settings">
      <section className="execution-permission-section">
        <h3>全局执行方式</h3>
        <p>此偏好应用于所有工作区，即使当前没有打开工作区也可调整。它只决定已准入工具何时请求确认，不扩大文件范围、受信任应用或外部服务的可用范围。</p>
        {permission === null ? <p aria-live="polite">{loading ? '正在读取当前执行权限…' : '当前执行权限不可用，请重试。'}</p> : <details className="agent-permission-menu" aria-busy={saving}>
          <summary><span>权限设置</span><strong>{activePermission?.label ?? permission}</strong></summary>
          <div className="agent-permission-options">
            {PERMISSION_OPTIONS.map((option) => (
              <button type="button" disabled={saving || loading} aria-pressed={option.value === permission} className={option.value === permission ? 'active' : ''} key={option.value} onClick={() => void savePermission(option.value)}>
                <span><b>{option.label}</b><small>{option.description}</small></span>
                {option.value === permission && <span className="agent-permission-status">当前</span>}
              </button>
            ))}
          </div>
        </details>}
        {saving && <p aria-live="polite">正在保存执行权限；保存完成前仍按原设置运行。</p>}
        {permission === null && !loading && <button type="button" onClick={() => void loadPermission()}>重试读取</button>}
        <p className="agent-permission-note"><b>权限边界：</b>MCP 服务须在当前工作区启用，对受信任应用的操作仍受应用范围列表约束。选择自动模式意味着信任这些已启用服务的普通工具，不再逐个询问未知风险；请只启用可信服务。修改凭据、长期记忆、导入技能等受保护操作仍需专项确认；普通 MCP 发送或删除并非一律专项询问，在自动模式下可能直接执行。</p>
        {error && <p className="execution-permission-error" role="alert">{error}</p>}
      </section>
    </div>
  );
}

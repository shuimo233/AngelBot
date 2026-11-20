import { useEffect, useId } from 'react';
import { EXECUTION_PERMISSION_OPTIONS } from '$lib/execution-permission-copy';
import { useExecutionPermissionStore } from '$stores/executionPermission';
import { Button } from '../../../ui';
import { SettingsSection } from '../components/SettingsSection';
import './PermissionSettings.css';

/** Global execution preference, independent of the currently selected workspace. */
export function ExecutionPermissionSettings() {
  const modeId = useId();
  const permission = useExecutionPermissionStore((state) => state.permission);
  const loading = useExecutionPermissionStore((state) => state.loading);
  const saving = useExecutionPermissionStore((state) => state.saving);
  const error = useExecutionPermissionStore((state) => state.error);
  const loadPermission = useExecutionPermissionStore((state) => state.load);
  const savePermission = useExecutionPermissionStore((state) => state.save);

  useEffect(() => {
    void loadPermission();
  }, [loadPermission]);

  return (
    <div className="settings-page-content">
      <SettingsSection title="全局执行方式" description="适用于所有工作区，各自授权照常生效。">
        {permission === null ? <div className="permission-inline-state" aria-live="polite">
          <p>{loading ? '正在读取当前执行权限…' : '当前执行权限不可用，请重试。'}</p>
          {!loading && <Button size="small" onClick={() => void loadPermission()}>重试读取</Button>}
        </div> : (
          <div className="settings-rows execution-mode-rows" aria-busy={saving}>
            {EXECUTION_PERMISSION_OPTIONS.map((option) => {
              const current = option.value === permission;
              const descriptionId = `${modeId}-${option.value}`;
              return <button key={option.value} type="button" className="execution-mode-option"
                aria-label={option.label} aria-describedby={descriptionId} aria-pressed={current}
                aria-disabled={current || saving || loading}
                onClick={() => { if (!current && !saving && !loading) void savePermission(option.value); }}>
                <span className="execution-mode-marker" aria-hidden="true" />
                <span className="execution-mode-copy">
                  <strong>{option.label}</strong>
                  <span id={descriptionId}>{option.desc}</span>
                </span>
              </button>;
            })}
            <p className="permission-copy execution-mode-note">自动模式下，普通 MCP 发送或删除可能直接执行。修改凭据、长期记忆和导入技能仍需单独确认。</p>
          </div>
        )}
        {saving && <p className="permission-copy" aria-live="polite">正在保存执行权限，保存完成前仍按原设置运行。</p>}
        {error && <p className="permission-error" role="alert">{error}</p>}
      </SettingsSection>
    </div>
  );
}

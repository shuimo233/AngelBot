import { useState } from 'react';
import { open as openDirectoryDialog } from '@tauri-apps/plugin-dialog';
import { SettingsRow, SettingsSection } from '../components/SettingsSection';
import { Button, Dialog, InlineNotice, TextField } from '../../../ui';
import { clearAllUserData, exportData, exportEncryptedData, importData, importEncryptedData } from '$lib/commands/settings';
import { confirmRestoredProjectDirectories } from '$lib/commands/workspace';
import { useWorkspacesStore } from '$stores/workspaces';
import './DataSettings.css';

function isEncryptedBackup(data: string): boolean {
  try { return JSON.parse(data).format === 'angelbot.encrypted-backup'; }
  catch { return false; }
}

export function DataSettings() {
  const [exporting, setExporting] = useState(false);
  const [importing, setImporting] = useState(false);
  const [clearConfirmation, setClearConfirmation] = useState('');
  const [dialog, setDialog] = useState<'backup' | 'clear' | null>(null);
  const [backupPassword, setBackupPassword] = useState('');
  const [clearing, setClearing] = useState(false);
  const [error, setError] = useState<{ section: 'directories' | 'backup' | 'clear'; message: string } | null>(null);
  const workspaces = useWorkspacesStore((state) => state.workspaces);
  const loadWorkspaces = useWorkspacesStore((state) => state.loadWorkspaces);
  const pendingDirectories = workspaces.filter((item) => item.kind === 'project' && item.directoryConfirmed === false);
  const [directoryPaths, setDirectoryPaths] = useState<Record<string, string>>({});
  const [excludedDirectories, setExcludedDirectories] = useState<string[]>([]);
  const [confirmingDirectories, setConfirmingDirectories] = useState(false);
  const [directoryNotice, setDirectoryNotice] = useState<string | null>(null);
  const busy = exporting || importing || clearing || confirmingDirectories;
  const selectedDirectories = pendingDirectories.filter((item) => !excludedDirectories.includes(item.id));

  const chooseDirectory = async (id: string) => {
    setError(null);
    try {
      const selected = await openDirectoryDialog({ directory: true, multiple: false, title: '选择此项目在本机的目录' });
      if (typeof selected === 'string') setDirectoryPaths((paths) => ({ ...paths, [id]: selected }));
    } catch { setError({ section: 'directories', message: '无法打开文件夹选择器，请稍后重试。' }); }
  };

  const confirmDirectories = async () => {
    setConfirmingDirectories(true); setError(null); setDirectoryNotice(null);
    try {
      await confirmRestoredProjectDirectories(selectedDirectories
        .map((item) => ({ id: item.id, expectedPath: item.rootPath ?? '', path: directoryPaths[item.id] ?? item.rootPath ?? '' })));
      setDirectoryNotice(`已确认 ${selectedDirectories.length} 个项目目录。未选中的目录保持未授权；旧任务不会自动继续。`);
      await loadWorkspaces();
      setDirectoryPaths({}); setExcludedDirectories([]);
      if (useWorkspacesStore.getState().lastError) setError({
        section: 'directories', message: '目录已确认，但工作区列表未能刷新。请稍后重新打开设置；无需再次授权已确认的目录。',
      });
    } catch (reason) {
      setError({ section: 'directories', message: reason instanceof Error ? reason.message : String(reason) });
    } finally { setConfirmingDirectories(false); }
  };

  const handleExport = async () => {
    setExporting(true); setError(null);
    try {
      const encrypted = backupPassword.length > 0;
      const data = encrypted ? await exportEncryptedData(backupPassword) : await exportData();
      const url = URL.createObjectURL(new Blob([data], { type: 'application/json' }));
      const anchor = document.createElement('a');
      anchor.href = url; anchor.download = `angelbot-backup-${new Date().toISOString().slice(0, 10)}${encrypted ? '.encrypted' : ''}.json`;
      anchor.click(); URL.revokeObjectURL(url);
    } catch (reason) { setError({
      section: 'backup', message: reason instanceof Error ? reason.message : typeof reason === 'string' ? reason : '导出失败，请稍后重试。',
    }); }
    finally { setBackupPassword(''); setExporting(false); }
  };

  const handleImport = () => {
    const input = document.createElement('input');
    input.type = 'file'; input.accept = 'application/json';
    input.onchange = async (event) => {
      const file = (event.target as HTMLInputElement).files?.[0];
      if (!file) return;
      setImporting(true); setError(null);
      try {
        const data = await file.text();
        if (isEncryptedBackup(data) && !backupPassword) throw new Error('请输入备份密码');
        if (isEncryptedBackup(data)) await importEncryptedData(data, backupPassword);
        else await importData(data);
        window.location.reload();
      }
      catch (reason) { setError({
        section: 'backup', message: reason instanceof Error ? reason.message : typeof reason === 'string' ? reason : '导入失败：文件格式无效或无法恢复。',
      }); }
      finally { setBackupPassword(''); setImporting(false); }
    };
    input.click();
  };

  const handleClear = async () => {
    if (clearConfirmation !== '清空') return;
    setClearing(true); setError(null);
    try { await clearAllUserData(); localStorage.clear(); window.location.reload(); }
    catch { setDialog(null); setClearConfirmation(''); setError({ section: 'clear', message: '清空失败，原有数据未被删除。' }); }
    finally { setClearing(false); }
  };

  return <div className="settings-page-content">
    {(pendingDirectories.length > 0 || directoryNotice || error?.section === 'directories') && <SettingsSection
      title={pendingDirectories.length > 0 ? '确认恢复的项目目录' : '恢复的项目目录'}
      description={pendingDirectories.length > 0 ? '确认后授权访问所选目录，旧任务需重新发起。' : undefined}>
      {pendingDirectories.length > 0 && <div className="settings-rows">
        <fieldset disabled={busy} className="data-directories" aria-label="需要在本机重新确认的目录">
          {pendingDirectories.map((item) => <div key={item.id} className="data-directory">
            <label>
              <input type="checkbox" aria-label={item.name} checked={!excludedDirectories.includes(item.id)} onChange={(event) => setExcludedDirectories((ids) => event.target.checked ? ids.filter((id) => id !== item.id) : [...ids, item.id])} />
              <span><strong>{item.name}</strong><code>{directoryPaths[item.id] ?? item.rootPath}</code></span>
            </label>
            <Button variant="secondary" disabled={busy} onClick={() => void chooseDirectory(item.id)}>选择文件夹</Button>
          </div>)}
        </fieldset>
        <SettingsRow title="目录授权">
          <Button disabled={busy || selectedDirectories.length === 0} onClick={() => void confirmDirectories()}>{confirmingDirectories ? '正在确认…' : `确认选中的 ${selectedDirectories.length} 个目录`}</Button>
        </SettingsRow>
      </div>}
      {directoryNotice && <InlineNotice tone="success">{directoryNotice}</InlineNotice>}
      {error?.section === 'directories' && <InlineNotice tone="danger">{error.message}</InlineNotice>}
    </SettingsSection>}
    <SettingsSection title="备份与恢复">
      <div className="settings-rows">
        <TextField label="备份密码（可选）" type="password" autoComplete="new-password" value={backupPassword} onChange={(event) => setBackupPassword(event.target.value)}
          hint="至少 12 个字符。留空会导出未加密备份。" />
        <SettingsRow title="备份文件" description="恢复后会重新载入应用。">
          <Button variant="ghost" onClick={() => setDialog('backup')}>查看说明</Button>
          <Button variant="secondary" onClick={handleImport} disabled={busy}>{importing ? '正在导入…' : '导入数据'}</Button>
          <Button variant="secondary" onClick={handleExport} disabled={busy}>{exporting ? '正在导出…' : '导出数据'}</Button>
        </SettingsRow>
      </div>
      {error?.section === 'backup' && <InlineNotice tone="danger">{error.message}</InlineNotice>}
    </SettingsSection>
    <SettingsSection title="存储与隐私">
      <div className="settings-rows">
        <SettingsRow title="默认资料目录" description="兼容或迁移回滚模式可能使用旧目录。">
          <code className="data-default-path">%LOCALAPPDATA%\com.angelbot.desktop</code>
        </SettingsRow>
        <p className="settings-muted settings-rows-note">消息会发给所选模型服务商。模型 API 密钥存于系统凭据库。</p>
      </div>
    </SettingsSection>
    <SettingsSection title="危险操作" tone="danger">
      <div className="settings-rows">
        <SettingsRow title="清空本地数据" description="永久删除本地会话、记忆、任务和设置，无法恢复。">
          <Button variant="danger" disabled={busy} onClick={() => setDialog('clear')}>清空数据…</Button>
        </SettingsRow>
      </div>
      {error?.section === 'clear' && <InlineNotice tone="danger">{error.message}</InlineNotice>}
    </SettingsSection>
    <Dialog open={dialog === 'backup'} title="备份范围与恢复规则" onClose={() => setDialog(null)}>
      <dl className="settings-info-list backup-rules">
        <div>
          <dt>密钥</dt>
          <dd>模型 API 密钥不随备份迁移。仅加密备份可携带 MCP 专用密钥，恢复后存入系统凭据库。</dd>
        </div>
        <div>
          <dt>日常结果文件</dt>
          <dd>单个最多 16 MiB，合计 64 MiB，最多 1024 个。超限会提示。恢复遇到同名内容冲突会停止，保留原文件。</dd>
        </div>
        <div>
          <dt>项目和会话</dt>
          <dd>新版备份保留项目、主对话、会话分支及主 Agent 工具回执。恢复后请确认本机项目目录。备份不包含项目文件原件、子代理工作树或自动化执行队列。旧备份缺失的关联无法恢复。</dd>
        </div>
        <div>
          <dt>记忆与学习</dt>
          <dd>保留已保存的记忆、替代关系及新版开关。记忆修订历史和已学习的交流偏好不随备份迁移。任一侧原已关闭的功能保持关闭。旧备份未记录开关时，两项均关闭。</dd>
        </div>
      </dl>
      <div className="ui-inline"><Button onClick={() => setDialog(null)}>关闭</Button></div>
    </Dialog>
    <Dialog open={dialog === 'clear'} title="永久清空本地数据" role="alertdialog" initialFocusSelector="[data-cancel-clear]"
      onClose={() => { if (!clearing) { setDialog(null); setClearConfirmation(''); } }}>
      <p>此操作无法撤销。日常结果文件和系统凭据库中的 API 密钥会保留。</p>
      <TextField label="输入“清空”以确认" value={clearConfirmation} disabled={clearing} onChange={(event) => setClearConfirmation(event.target.value)} />
      <div className="ui-inline">
        <Button data-cancel-clear disabled={clearing} onClick={() => { setDialog(null); setClearConfirmation(''); }}>取消</Button>
        <Button variant="danger" busy={clearing} disabled={busy || clearConfirmation !== '清空'} onClick={handleClear}>确认清空</Button>
      </div>
    </Dialog>
  </div>;
}

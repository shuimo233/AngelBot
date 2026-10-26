import { useState } from 'react';
import { open as openDirectoryDialog } from '@tauri-apps/plugin-dialog';
import { SettingsSection } from '../components/SettingsSection';
import { clearAllUserData, exportData, exportEncryptedData, importData, importEncryptedData } from '$lib/commands/settings';
import { confirmRestoredProjectDirectories } from '$lib/commands/workspace';
import { useWorkspacesStore } from '$stores/workspaces';

const DownloadIcon = () => <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" /><polyline points="7 10 12 15 17 10" /><line x1="12" y1="15" x2="12" y2="3" /></svg>;
const UploadIcon = () => <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" /><polyline points="17 8 12 3 7 8" /><line x1="12" y1="3" x2="12" y2="15" /></svg>;
const TrashIcon = () => <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><polyline points="3 6 5 6 21 6" /><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" /></svg>;

function isEncryptedBackup(data: string): boolean {
  try { return JSON.parse(data).format === 'angelbot.encrypted-backup'; }
  catch { return false; }
}

export function DataSettings() {
  const [exporting, setExporting] = useState(false);
  const [importing, setImporting] = useState(false);
  const [clearConfirmation, setClearConfirmation] = useState('');
  const [backupPassword, setBackupPassword] = useState('');
  const [clearing, setClearing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const workspaces = useWorkspacesStore((state) => state.workspaces);
  const loadWorkspaces = useWorkspacesStore((state) => state.loadWorkspaces);
  const pendingDirectories = workspaces.filter((item) => item.kind === 'project' && item.directoryConfirmed === false);
  const [directoryPaths, setDirectoryPaths] = useState<Record<string, string>>({});
  const [excludedDirectories, setExcludedDirectories] = useState<string[]>([]);
  const [confirmingDirectories, setConfirmingDirectories] = useState(false);
  const [directoryNotice, setDirectoryNotice] = useState<string | null>(null);
  const busy = exporting || importing || clearing || confirmingDirectories;

  const chooseDirectory = async (id: string) => {
    setError(null);
    try {
      const selected = await openDirectoryDialog({ directory: true, multiple: false, title: '选择此项目在本机的目录' });
      if (typeof selected === 'string') setDirectoryPaths((paths) => ({ ...paths, [id]: selected }));
    } catch { setError('无法打开文件夹选择器，请稍后重试。'); }
  };

  const confirmDirectories = async () => {
    setConfirmingDirectories(true); setError(null); setDirectoryNotice(null);
    const selected = pendingDirectories.filter((item) => !excludedDirectories.includes(item.id));
    try {
      await confirmRestoredProjectDirectories(selected
        .map((item) => ({ id: item.id, expectedPath: item.rootPath ?? '', path: directoryPaths[item.id] ?? item.rootPath ?? '' })));
      setDirectoryNotice(`已确认 ${selected.length} 个项目目录。未选中的目录保持未授权；旧任务不会自动继续。`);
      await loadWorkspaces();
      setDirectoryPaths({}); setExcludedDirectories([]);
      if (useWorkspacesStore.getState().lastError) setError('目录已确认，但工作区列表未能刷新。请稍后重新打开设置；无需再次授权已确认的目录。');
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
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
    } catch (reason) { setError(reason instanceof Error ? reason.message : typeof reason === 'string' ? reason : '导出失败，请稍后重试。'); }
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
      catch (reason) { setError(reason instanceof Error ? reason.message : typeof reason === 'string' ? reason : '导入失败：文件格式无效或无法恢复。'); }
      finally { setBackupPassword(''); setImporting(false); }
    };
    input.click();
  };

  const handleClear = async () => {
    if (clearConfirmation !== '清空') return;
    setClearing(true); setError(null);
    try { await clearAllUserData(); localStorage.clear(); window.location.reload(); }
    catch { setError('清空失败，原有数据未被删除。'); }
    finally { setClearing(false); }
  };

  return <div className="settings-page-content">
    <SettingsSection title="隐私总览" description="数据默认保存在本地；只有你选择的模型或工具服务会收到请求。" defaultOpen>
      <div className="settings-info-list">
        <p><strong>本地数据：</strong>会话、记忆、任务和设置保存在本地 SQLite 数据库。</p>
        <p><strong>模型请求：</strong>消息会发送给当前选择的模型服务商；模型 API 密钥保存在系统 Keychain，不写入数据备份。</p>
        <p><strong>外部工具：</strong>文件、桌面操作和 MCP 受独立权限与确认流程约束。</p>
      </div>
    </SettingsSection>
    {error && <p className="settings-error" role="alert">{error}</p>}
    {directoryNotice && <p className="settings-muted" role="status">{directoryNotice}</p>}
    {pendingDirectories.length > 0 && <SettingsSection title="确认恢复的项目目录" description="历史已经恢复，目录尚未授权；不会自动继续旧任务。" defaultOpen>
      <p className="settings-muted">请核对以下文件夹。一次确认选中的项目后，AngelBot 才能按原有操作权限访问这些目录；不逐文件询问。未选中的项目仍可查看历史。</p>
      <fieldset disabled={busy} className="restored-directory-list">
        <legend className="settings-muted">需要在本机重新确认的目录</legend>
        {pendingDirectories.map((item) => <div key={item.id} className="restored-directory-row">
          <label><input type="checkbox" checked={!excludedDirectories.includes(item.id)} onChange={(event) => setExcludedDirectories((ids) => event.target.checked ? ids.filter((id) => id !== item.id) : [...ids, item.id])} /> {item.name}</label>
          <code>{directoryPaths[item.id] ?? item.rootPath}</code>
          <button type="button" onClick={() => void chooseDirectory(item.id)}>选择文件夹</button>
        </div>)}
      </fieldset>
      <button type="button" className="data-action-btn" disabled={busy || pendingDirectories.every((item) => excludedDirectories.includes(item.id))} onClick={() => void confirmDirectories()}>{confirmingDirectories ? '正在确认…' : `确认选中的 ${pendingDirectories.filter((item) => !excludedDirectories.includes(item.id)).length} 个目录`}</button>
    </SettingsSection>}
    <SettingsSection title="数据备份" description="导出或导入 AngelBot 本地数据">
      <label className="text-field">
        <span className="text-field-label">备份密码（可选）</span>
        <input type="password" autoComplete="new-password" value={backupPassword} onChange={(event) => setBackupPassword(event.target.value)} placeholder="至少 12 个字符；仅用于本次操作，不会保存" />
      </label>
      <p className="settings-muted">普通备份不包含 MCP 专用密钥；设置备份密码后，加密备份可携带这些密钥。恢复时会写入系统凭据库，不会回显原值。</p>
      <p className="settings-muted">备份包含日常结果文件（每个最多 16 MiB，合计 64 MiB，最多 1024 个）；超限会提示，不会静默遗漏。普通备份中的文件内容未加密，私人资料建议使用备份密码。恢复保留现有结果，同名内容冲突时停止，不会覆盖。</p>
      <p className="settings-muted">新版备份保留项目、主对话、会话分支及主 Agent 工具回执；恢复后需在此页一次确认本机项目目录。它不包含项目文件原件、子代理工作树或自动化执行队列，不能代替整机备份；请另行保留原项目文件。旧版备份未保存的关联无法补回。</p>
      <div className="data-actions">
        <button className="data-action-btn" type="button" onClick={handleExport} disabled={busy}><span className="action-icon"><DownloadIcon /></span><span className="action-info"><span className="action-title">{exporting ? '正在导出…' : '导出数据'}</span><span className="action-desc">下载本地数据的备份文件</span></span></button>
        <button className="data-action-btn" type="button" onClick={handleImport} disabled={busy}><span className="action-icon"><UploadIcon /></span><span className="action-info"><span className="action-title">{importing ? '正在导入…' : '导入数据'}</span><span className="action-desc">从备份文件恢复数据</span></span></button>
      </div>
    </SettingsSection>
    <SettingsSection title="存储位置" description="数据存储路径"><div className="storage-path"><code>%LOCALAPPDATA%/angelbot/</code></div></SettingsSection>
    <SettingsSection title="危险操作" description="永久删除本地会话、记忆、任务和设置，无法恢复。">
      <label className="text-field"><span className="text-field-label">输入“清空”以确认</span><input value={clearConfirmation} onChange={(event) => setClearConfirmation(event.target.value)} /></label>
      <button className="danger-action-btn" type="button" disabled={busy || clearConfirmation !== '清空'} onClick={handleClear}><span className="action-icon"><TrashIcon /></span><span className="action-info"><span className="action-title">{clearing ? '正在清空…' : '永久清空本地数据'}</span><span className="action-desc">不会清除日常结果文件或系统 Keychain 中的 API 密钥</span></span></button>
    </SettingsSection>
  </div>;
}

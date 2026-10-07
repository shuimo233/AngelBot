import { useState } from 'react';
import { SettingsSection } from '../components/SettingsSection';
import { clearAllUserData, exportData, exportEncryptedData, importData, importEncryptedData } from '$lib/commands/settings';

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

  const handleExport = async () => {
    setExporting(true); setError(null);
    try {
      const encrypted = backupPassword.length > 0;
      const data = encrypted ? await exportEncryptedData(backupPassword) : await exportData();
      const url = URL.createObjectURL(new Blob([data], { type: 'application/json' }));
      const anchor = document.createElement('a');
      anchor.href = url; anchor.download = `angelbot-backup-${new Date().toISOString().slice(0, 10)}${encrypted ? '.encrypted' : ''}.json`;
      anchor.click(); URL.revokeObjectURL(url);
    } catch { setError('导出失败，请稍后重试。'); }
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
      catch { setError('导入失败：文件格式无效或无法恢复。'); }
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
    <SettingsSection title="数据备份" description="导出或导入 AngelBot 本地数据">
      <label className="text-field">
        <span className="text-field-label">备份密码（可选）</span>
        <input type="password" autoComplete="new-password" value={backupPassword} onChange={(event) => setBackupPassword(event.target.value)} placeholder="至少 12 个字符；仅用于本次操作，不会保存" />
      </label>
      <p className="settings-muted">普通备份不包含 MCP 专用密钥；设置备份密码后，加密备份可携带这些密钥。恢复时会写入系统凭据库，不会回显原值。</p>
      <div className="data-actions">
        <button className="data-action-btn" type="button" onClick={handleExport} disabled={exporting}><span className="action-icon"><DownloadIcon /></span><span className="action-info"><span className="action-title">{exporting ? '正在导出…' : '导出数据'}</span><span className="action-desc">下载本地数据的备份文件</span></span></button>
        <button className="data-action-btn" type="button" onClick={handleImport} disabled={importing}><span className="action-icon"><UploadIcon /></span><span className="action-info"><span className="action-title">{importing ? '正在导入…' : '导入数据'}</span><span className="action-desc">从备份文件恢复数据</span></span></button>
      </div>
    </SettingsSection>
    <SettingsSection title="存储位置" description="数据存储路径"><div className="storage-path"><code>%APPDATA%/AngelBot/</code></div></SettingsSection>
    <SettingsSection title="危险操作" description="永久删除本地会话、记忆、任务和设置，无法恢复。">
      <label className="text-field"><span className="text-field-label">输入“清空”以确认</span><input value={clearConfirmation} onChange={(event) => setClearConfirmation(event.target.value)} /></label>
      <button className="danger-action-btn" type="button" disabled={clearing || clearConfirmation !== '清空'} onClick={handleClear}><span className="action-icon"><TrashIcon /></span><span className="action-info"><span className="action-title">{clearing ? '正在清空…' : '永久清空本地数据'}</span><span className="action-desc">不会清除系统 Keychain 中的 API 密钥</span></span></button>
    </SettingsSection>
  </div>;
}

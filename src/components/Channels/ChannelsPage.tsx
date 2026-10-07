import { useEffect, useState } from 'react';
import {
  clearChannelCredential, confirmChannelMessage, createChannelConnection, getChannelAudit,
  getChannelConnections, revokeChannelConnection, saveChannelCredential, setChannelEnabled,
  setChannelProactivePolicy, type ChannelAuditEntry, type ChannelConnection,
} from '$lib/commands/channel';

export function ChannelsPage() {
  const [connections, setConnections] = useState<ChannelConnection[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [name, setName] = useState('');
  const [credential, setCredential] = useState<Record<string, string>>({});
  const [review, setReview] = useState(false);
  const [audit, setAudit] = useState<Record<string, ChannelAuditEntry[]>>({});
  const load = () => getChannelConnections().then(setConnections).catch(() => setError('无法加载渠道连接。'));
  useEffect(() => { load(); }, []);
  const run = async (action: () => Promise<void>) => { try { setError(null); await action(); await load(); } catch (e) { setError(e instanceof Error ? e.message : '渠道操作失败。'); } };
  return <section aria-label="渠道与远程访问" className="settings-page">
    <h2>渠道与远程访问</h2>
    <p>外部渠道的每次消息投递都必须在桌面端确认并写入审计记录。</p>
    <div className="settings-section"><strong>本地安全边界</strong><p>凭据默认遮罩，仅写入操作系统钥匙串；SQLite 不保存任何凭据内容。</p></div>
    <div className="settings-section"><label htmlFor="channel-name">渠道名称</label><input id="channel-name" value={name} onChange={event => setName(event.target.value)} placeholder="例如：个人消息渠道" /><button type="button" disabled={!name.trim()} onClick={() => setReview(true)}>登记渠道</button></div>
    {review && <div className="settings-section" role="alert"><strong>确认登记渠道</strong><p>登记不会保存凭据或启用消息收发；启用后仍需逐次确认副作用。</p><button type="button" onClick={() => run(async () => { await createChannelConnection({ channelType: 'custom', displayName: name, permissionSummary: '消息收发与外部数据传输需逐次确认' }); setName(''); setReview(false); })}>允许一次</button><button type="button" onClick={() => setReview(false)}>取消</button></div>}
    {error && <p role="alert">{error}</p>}
    {connections.map(connection => <div key={connection.id} className="settings-section">
      <strong>{connection.displayName}</strong>
      <p>状态：{connection.enabled ? '已启用' : '未启用'}</p><p>数据去向：{connection.channelType === 'custom' ? '登记的外部渠道服务' : connection.channelType}</p><p>权限范围：{connection.permissionSummary}</p>
      <p>主动行为：{connection.proactiveDailyLimit > 0 ? `每天最多 ${connection.proactiveDailyLimit} 次；原因：${connection.proactiveReason || '未说明'}` : '已关闭'}</p>
      <label>渠道凭据（仅保存在系统钥匙串）<input type="password" autoComplete="off" value={credential[connection.id] ?? ''} onChange={event => setCredential(current => ({ ...current, [connection.id]: event.target.value }))} placeholder={connection.credentialConfigured ? '已配置，输入新值可替换' : '输入渠道凭据'} /></label>
      <button type="button" disabled={!(credential[connection.id] ?? '').trim()} onClick={() => run(async () => { await saveChannelCredential(connection.id, credential[connection.id]); setCredential(current => ({ ...current, [connection.id]: '' })); })}>安全保存凭据</button>
      {connection.credentialConfigured && <button type="button" onClick={() => run(() => clearChannelCredential(connection.id))}>移除凭据</button>}
      <button type="button" onClick={() => run(async () => { const reason = window.prompt('主动触发原因（留空将关闭）', connection.proactiveReason) ?? connection.proactiveReason; const limit = reason.trim() ? Number(window.prompt('每天最多次数（1-24）', String(connection.proactiveDailyLimit || 1)) || 0) : 0; await setChannelProactivePolicy(connection.id, reason.trim(), limit); })}>主动行为设置</button>
      <button type="button" disabled={!connection.enabled} onClick={() => run(() => confirmChannelMessage(connection.id, '用户确认的测试投递', true))}>确认测试投递</button>
      <button type="button" onClick={() => run(() => setChannelEnabled(connection.id, !connection.enabled))}>{connection.enabled ? '断开连接' : '启用连接'}</button>
      <button type="button" onClick={async () => { const entries = await getChannelAudit(connection.id); setAudit(current => ({ ...current, [connection.id]: entries })); }}>查看审计</button><button type="button" onClick={() => run(() => revokeChannelConnection(connection.id))}>移除渠道</button>
      {audit[connection.id]?.map(item => <p key={item.id}><small>{item.action}: {item.details || '已记录'}</small></p>)}
    </div>)}
  </section>;
}

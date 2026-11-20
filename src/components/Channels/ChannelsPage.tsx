import { useEffect, useState } from 'react';
import { Button, TextField } from '../../ui';
import { SettingsRow, SettingsSection } from '../Settings/components/SettingsSection';
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
  return <section aria-label="渠道与远程访问" className="settings-page-content channels-settings">
    <SettingsSection title="添加渠道">
      <div className="settings-rows channel-form-panel">
        <div className="settings-field-action">
          <TextField id="channel-name" label="渠道名称" value={name} onChange={event => setName(event.target.value)} placeholder="例如：个人消息渠道" />
          <Button variant="primary" disabled={!name.trim()} onClick={() => setReview(true)}>登记渠道</Button>
        </div>
        {review && <section className="settings-inline-review channel-review" role="alert">
          <h4>确认登记渠道</h4>
          <p>仅登记渠道。凭据与连接状态可随后设置。</p>
          <div className="settings-form-actions">
            <Button onClick={() => setReview(false)}>取消</Button>
            <Button variant="primary" onClick={() => run(async () => { await createChannelConnection({ channelType: 'custom', displayName: name, permissionSummary: '消息收发与外部数据传输需逐次确认' }); setName(''); setReview(false); })}>允许一次</Button>
          </div>
        </section>}
      </div>
    </SettingsSection>
    {error && <p className="settings-error" role="alert">{error}</p>}
    {connections.map(connection => <SettingsSection key={connection.id} title={connection.displayName} status={connection.enabled ? '已启用' : '未启用'}
      action={<Button size="small" onClick={() => run(() => setChannelEnabled(connection.id, !connection.enabled))}>{connection.enabled ? '断开连接' : '启用连接'}</Button>}>
      <div className="settings-rows channel-form-panel">
        <dl className="channel-facts">
          <div><dt>数据去向</dt><dd>{connection.channelType === 'custom' ? '登记的外部渠道服务' : connection.channelType}</dd></div>
          <div><dt>权限范围</dt><dd>{connection.permissionSummary}</dd></div>
        </dl>
        <div className="settings-field-action">
          <TextField label="渠道凭据（仅存系统凭据库）" type="password" autoComplete="off" value={credential[connection.id] ?? ''} onChange={event => setCredential(current => ({ ...current, [connection.id]: event.target.value }))} placeholder={connection.credentialConfigured ? '输入新凭据可替换' : undefined} />
          <div className="settings-form-actions">
            {connection.credentialConfigured && <Button variant="ghost" onClick={() => run(() => clearChannelCredential(connection.id))}>移除凭据</Button>}
            <Button disabled={!(credential[connection.id] ?? '').trim()} onClick={() => run(async () => { await saveChannelCredential(connection.id, credential[connection.id]); setCredential(current => ({ ...current, [connection.id]: '' })); })}>安全保存凭据</Button>
          </div>
        </div>
        <SettingsRow title="主动行为" description={connection.proactiveDailyLimit > 0 ? `每天最多 ${connection.proactiveDailyLimit} 次 · ${connection.proactiveReason || '未说明'}` : '已关闭'}>
          <Button size="small" onClick={() => run(async () => { const reason = window.prompt('主动触发原因（留空将关闭）', connection.proactiveReason) ?? connection.proactiveReason; const limit = reason.trim() ? Number(window.prompt('每天最多次数（1-24）', String(connection.proactiveDailyLimit || 1)) || 0) : 0; await setChannelProactivePolicy(connection.id, reason.trim(), limit); })}>主动行为设置</Button>
        </SettingsRow>
        <SettingsRow title="消息投递" description="每次投递均需确认。">
          <Button size="small" disabled={!connection.enabled} onClick={() => run(() => confirmChannelMessage(connection.id, '用户确认的测试投递', true))}>确认测试投递</Button>
        </SettingsRow>
        <SettingsRow title="投递记录">
          <Button size="small" onClick={async () => {
            setError(null);
            try {
              const entries = await getChannelAudit(connection.id);
              setAudit(current => ({ ...current, [connection.id]: entries }));
            } catch {
              setError('无法读取渠道审计记录。');
            }
          }}>查看审计</Button>
        </SettingsRow>
        {audit[connection.id] && <div className="channel-audit" role="status">
          {audit[connection.id].length ? <ul>{audit[connection.id].map(item => <li key={item.id}>{item.action}: {item.details || '已记录'}</li>)}</ul> : '暂无审计记录。'}
        </div>}
        <SettingsRow title="连接管理">
          <Button size="small" variant="danger" onClick={() => run(() => revokeChannelConnection(connection.id))}>移除渠道</Button>
        </SettingsRow>
      </div>
    </SettingsSection>)}
  </section>;
}

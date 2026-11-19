import { useEffect, useRef, useState } from 'react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import {
  confirmDesktopObservationScope,
  deleteDesktopTrustedApp,
  getDesktopTrustedAppStatuses,
  getDesktopTrustedApps,
  inspectDesktopDraftTargets,
  saveDesktopTrustedApp,
  type DesktopCapability,
  type DesktopDraftTargetCandidate,
  type TrustedDesktopApp,
  type TrustedDesktopAppRuntimeStatus,
} from '$lib/commands/desktop';
import { Button, Input, TextField } from '../../../ui';
import { SettingsSection } from '../components/SettingsSection';
import './PermissionSettings.css';

const emptyForm = { id: '', displayName: '', executablePath: '', launch: true, draft: false, fill: false, interact: false, draftSelector: '', enabled: true };

function runtimeStatusLabel(status: TrustedDesktopAppRuntimeStatus | undefined) {
  if (status === 'available') return '可供 AngelBot 使用';
  if (status === 'disabled') return '已停用';
  if (status === 'executableUnavailable') return '程序不可用，请重新选择';
  return '状态未知';
}

function capabilityLabel(capability: DesktopCapability) {
  if (capability === 'launch') return '打开应用';
  if (capability === 'draft') return '填写消息草稿';
  if (capability === 'fill') return '填写文本字段';
  if (capability === 'interact') return '操作控件';
  return '查看页面';
}

function discoveryErrorLabel(reason: unknown) {
  const message = reason instanceof Error ? reason.message : String(reason);
  if (message.startsWith('TARGET_NOT_FOUND:')) return '未找到应用主窗口。请先打开应用并进入包含输入框的页面。';
  if (message.startsWith('TARGET_AMBIGUOUS:')) return '应用打开了多个主窗口。请只保留需要填写的窗口后重试。';
  if (message.startsWith('SCAN_LIMIT:')) return '当前页面可编辑控件过多，无法安全辨认。请切换到输入框较少的页面后重试。';
  if (message.startsWith('SCAN_TIMEOUT:')) return '查找超时。请进入目标页面并关闭不相关窗口后重试。';
  return message;
}

export function DesktopControlSettings() {
  const editorRef = useRef<HTMLDivElement>(null);
  const [apps, setApps] = useState<TrustedDesktopApp[]>([]);
  const [statuses, setStatuses] = useState<Record<string, TrustedDesktopAppRuntimeStatus>>({});
  const [form, setForm] = useState(emptyForm);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [scopeError, setScopeError] = useState<string | null>(null);
  const [discovery, setDiscovery] = useState<{ appId: string; executablePath: string; candidates: DesktopDraftTargetCandidate[] } | null>(null);
  const [discoveryBusy, setDiscoveryBusy] = useState(false);
  const [discoveryError, setDiscoveryError] = useState<string | null>(null);
  const discoveryRequest = useRef(0);
  const clearDiscovery = () => {
    discoveryRequest.current += 1;
    setDiscovery(null);
    setDiscoveryBusy(false);
    setDiscoveryError(null);
  };
  const reload = async () => {
    setLoading(true);
    try {
      const [nextApps, nextStatuses] = await Promise.all([
        getDesktopTrustedApps(),
        getDesktopTrustedAppStatuses(),
      ]);
      setApps(nextApps);
      setStatuses(Object.fromEntries(nextStatuses.map((item) => [item.id, item.status])));
    } finally {
      setLoading(false);
    }
  };
  useEffect(() => { reload().catch((reason) => setError(String(reason))); }, []);

  const save = async () => {
    const capabilities: DesktopCapability[] = [
      ...(form.launch ? ['launch' as const] : []),
      ...(form.draft ? ['draft' as const] : []),
      ...(form.fill ? ['fill' as const] : []),
      ...(form.interact ? ['interact' as const] : []),
    ];
    setBusy(true); setError(null);
    try {
      await saveDesktopTrustedApp({ id: form.id, displayName: form.displayName, executablePath: form.executablePath, capabilities, draftSelector: form.draft ? form.draftSelector : null, enabled: form.enabled, createdAt: 0, updatedAt: 0 });
      clearDiscovery(); setForm(emptyForm); await reload();
    } catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)); }
    finally { setBusy(false); }
  };

  const edit = (app: TrustedDesktopApp) => {
    clearDiscovery(); setError(null);
    setForm({ id: app.id, displayName: app.displayName, executablePath: app.executablePath, launch: app.capabilities.includes('launch'), draft: app.capabilities.includes('draft'), fill: app.capabilities.includes('fill'), interact: app.capabilities.includes('interact'), draftSelector: app.draftSelector ?? '', enabled: app.enabled });
    editorRef.current?.focus({ preventScroll: true });
    editorRef.current?.scrollIntoView?.({ block: 'start' });
  };
  const pickExecutable = async () => {
    setError(null);
    try {
      const selected = await openDialog({
        multiple: false,
        directory: false,
        filters: [{ name: 'Windows 应用程序', extensions: ['exe'] }],
        title: '选择 AngelBot 可接触的应用',
      });
      if (typeof selected !== 'string') return;
      const filename = selected.split(/[\\/]/).pop() ?? '';
      const stem = filename.replace(/\.exe$/i, '');
      const suggestedId = stem.toLowerCase().replace(/[^a-z0-9_-]+/g, '-').replace(/^-+|-+$/g, '');
      clearDiscovery();
      setForm((current) => ({
        ...current,
        executablePath: selected,
        displayName: current.displayName || stem,
        id: current.id || suggestedId,
      }));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  };
  const remove = async (id: string) => {
    setBusy(true); setError(null);
    try {
      await deleteDesktopTrustedApp(id);
      if (form.id === id) { clearDiscovery(); setForm(emptyForm); }
      await reload();
    }
    catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)); }
    finally { setBusy(false); }
  };
  const editingApp = apps.find((app) => app.id === form.id);
  const editing = Boolean(editingApp);
  const legacyApps = apps.filter((app) => !app.capabilities.includes('observeImage'));
  const editingLegacyApp = Boolean(editingApp && !editingApp.capabilities.includes('observeImage'));
  const savedPathChanged = Boolean(editingApp && editingApp.executablePath !== form.executablePath);
  const canDiscover = Boolean(editingApp && !savedPathChanged);
  const activeDiscovery = discovery?.appId === form.id && discovery.executablePath === form.executablePath ? discovery : null;
  const selectorNotDiscovered = Boolean(activeDiscovery && form.draftSelector.trim() && !activeDiscovery.candidates.some((item) => item.name === form.draftSelector.trim()));
  const valid = form.id.trim() && form.displayName.trim() && form.executablePath.trim() && (!form.draft || form.draftSelector.trim()) && (!savedPathChanged || (!form.draft && !form.fill && !form.interact)) && !selectorNotDiscovered;
  const confirmLegacyApps = async () => {
    setBusy(true); setScopeError(null);
    try {
      await confirmDesktopObservationScope(legacyApps.map(({ id, executablePath }) => ({ id, executablePath })));
      await reload();
    } catch (reason) { setScopeError(reason instanceof Error ? reason.message : String(reason)); }
    finally { setBusy(false); }
  };
  const discoverTargets = async () => {
    if (!editingApp || !canDiscover) return;
    const requestId = ++discoveryRequest.current;
    setDiscovery(null); setDiscoveryError(null); setDiscoveryBusy(true);
    try {
      const result = await inspectDesktopDraftTargets(editingApp.id);
      if (requestId !== discoveryRequest.current) return;
      if (result.appId !== editingApp.id) throw new Error('应用已切换，请重新查找输入框。');
      setDiscovery({ appId: result.appId, executablePath: editingApp.executablePath, candidates: result.candidates });
    } catch (reason) {
      if (requestId === discoveryRequest.current) setDiscoveryError(discoveryErrorLabel(reason));
    } finally {
      if (requestId === discoveryRequest.current) setDiscoveryBusy(false);
    }
  };

  return <div className="settings-page-content desktop-permissions">
    <SettingsSection title="受信任应用">
      {loading && <p className="permission-copy" aria-live="polite">正在检查应用状态…</p>}
      {!loading && apps.length === 0 && <p className="permission-copy">尚未授权应用。</p>}
      {apps.length > 0 && <ul className="settings-rows desktop-permission-apps">{apps.map((app) => {
        const runtimeStatus = statuses[app.id];
        const actions = app.capabilities.filter((capability) => capability !== 'observe' && capability !== 'observeImage');
        const scope = app.capabilities.includes('observeImage') ? '可查看结构与图像' : app.capabilities.includes('observe') ? '仅结构，图像需确认' : '旧配置，尚未允许查看';
        return <li className="desktop-permission-app" key={app.id}>
          <div><strong>{app.displayName}</strong><code>{app.executablePath}</code><small className={runtimeStatus === 'executableUnavailable' ? 'permission-error' : undefined}>{runtimeStatusLabel(runtimeStatus)} · {scope} · {actions.length ? actions.map(capabilityLabel).join(' · ') : '仅查看'}</small></div>
          <div className="settings-form-actions">
            <Button size="small" variant="ghost" disabled={busy} onClick={() => remove(app.id)}>移除</Button>
            <Button size="small" disabled={busy} onClick={() => edit(app)}>编辑</Button>
          </div>
        </li>;
      })}</ul>}
      {!loading && legacyApps.length > 0 && <div className="desktop-permission-upgrade permission-related-action">
        <div className="permission-related-copy">
          <strong>旧配置需要确认</strong>
          <p>{legacyApps.map((app) => app.displayName).join('、')} · 确认后可把窗口图像发给模型，可能含隐私。</p>
        </div>
        <Button size="small" disabled={busy} onClick={confirmLegacyApps}>允许查看上述应用</Button>
        {scopeError && <p className="permission-error" role="alert">确认失败：{scopeError}</p>}
      </div>}
    </SettingsSection>
    <SettingsSection title={editing ? '编辑受信任应用' : '添加受信任应用'}>
      <div className="settings-rows desktop-permission-form" ref={editorRef} tabIndex={-1}>
        <div className="desktop-permission-field">
          <label className="ui-field-label" htmlFor="desktop-executable-path">程序路径</label>
          <div className="settings-field-action">
            <Input id="desktop-executable-path" value={form.executablePath} onChange={(event) => { clearDiscovery(); setForm({ ...form, executablePath: event.target.value }); }} placeholder="选择或填写完整的 .exe 路径" />
            <Button disabled={busy} onClick={pickExecutable}>选择程序</Button>
          </div>
        </div>
        <div className="desktop-permission-identity">
          <TextField label="显示名称" value={form.displayName} onChange={(event) => setForm({ ...form, displayName: event.target.value })} placeholder="例如 Codex" />
          <TextField label="内部标识" value={form.id} disabled={editing} onChange={(event) => setForm({ ...form, id: event.target.value })} placeholder="选择程序后自动生成" />
        </div>
        <fieldset className="desktop-permission-options">
          <legend>允许操作</legend>
          <label><input type="checkbox" checked={form.launch} onChange={(event) => setForm({ ...form, launch: event.target.checked })} />允许打开应用</label>
          <label><input type="checkbox" checked={form.draft} onChange={(event) => { clearDiscovery(); setForm({ ...form, draft: event.target.checked }); }} />允许填写消息草稿</label>
          <label><input type="checkbox" checked={form.fill} onChange={(event) => setForm({ ...form, fill: event.target.checked })} />允许填写普通文本字段</label>
          <label><input type="checkbox" checked={form.interact} onChange={(event) => setForm({ ...form, interact: event.target.checked })} />操作控件</label>
        </fieldset>
        {form.draft && <div className="desktop-permission-draft">
          <TextField label="草稿输入框名称" value={form.draftSelector} onChange={(event) => setForm({ ...form, draftSelector: event.target.value })} placeholder="查找或手动填写" hint="按名称定位唯一的普通输入框。" />
          <div className="desktop-permission-discovery permission-related-action">
            <div className="permission-related-copy">
              {!editing && <small>新应用请先只勾选「允许打开应用」并保存，再编辑查找。</small>}
              {savedPathChanged && <small>请先取消草稿权限并保存新路径，再查找。</small>}
              {canDiscover && <small>请先打开目标输入框所在的窗口。</small>}
            </div>
            <Button size="small" disabled={busy || discoveryBusy || !canDiscover} onClick={discoverTargets}>{discoveryBusy ? '正在查找…' : '查找当前窗口的输入框'}</Button>
          </div>
          {discoveryError && <p className="permission-error" role="alert">查找失败：{discoveryError}</p>}
          {activeDiscovery && <div className="desktop-permission-results" aria-live="polite">
            {activeDiscovery.candidates.length === 0 ? <p>没有可安全填写的输入框。请核对窗口或手动填写名称。</p> : <>
              <p>选定输入框后保存。</p>
              <ul>{activeDiscovery.candidates.map((candidate) => <li key={candidate.name}>
                <Button size="small" onClick={() => setForm((current) => ({ ...current, draftSelector: candidate.name }))}
                  aria-pressed={form.draftSelector === candidate.name}>选择“{candidate.name}”</Button>
              </li>)}</ul>
            </>}
            {selectorNotDiscovered && <p className="permission-warning">结果中没有此名称。请核对窗口或重新选择。</p>}
          </div>}
        </div>}
        <div className="desktop-permission-copy">
          <strong>查看范围</strong>
          <p>读取目标窗口的控件，含隐私的窗口图像可能发给模型。截图不保存。</p>
          <p>完全访问可直接查看图像，其余模式需确认。</p>
          <p>最多读取 80 字可见静态文字，跳过编辑框值、密码、隐藏内容和后台页面。</p>
          {editingLegacyApp && <p className="permission-warning">保存即授权图像查看。</p>}
          {form.fill && <p>填写需逐次确认，应用可能自动保存或同步。限普通文本字段。</p>}
          {form.interact && <>
            <p>控件操作需逐次确认，可读取最多 80 字的下拉项名称。</p>
            <p>图像点击需确认位置，窗口或画面变化时停止。操作可能触发发送或删除。</p>
          </>}
          {savedPathChanged && form.fill && <p className="permission-warning">请先取消普通文本字段填写权限，再保存新路径。</p>}
          {savedPathChanged && form.interact && <p className="permission-warning">请先取消操作控件权限，再保存新路径。</p>}
        </div>
        <div className="desktop-permission-submit">
          <label><input type="checkbox" checked={form.enabled} onChange={(event) => setForm({ ...form, enabled: event.target.checked })} />启用此配置</label>
          <div className="settings-form-actions">
            {form.id && <Button variant="ghost" disabled={busy} onClick={() => { clearDiscovery(); setForm(emptyForm); }}>取消</Button>}
            <Button variant="primary" disabled={busy || !valid} onClick={save}>{editingLegacyApp ? '保存并允许查看' : editing ? '保存配置' : '添加并允许查看'}</Button>
          </div>
        </div>
      </div>
      {error && <p className="permission-error" role="alert">{error}</p>}
    </SettingsSection>
  </div>;
}

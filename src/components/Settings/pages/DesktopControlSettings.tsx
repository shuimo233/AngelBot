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

  return <div className="settings-page-content desktop-control-settings">
    <section className="desktop-control-card">
      <p className="desktop-control-eyebrow">DESKTOP CONTROL</p><h3>受信任的电脑操作</h3>
      <p>AngelBot 优先使用项目文件能力和按工作区启用的 MCP；只有应用没有稳定接口时，才使用这里授权的 Windows 桌面操作。</p>
      <dl className="desktop-control-layers">
        <div><dt>项目文件</dt><dd>读写、创建目录，并可在资源管理器中定位当前项目内的文件。</dd></div>
        <div><dt>MCP</dt><dd>应用提供 MCP 或本地连接器时，使用结构化工具完成操作。</dd></div>
        <div><dt>桌面回退</dt><dd>查看已授权应用的窗口结构或单窗口图像，或通过 Windows UI Automation 填写文本、操作已明确授权的控件；每次写入或操作前需要核对并确认。</dd></div>
      </dl>
      <div className="desktop-control-safety"><b>安全边界</b><span>添加或保存应用后，AngelBot 可读取该应用当前窗口的部分控件名称和标识，必要时获取单窗口图像并交给当前模型。图像可能包含私人内容；不会采集全桌面，不保存截图，检测到密码控件、锁屏或受保护窗口时拒绝捕获，但不能保证识别所有敏感内容。请只添加你愿意交给模型查看的应用。图像查看遵循对话中的整体操作权限；“完全访问”不逐次询问，其他模式先确认。能否打开应用、填写消息草稿或其他文本字段、操作控件由下方分别设置；图像本身不授予点击或输入权限，每次写入或操作前仍会核对具体目标，文本写入还会核对完整内容。文本填写不包含点击发送，目标应用可能自动保存或同步；调用、选中、展开或收起控件默认关闭，单独授权后仍须逐次确认，可能触发发送或删除，不能仅凭控件名称推断安全或权限。未登记程序和歧义控件会被拒绝。</span></div>
    </section>
    <section className="desktop-control-card">
      <h3>{editing ? '编辑受信任应用' : '添加受信任应用'}</h3>
      <label htmlFor="desktop-executable-path">程序路径</label>
      <div className="desktop-control-path-field">
        <input id="desktop-executable-path" value={form.executablePath} onChange={(event) => { clearDiscovery(); setForm({ ...form, executablePath: event.target.value }); }} placeholder="选择或填写完整的 .exe 路径" />
        <button type="button" className="secondary" disabled={busy} onClick={pickExecutable}>选择程序</button>
      </div>
      <label>显示名称<input value={form.displayName} onChange={(event) => setForm({ ...form, displayName: event.target.value })} placeholder="例如 Codex" /></label>
      <label>内部标识<input value={form.id} disabled={editing} onChange={(event) => setForm({ ...form, id: event.target.value })} placeholder="选择程序后自动生成" /><small>用于让 AngelBot 准确选择应用，通常不需要手动修改。</small></label>
      <div className="desktop-control-capabilities">
        <label><input type="checkbox" checked={form.launch} onChange={(event) => setForm({ ...form, launch: event.target.checked })} />允许打开应用</label>
        <label><input type="checkbox" checked={form.draft} onChange={(event) => { clearDiscovery(); setForm({ ...form, draft: event.target.checked }); }} />允许填写消息草稿</label>
        <label><input type="checkbox" checked={form.fill} onChange={(event) => setForm({ ...form, fill: event.target.checked })} />允许填写普通文本字段</label>
        <label><input type="checkbox" checked={form.interact} onChange={(event) => setForm({ ...form, interact: event.target.checked })} />操作控件</label>
        <label><input type="checkbox" checked={form.enabled} onChange={(event) => setForm({ ...form, enabled: event.target.checked })} />启用此配置</label>
      </div>
      {!form.launch && !form.draft && !form.fill && !form.interact && <p>仅查看：AngelBot 可查看此应用的窗口结构或单窗口图像，但不能替你打开应用、填写文本或操作控件。</p>}
      {editingLegacyApp && <p>这是旧版查看范围，尚未允许单窗口图像。保存后会允许将此应用的单窗口图像交给当前模型；如不需要，请保持原配置。</p>}
      {form.fill && <p>允许后可填写当前窗口中经核对的普通文本字段；目标应用可能自动保存或同步。此权限不包含调用、选中、展开或收起控件、发送消息或修改密码。</p>}
      {form.interact && <p>允许后可逐次确认并调用、选中、展开或收起当前窗口中经核对的控件；可能触发发送、删除或其他不可撤销后果。控件名称不代表安全或授权；调用回执只表示操作请求已发出，其他回执仅确认控件状态，仍需检查任务结果，不代表目标已完成。</p>}
      {savedPathChanged && form.fill && <p>程序路径已修改；请先取消普通文本字段填写权限并保存新路径，再单独授权新程序。</p>}
      {savedPathChanged && form.interact && <p>程序路径已修改；请先取消操作控件权限并保存新路径，再单独授权新程序。</p>}
      {form.draft && <>
        <label>草稿输入框名称<input value={form.draftSelector} onChange={(event) => setForm({ ...form, draftSelector: event.target.value })} placeholder="可从当前窗口查找，或手动填写" /><small>只按名称定位唯一的普通输入框；不会读取其中的文字。</small></label>
        <div className="desktop-control-discovery">
          <button type="button" className="secondary" disabled={busy || discoveryBusy || !canDiscover} onClick={discoverTargets}>{discoveryBusy ? '正在查找…' : '查找当前窗口的输入框'}</button>
          {!editing && <small>新应用请先只勾选“允许打开应用”并保存，然后编辑它来查找输入框。</small>}
          {savedPathChanged && <small>程序路径已修改；请先取消草稿权限并保存新路径，再重新查找输入框。</small>}
          {canDiscover && <small>先打开该应用并让目标输入框可见。已停用的应用也可以在这里配置，启用后 AngelBot 才能使用。</small>}
        </div>
        {discoveryError && <p className="desktop-control-error" role="alert">查找失败：{discoveryError}</p>}
        {activeDiscovery && <div className="desktop-control-discovery-results" aria-live="polite">
          {activeDiscovery.candidates.length === 0 ? <p>当前窗口没有可安全填写的输入框。请确认输入框可见、未被禁用，或手动核对名称。</p> : <>
            <p>找到 {activeDiscovery.candidates.length} 个可填写输入框。选择后仍需保存配置；不会立即授权或填写。</p>
            <ul>{activeDiscovery.candidates.map((candidate) => <li key={candidate.name}><button type="button" className={form.draftSelector === candidate.name ? 'is-selected' : ''} onClick={() => setForm((current) => ({ ...current, draftSelector: candidate.name }))} aria-pressed={form.draftSelector === candidate.name}>选择“{candidate.name}”</button></li>)}</ul>
          </>}
          {selectorNotDiscovered && <p className="desktop-control-discovery-warning">当前填写的名称未出现在本次查找结果中。请打开正确窗口后重试，或从列表重新选择。</p>}
        </div>}
      </>}
      <div className="desktop-control-actions"><button disabled={busy || !valid} onClick={save}>{editingLegacyApp ? '保存并允许查看' : editing ? '保存配置' : '添加并允许查看'}</button>{form.id && <button className="secondary" disabled={busy} onClick={() => { clearDiscovery(); setForm(emptyForm); }}>取消</button>}</div>
      {error && <p className="desktop-control-error" role="alert">{error}</p>}
    </section>
    <section className="desktop-control-card"><h3>可接触的应用</h3>
      {loading && <p aria-live="polite">正在检查应用状态…</p>}
      {!loading && apps.length === 0 && <p>尚未添加应用。默认情况下，AngelBot 无权操作任何外部程序。</p>}
      {!loading && legacyApps.length > 0 && <div className="desktop-control-safety desktop-control-upgrade">
        <b>旧配置需要确认</b>
        <span>以下 {legacyApps.length} 个应用尚未允许单窗口图像：{legacyApps.map((app) => app.displayName).join('、')}。原有结构查看和操作权限保持不变。确认后可将这些应用的单窗口图像交给当前模型，可能包含私人内容；不采集全桌面，不保存截图。图像查看遵循整体操作权限，不授予点击或输入权限。你也可以逐个编辑并保存。</span>
        <button type="button" className="secondary" disabled={busy} onClick={confirmLegacyApps}>允许查看上述应用</button>
        {scopeError && <p className="desktop-control-error" role="alert">确认失败：{scopeError}</p>}
      </div>}
      {apps.map((app) => {
        const runtimeStatus = statuses[app.id];
        const actions = app.capabilities.filter((capability) => capability !== 'observe' && capability !== 'observeImage');
        const scope = app.capabilities.includes('observeImage') ? '可查看结构与图像' : app.capabilities.includes('observe') ? '仅结构，图像需确认' : '旧配置，尚未允许查看';
        return <div className="desktop-control-row" key={app.id}><div><b>{app.displayName}</b><code>{app.executablePath}</code><small className={runtimeStatus === 'executableUnavailable' ? 'is-unavailable' : ''}>{runtimeStatusLabel(runtimeStatus)} · {scope} · {actions.length ? actions.map(capabilityLabel).join(' · ') : '仅查看'}</small></div><button disabled={busy} onClick={() => edit(app)}>编辑</button><button disabled={busy} onClick={() => remove(app.id)}>移除</button></div>;
      })}
    </section>
  </div>;
}

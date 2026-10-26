import { useEffect, useRef, useState } from 'react';
import { SettingsSection } from '../components/SettingsSection';
import { TextField } from '../components/TextField';
import { ToggleField } from '../components/ToggleField';
import {
  clearMcpEnv,
  clearBrowserTempFiles,
  deleteMcpServer,
  getMcpServerStatus,
  getMcpServers,
  prepareBrowserMcp,
  previewBrowserCleanup,
  removeMcpEnvVar,
  refreshMcpTools,
  saveMcpServer,
  setMcpEnvVar,
  startMcpServer,
  stopMcpServer,
  type McpServerStatus,
  type McpTool,
  type BrowserCleanupPreview,
} from '$lib/commands/mcp';
import { getWorkspaces, type Workspace } from '$lib/commands/workspace';
import type { McpServer } from '$types';

type McpRuntimeState = {
  status: McpServerStatus;
  tools: McpTool[] | null;
  failed?: boolean;
};

type RuntimeAction = { serverId: string; kind: 'check' | 'stop' };
type ServerDraft = Pick<McpServer, 'name' | 'command' | 'args'> & { id?: string };
const validEnvKey = /^[A-Za-z_][A-Za-z0-9_]*$/;

const stoppedStatus = (serverId: string): McpServerStatus => ({
  server_id: serverId,
  status: 'stopped',
  error: null,
  tools_count: null,
});

function connectionHint(reason: unknown, command: string): string {
  // MCP output and operating-system errors can contain user data. Match only
  // known transport failures and render our own fixed text, never the source.
  const detail = typeof reason === 'string' ? reason : reason instanceof Error ? reason.message : '';
  if (detail.includes('MCP server response timed out')) {
    return /^"?npx(?:\.cmd)?"?$/i.test(command.trim())
      ? '首次运行可能需要下载依赖；连接等待已超时。请检查网络和包名后重试。'
      : '服务未及时响应；请确认它可以启动并使用 MCP stdio 协议。';
  }
  if (detail.includes('Failed to start MCP server')) {
    return '无法启动命令；请检查可执行文件路径。Windows 使用 npx 时请填写 npx.cmd。';
  }
  if (detail.includes('MCP 凭据') || detail.includes('系统 MCP 凭据库')) {
    return '无法读取服务凭据；请在“配置变量”中检查或重置。';
  }
  if (detail.includes('Server exited') || detail.includes('MCP server closed stdout')) {
    return '服务启动后提前退出；请检查包名、依赖和该服务所需的环境变量。';
  }
  if (detail.includes('MCP init error')) {
    return '服务拒绝了 MCP 初始化；请检查服务版本和启动参数。';
  }
  return '连接错误；请检查服务配置和本机依赖。';
}

const PlusIcon = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <line x1="12" y1="5" x2="12" y2="19" />
    <line x1="5" y1="12" x2="19" y2="12" />
  </svg>
);

const TrashIcon = () => (
  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <polyline points="3 6 5 6 21 6" />
    <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
  </svg>
);

function runtimeLabel(server: McpServer, runtime: McpRuntimeState | undefined, action: RuntimeAction['kind'] | null): string {
  if (server.envUnavailable) return '凭据不可用';
  if (!server.enabled) return '已停用';
  if (action === 'stop') return '正在停止';
  if (action === 'check') return runtime?.status.status === 'running' ? '正在检查工具' : '正在连接';
  if (runtime?.status.status === 'starting') return '正在连接';
  if (runtime?.failed) return runtime.status.status === 'running' ? '检查失败' : '连接失败';
  if (runtime?.status.status === 'running') {
    return runtime.tools === null ? '运行中 · 待检查' : `运行中 · ${runtime.tools.length} 个工具`;
  }
  if (runtime?.status.status === 'error') return '连接失败';
  if (server.catalogError) return '目录需重新检查';
  return server.catalogReady ? '按需连接 · 目录已准备' : '未连接';
}

function runtimeTone(server: McpServer, runtime: McpRuntimeState | undefined, action: RuntimeAction['kind'] | null): string {
  if (server.envUnavailable) return 'disconnected';
  if (!server.enabled) return '';
  if (action || runtime?.status.status === 'starting') return 'connecting';
  if (runtime?.failed) return 'disconnected';
  if (runtime?.status.status === 'running') return 'connected';
  if (runtime?.status.status === 'error') return 'disconnected';
  return '';
}

export function McpServersSettings() {
  const [servers, setServers] = useState<McpServer[]>([]);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [runtimeByServer, setRuntimeByServer] = useState<Record<string, McpRuntimeState>>({});
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showAddForm, setShowAddForm] = useState(false);
  const [saving, setSaving] = useState(false);
  const [preparingBrowser, setPreparingBrowser] = useState(false);
  const [cleanup, setCleanup] = useState<{ busy: boolean; preview: BrowserCleanupPreview | null; message: string | null; error: string | null }>({ busy: false, preview: null, message: null, error: null });

  const inspectCleanup = async () => {
    if (cleanup.busy) return;
    setCleanup({ busy: true, preview: null, message: null, error: null });
    try {
      const preview = await previewBrowserCleanup();
      setCleanup({ busy: false, preview, message: preview.fileCount ? null : '没有可清理的 7 天前临时文件，只读文件会保留。', error: null });
    } catch (reason) {
      setCleanup({ busy: false, preview: null, message: null, error: typeof reason === 'string' ? reason : '无法检查临时文件，请检查占用和数据目录后重新查看。' });
    }
  };

  const confirmCleanup = async () => {
    if (cleanup.busy || !cleanup.preview) return;
    const preview = cleanup.preview;
    // Consume the preview before dispatch. A failure never offers replay of an
    // old destructive request; another explicit inspection is required.
    setCleanup({ busy: true, preview: null, message: null, error: null });
    try {
      const result = await clearBrowserTempFiles(preview);
      const message = `已清理 ${result.deletedFiles} 个旧临时文件。${result.partial ? '部分文件未能确认清理，不会自动重试。' : ''}${result.remainingFiles ? `仍有约 ${result.remainingFiles} 个旧文件，可重新查看下一批。` : '正式成果与近期文件未改动。'}`;
      setCleanup({ busy: false, preview: null, error: result.partial ? message : null, message: result.partial ? null : message });
    } catch (reason) {
      setCleanup({ busy: false, preview: null, message: null, error: typeof reason === 'string' ? reason : '清理未能完成；请重新查看，不要重复旧请求。' });
    }
  };
  const [runtimeAction, setRuntimeAction] = useState<RuntimeAction | null>(null);
  const actionGeneration = useRef<Record<string, number>>({});
  const [newServer, setNewServer] = useState<ServerDraft>({ name: '', command: '', args: '' });
  const [editingServer, setEditingServer] = useState<(ServerDraft & { id: string }) | null>(null);
  const [editingEnvServerId, setEditingEnvServerId] = useState<string | null>(null);
  const [envKeyDraft, setEnvKeyDraft] = useState('');
  const [envValueDraft, setEnvValueDraft] = useState('');
  const [confirmRemoveEnvKey, setConfirmRemoveEnvKey] = useState<string | null>(null);
  const [confirmClearEnvServerId, setConfirmClearEnvServerId] = useState<string | null>(null);

  const prepareBrowser = async () => {
    const personal = workspaces.find((workspace) => workspace.kind === 'personal');
    if (!personal?.activeSessionId || preparingBrowser) return;
    setPreparingBrowser(true);
    setError(null);
    try {
      setNewServer(await prepareBrowserMcp(personal.activeSessionId));
    } catch {
      setError('无法准备日常结果区，未更改浏览器配置。请重新打开设置后再试。');
    } finally { setPreparingBrowser(false); }
  };

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const [items, availableWorkspaces] = await Promise.all([getMcpServers(), getWorkspaces()]);
        const statusResults = await Promise.allSettled(items.map((server) => getMcpServerStatus(server.id)));
        if (cancelled) return;
        setServers(items);
        setWorkspaces(availableWorkspaces);
        setRuntimeByServer(Object.fromEntries(statusResults.flatMap((result, index) => (
          result.status === 'fulfilled'
            ? [[items[index].id, { status: result.value, tools: null }]]
            : []
        ))));
      } catch {
        if (!cancelled) setError('无法读取 MCP 服务配置。');
      } finally {
        if (!cancelled) setLoading(false);
      }
    };
    void load();
    return () => { cancelled = true; };
  }, []);

  useEffect(() => {
    if (runtimeAction?.kind !== 'check') return;
    const serverId = runtimeAction.serverId;
    const generation = actionGeneration.current[serverId];
    let cancelled = false;
    const updateStartingStatus = async () => {
      const status = await getMcpServerStatus(serverId).catch(() => null);
      if (
        !cancelled && actionGeneration.current[serverId] === generation && status
        && (status.status === 'starting' || status.status === 'running')
      ) {
        setRuntimeByServer((current) => ({
          ...current,
          [serverId]: { status, tools: current[serverId]?.tools ?? null },
        }));
      }
    };
    void updateStartingStatus();
    const timer = window.setInterval(() => void updateStartingStatus(), 250);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [runtimeAction]);

  const setRuntime = (serverId: string, next: McpRuntimeState) => {
    setRuntimeByServer((current) => ({ ...current, [serverId]: next }));
  };

  const persist = async (server: McpServer): Promise<boolean> => {
    setSaving(true);
    setError(null);
    try {
      await saveMcpServer(server);
      setServers(await getMcpServers());
      return true;
    } catch {
      setError('保存 MCP 服务失败，请检查配置后重试。');
      return false;
    } finally {
      setSaving(false);
    }
  };

  const handleAdd = async () => {
    if (!newServer.name.trim() || !newServer.command.trim()) return;
    const server: McpServer = { ...newServer, id: newServer.id ?? crypto.randomUUID(), env: '', envKeys: [], envUnavailable: false, enabled: true, enabledWorkspaceIds: [] };
    if (await persist(server)) {
      setNewServer({ name: '', command: '', args: '' });
      setShowAddForm(false);
    }
  };

  const handleEdit = async (server: McpServer) => {
    if (!editingServer || editingServer.id !== server.id || !editingServer.name.trim() || !editingServer.command.trim()) return;
    const configurationChanged = editingServer.command !== server.command || editingServer.args !== server.args;
    if (await persist({ ...server, name: editingServer.name, command: editingServer.command, args: editingServer.args })) {
      if (configurationChanged) setRuntime(server.id, { status: stoppedStatus(server.id), tools: [] });
      setEditingServer(null);
    }
  };

  const handleToggle = async (server: McpServer) => {
    const next = { ...server, enabled: !server.enabled };
    if (await persist(next) && !next.enabled) {
      setRuntime(server.id, { status: stoppedStatus(server.id), tools: [] });
    }
  };

  const handleWorkspaceToggle = (server: McpServer, workspaceId: string) => {
    const enabledWorkspaceIds = server.enabledWorkspaceIds.includes(workspaceId)
      ? server.enabledWorkspaceIds.filter((id) => id !== workspaceId)
      : [...server.enabledWorkspaceIds, workspaceId];
    void persist({ ...server, enabledWorkspaceIds });
  };

  const closeEnvEditor = () => {
    setEditingEnvServerId(null);
    setEnvKeyDraft('');
    setEnvValueDraft('');
    setConfirmRemoveEnvKey(null);
    setConfirmClearEnvServerId(null);
  };

  const markEnvironmentChanged = (serverId: string, envKeys: string[]) => {
    actionGeneration.current[serverId] = (actionGeneration.current[serverId] ?? 0) + 1;
    setServers((items) => items.map((item) => item.id === serverId
      ? { ...item, envKeys, envUnavailable: false, enabledWorkspaceIds: [], catalogReady: false, catalogError: null }
      : item));
    setRuntime(serverId, { status: stoppedStatus(serverId), tools: [] });
  };

  const handleSaveEnv = async (server: McpServer) => {
    const key = envKeyDraft.trim();
    if (!validEnvKey.test(key) || !envValueDraft) return;
    setSaving(true);
    setError(null);
    try {
      await setMcpEnvVar(server.id, key, envValueDraft);
      markEnvironmentChanged(server.id, [...new Set([...(server.envKeys ?? []), key])]);
      setEnvKeyDraft('');
      setEnvValueDraft('');
      setConfirmRemoveEnvKey(null);
    } catch {
      setEnvValueDraft('');
      setError('保存环境变量失败；请检查变量名后重试。');
    } finally {
      setSaving(false);
    }
  };

  const handleRemoveEnv = async (server: McpServer, key: string) => {
    setSaving(true);
    setError(null);
    try {
      await removeMcpEnvVar(server.id, key);
      markEnvironmentChanged(server.id, (server.envKeys ?? []).filter((item) => item !== key));
      setConfirmRemoveEnvKey(null);
      if (envKeyDraft === key) {
        setEnvKeyDraft('');
        setEnvValueDraft('');
      }
    } catch {
      setError('移除环境变量失败，请重试。');
    } finally {
      setSaving(false);
    }
  };

  const handleClearEnv = async (server: McpServer) => {
    setSaving(true);
    setError(null);
    try {
      await clearMcpEnv(server.id);
      markEnvironmentChanged(server.id, []);
      setEnvKeyDraft('');
      setEnvValueDraft('');
      setConfirmClearEnvServerId(null);
    } catch {
      setError('重置 MCP 凭据失败，请检查系统凭据库后重试。');
    } finally {
      setSaving(false);
    }
  };

  const handleCheck = async (server: McpServer) => {
    if (!server.enabled || server.envUnavailable) return;
    const generation = (actionGeneration.current[server.id] ?? 0) + 1;
    actionGeneration.current[server.id] = generation;
    setRuntimeAction({ serverId: server.id, kind: 'check' });
    setRuntimeByServer((current) => ({
      ...current,
      [server.id]: { status: current[server.id]?.status ?? stoppedStatus(server.id), tools: null },
    }));
    setError(null);
    try {
      const current = await getMcpServerStatus(server.id);
      if (actionGeneration.current[server.id] !== generation) return;
      const status = current.status === 'running' ? current : await startMcpServer(server.id);
      if (actionGeneration.current[server.id] !== generation) return;
      const tools = await refreshMcpTools(server.id);
      if (actionGeneration.current[server.id] !== generation) return;
      setServers((current) => current.map((item) => item.id === server.id ? { ...item, catalogReady: true, catalogError: null } : item));
      setRuntime(server.id, { status: { ...status, tools_count: tools.length }, tools });
    } catch (reason) {
      if (actionGeneration.current[server.id] !== generation) return;
      const status = await getMcpServerStatus(server.id).catch(() => stoppedStatus(server.id));
      if (actionGeneration.current[server.id] !== generation) return;
      setRuntime(server.id, {
        status,
        tools: null,
        failed: true,
      });
      setError(`无法连接“${server.name}”。${connectionHint(reason, server.command)}`);
    } finally {
      setRuntimeAction((current) => current?.serverId === server.id && current.kind === 'check' ? null : current);
    }
  };

  const handleStop = async (server: McpServer) => {
    actionGeneration.current[server.id] = (actionGeneration.current[server.id] ?? 0) + 1;
    setRuntimeAction({ serverId: server.id, kind: 'stop' });
    setError(null);
    try {
      const status = await stopMcpServer(server.id);
      setRuntime(server.id, { status, tools: [] });
    } catch {
      const status = await getMcpServerStatus(server.id).catch(() => stoppedStatus(server.id));
      setRuntime(server.id, { status, tools: null });
      if (status.status !== 'stopped') setError(`无法停止“${server.name}”，请重试。`);
    } finally {
      setRuntimeAction((current) => current?.serverId === server.id && current.kind === 'stop' ? null : current);
    }
  };

  const handleDelete = async (id: string) => {
    setSaving(true);
    setError(null);
    try {
      await deleteMcpServer(id);
      setServers((items) => items.filter((server) => server.id !== id));
      setRuntimeByServer((current) => {
        const { [id]: _removed, ...remaining } = current;
        return remaining;
      });
    } catch {
      setError('删除 MCP 服务失败，请重试。');
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="settings-page-content">
      <SettingsSection title="MCP 服务" description="服务只有在明确启用到工作区，并由你连接检查后，才会出现在对应主对话的工具面。检查只读取工具清单，不会调用任何工具。" defaultOpen>
        <p className="settings-muted">MCP 服务不会自动继承 AngelBot 的模型密钥等环境变量。添加服务后，可为该服务单独配置环境变量；请勿将密钥写入命令或参数。</p>
        <p className="settings-muted">使用 npx 的首次连接可能下载并运行第三方包，请先确认来源；连接过程中可随时取消。</p>
        <p className="settings-muted">新服务请先连接一次以准备工具目录。之后仅在任务实际调用时启动，不随应用启动；停止连接会结束进程但保留目录，停用服务才会禁止后续调用。修改命令、参数或凭据后需重新连接。</p>
        <p className="settings-muted">把服务启用到工作区会让该工作区的 AngelBot 使用它的工具；在“工作区自动”或“完全访问”执行权限下，普通工具可能直接运行。只启用你信任的服务，敏感操作是否专项询问仍以现有保护规则为准。</p>
        {error && <p className="settings-error" role="alert">{error}</p>}
        {loading ? <p className="settings-muted">正在读取配置…</p> : (
          <div className="server-list">
            {servers.map((server) => {
              const runtime = runtimeByServer[server.id];
              const action = runtimeAction?.serverId === server.id ? runtimeAction.kind : null;
              const busy = runtimeAction !== null;
              const running = runtime?.status.status === 'running';
              const canStopCheck = action === 'check' && (runtime?.status.status === 'starting' || running);
              const tools = runtime?.tools ?? [];
              const editDraft = editingServer?.id === server.id ? editingServer : null;
              const envEditorOpen = editingEnvServerId === server.id;
              const envKeys = server.envKeys ?? [];
              return (
                <div key={server.id} className="server-item">
                <div className="server-info">
                  <div className="server-header">
                    <span className="server-name">{server.name}</span>
                    <span className="status-indicator">
                      <span className={`status-dot ${runtimeTone(server, runtime, action)}`} />
                      <span className="status-text">{runtimeLabel(server, runtime, action)}</span>
                    </span>
                  </div>
                  {server.catalogError && <p className="settings-error">{server.catalogError}</p>}
                  {editDraft ? (
                    <div className="add-server-form">
                      <TextField label="名称" value={editDraft.name} onChange={(name) => setEditingServer({ ...editDraft, name })} />
                      <TextField label="命令" value={editDraft.command} onChange={(command) => setEditingServer({ ...editDraft, command })} />
                      <TextField label="参数" value={editDraft.args} onChange={(args) => setEditingServer({ ...editDraft, args })} />
                      <div className="field-hint">Windows 下使用 npx 时，命令填写 npx.cmd；-y 和包名填写在参数栏。</div>
                      <div className="field-hint">路径包含空格时可用英文双引号包裹。</div>
                      <div className="field-hint">修改命令或参数会停止服务并清除已有工作区授权；保存后需重新授权并连接检查。</div>
                      <div className="form-actions">
                        <button className="btn btn-secondary" type="button" disabled={saving} onClick={() => setEditingServer(null)}>取消</button>
                        <button className="btn btn-primary" type="button" disabled={saving || !editDraft.name.trim() || !editDraft.command.trim()} onClick={() => void handleEdit(server)}>保存修改</button>
                      </div>
                    </div>
                  ) : envEditorOpen ? (
                    <div className="add-server-form">
                      {server.envUnavailable ? (
                        <>
                          <p className="mcp-runtime-error" role="alert">无法读取此服务在系统凭据库中的环境变量。旧值不会显示；重置后请重新填写所需值。</p>
                          {confirmClearEnvServerId === server.id ? (
                            <>
                              <div className="field-hint">重置会清除这个服务的所有已保存环境变量，并停止服务、清除工作区授权。旧值无法从此处找回。</div>
                              <div className="form-actions">
                                <button className="btn btn-secondary" type="button" disabled={saving} onClick={() => setConfirmClearEnvServerId(null)}>取消重置</button>
                                <button className="btn btn-primary" type="button" disabled={saving} onClick={() => void handleClearEnv(server)}>确认重置凭据</button>
                              </div>
                            </>
                          ) : (
                            <div className="form-actions">
                              <button className="btn btn-secondary" type="button" disabled={saving} onClick={closeEnvEditor}>完成</button>
                              <button className="btn btn-secondary" type="button" disabled={saving} onClick={() => setConfirmClearEnvServerId(server.id)}>重置凭据</button>
                            </div>
                          )}
                        </>
                      ) : (
                        <>
                      <div className="field-hint">变量值仅在此输入一次，保存后不会显示。新增、更换或移除变量会停止服务并清除已有工作区授权；之后需重新授权并连接检查。</div>
                      {envKeys.length > 0 ? (
                        <div>
                          <div className="field-hint">已安全保存的环境变量</div>
                          {envKeys.map((key) => (
                            <div key={key} className="mcp-runtime-controls">
                              <code>{key}</code><span className="field-hint">已安全保存</span>
                              <button className="btn btn-secondary" type="button" disabled={saving} onClick={() => { setEnvKeyDraft(key); setEnvValueDraft(''); setConfirmRemoveEnvKey(null); }} aria-label={`替换环境变量 ${key}`}>替换</button>
                              {confirmRemoveEnvKey === key ? (
                                <>
                                  <button className="btn btn-secondary" type="button" disabled={saving} onClick={() => void handleRemoveEnv(server, key)} aria-label={`确认移除环境变量 ${key}`}>确认移除</button>
                                  <button className="btn btn-secondary" type="button" disabled={saving} onClick={() => setConfirmRemoveEnvKey(null)}>取消移除</button>
                                </>
                              ) : (
                                <button className="btn btn-secondary" type="button" disabled={saving} onClick={() => setConfirmRemoveEnvKey(key)} aria-label={`移除环境变量 ${key}`}>移除</button>
                              )}
                            </div>
                          ))}
                        </div>
                      ) : <div className="field-hint">尚未配置环境变量。</div>}
                      <label className="field input-field">
                        <span className="field-label">变量名</span>
                        <input value={envKeyDraft} placeholder="例如 GITHUB_PERSONAL_ACCESS_TOKEN" autoComplete="off" spellCheck={false} disabled={saving} onChange={(event) => setEnvKeyDraft(event.target.value)} />
                      </label>
                      <label className="field input-field">
                        <span className="field-label">新变量值</span>
                        <input type="password" value={envValueDraft} placeholder="输入后保存；现有值不会显示" autoComplete="new-password" spellCheck={false} disabled={saving} onChange={(event) => setEnvValueDraft(event.target.value)} />
                      </label>
                      <div className="field-hint">变量名只能使用英文字母、数字和下划线，且不能以数字开头。保存同名变量会替换原值；留空不会更改。</div>
                      <div className="form-actions">
                        <button className="btn btn-secondary" type="button" disabled={saving} onClick={closeEnvEditor}>完成</button>
                        <button className="btn btn-primary" type="button" disabled={saving || !validEnvKey.test(envKeyDraft.trim()) || !envValueDraft} onClick={() => void handleSaveEnv(server)}>保存变量</button>
                      </div>
                        </>
                      )}
                    </div>
                  ) : (
                    <>
                  <code className="server-command">{server.command} {server.args}</code>
                  <div className="field-hint">环境变量：{server.envUnavailable ? '凭据不可用' : envKeys.length ? `${envKeys.join('、')}（已安全保存）` : '尚未配置'}</div>
                  {server.envUnavailable && <p className="mcp-runtime-error">系统凭据库中的 MCP 凭据无法读取；请打开“修复凭据”重置后重新配置。</p>}
                  <fieldset className="mcp-workspace-access">
                    <legend>允许在这些工作区使用</legend>
                    {workspaces.map((workspace) => (
                      <label key={workspace.id}>
                        <input
                          type="checkbox"
                          checked={server.enabledWorkspaceIds.includes(workspace.id)}
                          disabled={saving || busy || !server.enabled || server.envUnavailable}
                          onChange={() => handleWorkspaceToggle(server, workspace.id)}
                        />
                        {workspace.name}
                      </label>
                    ))}
                  </fieldset>
                  <div className="mcp-runtime-controls">
                    <button className="btn btn-secondary" type="button" disabled={saving || busy || editingServer !== null || editingEnvServerId !== null} onClick={() => { setShowAddForm(false); setEditingEnvServerId(server.id); setEnvKeyDraft(''); setEnvValueDraft(''); setConfirmRemoveEnvKey(null); setConfirmClearEnvServerId(null); }} aria-label={`${server.envUnavailable ? '修复' : '配置'} ${server.name} 的环境变量`}>{server.envUnavailable ? '修复凭据' : '配置变量'}</button>
                    <button
                      className="btn btn-secondary"
                      type="button"
                      disabled={!server.enabled || server.envUnavailable || busy || saving}
                      onClick={() => void handleCheck(server)}
                    >
                      {running ? '重新检查' : '连接并检查'}
                    </button>
                    {(running || canStopCheck) && (
                      <button className="btn btn-secondary" type="button" disabled={(busy && !canStopCheck) || saving} onClick={() => void handleStop(server)}>
                        {running ? '停止' : '取消连接'}
                      </button>
                    )}
                  </div>
                  {running && runtime?.tools !== null && !runtime?.failed && (
                    <div className="mcp-runtime-summary" aria-live="polite">
                      <span>已发现 {tools.length} 个工具；下次发送已授权工作区的消息时可供 AngelBot 使用。</span>
                      {tools.length > 0 && (
                        <span className="mcp-runtime-tools">{tools.slice(0, 8).map((tool) => tool.name).join(' · ')}{tools.length > 8 ? ' · …' : ''}</span>
                      )}
                    </div>
                  )}
                  {runtime?.status.error && <p className="mcp-runtime-error">{connectionHint(runtime.status.error, server.command)}</p>}
                    </>
                  )}
                </div>
                <div className="server-actions">
                  <ToggleField label={`启用 ${server.name}`} checked={server.enabled} disabled={saving || busy || editDraft !== null || envEditorOpen} onChange={() => void handleToggle(server)} />
                  <button className="btn btn-secondary" type="button" disabled={saving || busy || editingServer !== null || editingEnvServerId !== null} onClick={() => { setShowAddForm(false); setEditingServer({ id: server.id, name: server.name, command: server.command, args: server.args }); }} aria-label={`编辑 ${server.name}`}>编辑</button>
                  <button className="delete-server-btn" type="button" disabled={saving || busy || editDraft !== null || envEditorOpen} onClick={() => void handleDelete(server.id)} aria-label={`删除 ${server.name}`}><TrashIcon /></button>
                </div>
                </div>
              );
            })}
            {!servers.length && <p className="settings-muted">尚未添加 MCP 服务。</p>}
          </div>
        )}

        {showAddForm ? (
          <div className="add-server-form">
            <SettingsSection title="网页操作（Playwright）" description="可选：使用 Microsoft 官方 MCP 服务，复用现有模型和执行权限。">
              <p className="settings-muted">需要已安装 Node.js 20 或更新版本及 Microsoft Edge。首次连接会下载并运行固定版本的第三方服务。</p>
              <p className="settings-muted">使用独立、可见的临时浏览器会话，不接管你已有的标签页或登录账号；停止浏览器会话后不保留登录和页面存储，重开时需重新核对任务结果。</p>
              <p className="settings-muted">临时会话不是系统、文件或网络安全沙箱。该服务包含脚本、上传和文件相关工具；只启用你完全信任的服务。网页内容可能发给当前模型，省略图像响应不等于禁止截图落盘。</p>
              <p className="settings-muted">这里只填入配置，不会启动或授权。添加后仍需选择允许的工作区，再连接检查；不自动接入网页自行注册的工具。</p>
              <p className="settings-muted">新配置将快照、日志及下载放入独立的浏览器临时区，不随备份。需要保留的下载由 AngelBot 复制到正式结果区后交付；也可在文件工作台查看并另存。停止服务不会自动删除文件，已有服务配置保持不变。</p>
              <button className="btn btn-secondary" type="button" disabled={saving || preparingBrowser || !workspaces.some((workspace) => workspace.kind === 'personal' && workspace.activeSessionId)} onClick={() => void prepareBrowser()}>{preparingBrowser ? '正在准备…' : '填入浏览器配置'}</button>
            </SettingsSection>
            <TextField label="名称" value={newServer.name} placeholder="服务名称" onChange={(name) => setNewServer({ ...newServer, name })} />
            <TextField label="命令" value={newServer.command} placeholder="npx.cmd" onChange={(command) => setNewServer({ ...newServer, command })} />
            <TextField label="参数" value={newServer.args} placeholder="-y @modelcontextprotocol/server-..." onChange={(args) => setNewServer({ ...newServer, args })} />
            <div className="field-hint">Windows 下使用 npx 时，命令填写 npx.cmd；-y 和包名填写在参数栏。</div>
            <div className="field-hint">路径包含空格时可用英文双引号包裹。</div>
            <div className="field-hint">添加服务后，可点击“配置变量”安全保存此服务需要的专用密钥。</div>
            <div className="form-actions">
              <button className="btn btn-secondary" type="button" onClick={() => setShowAddForm(false)}>取消</button>
              <button className="btn btn-primary" type="button" disabled={saving || !newServer.name.trim() || !newServer.command.trim()} onClick={handleAdd}>添加</button>
            </div>
          </div>
        ) : (
          <button className="add-server-btn" type="button" disabled={loading || saving || editingServer !== null || editingEnvServerId !== null} onClick={() => setShowAddForm(true)}><PlusIcon />添加服务</button>
        )}
      </SettingsSection>
        <SettingsSection title="浏览器临时文件管理" description="仅处理已登记临时区，不按扩展名扫描普通成果或旧配置目录。">
          <p className="settings-muted">先停止使用临时区的浏览器服务，处理当前或待确认的任务。这里只清理创建和修改均已超过 7 天的文件，每批最多 1024 个；只读文件、近期文件、目录和正式成果保留。</p>
          <button className="btn btn-secondary" type="button" disabled={cleanup.busy || loading || saving} onClick={() => void inspectCleanup()}>{cleanup.busy ? '处理中…' : '查看 7 天前文件'}</button>
          {cleanup.preview && cleanup.preview.fileCount > 0 && <div>
            <p>本批 {cleanup.preview.fileCount} 个文件，共 {(cleanup.preview.bytes / 1024 / 1024).toFixed(2)} MiB；共有 {cleanup.preview.eligibleFiles} 个符合条件的旧文件。</p>
            <p className="settings-muted">可能含尚未交付的下载，请先另存需要的文件。删除不可撤销；文件清单变化或预览超过 5 分钟需重新确认，不会自动重试。</p>
            <details><summary>查看本批完整清单</summary><ul style={{ maxHeight: 200, overflow: 'auto' }}>{cleanup.preview.files.map((file) => <li key={file}>{file}</li>)}</ul></details>
            <button className="btn btn-secondary" type="button" disabled={cleanup.busy} onClick={() => void confirmCleanup()}>删除本批旧临时文件</button>
          </div>}
          {cleanup.error && <p role="alert">{cleanup.error}</p>}
          {cleanup.message && <p role="status">{cleanup.message}</p>}
        </SettingsSection>
    </div>
  );
}

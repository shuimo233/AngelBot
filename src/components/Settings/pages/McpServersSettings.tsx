import { useEffect, useRef, useState } from 'react';
import { SettingsRow, SettingsSection } from '../components/SettingsSection';
import { TextField } from '../components/TextField';
import { Button, IconButton, TextField as NativeTextField } from '../../../ui';
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
const npxCommand = /^"?npx(?:\.cmd)?"?$/i;

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
    return npxCommand.test(command.trim())
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
    <div className="settings-page-content mcp-settings">
      <SettingsSection title="已配置服务" action={
        <Button variant="primary" disabled={loading || saving || showAddForm || editingServer !== null || editingEnvServerId !== null} onClick={() => setShowAddForm(true)}>
          <PlusIcon />添加服务
        </Button>
      }>
        {error && <p className="settings-error" role="alert">{error}</p>}
        {showAddForm && (
          <div className="add-server-form" role="group" aria-label="添加服务">
            <div className="settings-rows">
              <SettingsRow title="网页操作（Playwright）" description="需 Node.js 20+ 和 Edge。首次连接会下载并运行固定版本。">
                <Button size="small" disabled={saving || preparingBrowser || !workspaces.some((workspace) => workspace.kind === 'personal' && workspace.activeSessionId)} onClick={() => void prepareBrowser()}>{preparingBrowser ? '正在准备…' : '填入浏览器配置'}</Button>
              </SettingsRow>
              {newServer.id && <div className="settings-info-list settings-rows-note">
                <p>独立会话关闭后清除登录和页面数据。网页内容可能发给模型，截图可能落盘。</p>
                <p>临时会话不隔离系统、文件或网络。快照、日志和下载不随备份，所需文件请另存。</p>
              </div>}
            </div>
            <TextField label="名称" value={newServer.name} placeholder="服务名称" onChange={(name) => setNewServer({ ...newServer, name })} />
            <TextField label="命令" value={newServer.command} placeholder="npx.cmd" onChange={(command) => setNewServer({ ...newServer, command })} />
            <TextField label="参数" value={newServer.args} placeholder="-y @modelcontextprotocol/server-..." onChange={(args) => setNewServer({ ...newServer, args })} />
            <div className="field-hint">Windows 用 npx.cmd，参数填 -y 和包名。含空格的路径用英文双引号。</div>
            <div className="form-actions">
              <button className="btn btn-secondary" type="button" onClick={() => setShowAddForm(false)}>取消</button>
              <button className="btn btn-primary" type="button" disabled={saving || !newServer.name.trim() || !newServer.command.trim()} onClick={handleAdd}>添加</button>
            </div>
          </div>
        )}
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
                <div className="mcp-server-heading">
                  <div className="server-header">
                    <span className="server-name">{server.name}</span>
                  </div>
                  <div className="server-actions">
                    <Button size="small" variant="ghost" disabled={saving || busy || editingServer !== null || editingEnvServerId !== null} onClick={() => { setShowAddForm(false); setEditingServer({ id: server.id, name: server.name, command: server.command, args: server.args }); }} aria-label={`编辑 ${server.name}`}>编辑</Button>
                    <IconButton size="small" disabled={saving || busy || editDraft !== null || envEditorOpen} onClick={() => void handleDelete(server.id)} label={`删除 ${server.name}`}><TrashIcon /></IconButton>
                  </div>
                </div>
                <div className="server-info">
                  {server.catalogError && <p className="settings-error">{server.catalogError}</p>}
                  {editDraft ? (
                    <div className="add-server-form">
                      <TextField label="名称" value={editDraft.name} onChange={(name) => setEditingServer({ ...editDraft, name })} />
                      <TextField label="命令" value={editDraft.command} onChange={(command) => setEditingServer({ ...editDraft, command })} />
                      <TextField label="参数" value={editDraft.args} onChange={(args) => setEditingServer({ ...editDraft, args })} />
                      <div className="field-hint">含空格的路径用英文双引号。改动命令或参数后，服务会停止，需重新授权并检查连接。</div>
                      <div className="form-actions">
                        <button className="btn btn-secondary" type="button" disabled={saving} onClick={() => setEditingServer(null)}>取消</button>
                        <button className="btn btn-primary" type="button" disabled={saving || !editDraft.name.trim() || !editDraft.command.trim()} onClick={() => void handleEdit(server)}>保存修改</button>
                      </div>
                    </div>
                  ) : envEditorOpen ? (
                    <div className="add-server-form">
                      {server.envUnavailable ? (
                        <>
                          <p className="mcp-runtime-error" role="alert">无法读取系统凭据库中的环境变量。</p>
                          {confirmClearEnvServerId === server.id ? (
                            <>
                              <div className="field-hint">重置会删除全部变量（不可恢复）、停止服务并撤销工作区授权。</div>
                              <div className="form-actions">
                                <button className="btn btn-secondary" type="button" disabled={saving} onClick={() => setConfirmClearEnvServerId(null)}>取消重置</button>
                                <Button variant="danger" disabled={saving} onClick={() => void handleClearEnv(server)}>确认重置凭据</Button>
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
                      <div className="field-hint">修改变量会停止服务，需重新授权并检查连接。</div>
                      {envKeys.length > 0 ? (
                        <div>
                          {envKeys.map((key) => (
                            <div key={key} className="mcp-runtime-controls">
                              <code>{key}</code><span className="field-hint">已安全保存</span>
                              <button className="btn btn-secondary" type="button" disabled={saving} onClick={() => { setEnvKeyDraft(key); setEnvValueDraft(''); setConfirmRemoveEnvKey(null); }} aria-label={`替换环境变量 ${key}`}>替换</button>
                              {confirmRemoveEnvKey === key ? (
                                <>
                                  <Button size="small" disabled={saving} onClick={() => setConfirmRemoveEnvKey(null)}>取消移除</Button>
                                  <Button size="small" variant="danger" disabled={saving} onClick={() => void handleRemoveEnv(server, key)} aria-label={`确认移除环境变量 ${key}`}>确认移除</Button>
                                </>
                              ) : (
                                <button className="btn btn-secondary" type="button" disabled={saving} onClick={() => setConfirmRemoveEnvKey(key)} aria-label={`移除环境变量 ${key}`}>移除</button>
                              )}
                            </div>
                          ))}
                        </div>
                      ) : <div className="field-hint">尚未配置环境变量。</div>}
                      <NativeTextField label="变量名" value={envKeyDraft} placeholder="例如 GITHUB_PERSONAL_ACCESS_TOKEN" autoComplete="off" spellCheck={false} disabled={saving} onChange={(event) => setEnvKeyDraft(event.target.value)} />
                      <NativeTextField label="新变量值" type="password" value={envValueDraft} placeholder="现有值不会显示" hint="服务需单独配置密钥，请勿将密钥写入命令或参数。" autoComplete="new-password" spellCheck={false} disabled={saving} onChange={(event) => setEnvValueDraft(event.target.value)} />
                      <div className="field-hint">变量名可用英文字母、数字和下划线，首位不能是数字。同名值会被替换。</div>
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
                  <SettingsRow title="环境变量" description={server.envUnavailable ? '系统凭据库读取失败' : envKeys.length ? `${envKeys.join('、')}（已安全保存）` : '尚未配置'}>
                    <Button size="small" disabled={saving || busy || editingServer !== null || editingEnvServerId !== null} onClick={() => { setShowAddForm(false); setEditingEnvServerId(server.id); setEnvKeyDraft(''); setEnvValueDraft(''); setConfirmRemoveEnvKey(null); setConfirmClearEnvServerId(null); }} aria-label={`${server.envUnavailable ? '修复' : '配置'} ${server.name} 的环境变量`}>{server.envUnavailable ? '修复凭据' : '配置变量'}</Button>
                  </SettingsRow>
                  <div className="mcp-service-access">
                    <ToggleField label={`启用 ${server.name}`} checked={server.enabled} disabled={saving || busy} onChange={() => void handleToggle(server)} />
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
                  </div>
                  <SettingsRow title="服务连接" description={
                    <div>
                      <span className="status-indicator" aria-live="polite">
                        <span className={`status-dot ${runtimeTone(server, runtime, action)}`} />
                        <span className="status-text">{runtimeLabel(server, runtime, action)}</span>
                      </span>
                      {running
                        ? <p className="settings-row-description">停止后保留工具清单。</p>
                        : npxCommand.test(server.command.trim()) && server.enabled && !server.envUnavailable && !server.catalogReady &&
                          <p className="settings-row-description">首次连接可能下载并运行第三方包，请核对来源。</p>}
                    </div>
                  }>
                    {(running || canStopCheck) && (
                      <Button size="small" variant="ghost" disabled={(busy && !canStopCheck) || saving} onClick={() => void handleStop(server)}>
                        {running ? '停止' : '取消连接'}
                      </Button>
                    )}
                    <Button size="small"
                      disabled={!server.enabled || server.envUnavailable || busy || saving}
                      onClick={() => void handleCheck(server)}
                    >
                      {running ? '重新检查' : '连接并检查'}
                    </Button>
                  </SettingsRow>
                  {running && runtime?.tools !== null && !runtime?.failed && (
                    <div className="mcp-runtime-summary" aria-live="polite">
                      <span>已发现 {tools.length} 个工具，可供获准的工作区使用。</span>
                      {tools.length > 0 && (
                        <span className="mcp-runtime-tools">{tools.slice(0, 8).map((tool) => tool.name).join(' · ')}{tools.length > 8 ? ' · …' : ''}</span>
                      )}
                    </div>
                  )}
                  {runtime?.status.error && <p className="mcp-runtime-error">{connectionHint(runtime.status.error, server.command)}</p>}
                    </>
                  )}
                </div>
                </div>
              );
            })}
            {!servers.length && <p className="settings-muted">尚未添加 MCP 服务。</p>}
          </div>
        )}
      </SettingsSection>
      <SettingsSection title="浏览器临时文件管理">
        <div className="settings-rows">
          <SettingsRow title="清理临时文件" description="清理前请停止浏览器服务，处理待确认任务。每批最多 1,024 个。">
            <Button size="small" disabled={cleanup.busy || loading || saving} onClick={() => void inspectCleanup()}>{cleanup.busy ? '处理中…' : '查看 7 天前文件'}</Button>
          </SettingsRow>
        </div>
        {cleanup.preview && cleanup.preview.fileCount > 0 && <div>
          <p>本批 {cleanup.preview.fileCount} 个文件，共 {(cleanup.preview.bytes / 1024 / 1024).toFixed(2)} MiB；共有 {cleanup.preview.eligibleFiles} 个符合条件的旧文件。</p>
          <p className="settings-muted">删除不可撤销，所需下载请先另存。清单变化或超过 5 分钟需重新确认。</p>
          <details><summary>查看本批完整清单</summary><ul style={{ maxHeight: 200, overflow: 'auto' }}>{cleanup.preview.files.map((file) => <li key={file}>{file}</li>)}</ul></details>
          <div className="settings-form-actions">
            <Button variant="danger" disabled={cleanup.busy} onClick={() => void confirmCleanup()}>删除本批旧临时文件</Button>
          </div>
        </div>}
        {cleanup.error && <p role="alert">{cleanup.error}</p>}
        {cleanup.message && <p role="status">{cleanup.message}</p>}
      </SettingsSection>
    </div>
  );
}

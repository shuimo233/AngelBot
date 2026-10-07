/**
 * MCP server commands - typed wrappers around Tauri IPC
 */
import { invoke } from '../invoke';
import type { McpServer } from '$types';

/** A local MCP process status, separate from whether a workspace may use it. */
export type McpServerStatus = {
  server_id: string;
  status: 'stopped' | 'starting' | 'running' | 'error';
  error: string | null;
  tools_count: number | null;
};

/** Discovery data is untrusted metadata; the UI never treats it as instructions. */
export type McpTool = {
  name: string;
  description: string;
  inputSchema: unknown;
};

export async function getMcpServers(): Promise<McpServer[]> {
  return invoke<McpServer[]>('get_mcp_servers');
}

export async function saveMcpServer(server: McpServer): Promise<void> {
  return invoke<void>('save_mcp_server', { server });
}

/** Write-only credential commands. Neither command returns a stored value. */
export async function setMcpEnvVar(serverId: string, key: string, value: string): Promise<void> {
  return invoke<void>('set_mcp_env_var', { serverId, key, value });
}

export async function removeMcpEnvVar(serverId: string, key: string): Promise<void> {
  return invoke<void>('remove_mcp_env_var', { serverId, key });
}

/** Recovery when a service's saved environment cannot be read. */
export async function clearMcpEnv(serverId: string): Promise<void> {
  return invoke<void>('clear_mcp_env', { serverId });
}

export async function deleteMcpServer(id: string): Promise<void> {
  return invoke<void>('delete_mcp_server', { id });
}

/** Explicit user-owned lifecycle controls for a configured MCP service. */
export async function startMcpServer(serverId: string): Promise<McpServerStatus> {
  return invoke<McpServerStatus>('start_mcp_server', { serverId });
}

export async function stopMcpServer(serverId: string): Promise<McpServerStatus> {
  return invoke<McpServerStatus>('stop_mcp_server', { serverId });
}

export async function getMcpServerStatus(serverId: string): Promise<McpServerStatus> {
  return invoke<McpServerStatus>('get_mcp_server_status', { serverId });
}

export async function listMcpTools(serverId: string): Promise<McpTool[]> {
  return invoke<McpTool[]>('list_mcp_tools', { serverId });
}

export async function refreshMcpTools(serverId: string): Promise<McpTool[]> {
  return invoke<McpTool[]>('refresh_mcp_tools', { serverId });
}

export async function testApiConnection(url: string, apiKey?: string, provider?: string): Promise<boolean> {
  return invoke<boolean>('test_api_connection', { url, apiKey, provider });
}

export async function checkOllama(): Promise<boolean> {
  return invoke<boolean>('check_ollama');
}

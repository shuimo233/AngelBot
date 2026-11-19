import type { AgentExecutionPermission } from './commands/settings';

export const EXECUTION_PERMISSION_OPTIONS: readonly {
  value: AgentExecutionPermission;
  label: string;
  desc: string;
}[] = [
  { value: 'ask', label: '请求批准', desc: '需批准时逐次询问。' },
  { value: 'workspace_auto', label: '工作区自动', desc: '自动新建、覆盖项目文件及调用已启用的 MCP。其他文件操作按规则确认。' },
  { value: 'full_access', label: '完全访问', desc: '自动调用已授权工具。受保护操作仍需确认。' },
];

export interface McpToolIdentity {
  server: string;
  tool: string;
}

const TOOL_LABELS: Record<string, string> = {
  agent_loop: '执行任务步骤',
  read_file: '读取文件',
  write_file: '写入文件',
  edit_file: '修改文件',
  watch_file: '监听文件',
  list_workspace_items: '查看项目文件',
  organize_workspace_item: '整理项目文件',
  inspect_desktop_capabilities: '检查电脑能力',
  observe_trusted_app_window: '查看已授权应用窗口',
  open_trusted_app: '打开应用',
  open_windows_setting: '打开系统设置',
  reveal_workspace_item: '在文件管理器中显示',
  prepare_message_draft: '填写消息草稿',
  set_trusted_app_text: '填写应用输入框',
  operate_trusted_app_control: '操作应用控件',
  // Render compatibility for completed history only; never an active tool.
  invoke_trusted_app_control: '操作应用按钮（旧记录）',
  schedule_reminder: '创建提醒',
  list_reminders: '查看提醒',
  cancel_reminder: '取消提醒',
  web_search: '联网搜索',
  delegate_network_exploration: '联网委派',
  remember_fact: '写入记忆',
  recall_memories: '查询记忆',
  update_memory: '更新记忆',
  forget_memory: '遗忘记忆',
  create_goal: '创建目标',
  update_goal_progress: '更新目标',
  complete_goal: '完成目标',
};

function displayPart(value: string) {
  return value.replace(/_/g, ' ');
}

/** Parse the stable, user-readable MCP registry name without trusting it as authority. */
export function parseMcpToolIdentity(toolName: string): McpToolIdentity | null {
  const match = /^mcp_([a-z0-9_]+)__([a-z0-9_]+)_([a-f0-9]{8})$/.exec(toolName);
  if (!match) return null;
  return { server: displayPart(match[1]), tool: displayPart(match[2]) };
}

export function getToolDisplayName(toolName: string) {
  const mcp = parseMcpToolIdentity(toolName);
  if (mcp) return `外部服务：${mcp.server} / ${mcp.tool}`;
  if (toolName.startsWith('mcp_')) return '调用外部服务';
  return TOOL_LABELS[toolName] ?? toolName;
}

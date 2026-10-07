import { describe, expect, it } from 'vitest';
import { getToolDisplayName, parseMcpToolIdentity } from './tool-display';

describe('tool display', () => {
  it('shows a stable MCP registry name as a service and operation', () => {
    const toolName = 'mcp_calendar__search_events_a1b2c3d4';

    expect(parseMcpToolIdentity(toolName)).toEqual({ server: 'calendar', tool: 'search events' });
    expect(getToolDisplayName(toolName)).toBe('外部服务：calendar / search events');
  });

  it('keeps unknown or legacy MCP names safe and understandable', () => {
    expect(parseMcpToolIdentity('mcp_send_email')).toBeNull();
    expect(getToolDisplayName('mcp_send_email')).toBe('调用外部服务');
  });

  it('keeps desktop capability discovery user-readable', () => {
    expect(getToolDisplayName('inspect_desktop_capabilities')).toBe('检查电脑能力');
    expect(getToolDisplayName('observe_trusted_app_window')).toBe('查看已授权应用窗口');
    expect(getToolDisplayName('reveal_workspace_item')).toBe('在文件管理器中显示');
    expect(getToolDisplayName('set_trusted_app_text')).toBe('填写应用输入框');
    expect(getToolDisplayName('operate_trusted_app_control')).toBe('操作应用控件');
    expect(getToolDisplayName('invoke_trusted_app_control')).toBe('操作应用按钮（旧记录）');
    expect(getToolDisplayName('web_search')).toBe('联网搜索');
    expect(getToolDisplayName('delegate_network_exploration')).toBe('联网委派');
  });
});

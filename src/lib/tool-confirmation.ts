import { parseMcpToolIdentity } from './tool-display';
import type { DesktopActionOperation, DesktopControlAction } from './commands/desktop';

type ToolArguments = Record<string, unknown>;

/** Native actions require a backend-attested preview, not a model-authored summary. */
export function desktopActionOperation(toolName: string, rawArguments?: string | unknown): DesktopActionOperation | null {
  if (toolName === 'prepare_message_draft') return 'draft';
  if (toolName === 'set_trusted_app_text') return 'field';
  if (toolName === 'operate_trusted_app_control') {
    const action = parseArguments(rawArguments).action;
    if (action === 'invoke' || action === 'select' || action === 'expand' || action === 'collapse'
      || action === 'scrollup' || action === 'scrolldown') return action;
  }
  return null;
}

/**
 * Reserved approval names must never fall back to a generic allow button.
 * The retired invoke name is blocked only: it is not inspected or executable.
 */
export function requiresDesktopActionPreview(toolName: string): boolean {
  return toolName === 'prepare_message_draft' || toolName === 'set_trusted_app_text'
    || toolName === 'operate_trusted_app_control' || toolName === 'invoke_trusted_app_control';
}

export const DESKTOP_CONTROL_ACTION_LABELS: Record<DesktopControlAction, string> = {
  invoke: '调用控件', select: '选中控件', expand: '展开控件', collapse: '收起控件',
  scrollup: '向上小幅滚动', scrolldown: '向下小幅滚动',
};

function parseArguments(value: string | unknown): ToolArguments {
  if (typeof value === 'string') {
    try {
      const parsed = JSON.parse(value) as unknown;
      return parsed && typeof parsed === 'object' && !Array.isArray(parsed)
        ? parsed as ToolArguments
        : {};
    } catch {
      return {};
    }
  }
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as ToolArguments
    : {};
}

function textArgument(args: ToolArguments, key: string) {
  const value = args[key];
  return typeof value === 'string' && value.trim() ? value.trim() : null;
}

function shortened(value: string, limit = 96) {
  return value.length > limit ? `${value.slice(0, limit)}…` : value;
}

type NetworkExplorationAction = 'search' | 'fetch';

const NETWORK_EXPLORATION_ACTION_LABELS: Record<NetworkExplorationAction, string> = {
  search: '搜索',
  fetch: '抓取',
};

const MAX_NETWORK_EXPLORATION_OPERATIONS = 12;
const MAX_NETWORK_EXPLORATION_RESPONSE_KIB = 512;
const MAX_NETWORK_EXPLORATION_REDIRECTS = 3;

/**
 * The confirmation may name a host, but must never echo a supplied URL,
 * including its path, query parameters, or credentials.
 */
function safeProviderHost(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const candidate = value.trim().toLowerCase();
  if (!candidate || candidate.length > 253 || /[\\/:?#@\s]/.test(candidate)) return null;

  try {
    const hostname = new URL(`https://${candidate}`).hostname.toLowerCase();
    return hostname === candidate ? hostname : null;
  } catch {
    return null;
  }
}

function safeFetchHost(value: unknown): string | null {
  if (typeof value !== 'string') return null;

  try {
    const url = new URL(value.trim());
    const hostname = url.hostname.toLowerCase();
    return url.protocol === 'https:' && hostname && hostname.length <= 253 ? hostname : null;
  } catch {
    return null;
  }
}

function networkExplorationSummary(args: ToolArguments) {
  const hosts = new Set<string>();
  const actions = new Set<NetworkExplorationAction>();
  const operations = Array.isArray(args.explorer_operations)
    ? args.explorer_operations.slice(0, MAX_NETWORK_EXPLORATION_OPERATIONS)
    : [];

  for (const operation of operations) {
    if (!operation || typeof operation !== 'object' || Array.isArray(operation)) continue;
    const record = operation as ToolArguments;

    if (record.kind === 'search') {
      actions.add('search');
      const host = safeProviderHost(record.provider_host);
      if (host) hosts.add(host);
    } else if (record.kind === 'fetch') {
      actions.add('fetch');
      const host = safeFetchHost(record.url);
      if (host) hosts.add(host);
    }
  }

  const hostText = hosts.size > 0 ? `站点 ${[...hosts].join('、')}` : '站点将由系统验证';
  const actionText = actions.size > 0
    ? `操作 ${[...actions].map((action) => NETWORK_EXPLORATION_ACTION_LABELS[action]).join('、')}`
    : '操作将由系统验证';

  return `委派受限联网探索：${hostText}；${actionText}；上限为最多 ${MAX_NETWORK_EXPLORATION_OPERATIONS} 次网络操作、单次响应 ${MAX_NETWORK_EXPLORATION_RESPONSE_KIB} KiB、最多 ${MAX_NETWORK_EXPLORATION_REDIRECTS} 次重定向`;
}

const WINDOWS_SETTING_LABELS: Record<string, string> = {
  display: '显示',
  sound: '声音',
  network: '网络和 Internet',
  bluetooth: '蓝牙和设备',
  apps: '已安装的应用',
  notifications: '通知',
  privacy: '隐私',
  default_apps: '默认应用',
  windows_update: 'Windows 更新',
};

/**
 * Describe a pending side effect without exposing message bodies, file
 * contents, credentials, selectors, or raw external-service arguments.
 */
export function getToolConfirmationSummary(toolName: string, rawArguments: string | unknown) {
  const args = parseArguments(rawArguments);
  const path = textArgument(args, 'path');
  const appId = textArgument(args, 'app_id');

  switch (toolName) {
    case 'write_file':
      return path ? `写入项目文件：${shortened(path)}` : '写入当前项目中的文件';
    case 'edit_file':
      return path ? `修改项目文件：${shortened(path)}` : '修改当前项目中的文件';
    case 'create_directory':
      return path ? `创建项目目录：${shortened(path)}` : '在当前项目中创建目录';
    case 'run_project_command': {
      const command = textArgument(args, 'command');
      return command ? `在当前项目运行：${shortened(command)}` : '在当前项目运行命令';
    }
    case 'organize_workspace_item': {
      const operation = textArgument(args, 'operation') === 'copy' ? '复制' : '移动或重命名';
      const source = textArgument(args, 'source');
      const destination = textArgument(args, 'destination');
      return source && destination
        ? `${operation}：${shortened(source, 48)} → ${shortened(destination, 48)}`
        : `${operation}当前项目中的文件`;
    }
    case 'schedule_reminder': {
      const title = textArgument(args, 'title');
      const when = textArgument(args, 'when');
      return `创建提醒${title ? `：${shortened(title, 56)}` : ''}${when ? `（${shortened(when, 40)}）` : ''}`;
    }
    case 'cancel_reminder':
      return '取消当前工作区中的这条提醒';
    case 'create_automation': {
      const title = textArgument(args, 'title');
      const schedule = textArgument(args, 'schedule');
      return `创建自动化${title ? `：${shortened(title, 56)}` : ''}${schedule ? `（${shortened(schedule, 40)}）` : ''}`;
    }
    case 'open_trusted_app':
      return appId ? `打开已信任应用：${shortened(appId, 64)}` : '打开一个已信任应用';
    case 'observe_trusted_app_window':
      return args.mode === 'image'
        ? '截取已授权应用的单个窗口图像，发送给当前配置的模型用于本轮理解。窗口可能包含私人内容；图像仅临时使用，不存储截图，也不授予控件操作权限。'
        : '查看已授权应用的窗口控件结构，不截取或发送窗口图像。';
    case 'open_windows_setting': {
      const page = textArgument(args, 'page');
      return `打开 Windows 设置${page ? `：${WINDOWS_SETTING_LABELS[page] ?? shortened(page, 48)}` : ''}`;
    }
    case 'reveal_workspace_item':
      return path ? `在文件管理器中显示：${shortened(path)}` : '在文件管理器中显示项目内容';
    case 'prepare_message_draft':
      return '在已信任应用中写入草稿。目标窗口和完整内容以实时预检为准；AngelBot 不点击发送，应用可能自动保存或同步。';
    case 'set_trusted_app_text':
      return '在已信任应用中填写一个输入框。目标窗口、控件和完整内容以实时预检为准；应用可能自动保存或同步。';
    case 'operate_trusted_app_control': {
      const operation = desktopActionOperation(toolName, rawArguments);
      const actionLabel = operation && operation !== 'draft' && operation !== 'field'
        ? DESKTOP_CONTROL_ACTION_LABELS[operation] : '操作控件（动作无效）';
      return `在已信任应用中${actionLabel}。目标窗口与控件以实时预检为准；可能触发发送、删除等后果，控件名称不代表安全或授权。请求已发出或控件状态已校验不代表目标已完成。`;
    }
    case 'invoke_trusted_app_control':
      return '旧版控件操作已停用，不能批准；请拒绝并重新发起带有明确动作的请求。';
    case 'import_skill_from_github': {
      const url = textArgument(args, 'url') ?? textArgument(args, 'repo_url');
      return url ? `从 GitHub 导入技能：${shortened(url)}` : '从 GitHub 导入技能';
    }
    case 'web_search':
      // The confirmation must make the external boundary explicit, while the
      // raw query remains out of the compact timeline because it can contain
      // private user context or provider-facing syntax.
      return '向已配置的搜索服务发送一次公开网页查询';
    case 'delegate_network_exploration':
      return networkExplorationSummary(args);
    case 'materialize_delegated_change':
      return '将已审校的候选修改写入当前项目';
    default:
      if (parseMcpToolIdentity(toolName)) return '调用当前工作区已启用的外部服务一次';
      return '允许 AngelBot 执行此操作一次';
  }
}

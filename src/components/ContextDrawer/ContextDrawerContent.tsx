import { useEffect, useMemo, useState } from 'react';
import { getContextUsage, type ContextUsage } from '$lib/commands/session';
import { PROVIDER_REGISTRY } from '$lib/providers';
import { useMessagesStore } from '$stores/messages';
import { useSessionsStore } from '$stores/sessions';
import { useSettingsStore } from '$stores/settings';
import type { Message, ToolResult } from '$types';
import './ContextDrawerContent.css';

interface ContextDrawerContentProps {
  onClose: () => void;
}

/** 用户可读的工具名称（不暴露内部实现名） */
const TOOL_LABELS: Record<string, string> = {
  read_file: '读取文件',
  write_file: '写入文件',
  create_directory: '创建目录',
  list_workspace_items: '查看项目文件',
  organize_workspace_item: '整理项目文件',
  run_project_command: '运行项目命令',
  think: '思考',
  verify_result: '验证结果',
  watch_file: '监听文件变化',
  schedule_reminder: '设置提醒',
  list_reminders: '查看提醒',
  cancel_reminder: '取消提醒',
  extract_entities: '提取关键信息',
  remember_fact: '记住信息',
  recall_memories: '查找记忆',
  update_memory: '更新记忆',
  forget_memory: '遗忘记忆',
  create_goal: '创建目标',
  update_goal_progress: '更新目标进度',
  complete_goal: '完成目标',
  traverse_graph: '查找关联信息',
  extract_constraints: '记录偏好约束',
  check_constraint: '检查偏好约束',
  inspect_desktop_capabilities: '检查电脑能力',
  open_trusted_app: '打开应用',
  open_windows_setting: '打开系统设置',
  reveal_workspace_item: '在文件管理器中显示',
  prepare_message_draft: '填写消息草稿',
  set_trusted_app_text: '填写应用输入框',
};

function getToolLabel(name: string) {
  return TOOL_LABELS[name] ?? (name.startsWith('mcp_') ? 'MCP 工具' : name);
}

const TASK_STATUS_LABELS: Record<string, string> = {
  awaiting_confirmation: '等待确认',
  continue_suggested: '可继续',
  needs_attention: '需要处理',
  running: '执行中',
  paused: '已暂停',
  stopped: '已停止',
  failed: '失败',
  completed: '已完成',
};

const TERMINAL_REASON_LABELS: Record<string, string> = {
  awaiting_confirmation: '等待确认',
  needs_attention: '需要关注',
  no_progress: '无进展',
  side_effect_pause: '副作用暂停',
  wall_clock_budget: '超过时间预算',
  tool_failure_limit: '工具失败过多',
  user_stopped: '用户停止',
  provider_unavailable: 'Provider 不可用',
};

function terminalReasonLabel(reason: string | undefined): string | null {
  if (!reason || reason === 'completed') return null;
  return TERMINAL_REASON_LABELS[reason] ?? reason;
}

function getTaskStatusLabel(status: string) {
  return TASK_STATUS_LABELS[status] ?? status.replace(/_/g, ' ');
}

function formatTokens(value: number) {
  return value >= 1000 ? `${(value / 1000).toFixed(value >= 10_000 ? 0 : 1)}k` : String(value);
}

function parseArguments(raw: string): Record<string, unknown> {
  try {
    const parsed: unknown = JSON.parse(raw);
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      return parsed as Record<string, unknown>;
    }
  } catch {
    // ignore malformed arguments
  }
  return {};
}

/** 记忆召回结果按行解析，与 ChatArea 的展示口径一致 */
function parseRecalledMemories(output: string) {
  const trimmed = output.trim();
  if (!trimmed || /no .*memor|没有|沒有/i.test(trimmed)) return [];
  return trimmed.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
}

/** 从 "[category] content (重要性:0.8)" 中拆出类别与摘要 */
function parseMemoryLine(line: string) {
  const match = line.match(/^\[(.+?)\]\s*(.*?)\s*(?:\(重要性[:：][\d.]+\))?$/);
  if (!match) return { category: null, content: line };
  return { category: match[1], content: match[2] || line };
}

type ToolStatus = 'awaiting' | 'done' | 'failed' | 'running';

const TOOL_STATUS_LABELS: Record<ToolStatus, string> = {
  awaiting: '等待确认',
  done: '已完成',
  failed: '失败',
  running: '进行中',
};

function resolveToolStatus(result: ToolResult | undefined, liveStatus: string | undefined): ToolStatus {
  if (result?.confirmationRequired && result.confirmationStatus === 'pending') return 'awaiting';
  if (result) return result.success ? 'done' : 'failed';
  if (liveStatus === 'completed') return 'done';
  if (liveStatus === 'failed') return 'failed';
  return 'running';
}

export function ContextDrawerContent({ onClose }: ContextDrawerContentProps) {
  const messages = useMessagesStore((state) => state.messages);
  const toolExecuting = useMessagesStore((state) => state.toolExecuting);
  const executingTool = useMessagesStore((state) => state.executingTool);
  const liveStatus = useMessagesStore((state) => state.liveStatus);
  const liveToolSteps = useMessagesStore((state) => state.liveToolSteps);
  const activeSession = useSessionsStore((state) => state.activeSession);
  const apiConfig = useSettingsStore((state) => state.activeApiConfig);

  const [usage, setUsage] = useState<ContextUsage | null>(null);
  const sessionId = activeSession?.id;

  useEffect(() => {
    if (!sessionId) {
      setUsage(null);
      return;
    }
    let cancelled = false;
    getContextUsage(sessionId)
      .then((result) => { if (!cancelled) setUsage(result); })
      .catch(() => { if (!cancelled) setUsage(null); });
    return () => { cancelled = true; };
  }, [sessionId, messages.length]);

  /** “本轮”：最近一条 assistant 消息及其同轮工具调用 */
  const round = useMemo(() => {
    const lastAssistant = [...messages].reverse().find((message) => message.role === 'assistant');
    return {
      message: lastAssistant as Message | undefined,
      toolCalls: lastAssistant?.toolCalls ?? [],
      toolResults: lastAssistant?.toolResults ?? [],
      taskRun: lastAssistant?.taskRun,
      taskFacts: lastAssistant?.taskFacts,
    };
  }, [messages]);

  /** 本轮工具（消息内调用 + 仍在进行中的实时步骤），按状态汇总 */
  const toolEntries = useMemo(() => {
    const entries = round.toolCalls.map((call) => {
      const result = round.toolResults.find((item) => item.callId === call.id);
      const live = liveToolSteps[call.id];
      return {
        id: call.id,
        name: call.name,
        status: resolveToolStatus(result, live?.status),
      };
    });
    const known = new Set(entries.map((entry) => entry.id));
    for (const step of Object.values(liveToolSteps)) {
      if (known.has(step.callId)) continue;
      entries.push({
        id: step.callId,
        name: step.toolName,
        status: step.status === 'completed' ? 'done' : step.status === 'failed' ? 'failed' : 'running',
      });
    }
    return entries;
  }, [round.toolCalls, round.toolResults, liveToolSteps]);

  /** 本轮召回的记忆：recall_memories 的结果行 + 触发它的查询 */
  const recalledMemories = useMemo(() => {
    const items: { id: string; category: string | null; content: string; query: string | null }[] = [];
    for (const call of round.toolCalls) {
      if (call.name !== 'recall_memories') continue;
      const result = round.toolResults.find((item) => item.callId === call.id);
      if (!result?.success) continue;
      const args = parseArguments(call.arguments);
      const query = typeof args.query === 'string' ? args.query : null;
      for (const line of parseRecalledMemories(result.output)) {
        const { category, content } = parseMemoryLine(line);
        items.push({ id: `${call.id}-${items.length}`, category, content, query });
      }
    }
    return items;
  }, [round.toolCalls, round.toolResults]);

  /** 本轮读取的资料：read_file 调用的路径与读取状态 */
  const readMaterials = useMemo(() => {
    const items: { id: string; path: string; status: ToolStatus }[] = [];
    for (const call of round.toolCalls) {
      if (call.name !== 'read_file') continue;
      const result = round.toolResults.find((item) => item.callId === call.id);
      const args = parseArguments(call.arguments);
      const path = typeof args.path === 'string' ? args.path : '未知路径';
      items.push({ id: call.id, path, status: resolveToolStatus(result, liveToolSteps[call.id]?.status) });
    }
    return items;
  }, [round.toolCalls, round.toolResults, liveToolSteps]);

  const pendingConfirmations = toolEntries.filter((entry) => entry.status === 'awaiting').length;

  const providerName = apiConfig ? (PROVIDER_REGISTRY[apiConfig.provider]?.name ?? apiConfig.provider) : '未配置';
  const modelName = apiConfig?.model?.trim() || '未配置';
  const hasUsage = Boolean(
    usage && Number.isFinite(usage.estimatedTokens) && Number.isFinite(usage.contextLimit),
  );

  return (
    <div className="drawer-content">
      <div className="drawer-content-header">
        <span className="drawer-content-title">本轮上下文</span>
        <button type="button" className="drawer-content-close" onClick={onClose} aria-label="关闭上下文抽屉">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true">
            <line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" />
          </svg>
        </button>
      </div>

      <div className="drawer-content-body">
        <section className="drawer-section" aria-label="本轮">
          <h3 className="drawer-section-title">本轮</h3>
          <dl className="drawer-facts">
            <div className="drawer-fact">
              <dt>模型</dt>
              <dd>{providerName} · {modelName}</dd>
            </div>
            <div className="drawer-fact">
              <dt>工作目录</dt>
              <dd>{activeSession?.workDir || '未设置工作目录'}</dd>
            </div>
            <div className="drawer-fact">
              <dt>上下文用量</dt>
              <dd>
                {hasUsage && usage
                  ? `${formatTokens(usage.estimatedTokens)} / ${formatTokens(usage.contextLimit)}${usage.isCompressed ? ' · 已压缩' : ''}`
                  : sessionId ? '正在读取…' : '暂无会话'}
              </dd>
            </div>
            <div className="drawer-fact">
              <dt>当前运行</dt>
              <dd>
                {toolExecuting
                  ? liveStatus || `正在执行：${executingTool ? getToolLabel(executingTool) : '工具'}`
                  : '没有正在运行的任务'}
              </dd>
            </div>
            <div className="drawer-fact">
              <dt>待确认</dt>
              <dd>{pendingConfirmations > 0 ? `${pendingConfirmations} 项待确认` : '无待确认事项'}</dd>
            </div>
          </dl>
        </section>

        <section className="drawer-section" aria-label="使用的记忆">
          <h3 className="drawer-section-title">使用的记忆</h3>
          {recalledMemories.length === 0 ? (
            <p className="drawer-empty">本轮还没有召回记忆。</p>
          ) : (
            <ul className="drawer-list">
              {recalledMemories.map((memory) => (
                <li className="drawer-item" key={memory.id}>
                  <div className="drawer-item-main">
                    {memory.category && <span className="drawer-tag">{memory.category}</span>}
                    <span className="drawer-item-text">{memory.content}</span>
                  </div>
                  {memory.query && <div className="drawer-item-meta">因查询「{memory.query}」被召回</div>}
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="drawer-section" aria-label="使用的资料">
          <h3 className="drawer-section-title">使用的资料</h3>
          {readMaterials.length === 0 ? (
            <p className="drawer-empty">本轮还没有读取资料。</p>
          ) : (
            <ul className="drawer-list">
              {readMaterials.map((material) => (
                <li className="drawer-item" key={material.id}>
                  <div className="drawer-item-main">
                    <span className="drawer-item-text drawer-item-path">{material.path}</span>
                    <span className={`drawer-status drawer-status-${material.status}`}>
                      {TOOL_STATUS_LABELS[material.status]}
                    </span>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="drawer-section" aria-label="工具与技能">
          <h3 className="drawer-section-title">工具与技能</h3>
          {toolEntries.length === 0 ? (
            <p className="drawer-empty">本轮还没有使用工具。</p>
          ) : (
            <ul className="drawer-list">
              {toolEntries.map((entry) => (
                <li className="drawer-item" key={entry.id}>
                  <div className="drawer-item-main">
                    <span className="drawer-item-text">{getToolLabel(entry.name)}</span>
                    <span className={`drawer-status drawer-status-${entry.status}`}>
                      {TOOL_STATUS_LABELS[entry.status]}
                    </span>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="drawer-section" aria-label="运行记录">
          <h3 className="drawer-section-title">运行记录</h3>
          {round.taskRun ? (
            <div className="drawer-run">
              <div className="drawer-item-main">
                <span className="drawer-item-text">{round.taskRun.goal}</span>
                <span className={`drawer-status drawer-status-${round.taskRun.status === 'completed' ? 'done' : round.taskRun.status === 'failed' || round.taskRun.status === 'needs_attention' ? 'failed' : round.taskRun.status === 'awaiting_confirmation' ? 'awaiting' : 'running'}`}>
                  {getTaskStatusLabel(round.taskRun.status)}
                </span>
              </div>
              <div className="drawer-item-meta">
                已完成 {round.taskRun.completedStepCount}/{round.taskRun.stepCount} 步
                {terminalReasonLabel(round.taskFacts?.terminalReason) && (
                  <> · <span className="drawer-reason">{terminalReasonLabel(round.taskFacts?.terminalReason)}</span></>
                )}
              </div>
              {round.taskFacts && round.taskFacts.modifiedFiles && round.taskFacts.modifiedFiles.length > 0 && (
                <ul className="drawer-modified-files" aria-label="本轮修改">
                  {round.taskFacts.modifiedFiles.map((path) => (
                    <li key={path} className="drawer-modified-file-item">{path}</li>
                  ))}
                </ul>
              )}
              {round.taskFacts?.pendingConfirmation && (
                <p className="drawer-pending-confirmation">等待 {round.taskFacts.pendingConfirmation.toolName} 确认</p>
              )}
            </div>
          ) : toolExecuting && liveStatus ? (
            <p className="drawer-empty">{liveStatus}</p>
          ) : (
            <p className="drawer-empty">本轮还没有运行记录。</p>
          )}
        </section>
      </div>
    </div>
  );
}

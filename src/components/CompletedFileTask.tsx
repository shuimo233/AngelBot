import { authoritativeToolResultsByCallId } from '$lib/commands/agent-event';
import { isResultUnknown } from '$lib/tool-action-result';
import type { Message, ToolCall } from '$types';
import { WorkspaceFileLink, workspaceFilePath } from './MessageMarkdown';
import './CompletedFileTask.css';

const fileActions: Record<string, string> = { read_file: '读取', write_file: '写入', edit_file: '编辑' };
const visibleStepLimit = 20;

function filePath(call: ToolCall): string | null {
  try {
    return workspaceFilePath(JSON.parse(call.arguments)?.path);
  } catch {
    return null;
  }
}

function fileReceipts(final: Message, byId: ReadonlyMap<string, Message>) {
  // Approval continuations are linked to their preceding assistant reply, not
  // to nearby turns. Tool IDs collapse repeated projections of the same call.
  const chain: Message[] = [];
  const visited = new Set<string>();
  let current: Message | undefined = final;
  while (current?.role === 'assistant' && !visited.has(current.id)) {
    chain.push(current);
    visited.add(current.id);
    current = current.parentId ? byId.get(current.parentId) : undefined;
  }
  chain.reverse();
  const results = authoritativeToolResultsByCallId(chain.flatMap(message => message.toolResults ?? []));
  const calls = new Map<string, ToolCall>();
  for (const message of chain) {
    for (const call of message.toolCalls ?? []) if (!calls.has(call.id)) calls.set(call.id, call);
  }
  return { results, calls };
}

export type DeliveredFile = { path: string; readBack: boolean };

export function completedReplyDeliveries(messages: readonly Message[], sessionId: string) {
  const byId = new Map(messages.filter(message => message.sessionId === sessionId).map(message => [message.id, message]));
  const deliveries = new Map<string, DeliveredFile[]>();
  for (const final of byId.values()) {
    if (final.role !== 'assistant' || final.taskRun?.status !== 'completed') continue;
    const { calls, results } = fileReceipts(final, byId);
    if ([...results.values()].some(isResultUnknown)) continue;
    const written = new Map<string, DeliveredFile>();
    for (const call of calls.values()) {
      if (!Object.prototype.hasOwnProperty.call(fileActions, call.name)) continue;
      const path = filePath(call);
      const result = results.get(call.id);
      if (!path || result?.toolName !== call.name || result.confirmationStatus === 'rejected' || result.confirmationStatus === 'cancelled') continue;
      const succeeded = result.success && result.confirmationStatus !== 'pending' && result.verification?.matched !== false;
      if (call.name === 'read_file') {
        if (succeeded && written.has(path)) written.set(path, { path, readBack: true });
      } else if (succeeded) {
        written.set(path, { path, readBack: false });
      } else {
        written.delete(path);
      }
    }
    if (written.size) deliveries.set(final.id, [...written.values()]);
  }
  return deliveries;
}

export function ReplyFileDelivery({ files }: { files: DeliveredFile[] }) {
  return <ul className="reply-file-delivery" aria-label="本次交付文件">
    {files.map(({ path, readBack }) => <li key={path}>
      <span>{readBack ? '已写入并读回' : '已写入'}</span>
      <WorkspaceFileLink path={path}>{path}</WorkspaceFileLink>
    </li>)}
  </ul>;
}

export function latestCompletedFileTask(messages: readonly Message[], sessionId: string) {
  const scoped = messages.filter(message => message.sessionId === sessionId);
  const byId = new Map(scoped.map(message => [message.id, message]));
  for (let index = scoped.length - 1; index >= 0; index--) {
    const final = scoped[index];
    if (final.role !== 'assistant' || final.taskRun?.status !== 'completed') continue;

    const { results, calls } = fileReceipts(final, byId);

    const written = new Set<string>();
    const steps = [...calls.values()].flatMap(call => {
      if (!Object.prototype.hasOwnProperty.call(fileActions, call.name)) return [];
      const result = results.get(call.id);
      if (!result || result.toolName !== call.name || result.confirmationStatus === 'pending'
        || result.confirmationStatus === 'cancelled') return [];
      const path = filePath(call);
      if (!path) return [];
      const verified = result.verification?.matched !== false;
      const succeeded = result.success && verified && result.confirmationStatus !== 'rejected';
      const action = call.name === 'read_file' && succeeded && written.has(path) ? '读回' : fileActions[call.name];
      if (call.name !== 'read_file' && succeeded) written.add(path);
      const status = result.confirmationStatus === 'rejected' ? '已拒绝'
        : !verified ? '复核不一致'
        : !succeeded ? '失败'
        : result.confirmationStatus === 'approved' ? '批准后完成' : '完成';
      return [{ id: call.id, action, path, status }];
    });
    if (steps.length) {
      return { id: final.id, goal: final.taskRun.goal.trim().slice(0, 160), total: steps.length, steps: steps.slice(-visibleStepLimit) };
    }
  }
  return null;
}

export function CompletedFileTask({ task }: { task: NonNullable<ReturnType<typeof latestCompletedFileTask>> }) {
  return (
    <details className="workspace-completed-file-task">
      <summary>最近完成的文件任务 <span>{task.total} 个文件步骤</span></summary>
      {task.goal && <p className="workspace-completed-file-task__goal">{task.goal}</p>}
      {task.total > task.steps.length && <p>显示最近 {task.steps.length} 个步骤</p>}
      <ol aria-label="已记录的文件步骤">
        {task.steps.map(step => <li key={step.id}><span>{step.action} <code>{step.path}</code></span><span>{step.status}</span></li>)}
      </ol>
    </details>
  );
}

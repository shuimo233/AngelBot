import { authoritativeToolResultsByCallId } from '$lib/commands/agent-event';
import { isResultUnknown } from '$lib/tool-action-result';
import type { Message, ToolCall } from '$types';

export function unknownResultCalls(message: Message): ToolCall[] {
  const results = authoritativeToolResultsByCallId(message.toolResults ?? []);
  return (message.toolCalls ?? []).filter((call) => {
    const result = results.get(call.id);
    return result?.toolName === call.name && !['pending', 'rejected', 'cancelled'].includes(result.confirmationStatus ?? '')
      && isResultUnknown(result);
  });
}

export function latestResultToReview(messages: readonly Message[], sessionId: string | undefined) {
  if (!sessionId) return undefined;
  const completed = new Set<string>();
  for (let index = messages.length - 1; index >= 0; index--) {
    const message = messages[index];
    const task = message.taskRun;
    if (message.sessionId !== sessionId || message.role !== 'assistant' || !task?.id) continue;
    if (task.status === 'completed') {
      completed.add(task.id);
      continue;
    }
    if (!completed.has(task.id) && unknownResultCalls(message).length > 0) return message;
  }
}

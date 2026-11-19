import { useEffect, useMemo, useState } from 'react';
import { useKnowledgeStore } from '$stores/knowledge';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspacesStore } from '$stores/workspaces';
import { AssistantAvatar, useActiveAssistantName } from './AssistantAvatar';

export { pickTodayAutomations } from './WorkspaceReminders';

const DISMISS_PREFIX = 'angelbot.emptyState.dismissed.';

export type SuggestionAction = 'open-knowledge';

export interface EmptyStateSuggestion {
  key: string;
  text: string;
  action: SuggestionAction;
}

export interface StarterPrompt {
  key: string;
  title: string;
  prompt: string;
}

const PERSONAL_STARTERS: StarterPrompt[] = [
  {
    key: 'plan-today',
    title: '整理今天的安排',
    prompt: '帮我把今天要处理的事情整理成按优先级排序的行动清单。先问我缺少的关键信息。',
  },
  {
    key: 'create-reminder',
    title: '创建一个提醒',
    prompt: '请帮我创建一个提醒：',
  },
  {
    key: 'use-knowledge',
    title: '根据资料整理内容',
    prompt: '根据我的资料库，整理一份关于……的摘要，并标注不确定信息。',
  },
];

const PROJECT_STARTERS: StarterPrompt[] = [
  {
    key: 'understand-project',
    title: '了解当前项目',
    prompt: '先阅读当前项目的关键文件，概括它的目标、当前状态、风险和下一步。不要修改文件。',
  },
  {
    key: 'diagnose-problem',
    title: '处理一个明确问题',
    prompt: '请先定位并说明这个问题的原因，再给出最小修改方案：',
  },
  {
    key: 'review-changes',
    title: '检查近期变更',
    prompt: '检查当前项目的未提交修改，指出潜在问题并给出验证建议，不要直接改动。',
  },
];

export function starterPromptsForWorkspace(isProject: boolean): StarterPrompt[] {
  return isProject ? PROJECT_STARTERS : PERSONAL_STARTERS;
}

export function greetingForHour(hour: number): string {
  if (hour < 12) return '早上好';
  if (hour < 18) return '下午好';
  return '晚上好';
}

/** 建议全部来自真实本地状态；null 表示尚未加载完成。 */
export function buildSuggestions(input: { knowledgeCount: number | null }): EmptyStateSuggestion[] {
  const suggestions: EmptyStateSuggestion[] = [];
  if (input.knowledgeCount === 0) {
    suggestions.push({ key: 'no-knowledge', text: '添加你的第一条资料，让 AngelBot 能引用它', action: 'open-knowledge' });
  }
  return suggestions;
}

export function isSuggestionDismissed(key: string): boolean {
  try {
    return localStorage.getItem(DISMISS_PREFIX + key) === '1';
  } catch {
    return false;
  }
}

export function dismissSuggestion(key: string): void {
  try {
    localStorage.setItem(DISMISS_PREFIX + key, '1');
  } catch {
    // localStorage 不可用时忽略，建议下次仍会展示
  }
}

export function ChatStarterPrompts({ isProject, hidden, onDraftSelect }: {
  isProject: boolean;
  hidden: boolean;
  onDraftSelect: (prompt: string) => void;
}) {
  return (
    <section className="messages-empty-starters" aria-label="对话示例" aria-hidden={hidden || undefined}>
      {starterPromptsForWorkspace(isProject).map((starter) => (
        <button key={starter.key} type="button" className="messages-empty-starter"
          disabled={hidden} onClick={() => onDraftSelect(starter.prompt)}>
          {starter.title}
        </button>
      ))}
    </section>
  );
}

export function ChatEmptyState() {
  const assistantName = useActiveAssistantName();
  const knowledgeEntries = useKnowledgeStore((state) => state.entries);
  const knowledgeLoading = useKnowledgeStore((state) => state.isLoading);
  const setPage = useNavigationStore((state) => state.setPage);
  const workspaces = useWorkspacesStore((state) => state.workspaces);
  const activeWorkspaceId = useWorkspacesStore((state) => state.activeWorkspaceId);

  const [dismissedKeys, setDismissedKeys] = useState<string[]>([]);

  useEffect(() => {
    if (knowledgeEntries.length === 0 && !knowledgeLoading) {
      void useKnowledgeStore.getState().loadKnowledge();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const activeWorkspace = workspaces.find((workspace) => workspace.id === activeWorkspaceId);
  const isProject = activeWorkspace?.kind === 'project';

  const suggestions = useMemo(
    () => buildSuggestions({
      knowledgeCount: knowledgeLoading ? null : knowledgeEntries.length,
    }).filter((item) => !dismissedKeys.includes(item.key) && !isSuggestionDismissed(item.key)),
    [knowledgeEntries.length, knowledgeLoading, dismissedKeys],
  );

  const handleSuggestion = (suggestion: EmptyStateSuggestion) => {
    if (suggestion.action === 'open-knowledge') setPage('knowledge');
  };

  const handleDismiss = (key: string) => {
    dismissSuggestion(key);
    setDismissedKeys((prev) => [...prev, key]);
  };

  return (
    <div className="messages-empty">
      {activeWorkspace?.kind === 'personal' ? <div className="messages-empty-identity">
        <AssistantAvatar />
        <div><p className="messages-empty-name">{assistantName}</p><h2 className="messages-empty-title">{greetingForHour(new Date().getHours())}</h2></div>
      </div> : <h2 className="messages-empty-title">{greetingForHour(new Date().getHours())}</h2>}
      {isProject && <p className="messages-empty-hint">在工作台查看文件和任务。</p>}

      {suggestions.length > 0 && (
        <ul className="messages-empty-suggestions">
          {suggestions.map((suggestion) => (
            <li key={suggestion.key} className="messages-empty-suggestion">
              <button
                type="button"
                className="messages-empty-suggestion-action"
                onClick={() => handleSuggestion(suggestion)}
              >
                {suggestion.text}
              </button>
              <button
                type="button"
                className="messages-empty-suggestion-dismiss"
                aria-label={`忽略建议：${suggestion.text}`}
                onClick={() => handleDismiss(suggestion.key)}
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

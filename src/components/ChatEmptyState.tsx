import { useEffect, useMemo, useState } from 'react';
import { useKnowledgeStore } from '$stores/knowledge';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspacesStore } from '$stores/workspaces';
import { WorkspaceReminders } from './WorkspaceReminders';

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
  description: string;
  prompt: string;
}

const PERSONAL_STARTERS: StarterPrompt[] = [
  {
    key: 'plan-today',
    title: '整理今天的安排',
    description: '把零散事项变成可执行的优先级清单',
    prompt: '帮我把今天要处理的事情整理成按优先级排序的行动清单。先问我缺少的关键信息。',
  },
  {
    key: 'create-reminder',
    title: '创建一个提醒',
    description: '说明时间和事项，由 AngelBot 记住',
    prompt: '请帮我创建一个提醒：',
  },
  {
    key: 'use-knowledge',
    title: '根据资料整理内容',
    description: '引用已保存的资料，并标注不确定信息',
    prompt: '根据我的资料库，整理一份关于……的摘要，并标注不确定信息。',
  },
];

const PROJECT_STARTERS: StarterPrompt[] = [
  {
    key: 'understand-project',
    title: '了解当前项目',
    description: '概括目标、现状、风险和下一步',
    prompt: '先阅读当前项目的关键文件，概括它的目标、当前状态、风险和下一步。不要修改文件。',
  },
  {
    key: 'diagnose-problem',
    title: '处理一个明确问题',
    description: '先定位原因，再给出最小修改方案',
    prompt: '请先定位并说明这个问题的原因，再给出最小修改方案：',
  },
  {
    key: 'review-changes',
    title: '检查近期变更',
    description: '寻找风险并给出验证建议，不直接改动',
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

export function ChatEmptyState({
  onDraftSelect,
  showStarters = true,
}: {
  onDraftSelect?: (prompt: string) => void;
  showStarters?: boolean;
}) {
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
  const starters = starterPromptsForWorkspace(isProject);

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
      <div className="messages-empty-icon" aria-hidden="true">
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
          <path d="M6 8.5h12M6 12h8M6 15.5h5" />
          <path d="M4 4.5h16v15H8l-4 2v-17z" />
        </svg>
      </div>
      <div className="messages-empty-context">
        {isProject ? `项目 · ${activeWorkspace?.name ?? ''}` : '个人空间'}
      </div>
      <div className="messages-empty-title">{greetingForHour(new Date().getHours())}</div>
      <div className="messages-empty-hint">
        {isProject
          ? '直接描述目标。AngelBot 会在这个项目的文件、历史与受控委派范围内继续工作。'
          : '直接说出想处理的事。需要具体项目文件时，AngelBot 会切换到对应项目再继续。'}
      </div>

      {onDraftSelect && showStarters && (
        <section className="messages-empty-starters" aria-labelledby="messages-empty-starters-title">
          <div id="messages-empty-starters-title" className="messages-empty-section-title">可以从这里开始</div>
          <div className="messages-empty-starter-list">
            {starters.map((starter) => (
              <button
                key={starter.key}
                type="button"
                className="messages-empty-starter"
                onClick={() => onDraftSelect(starter.prompt)}
              >
                <span className="messages-empty-starter-title">{starter.title}</span>
                <span className="messages-empty-starter-description">{starter.description}</span>
                <span className="messages-empty-starter-arrow" aria-hidden="true">→</span>
              </button>
            ))}
          </div>
        </section>
      )}

      <WorkspaceReminders workspaceId={activeWorkspace?.id} />

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

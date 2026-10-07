import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  ChatEmptyState,
  buildSuggestions,
  greetingForHour,
  pickTodayAutomations,
  starterPromptsForWorkspace,
} from './ChatEmptyState';
import { useKnowledgeStore } from '$stores/knowledge';
import { useNavigationStore } from '$stores/navigation';
import { useWorkspacesStore } from '$stores/workspaces';
import type { Automation } from '$lib/commands/automation';

vi.mock('$lib/commands/automation', () => ({
  getAutomations: vi.fn(),
}));
vi.mock('$lib/commands/memory', () => ({
  getMemories: vi.fn().mockResolvedValue([]),
}));

import { getAutomations } from '$lib/commands/automation';

const getAutomationsMock = getAutomations as ReturnType<typeof vi.fn>;

const makeAutomation = (overrides: Partial<Automation>): Automation => ({
  id: 'a1',
  title: '整理笔记',
  prompt: '',
  triggerKind: 'schedule',
  triggerValue: '每天 21:00',
  enabled: true,
  permissionSummary: '每次运行前询问',
  executorKind: 'agent',
  workspaceId: 'personal',
  scriptArgs: [],
  timeoutSeconds: 300,
  ...overrides,
});

const todayAt = (hour: number) => {
  const date = new Date();
  date.setHours(hour, 0, 0, 0);
  return Math.floor(date.getTime() / 1000);
};

describe('greetingForHour', () => {
  it('按时间段返回问候语', () => {
    expect(greetingForHour(8)).toBe('早上好');
    expect(greetingForHour(14)).toBe('下午好');
    expect(greetingForHour(22)).toBe('晚上好');
  });
});

describe('pickTodayAutomations', () => {
  it('只保留启用且下次运行在今天的任务，最多 3 条并按时间排序', () => {
    const items: Automation[] = [
      makeAutomation({ id: 'late', title: '晚间任务', nextRunAt: todayAt(21) }),
      makeAutomation({ id: 'early', title: '早间任务', nextRunAt: todayAt(8) }),
      makeAutomation({ id: 'disabled', title: '已暂停', enabled: false, nextRunAt: todayAt(9) }),
      makeAutomation({ id: 'tomorrow', title: '明天任务', nextRunAt: todayAt(9) + 86400 }),
      makeAutomation({ id: 'no-run', title: '未排期', nextRunAt: undefined }),
      makeAutomation({ id: 'mid', title: '午间任务', nextRunAt: todayAt(12) }),
      makeAutomation({ id: 'extra', title: '第四条', nextRunAt: todayAt(23) }),
    ];
    const picked = pickTodayAutomations(items, new Date(), 'personal');
    expect(picked.map((item) => item.id)).toEqual(['early', 'mid', 'late']);
  });

  it('指定工作区时不会展示其他项目的提醒', () => {
    const items = [
      makeAutomation({ id: 'personal', workspaceId: 'personal', nextRunAt: todayAt(9) }),
      makeAutomation({ id: 'project', workspaceId: 'project-a', nextRunAt: todayAt(10) }),
    ];

    expect(pickTodayAutomations(items, new Date(), 'personal').map((item) => item.id)).toEqual(['personal']);
  });
});

describe('buildSuggestions', () => {
  it('没有资料时给出资料库建议', () => {
    const suggestions = buildSuggestions({ knowledgeCount: 0 });
    expect(suggestions.map((item) => item.key)).toEqual(['no-knowledge']);
  });

  it('数据未加载完成时不给建议', () => {
    expect(buildSuggestions({ knowledgeCount: null })).toEqual([]);
  });
});

describe('starterPromptsForWorkspace', () => {
  it('个人空间和项目给出不同的任务起点', () => {
    expect(starterPromptsForWorkspace(false).map((item) => item.key)).toEqual([
      'plan-today', 'create-reminder', 'use-knowledge',
    ]);
    expect(starterPromptsForWorkspace(true).map((item) => item.key)).toEqual([
      'understand-project', 'diagnose-problem', 'review-changes',
    ]);
  });
});

describe('ChatEmptyState', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    getAutomationsMock.mockResolvedValue([]);
    useWorkspacesStore.setState({
      workspaces: [{ id: 'personal', kind: 'personal', name: 'Personal', activeSessionId: 's1', createdAt: 1, updatedAt: 1 }],
      activeWorkspaceId: 'personal',
    });
    useKnowledgeStore.setState({ entries: [{
      id: 'k1', content: '偏好浓缩咖啡', category: 'knowledge', importance: 5, source: 'knowledge', createdAt: 1, updatedAt: 1,
    }], isLoading: false, error: null });
    useNavigationStore.setState({ currentPage: 'chat' });
  });

  it('显示时间问候和发消息引导', () => {
    render(<ChatEmptyState />);
    expect(screen.getByText(/^(早上好|下午好|晚上好)$/)).toBeInTheDocument();
    expect(screen.getByText(/直接说出想处理的事/)).toBeInTheDocument();
  });

  it('个人空间任务起点只写入草稿，不直接发送', async () => {
    const user = userEvent.setup();
    const onDraftSelect = vi.fn();
    render(<ChatEmptyState onDraftSelect={onDraftSelect} />);

    await user.click(screen.getByRole('button', { name: /整理今天的安排/ }));
    expect(onDraftSelect).toHaveBeenCalledWith('帮我把今天要处理的事情整理成按优先级排序的行动清单。先问我缺少的关键信息。');
  });

  it('用户已在输入时不再展示会覆盖草稿的任务起点', () => {
    render(<ChatEmptyState onDraftSelect={vi.fn()} showStarters={false} />);
    expect(screen.queryByText('可以从这里开始')).not.toBeInTheDocument();
  });

  it('项目空状态说明当前唯一主对话的工作边界', () => {
    useWorkspacesStore.setState({
      workspaces: [{ id: 'project', kind: 'project', name: 'AngelBot', rootPath: 'D:\\AngelBot', activeSessionId: 's2', createdAt: 1, updatedAt: 1 }],
      activeWorkspaceId: 'project',
    });
    render(<ChatEmptyState />);

    expect(screen.getByText('项目 · AngelBot')).toBeInTheDocument();
    expect(screen.getByText(/这个项目的文件、历史与受控委派范围/)).toBeInTheDocument();
  });

  it('项目任务起点优先读取和诊断，不默认修改', () => {
    useWorkspacesStore.setState({
      workspaces: [{ id: 'project', kind: 'project', name: 'AngelBot', rootPath: 'D:\\AngelBot', activeSessionId: 's2', createdAt: 1, updatedAt: 1 }],
      activeWorkspaceId: 'project',
    });
    render(<ChatEmptyState onDraftSelect={vi.fn()} />);

    expect(screen.getByRole('button', { name: /了解当前项目/ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /检查近期变更/ })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /整理今天的安排/ })).not.toBeInTheDocument();
  });

  it('使用共享事项回看展示当前空间的待执行事项', async () => {
    const user = userEvent.setup();
    getAutomationsMock.mockResolvedValue([
      makeAutomation({ id: 'a1', title: '整理笔记', triggerValue: '每天 21:00', workspaceId: 'personal', nextRunAt: todayAt(21) }),
      makeAutomation({ id: 'a2', title: '已暂停任务', enabled: false, nextRunAt: todayAt(9) }),
      makeAutomation({ id: 'a3', title: '明天的任务', nextRunAt: todayAt(9) + 86400 }),
    ]);
    render(<ChatEmptyState />);

    await user.click(await screen.findByText('事项回看'));
    expect(screen.getByText('整理笔记')).toBeInTheDocument();
    expect(screen.getByText(/自动任务 · 今天/)).toBeInTheDocument();
    expect(screen.queryByText('已暂停任务')).not.toBeInTheDocument();
    expect(screen.getByText('明天的任务')).toBeInTheDocument();
  });

  it('自动化加载失败时静默降级为问候和引导', async () => {
    getAutomationsMock.mockRejectedValue(new Error('ipc down'));
    render(<ChatEmptyState />);

    await waitFor(() => expect(getAutomationsMock).toHaveBeenCalled());
    expect(screen.getByText(/^(早上好|下午好|晚上好)$/)).toBeInTheDocument();
    expect(screen.getByText(/直接说出想处理的事/)).toBeInTheDocument();
    expect(screen.queryByText('事项回看')).not.toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('尚未解析到真实空间时不会加载或展示其他空间提醒', () => {
    useWorkspacesStore.setState({ workspaces: [], activeWorkspaceId: 'unknown' });
    render(<ChatEmptyState />);

    expect(getAutomationsMock).not.toHaveBeenCalled();
    expect(screen.queryByText('事项回看')).not.toBeInTheDocument();
  });

  it('资料库为空时显示建议，点击后跳转到资料库', async () => {
    const user = userEvent.setup();
    useKnowledgeStore.setState({ entries: [], isLoading: false, error: null });
    render(<ChatEmptyState />);

    const suggestion = await screen.findByText('添加你的第一条资料，让 AngelBot 能引用它');
    await user.click(suggestion);
    expect(useNavigationStore.getState().currentPage).toBe('knowledge');
  });

  it('忽略建议后写入 localStorage 且不再出现', async () => {
    const user = userEvent.setup();
    useKnowledgeStore.setState({ entries: [], isLoading: false, error: null });
    const { unmount } = render(<ChatEmptyState />);

    await user.click(await screen.findByRole('button', { name: '忽略建议：添加你的第一条资料，让 AngelBot 能引用它' }));
    expect(localStorage.getItem('angelbot.emptyState.dismissed.no-knowledge')).toBe('1');
    expect(screen.queryByText('添加你的第一条资料，让 AngelBot 能引用它')).not.toBeInTheDocument();

    unmount();
    render(<ChatEmptyState />);
    await waitFor(() => expect(getAutomationsMock).toHaveBeenCalled());
    expect(screen.queryByText('添加你的第一条资料，让 AngelBot 能引用它')).not.toBeInTheDocument();
  });
});

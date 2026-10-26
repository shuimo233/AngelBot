import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { dismissLearnedPreference, getLearnedPreferences } from '$lib/commands/memory';
import { LearnedHabits } from './LearnedHabits';

vi.mock('$lib/commands/memory', () => ({
  getLearnedPreferences: vi.fn(),
  dismissLearnedPreference: vi.fn(),
}));

const habit = { id: 'habit-1', content: '喜欢先说结论，再按需展开', ready: true, updatedAt: 1 };

describe('LearnedHabits', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getLearnedPreferences).mockResolvedValue([habit]);
    vi.mocked(dismissLearnedPreference).mockResolvedValue(undefined);
  });

  it('shows actual learned records, permits dismissal while paused, and reloads persisted state', async () => {
    const user = userEvent.setup();
    const view = render(<LearnedHabits enabled />);
    expect(await screen.findByText(habit.content)).toBeInTheDocument();
    expect(screen.getByText('可作为回应参考')).toBeInTheDocument();
    view.rerender(<LearnedHabits enabled={false} />);
    expect(screen.getByText('已暂停沿用')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: `不再沿用：${habit.content}` }));
    await waitFor(() => expect(dismissLearnedPreference).toHaveBeenCalledWith(habit.id));
    expect(await screen.findByRole('status')).toHaveTextContent('已停止');
    expect(screen.queryByText(habit.content)).not.toBeInTheDocument();
    vi.mocked(getLearnedPreferences).mockResolvedValue([]);
    view.unmount();
    const emptyView = render(<LearnedHabits enabled />);
    expect(await screen.findByText(/自然聊天就好/)).toBeInTheDocument();
    emptyView.rerender(<LearnedHabits enabled={false} />);
    expect(screen.getByText(/暂时没有可显示的习惯记录；偏好学习已暂停/)).toBeInTheDocument();
    expect(screen.queryByText(/自然聊天就好/)).not.toBeInTheDocument();
    expect(getLearnedPreferences).toHaveBeenCalledTimes(2);
  });

  it('preserves a habit on failed dismissal and permits explicit retry after load failure', async () => {
    const user = userEvent.setup();
    vi.mocked(getLearnedPreferences).mockRejectedValueOnce(new Error('unavailable'));
    render(<LearnedHabits enabled />);
    expect(await screen.findByRole('alert')).toHaveTextContent('暂时无法读取');
    await user.click(screen.getByRole('button', { name: '刷新习惯' }));
    expect(await screen.findByText(habit.content)).toBeInTheDocument();
    vi.mocked(dismissLearnedPreference).mockRejectedValueOnce(new Error('write failed'));
    await user.click(screen.getByRole('button', { name: `不再沿用：${habit.content}` }));
    expect(await screen.findByRole('alert')).toHaveTextContent('未能停止沿用');
    expect(screen.getByText(habit.content)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: `不再沿用：${habit.content}` }));
    expect(await screen.findByRole('status')).toHaveTextContent('已停止');
  });
});

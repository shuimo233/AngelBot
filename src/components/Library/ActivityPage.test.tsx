import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { ActivityPage } from './ActivityPage';
import { getActivity } from '$lib/commands/activity';

vi.mock('$lib/commands/activity', () => ({ getActivity: vi.fn() }));
it('distinguishes loading, failure and first use and allows a deliberate retry', async () => {
  vi.mocked(getActivity).mockRejectedValueOnce(new Error('failed')).mockResolvedValueOnce([]);
  const user = userEvent.setup();
  render(<ActivityPage />);
  expect(screen.getByRole('status')).toHaveTextContent('正在读取');
  expect(await screen.findByRole('alert')).toHaveTextContent('暂时无法读取');
  expect(screen.queryByText('暂时没有使用记录')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: '重新加载' }));
  expect(await screen.findByText('暂时没有使用记录')).toBeVisible();
});

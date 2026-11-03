import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { MemoryPage } from './MemoryPage';
import { getMemories, type RustMemory } from '$lib/commands/memory';

vi.mock('$lib/commands/memory', () => ({ getMemories: vi.fn() }));
vi.mock('../Settings/pages/MemoryGovernanceSettings', () => ({ MemoryGovernanceSettings: () => <p>已有记忆管理</p> }));
const base: RustMemory = { id: 'pref', scope: 'global', category: 'preference', content: '先说结论', importance: 4, source: 'user', frequency: 0, last_mentioned: null, is_permanent: true, embedding: null, decay_factor: 1, forget_stage: 'active', created_at: 1, updated_at: 1 };

it('distinguishes personal memory from saved reference content and keeps the existing management path', async () => {
  vi.mocked(getMemories).mockResolvedValue([base, { ...base, id: 'doc', category: 'knowledge', content: '单独保存的参考文件' }]);
  const user = userEvent.setup();
  render(<MemoryPage />);
  expect(await screen.findByText('先说结论')).toBeVisible();
  expect(screen.queryByText('单独保存的参考文件')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: '管理记忆' }));
  expect(screen.getByText('已有记忆管理')).toBeInTheDocument();
  vi.mocked(getMemories).mockResolvedValue([{ ...base, content: '已修订的偏好' }]);
  await user.click(screen.getByRole('button', { name: '返回记忆概览' }));
  expect(await screen.findByText('已修订的偏好')).toBeVisible();
  expect(screen.queryByText('先说结论')).not.toBeInTheDocument();
});

it('does not show a first-use guide for a pending or failed read', async () => {
  vi.mocked(getMemories).mockRejectedValue(new Error('unavailable'));
  render(<MemoryPage />);
  expect(screen.getByRole('status')).toHaveTextContent('正在读取');
  expect(await screen.findByRole('alert')).toHaveTextContent('无法读取');
  expect(screen.queryByText('还没有可显示的长期记忆')).not.toBeInTheDocument();
});

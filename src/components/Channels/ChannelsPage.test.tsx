import { fireEvent, render, screen, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { ChannelsPage } from './ChannelsPage';
import * as channel from '$lib/commands/channel';

vi.mock('$lib/commands/channel', () => ({
  getChannelConnections: vi.fn(), createChannelConnection: vi.fn(),
  getChannelAudit: vi.fn(), revokeChannelConnection: vi.fn(),
  clearChannelCredential: vi.fn(), saveChannelCredential: vi.fn(),
  setChannelEnabled: vi.fn(), setChannelProactivePolicy: vi.fn(),
  confirmChannelMessage: vi.fn(),
}));

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(channel.getChannelConnections).mockResolvedValue([{
    id: 'local-channel', displayName: '个人渠道', channelType: 'custom', enabled: false,
    permissionSummary: '投递需确认', proactiveReason: '', proactiveDailyLimit: 0,
    credentialConfigured: false,
  }]);
});

it('groups connection facts and credential controls without enabling a channel on render', async () => {
  render(<ChannelsPage />);
  const connection = await screen.findByRole('region', { name: '个人渠道' });
  expect(within(connection).getByText('投递需确认')).toBeInTheDocument();
  expect(within(connection).getByLabelText('渠道凭据（仅保存在系统钥匙串）')).toHaveAttribute('type', 'password');
  expect(within(connection).getByRole('button', { name: '安全保存凭据' })).toBeDisabled();
  expect(within(connection).getByRole('button', { name: '确认测试投递' })).toBeDisabled();
  expect(channel.setChannelEnabled).not.toHaveBeenCalled();
  expect(channel.saveChannelCredential).not.toHaveBeenCalled();
  expect(channel.confirmChannelMessage).not.toHaveBeenCalled();
});

it('keeps registration behind its explicit local review after the form is regrouped', async () => {
  render(<ChannelsPage />);
  await screen.findByRole('region', { name: '个人渠道' });
  fireEvent.change(screen.getByLabelText('渠道名称'), { target: { value: '新渠道' } });
  fireEvent.click(screen.getByRole('button', { name: '登记渠道' }));
  const review = screen.getByRole('alert');
  expect(review).toHaveTextContent('登记不会保存凭据或启用消息收发');
  expect(channel.createChannelConnection).not.toHaveBeenCalled();
  fireEvent.click(within(review).getByRole('button', { name: '取消' }));
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(channel.createChannelConnection).not.toHaveBeenCalled();
});

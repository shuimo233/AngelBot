import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { DataSettings } from './DataSettings';

describe('DataSettings', () => {
  it('distinguishes ordinary and encrypted MCP credential backups without offering to reveal values', async () => {
    const user = userEvent.setup();
    render(<DataSettings />);
    await user.click(screen.getByRole('button', { name: '数据备份' }));

    expect(screen.getByText(/普通备份不包含 MCP 专用密钥/)).toBeInTheDocument();
    expect(screen.getByText(/加密备份可携带这些密钥/)).toBeInTheDocument();
    expect(screen.getByText(/恢复时会写入系统凭据库，不会回显原值/)).toBeInTheDocument();
  });
});

import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { UiLibrary } from './UiLibrary';

afterEach(cleanup);

describe('interactive UI library', () => {
  it('previews a confirmation without invoking file or model operations', async () => {
    render(<UiLibrary />);
    fireEvent.click(screen.getAllByRole('button', { name: '查看计划' })[0]);
    const dialog = screen.getByRole('dialog', { name: '执行前，先确认计划' });
    await waitFor(() => expect(document.activeElement).toBe(within(dialog).getByRole('button', { name: '先不执行' })));
    fireEvent.click(within(dialog).getByRole('button', { name: '确认示例计划' }));
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(screen.getByRole('status').textContent).toContain('没有执行实际文件操作');
  });

  it('connects validation errors to the field and can stop a busy example', () => {
    render(<UiLibrary />);
    fireEvent.click(screen.getByRole('button', { name: '测试示例校验' }));
    expect(screen.getByLabelText('API Key（仅示例）').getAttribute('aria-invalid')).toBe('true');
    fireEvent.change(screen.getByLabelText('API Key（仅示例）'), { target: { value: 'example-only' } });
    expect(screen.getByLabelText('API Key（仅示例）').getAttribute('aria-invalid')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: '试用执行状态' }));
    expect(screen.getByRole('button', { name: '正在处理' }).getAttribute('aria-busy')).toBe('true');
    fireEvent.click(screen.getByRole('button', { name: '停止示例' }));
    expect(screen.getByRole('button', { name: '试用执行状态' })).toBeDefined();
  });
});

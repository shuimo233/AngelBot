import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DesktopActionApproval } from './DesktopActionApproval';
import { preflightPendingDesktopAction, type DesktopActionPreview } from '$lib/commands/message';
import { desktopConfirmationOperation, requiresDesktopConfirmationPreview } from '$lib/tool-confirmation';

vi.mock('$lib/commands/message', () => ({ preflightPendingDesktopAction: vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });

function preview(): DesktopActionPreview {
  return { previewId: 'live-preview', operation: 'imageclick', appDisplayName: 'Fixture', executableName: 'fixture.exe',
    windowTitle: null, controlName: null, text: null, expiresAt: Date.now() + 60_000,
    image: { imageDataUrl: 'data:image/png;base64,aGVsbG8=', width: 400, height: 200, x: 100, y: 50 } };
}

describe('one-window visual click approval', () => {
  it('reserves the exact preview path and releases image pixels after one decision', async () => {
    expect(desktopConfirmationOperation('click_trusted_app_image')).toBe('imageclick');
    expect(requiresDesktopConfirmationPreview('click_trusted_app_image')).toBe(true);
    vi.mocked(preflightPendingDesktopAction).mockResolvedValue(preview());
    const resolve = vi.fn().mockResolvedValue(undefined);
    render(<DesktopActionApproval sessionId="s" messageId="m" callId="call" operation="imageclick" layout="stream" onResolve={resolve} />);
    const button = await screen.findByRole('button', { name: '允许单次窗口点击一次' });
    expect(button).toBeDisabled();
    const image = screen.getByRole('img', { name: '本次待点击的授权窗口' });
    Object.defineProperties(image, { naturalWidth: { value: 400 }, naturalHeight: { value: 200 } });
    fireEvent.load(image);
    await waitFor(() => expect(button).toBeEnabled());
    expect(screen.getByRole('img', { name: '本次待点击的授权窗口' })).toBeInTheDocument();
    const marker = screen.getByLabelText('点击位置 100, 50');
    expect(parseFloat(marker.style.left)).toBeCloseTo(25.125, 6);
    expect(parseFloat(marker.style.top)).toBeCloseTo(25.25, 6);
    fireEvent.click(button);
    await waitFor(() => expect(resolve).toHaveBeenCalledTimes(1));
    expect(resolve).toHaveBeenCalledWith('approved', 'live-preview');
    await waitFor(() => expect(screen.queryByRole('img')).not.toBeInTheDocument());
    expect(button).toBeDisabled();
    expect(screen.getByRole('button', { name: '拒绝' })).toBeDisabled();
  });

  it.each(['load-error', 'dimension-mismatch'])('refuses approval when the actual image has a %s', async (failure) => {
    vi.mocked(preflightPendingDesktopAction).mockResolvedValue(preview());
    const resolve = vi.fn();
    render(<DesktopActionApproval sessionId="s" messageId="m" callId="call" operation="imageclick" layout="stream" onResolve={resolve} />);
    const image = await screen.findByRole('img');
    if (failure === 'load-error') fireEvent.error(image);
    else {
      Object.defineProperties(image, { naturalWidth: { value: 401 }, naturalHeight: { value: 200 } });
      fireEvent.load(image);
    }
    await screen.findByRole('alert');
    const button = screen.getByRole('button', { name: '允许单次窗口点击一次' });
    expect(button).toBeDisabled();
    fireEvent.click(button);
    expect(resolve).not.toHaveBeenCalled();
    expect(screen.queryByRole('img')).not.toBeInTheDocument();
  });

  it.each(['missing', 'outside', 'fractional', 'url', 'expired', 'wrong-operation'])('never enables an unreviewable %s preview', async (change) => {
    const value = preview();
    if (change === 'missing') value.image = null;
    if (change === 'outside') value.image!.x = 400;
    if (change === 'fractional') value.image!.y = 1.2;
    if (change === 'url') value.image!.imageDataUrl = 'https://example.invalid/private.png';
    if (change === 'expired') value.expiresAt = Date.now() - 1;
    if (change === 'wrong-operation') value.operation = 'image';
    vi.mocked(preflightPendingDesktopAction).mockResolvedValue(value);
    const resolve = vi.fn();
    render(<DesktopActionApproval sessionId="s" messageId="m" callId="call" operation="imageclick" layout="compact" onResolve={resolve} />);
    await screen.findByRole('alert');
    const button = screen.getByRole('button', { name: '允许单次窗口点击一次' });
    expect(button).toBeDisabled();
    expect(screen.queryByRole('img')).not.toBeInTheDocument();
    fireEvent.click(button);
    expect(resolve).not.toHaveBeenCalled();
  });
});

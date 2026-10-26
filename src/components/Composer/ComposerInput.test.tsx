import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { createRef, useState } from 'react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ComposerInput, type ComposerInputProps, type ComposerInputHandle } from './ComposerInput';
import {
  getAgentExecutionPermission,
  setAgentExecutionPermission,
} from '$lib/commands/settings';
import { useThinkingEffortStore } from '$stores/thinkingEffort';
import { useExecutionPermissionStore } from '$stores/executionPermission';
import { ExecutionPermissionSettings } from '../Settings/pages/ExecutionPermissionSettings';
import { extractDocumentText } from '$lib/commands/message';

vi.mock('$lib/commands/message', () => ({ extractDocumentText: vi.fn() }));

vi.mock('$lib/commands/settings', () => ({
  getAgentExecutionPermission: vi.fn(),
  setAgentExecutionPermission: vi.fn(),
}));

const getPermissionMock = vi.mocked(getAgentExecutionPermission);
const setPermissionMock = vi.mocked(setAgentExecutionPermission);

describe('ComposerInput execution permission', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.stubEnv('MODE', 'development');
    setPermissionMock.mockImplementation(async (permission) => permission);
    useThinkingEffortStore.setState({ effort: 'medium' });
    useExecutionPermissionStore.setState({ permission: null, loading: true, saving: false, error: null });
  });

  it('recovers the persisted permission after an initial settings read failure', async () => {
    getPermissionMock
      .mockRejectedValueOnce(new Error('Dev HTTP is still starting'))
      .mockResolvedValue('full_access');

    render(
      <ComposerInput
        value=""
        onChange={vi.fn()}
        onSend={vi.fn()}
        onAbort={vi.fn()}
        isStreaming={false}
        toolPreset="default"
        onToolPresetChange={vi.fn()}
        soundEnabled={false}
        onSoundToggle={vi.fn()}
      />,
    );

    await waitFor(() => expect(getPermissionMock).toHaveBeenCalledTimes(2), { timeout: 3_000 });
    expect(screen.getByRole('button', { name: /权限 完全访问/ })).toHaveAttribute(
      'title',
      expect.stringContaining('按任务授权连续执行'),
    );
    expect(screen.getByRole('button', { name: /权限 完全访问/ })).toHaveAttribute('title', expect.stringContaining('已有保护操作仍会询问'));
  });

  it('keeps advanced model and reasoning choices behind one clear disclosure', async () => {
    const user = userEvent.setup();
    const onToolPresetChange = vi.fn();
    getPermissionMock.mockResolvedValue('ask');

    render(
      <ComposerInput
        value=""
        onChange={vi.fn()}
        onSend={vi.fn()}
        onAbort={vi.fn()}
        isStreaming={false}
        toolPreset="default"
        onToolPresetChange={onToolPresetChange}
        soundEnabled={false}
        onSoundToggle={vi.fn()}
      />,
    );

    expect(screen.queryByRole('button', { name: '高' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: /运行配置 中 · 默认工具/ }));
    expect(screen.getByRole('button', { name: '高' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '全部工具' })).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '高' }));
    await user.click(screen.getByRole('button', { name: '全部工具' }));
    expect(useThinkingEffortStore.getState().effort).toBe('high');
    expect(onToolPresetChange).toHaveBeenCalledWith('full');
  });

  it('updates the composer when the shared settings page commits a permission change', async () => {
    getPermissionMock.mockResolvedValue('ask');
    const user = userEvent.setup();
    render(<>
      <ComposerInput
        value=""
        onChange={vi.fn()}
        onSend={vi.fn()}
        onAbort={vi.fn()}
        isStreaming={false}
        toolPreset="default"
        onToolPresetChange={vi.fn()}
        soundEnabled={false}
        onSoundToggle={vi.fn()}
      />
      <ExecutionPermissionSettings />
    </>);

    await screen.findByRole('button', { name: /权限 请求批准/ });
    await user.click(screen.getByText('权限设置'));
    await user.click(screen.getByRole('button', { name: /完全访问/ }));
    await waitFor(() => expect(screen.getByRole('button', { name: /权限 完全访问/ })).toBeInTheDocument());
  });
});

function AttachmentComposer({ composerRef, ...props }: Partial<ComposerInputProps> & { composerRef?: React.Ref<ComposerInputHandle> }) {
  const [value, setValue] = useState('');
  return <ComposerInput ref={composerRef} value={value} onChange={setValue} onSend={() => true} onAbort={() => {}}
    isStreaming={false} toolPreset="default" onToolPresetChange={() => {}}
    soundEnabled={false} onSoundToggle={() => {}} scopeKey="session-one" {...props} />;
}

describe('ComposerInput text files', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.stubEnv('MODE', 'test');
    useExecutionPermissionStore.setState({ permission: 'ask', loading: false, saving: false, error: null });
  });

  it('permits file-only messages, discloses model sharing and restores snapshots after rejected acceptance', async () => {
    const user = userEvent.setup();
    let accept: (value: boolean) => void = () => {};
    const onSend = vi.fn(() => new Promise<boolean>((resolve) => { accept = resolve; }));
    render(<AttachmentComposer onSend={onSend} />);
    await user.upload(screen.getByLabelText('选择文本附件'), new File(['one,two\n1,2'], 'tasks.csv'));
    expect(await screen.findByText('tasks.csv')).toBeInTheDocument();
    expect(screen.getByText(/发送后内容将交给当前配置的模型/)).toBeVisible();
    expect(screen.getByLabelText('附件文字 tasks.csv')).not.toBeVisible();
    await user.click(screen.getByText('tasks.csv'));
    expect(screen.getByLabelText('附件文字 tasks.csv')).toBeVisible();
    expect(screen.getByLabelText('附件文字 tasks.csv').textContent).toBe('one,two\n1,2');
    expect(onSend).not.toHaveBeenCalled();
    await user.click(screen.getByText('tasks.csv'));
    await waitFor(() => expect(screen.getByLabelText('附件文字 tasks.csv')).not.toBeVisible());
    await user.click(screen.getByRole('button', { name: '发送' }));
    expect(onSend).toHaveBeenCalledWith('', [{ name: 'tasks.csv', text: 'one,two\n1,2' }]);
    expect(screen.queryByText('tasks.csv')).not.toBeInTheDocument();
    await act(async () => accept(false));
    expect(screen.getByRole('alert')).toHaveTextContent('附件仍保留');
    expect(screen.getByText('tasks.csv')).toBeInTheDocument();
    expect(screen.getByLabelText('附件文字 tasks.csv')).not.toBeVisible();
    await user.click(screen.getByRole('button', { name: '发送' }));
    await act(async () => accept(true));
    expect(screen.queryByText('tasks.csv')).not.toBeInTheDocument();
  });

  it('rejects unsupported pasted or selected files instead of displaying a misleading image preview', async () => {
    const user = userEvent.setup({ applyAccept: false });
    const onSend = vi.fn(() => true);
    render(<AttachmentComposer onSend={onSend} />);
    await user.upload(screen.getByLabelText('选择文本附件'), new File(['picture'], 'photo.png', { type: 'image/png' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('图片、PDF、旧版 Word/Excel 暂不支持');
    expect(screen.queryByRole('img')).not.toBeInTheDocument();
    const input = screen.getByPlaceholderText('给 AngelBot 发消息…');
    fireEvent.paste(input, { clipboardData: { items: [{ kind: 'file', getAsFile: () => new File(['pdf'], 'work.pdf') }] } });
    expect(await screen.findByRole('alert')).toHaveTextContent('暂不支持');
    expect(screen.getByRole('button', { name: '发送' })).toBeDisabled();

    // A failed batch must identify its bad document and preserve the old draft,
    // without quietly accepting the other file from the same selection.
    await user.type(input, '整理资料');
    await user.upload(screen.getByLabelText('选择文本附件'), new File(['existing'], 'existing.txt'));
    await screen.findByText('existing.txt');
    for (const source of ['selection', 'drop', 'paste']) {
      vi.mocked(extractDocumentText).mockRejectedValueOnce(new Error('XLSX 含公式，请导出所需工作表为 CSV。'));
      const files = [new File(['good'], 'good.txt'), new File(['PK'], 'budget.xlsx')];
      if (source === 'selection') await user.upload(screen.getByLabelText('选择文本附件'), files);
      else if (source === 'drop') fireEvent.drop(input, { dataTransfer: { files } });
      else fireEvent.paste(input, { clipboardData: { items: files.map((file) => ({ kind: 'file', getAsFile: () => file })) } });
      await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('无法添加“budget.xlsx”：XLSX 含公式'));
      expect(screen.getByRole('alert')).toHaveTextContent('本次选择未添加任何附件，已有附件和输入文字未改变');
      expect(screen.getByText('existing.txt')).toBeInTheDocument();
      expect(screen.queryByText('good.txt')).not.toBeInTheDocument();
      expect(screen.queryByText('budget.xlsx')).not.toBeInTheDocument();
      expect(input).toHaveValue('整理资料');
      expect(screen.getByRole('button', { name: '发送' })).toBeEnabled();
      expect(onSend).not.toHaveBeenCalled();
    }
    await user.upload(screen.getByLabelText('选择文本附件'), new File(['corrected'], 'budget.csv'));
    await screen.findByText('budget.csv');
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '发送' }));
    expect(onSend).toHaveBeenCalledWith('整理资料', [
      { name: 'existing.txt', text: 'existing' }, { name: 'budget.csv', text: 'corrected' },
    ]);
  });

  it('waits for document extraction before sending and discloses each limited scope', async () => {
    const user = userEvent.setup();
    for (const [extension, notice] of [['docx', /DOCX 只读取正文和表格文字/], ['xlsx', /XLSX 读取保存的原始单元格值/]] as const) {
      let finish: (value: { name: string; text: string }) => void = () => {};
      vi.mocked(extractDocumentText).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
      const onSend = vi.fn(() => false);
      const { unmount } = render(<AttachmentComposer onSend={onSend} />);
      const name = `arrangements.${extension}`;
      await user.upload(screen.getByLabelText('选择文本附件'), new File(['PK'], name));
      await waitFor(() => expect(extractDocumentText).toHaveBeenCalledWith(name, 'UEs='));
      expect(screen.getByRole('button', { name: '发送' })).toBeDisabled();
      expect(screen.getByText(/正在读取文件文字/)).toBeVisible();
      const snapshot = { name, text: `【${extension.toUpperCase()} 文字快照】\n正文` };
      await act(async () => finish(snapshot));
      expect(screen.getByText(notice)).toBeVisible();
      await user.click(screen.getByRole('button', { name: '发送' }));
      expect(onSend).toHaveBeenCalledWith('', [snapshot]);
      expect(screen.getByRole('alert')).toHaveTextContent('附件仍保留');
      expect(screen.getByText(name)).toBeInTheDocument();
      unmount();
    }
  });

  it('keeps later draft text and files when an older send finishes, and never drops files on streaming Enter', async () => {
    const user = userEvent.setup();
    let accept: (value: boolean) => void = () => {};
    const onSend = vi.fn(() => new Promise<boolean>((resolve) => { accept = resolve; }));
    const onSteer = vi.fn();
    const onFollowUp = vi.fn();
    const props = { onSend, onSteer, onFollowUp };
    const { rerender } = render(<AttachmentComposer {...props} />);
    await user.upload(screen.getByLabelText('选择文本附件'), new File(['first'], 'first.txt'));
    await screen.findByText('first.txt');
    await user.click(screen.getByRole('button', { name: '发送' }));
    rerender(<AttachmentComposer {...props} isStreaming />);
    await user.upload(screen.getByLabelText('选择文本附件'), new File(['next'], 'next.txt'));
    await screen.findByText('next.txt');
    const input = screen.getByRole('textbox');
    await user.type(input, '下一条{Enter}');
    expect(input).toHaveValue('下一条');
    expect(onSteer).not.toHaveBeenCalled();
    expect(onFollowUp).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: '引导' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '排队' })).toBeDisabled();
    await act(async () => accept(true));
    expect(screen.queryByText('first.txt')).not.toBeInTheDocument();
    expect(screen.getByText('next.txt')).toBeInTheDocument();
    expect(input).toHaveValue('下一条');
  });

  it('allows text-only steering while submitted files are in flight, and restores them if admission fails', async () => {
    const user = userEvent.setup();
    let accept: (value: boolean) => void = () => {};
    const onSend = vi.fn(() => new Promise<boolean>((resolve) => { accept = resolve; }));
    const onSteer = vi.fn();
    const { rerender } = render(<AttachmentComposer onSend={onSend} onSteer={onSteer} />);
    await user.upload(screen.getByLabelText('选择文本附件'), new File(['submitted'], 'submitted.txt'));
    await screen.findByText('submitted.txt');
    await user.click(screen.getByRole('button', { name: '发送' }));
    rerender(<AttachmentComposer onSend={onSend} onSteer={onSteer} isStreaming />);
    expect(screen.queryByText(/附件已暂存/)).not.toBeInTheDocument();
    await user.type(screen.getByRole('textbox'), '先给简短结果');
    expect(screen.getByRole('button', { name: '引导' })).toBeEnabled();
    await user.keyboard('{Enter}');
    expect(onSteer).toHaveBeenCalledWith('先给简短结果');
    expect(onSend).toHaveBeenCalledWith('', [{ name: 'submitted.txt', text: 'submitted' }]);
    await act(async () => accept(false));
    expect(screen.getByText('submitted.txt')).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('附件仍保留');
  });

  it('drops late reads and staged files on any scope change, even when returning to the original scope', async () => {
    const user = userEvent.setup();
    const readers: FileReader[] = [];
    const read = vi.spyOn(FileReader.prototype, 'readAsArrayBuffer').mockImplementation(function (this: FileReader) { readers.push(this); });
    try {
      const composerRef = createRef<ComposerInputHandle>();
      const { rerender } = render(<AttachmentComposer scopeKey="one" composerRef={composerRef} />);
      act(() => composerRef.current?.restoreFiles([{ name: 'staged.txt', text: 'private staged snapshot' }]));
      await user.click(screen.getByText('staged.txt'));
      expect(screen.getByLabelText('附件文字 staged.txt')).toBeVisible();
      await user.upload(screen.getByLabelText('选择文本附件'), new File(['private'], 'private.txt'));
      expect(readers).toHaveLength(1);
      rerender(<AttachmentComposer scopeKey="two" />);
      rerender(<AttachmentComposer scopeKey="one" />);
      await act(async () => {
        Object.defineProperty(readers[0], 'result', { value: new TextEncoder().encode('private').buffer });
        readers[0].dispatchEvent(new ProgressEvent('load'));
      });
      expect(screen.queryByText('private.txt')).not.toBeInTheDocument();
      expect(screen.queryByText('private staged snapshot')).not.toBeInTheDocument();
      expect(screen.getByRole('button', { name: '发送' })).toBeDisabled();
    } finally { read.mockRestore(); }
  });

  it('preserves a newly retyped identical text draft while text-only steering stays available', async () => {
    const user = userEvent.setup();
    let accept: (value: boolean) => void = () => {};
    const onSend = () => new Promise<boolean>((resolve) => { accept = resolve; });
    const onSteer = vi.fn();
    const { rerender } = render(<AttachmentComposer onSend={onSend} onSteer={onSteer} />);
    const input = screen.getByRole('textbox');
    await user.type(input, '继续');
    await user.click(screen.getByRole('button', { name: '发送' }));
    await user.clear(input);
    await user.type(input, '继续');
    rerender(<AttachmentComposer onSend={onSend} onSteer={onSteer} isStreaming />);
    expect(screen.getByRole('button', { name: '引导' })).toBeEnabled();
    await act(async () => accept(true));
    expect(input).toHaveValue('继续');
    await user.keyboard('{Enter}');
    expect(onSteer).toHaveBeenCalledWith('继续');
  });

  it('merges rapid selections and removes exact snapshots', async () => {
    const user = userEvent.setup();
    render(<AttachmentComposer />);
    const input = screen.getByLabelText('选择文本附件');
    fireEvent.change(input, { target: { files: [new File(['one'], 'one.txt')] } });
    fireEvent.change(input, { target: { files: [new File(['two'], 'two.txt')] } });
    await screen.findByText('one.txt');
    await screen.findByText('two.txt');
    await user.click(screen.getByText('one.txt'));
    expect(screen.getByLabelText('附件文字 one.txt')).toBeVisible();
    await user.click(screen.getByRole('button', { name: '移除附件 one.txt' }));
    expect(screen.queryByText('one.txt')).not.toBeInTheDocument();
    expect(screen.getByText('two.txt')).toBeInTheDocument();
    expect(screen.getByLabelText('附件文字 two.txt')).not.toBeVisible();
    await user.click(screen.getByText('two.txt'));
    expect(screen.getByLabelText('附件文字 two.txt')).toBeVisible();
    fireEvent.change(input, { target: { files: [new File(['<img src=x onerror="throw 1">\n**plain text**'], 'two.txt')] } });
    await waitFor(() => expect(screen.getAllByText('two.txt')).toHaveLength(2));
    await user.click(screen.getAllByRole('button', { name: '移除附件 two.txt' })[0]);
    expect(screen.getByLabelText('附件文字 two.txt')).not.toBeVisible();
    await user.click(screen.getByText('two.txt'));
    expect(screen.getByLabelText('附件文字 two.txt').textContent).toBe('<img src=x onerror="throw 1">\n**plain text**');
    expect(screen.queryByRole('img')).not.toBeInTheDocument();
  });

  it('merges rejected routed snapshots without replacing a later draft, and reports cap conflicts', async () => {
    const user = userEvent.setup();
    const composerRef = createRef<ComposerInputHandle>();
    render(<AttachmentComposer composerRef={composerRef} />);
    await user.upload(screen.getByLabelText('选择文本附件'), new File(['next'], 'next.txt'));
    await screen.findByText('next.txt');
    act(() => composerRef.current?.restoreFiles([{ name: 'rejected.txt', text: 'old snapshot' }]));
    expect(screen.getByText('next.txt')).toBeInTheDocument();
    expect(screen.getByText('rejected.txt')).toBeInTheDocument();
    act(() => composerRef.current?.restoreFiles(Array.from({ length: 4 }, (_, index) => ({ name: `overflow-${index}.txt`, text: 'old' }))));
    expect(screen.getByRole('alert')).toHaveTextContent('现有草稿未被覆盖');
    expect(screen.getByText('next.txt')).toBeInTheDocument();
    expect(screen.getByText('rejected.txt')).toBeInTheDocument();
    expect(screen.queryByText('overflow-0.txt')).not.toBeInTheDocument();
  });
});

import { createRef, useState } from 'react';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ConfirmDialog } from '../components/common/ConfirmDialog';
import { Button, Dialog, IconButton, Input, Select, SelectField, Switch, TextField } from './index';

describe('AngelBot UI controls', () => {
  it('uses native button keyboard behavior, refs and a safe default type', async () => {
    const user = userEvent.setup();
    const action = vi.fn();
    const submit = vi.fn();
    const ref = createRef<HTMLButtonElement>();
    render(<form onSubmit={submit}><Button ref={ref} variant="primary" onClick={action}>保存</Button></form>);

    expect(ref.current).toBe(screen.getByRole('button', { name: '保存' }));
    expect(ref.current).toHaveAttribute('type', 'button');
    await user.tab();
    await user.keyboard('{Enter}');
    expect(action).toHaveBeenCalledOnce();
    expect(submit).not.toHaveBeenCalled();
  });

  it('keeps a busy action named and prevents duplicate activation', async () => {
    const user = userEvent.setup();
    const action = vi.fn();
    const { rerender } = render(<Button onClick={action} busy>正在保存</Button>);
    const button = screen.getByRole('button', { name: '正在保存' });
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute('aria-busy', 'true');
    await user.click(button);
    expect(action).not.toHaveBeenCalled();

    rerender(<Button onClick={action} disabled>正在保存</Button>);
    expect(button).toBeDisabled();
    expect(button).not.toHaveAttribute('aria-busy');
  });

  it('gives icon actions an explicit name without exposing their decorative glyph', () => {
    render(<IconButton label="关闭窗口"><svg><title>装饰图标</title></svg></IconButton>);
    const button = screen.getByRole('button', { name: '关闭窗口' });
    expect(button).toHaveAttribute('title', '关闭窗口');
    expect(button.querySelector('svg')?.parentElement).toHaveAttribute('aria-hidden', 'true');
  });

  it('forwards native input/select attributes, change events and refs', async () => {
    const user = userEvent.setup();
    const inputRef = createRef<HTMLInputElement>();
    const selectRef = createRef<HTMLSelectElement>();
    const onChange = vi.fn();
    render(
      <>
        <label htmlFor="native-input">端口</label>
        <Input ref={inputRef} id="native-input" type="number" min={1} max={65535} onChange={onChange} />
        <label htmlFor="native-select">模式</label>
        <Select ref={selectRef} id="native-select" defaultValue="local">
          <option value="local">本地</option><option value="remote">远程</option>
        </Select>
      </>,
    );
    expect(inputRef.current).toBe(screen.getByRole('spinbutton', { name: '端口' }));
    expect(inputRef.current).toHaveAttribute('min', '1');
    expect(inputRef.current).toHaveAttribute('max', '65535');
    await user.type(inputRef.current!, '80');
    expect(onChange).toHaveBeenCalled();
    await user.selectOptions(selectRef.current!, 'remote');
    expect(selectRef.current).toHaveValue('remote');
  });

  it('connects labels, hints, caller descriptions and errors with unique stable IDs', () => {
    const { rerender } = render(
      <>
        <span id="external-description">不会上传</span>
        <TextField label="文件路径" hint="填写绝对路径" error="路径不存在" aria-describedby="external-description" />
        <TextField label="另一条路径" />
      </>,
    );
    const input = screen.getByRole('textbox', { name: '文件路径' });
    const originalId = input.id;
    expect(input.id).not.toBe(screen.getByRole('textbox', { name: '另一条路径' }).id);
    expect(input).toHaveAccessibleDescription('不会上传 填写绝对路径 路径不存在');
    expect(input).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByRole('alert')).toHaveTextContent('路径不存在');

    rerender(
      <>
        <span id="external-description">不会上传</span>
        <TextField label="文件路径" hint="填写绝对路径" aria-describedby="external-description" />
        <TextField label="另一条路径" />
      </>,
    );
    expect(input.id).toBe(originalId);
    expect(input).not.toHaveAttribute('aria-invalid');
    expect(input).toHaveAccessibleDescription('不会上传 填写绝对路径');
  });

  it('supports labeled multiline fields and native select options', async () => {
    const user = userEvent.setup();
    const textareaRef = createRef<HTMLTextAreaElement>();
    render(
      <>
        <TextField ref={textareaRef} id="instructions" label="说明" multiline rows={5} defaultValue="安静地工作" />
        <SelectField label="主题" hint="跟随你的喜好" defaultValue="light" options={[
          { value: 'light', label: '浅色' }, { value: 'dark', label: '深色' },
          { value: 'unavailable', label: '不可选', disabled: true },
        ]} />
      </>,
    );
    expect(textareaRef.current).toBe(screen.getByRole('textbox', { name: '说明' }));
    expect(textareaRef.current).toHaveAttribute('rows', '5');
    expect(textareaRef.current).toHaveValue('安静地工作');
    const select = screen.getByRole('combobox', { name: '主题' });
    expect(select).toHaveAccessibleDescription('跟随你的喜好');
    await user.selectOptions(select, 'dark');
    expect(select).toHaveValue('dark');
    expect(screen.getByRole('option', { name: '不可选' })).toBeDisabled();
  });

  it('keeps switches native, keyboard operable, labeled and disabled when requested', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const ref = createRef<HTMLInputElement>();
    render(<Switch ref={ref} label="通知" hint="仅在任务完成时" onChange={onChange} />);
    const checkbox = screen.getByRole('checkbox', { name: '通知' });
    expect(ref.current).toBe(checkbox);
    expect(checkbox).toHaveAccessibleDescription('仅在任务完成时');
    await user.tab();
    await user.keyboard(' ');
    expect(checkbox).toBeChecked();
    expect(onChange).toHaveBeenCalledOnce();
    await user.click(screen.getByText('通知'));
    expect(checkbox).not.toBeChecked();

    const disabledChange = vi.fn();
    render(<Switch label="停用的通知" disabled onChange={disabledChange} />);
    await user.click(screen.getByText('停用的通知'));
    expect(screen.getByRole('checkbox', { name: '停用的通知' })).not.toBeChecked();
    expect(disabledChange).not.toHaveBeenCalled();
  });
});

describe('AngelBot UI dialogs', () => {
  it('portals above its parent surface without bubbling content clicks into that parent', async () => {
    const parentClick = vi.fn();
    const close = vi.fn();
    const { container } = render(
      <section onClick={parentClick} style={{ overflow: 'hidden', transform: 'translateX(0)' }}>
        <Dialog open title="独立弹窗" onClose={close}><Button>弹窗操作</Button></Dialog>
      </section>,
    );
    const dialog = screen.getByRole('dialog', { name: '独立弹窗' });
    expect(dialog.parentElement?.parentElement).toBe(document.body);
    expect(container).not.toContainElement(dialog);
    await waitFor(() => expect(dialog).toHaveFocus());
    fireEvent.click(screen.getByRole('button', { name: '弹窗操作' }));
    expect(parentClick).not.toHaveBeenCalled();
    expect(close).not.toHaveBeenCalled();
  });

  it('traps focus, preserves it across caller renders and restores the trigger on Escape', async () => {
    const user = userEvent.setup();
    function Harness() {
      const [open, setOpen] = useState(false);
      const [count, setCount] = useState(0);
      return (
        <>
          <Button onClick={() => setOpen(true)}>打开弹窗</Button>
          <Dialog open={open} title="任务确认" onClose={() => setOpen(false)} initialFocusSelector="[data-cancel]">
            <Button data-cancel>取消</Button>
            <Button onClick={() => setCount(count + 1)}>更新 {count}</Button>
          </Dialog>
        </>
      );
    }
    render(<Harness />);
    const trigger = screen.getByRole('button', { name: '打开弹窗' });
    await user.click(trigger);
    const dialog = screen.getByRole('dialog', { name: '任务确认' });
    const cancel = screen.getByRole('button', { name: '取消' });
    await waitFor(() => expect(cancel).toHaveFocus());
    expect(dialog).toHaveAttribute('aria-modal', 'true');
    await user.tab({ shift: true });
    const update = screen.getByRole('button', { name: '更新 0' });
    expect(update).toHaveFocus();
    await user.click(update);
    expect(screen.getByRole('button', { name: '更新 1' })).toHaveFocus();
    await user.tab();
    expect(cancel).toHaveFocus();
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it('only dismisses backdrop clicks and provides a focus target without controls', async () => {
    const close = vi.fn();
    render(<Dialog open title="查看详情" onClose={close}><p>工作内容</p></Dialog>);
    const dialog = screen.getByRole('dialog', { name: '查看详情' });
    await waitFor(() => expect(dialog).toHaveFocus());
    fireEvent.click(screen.getByText('工作内容'));
    expect(close).not.toHaveBeenCalled();
    fireEvent.keyDown(window, { key: 'Tab' });
    expect(dialog).toHaveFocus();
    fireEvent.click(dialog.parentElement!);
    expect(close).toHaveBeenCalledOnce();
  });

  it('retains alertdialog semantics and focuses safe cancel in destructive confirmations', async () => {
    const cancel = vi.fn();
    const confirm = vi.fn();
    const { rerender } = render(<ConfirmDialog open title="删除记录" body="此操作无法撤销" danger onCancel={cancel} onConfirm={confirm} />);
    const dialog = screen.getByRole('alertdialog', { name: '删除记录' });
    expect(dialog).toHaveAccessibleDescription('此操作无法撤销');
    await waitFor(() => expect(screen.getByRole('button', { name: '取消' })).toHaveFocus());
    expect(screen.getByRole('button', { name: '确认' })).toHaveClass('btn-danger');

    rerender(<ConfirmDialog open title="删除记录" body="此操作无法撤销" danger loading onCancel={cancel} onConfirm={confirm} />);
    expect(screen.getByRole('button', { name: '取消' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '处理中…' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '处理中…' })).toHaveAttribute('aria-busy', 'true');
    await waitFor(() => expect(dialog).toHaveFocus());
  });
});

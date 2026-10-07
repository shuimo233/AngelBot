import { useId } from 'react';
import { Button, Dialog } from '../../ui';
import './ConfirmDialog.css';

interface ConfirmDialogProps {
  open: boolean;
  title: string;
  body: string;
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
  loading?: boolean;
  /** 确认失败后显示在对话框内的错误信息 */
  error?: string | null;
  onConfirm: () => void;
  onCancel: () => void;
}

/**
 * 应用内确认对话框，替代原生 confirm/alert。
 * Esc 或点击遮罩取消；打开时焦点落在取消按钮上，避免误触危险操作。
 */
export function ConfirmDialog({
  open,
  title,
  body,
  confirmLabel = '确认',
  cancelLabel = '取消',
  danger,
  loading,
  error,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const bodyId = `confirm-dialog-body-${useId()}`;

  return (
    <Dialog
      open={open}
      title={title}
      onClose={onCancel}
      role="alertdialog"
      aria-describedby={bodyId}
      initialFocusSelector={loading ? undefined : '[data-confirm-cancel]'}
      overlayClassName="confirm-dialog-overlay"
      className="confirm-dialog"
      titleClassName="confirm-dialog-title"
    >
      <p className="confirm-dialog-body" id={bodyId}>{body}</p>
      {error && <p className="confirm-dialog-error" role="alert">{error}</p>}
      <div className="confirm-dialog-actions">
        <Button data-confirm-cancel variant="secondary" className="btn btn-secondary" onClick={onCancel} disabled={loading}>
          {cancelLabel}
        </Button>
        <Button
          variant={danger ? 'danger' : 'primary'}
          className={danger ? 'btn btn-danger' : 'btn btn-primary'}
          onClick={onConfirm}
          busy={loading}
        >
          {loading ? '处理中…' : confirmLabel}
        </Button>
      </div>
    </Dialog>
  );
}

import { useCallback, useId, useRef, type HTMLAttributes, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useDialogFocus } from '../lib/useDialogFocus';

export interface DialogProps extends Omit<HTMLAttributes<HTMLDivElement>, 'title' | 'role'> {
  open: boolean;
  title: ReactNode;
  onClose: () => void;
  role?: 'dialog' | 'alertdialog';
  /** A selector scoped to the dialog, for example a safe cancel action. */
  initialFocusSelector?: string;
  overlayClassName?: string;
  titleClassName?: string;
}

export function Dialog({
  open,
  title,
  onClose,
  role = 'dialog',
  initialFocusSelector,
  overlayClassName = '',
  titleClassName = '',
  className = '',
  children,
  ...props
}: DialogProps) {
  const titleId = `ui-dialog-title-${useId()}`;
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  const handleClose = useCallback(() => onCloseRef.current(), []);
  const dialogRef = useDialogFocus<HTMLDivElement>({
    isOpen: open,
    onClose: handleClose,
    initialFocusSelector,
  });

  if (!open) return null;

  return createPortal(
    <div
      className={`ui-dialog-overlay ${overlayClassName}`.trim()}
      onClick={(event) => {
        event.stopPropagation();
        if (event.target === event.currentTarget) handleClose();
      }}
    >
      <div
        {...props}
        ref={dialogRef}
        className={`ui-dialog ${className}`.trim()}
        role={role}
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
      >
        <h3 className={`ui-dialog-title ${titleClassName}`.trim()} id={titleId}>{title}</h3>
        {children}
      </div>
    </div>,
    document.body,
  );
}

import { useEffect, useRef } from 'react';

const FOCUSABLE_SELECTOR = [
  'button:not([disabled])',
  '[href]',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(', ');

interface DialogFocusOptions {
  isOpen: boolean;
  onClose: () => void;
  initialFocusSelector?: string;
}

/** Keeps keyboard focus inside a modal surface and restores its trigger. */
export function useDialogFocus<T extends HTMLElement>({
  isOpen,
  onClose,
  initialFocusSelector,
}: DialogFocusOptions) {
  const dialogRef = useRef<T>(null);
  const returnFocusRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!isOpen) return;

    returnFocusRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;

    const frame = requestAnimationFrame(() => {
      const dialog = dialogRef.current;
      const initialFocus = initialFocusSelector
        ? dialog?.querySelector<HTMLElement>(initialFocusSelector)
        : dialog;
      initialFocus?.focus({ preventScroll: true });
    });

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onClose();
        return;
      }

      const dialog = dialogRef.current;
      if (event.key !== 'Tab' || !dialog) return;

      const focusable = Array.from(dialog.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR));
      if (focusable.length === 0) {
        event.preventDefault();
        dialog.focus();
        return;
      }

      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const active = document.activeElement;
      if (event.shiftKey && (active === first || active === dialog || !dialog.contains(active))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (active === last || !dialog.contains(active))) {
        event.preventDefault();
        first.focus();
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener('keydown', handleKeyDown);
      returnFocusRef.current?.focus({ preventScroll: true });
    };
  }, [initialFocusSelector, isOpen, onClose]);

  return dialogRef;
}

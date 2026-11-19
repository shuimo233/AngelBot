import { useEffect, useRef } from 'react';

const FOCUSABLE_SELECTOR = [
  'button:not([disabled])',
  '[href]',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  'details > summary:first-of-type:not([tabindex="-1"])',
  '[tabindex]:not([tabindex="-1"])',
].join(', ');

const modalStack: HTMLElement[] = [];

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
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!isOpen || !dialog) return;
    modalStack.push(dialog);

    returnFocusRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;

    const frame = requestAnimationFrame(() => {
      if (modalStack.at(-1) !== dialog) return;
      const initialFocus = initialFocusSelector
        ? dialog.querySelector<HTMLElement>(initialFocusSelector)
        : dialog;
      (initialFocus ?? dialog).focus({ preventScroll: true });
    });

    const handleKeyDown = (event: KeyboardEvent) => {
      if (modalStack.at(-1) !== dialog || event.defaultPrevented) return;
      if (event.key === 'Escape') {
        event.preventDefault();
        onCloseRef.current();
        return;
      }

      if (event.key !== 'Tab') return;

      const focusable = Array.from(dialog.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR))
        .filter((element) => {
          if (element.closest('[hidden], [inert]')) return false;
          // Responsive navigation stays mounted; do not trap focus in its hidden branch.
          for (let ancestor: HTMLElement | null = element; ancestor; ancestor = ancestor.parentElement) {
            if (ancestor instanceof HTMLDetailsElement && !ancestor.open
              && !ancestor.querySelector(':scope > summary')?.contains(element)) return false;
            const style = getComputedStyle(ancestor);
            if (style.display === 'none' || style.visibility === 'hidden') return false;
            if (ancestor === dialog) break;
          }
          return true;
        });
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
      const wasTop = modalStack.at(-1) === dialog;
      modalStack.splice(modalStack.indexOf(dialog), 1);
      if (wasTop) returnFocusRef.current?.focus({ preventScroll: true });
    };
  }, [initialFocusSelector, isOpen]);

  return dialogRef;
}

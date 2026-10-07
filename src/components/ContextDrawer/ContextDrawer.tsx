import { useDialogFocus } from '$lib/useDialogFocus';
import { ContextDrawerContent } from './ContextDrawerContent';
import './ContextDrawer.css';

interface ContextDrawerProps {
  isOpen: boolean;
  onClose: () => void;
}

/**
 * A keyboard-accessible drawer that explains the current conversation round:
 * model, context usage, recalled memories, read materials, tools, and run summary.
 * It stays out of the way until the user explicitly opens it.
 */
export function ContextDrawer({ isOpen, onClose }: ContextDrawerProps) {
  const drawerRef = useDialogFocus<HTMLDivElement>({ isOpen, onClose });

  if (!isOpen) return null;

  return (
    <div className="context-drawer-layer" role="presentation">
      <button className="context-drawer-scrim" aria-label="关闭上下文面板" onClick={onClose} />
      <div
        ref={drawerRef}
        className="context-drawer"
        role="dialog"
        aria-modal="true"
        aria-label="本轮上下文"
        tabIndex={-1}
      >
        <ContextDrawerContent onClose={onClose} />
      </div>
    </div>
  );
}

import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { useThemeStore } from '$stores/theme';

const MIN_WIDTH = 280;
const MAX_WIDTH = 760;
const DEFAULT_WIDTH = 336;
const WIDTH_KEY = 'workbenchWidth';
const COMPACT_QUERY = '(max-width: 1050px)';

export function useCompactWorkbench() {
  const [compact, setCompact] = useState(() => window.matchMedia?.(COMPACT_QUERY)?.matches ?? false);
  useEffect(() => {
    const query = window.matchMedia?.(COMPACT_QUERY);
    if (!query) return;
    const update = () => setCompact(query.matches);
    update();
    query.addEventListener?.('change', update);
    return () => query.removeEventListener?.('change', update);
  }, []);
  return compact;
}

function loadWidth() {
  try {
    const width = Number(localStorage.getItem(WIDTH_KEY) ?? DEFAULT_WIDTH);
    if (Number.isFinite(width)) return Math.round(Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, width)));
  } catch { /* The default still works without local storage. */ }
  return DEFAULT_WIDTH;
}

export function WorkbenchFrame({ children, compact }: { children: ReactNode; compact: boolean }) {
  const layout = useThemeStore((state) => state.layout);
  const panel = useRef<HTMLElement>(null);
  const [preferredWidth, setPreferredWidth] = useState(loadWidth);
  const [maximum, setMaximum] = useState(MAX_WIDTH);
  const [resizing, setResizing] = useState(false);
  const drag = useRef<{ id: number; startX: number; startWidth: number; preferred: number; width: number } | null>(null);
  const width = Math.min(preferredWidth, maximum);

  useEffect(() => {
    function measureLimit() {
      if (!panel.current) return;
      const limit = parseFloat(getComputedStyle(panel.current).maxWidth);
      setMaximum(Number.isFinite(limit) ? Math.floor(limit) : MAX_WIDTH);
    }
    measureLimit();
    window.addEventListener('resize', measureLimit);
    return () => window.removeEventListener('resize', measureLimit);
  }, [layout]);

  function saveWidth(next: number) {
    setPreferredWidth(next);
    try { localStorage.setItem(WIDTH_KEY, String(next)); }
    catch { /* Keep the new size for this open panel. */ }
  }

  function endDrag(pointerId: number) {
    const active = drag.current;
    if (active?.id !== pointerId) return;
    drag.current = null;
    setResizing(false);
    if (active.width === active.startWidth) setPreferredWidth(active.preferred);
    else saveWidth(active.width);
  }

  return (
    <aside ref={panel} id="conversation-workbench" className="right-panel" aria-label="工作侧栏"
      style={{
        '--right-panel-width': `${preferredWidth}px`,
        '--workbench-width-floor': `${MIN_WIDTH}px`,
        '--workbench-width-ceiling': `${MAX_WIDTH}px`,
      } as CSSProperties}>
      {!compact && <div className="workbench-resize-handle" role="separator" tabIndex={0}
        aria-label="调整工作台宽度" aria-orientation="vertical" aria-controls="conversation-workbench"
        aria-valuemin={MIN_WIDTH} aria-valuemax={maximum} aria-valuenow={width} aria-valuetext={`${width} 像素`}
        title="拖动调整宽度，双击恢复默认" data-resizing={resizing || undefined}
        onPointerDown={(event) => {
          if (event.button !== 0 || drag.current) return;
          event.currentTarget.setPointerCapture(event.pointerId);
          drag.current = { id: event.pointerId, startX: event.clientX, startWidth: width, preferred: preferredWidth, width };
          setResizing(true);
        }}
        onPointerMove={(event) => {
          const active = drag.current;
          if (active?.id !== event.pointerId) return;
          const next = Math.round(active.startWidth + active.startX - event.clientX);
          active.width = Math.max(MIN_WIDTH, Math.min(maximum, next));
          setPreferredWidth(active.width);
        }}
        onPointerUp={(event) => endDrag(event.pointerId)}
        onPointerCancel={(event) => endDrag(event.pointerId)}
        onLostPointerCapture={(event) => endDrag(event.pointerId)}
        onDoubleClick={() => saveWidth(DEFAULT_WIDTH)}
        onKeyDown={(event) => {
          const next = event.key === 'ArrowLeft' ? width + 16 : event.key === 'ArrowRight' ? width - 16
            : event.key === 'Home' ? MIN_WIDTH : event.key === 'End' ? maximum : null;
          if (next === null) return;
          event.preventDefault();
          const bounded = Math.max(MIN_WIDTH, Math.min(maximum, next));
          if (bounded !== width) saveWidth(bounded);
        }} />}
      {children}
    </aside>
  );
}

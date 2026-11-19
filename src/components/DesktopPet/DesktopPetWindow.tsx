import { useEffect, useRef, useState, useSyncExternalStore, type PointerEvent } from 'react';
import mascot from '../../../artwork/mascot/sd/default-transparent.png';
import { hideDesktopPet, openMainFromDesktopPet, startDesktopPetDrag } from '$lib/commands/desktop-pet';
import { readReducedMotionPreference } from '$stores/appearance';
import './DesktopPetWindow.css';

const DRAG_DISTANCE = 5;
const REACTION_DURATION_MS = 1050;

function subscribeToAppearance(onChange: () => void) {
  window.addEventListener('storage', onChange);
  return () => window.removeEventListener('storage', onChange);
}

export function DesktopPetWindow() {
  const origin = useRef<{ id: number; x: number; y: number } | null>(null);
  const didDrag = useRef(false);
  const pats = useRef(0);
  const [reaction, setReaction] = useState(0);
  const [error, setError] = useState('');
  const reduceMotion = useSyncExternalStore(subscribeToAppearance, readReducedMotionPreference);

  useEffect(() => {
    if (!reaction) return;
    const timeout = window.setTimeout(() => setReaction(0), REACTION_DURATION_MS);
    return () => window.clearTimeout(timeout);
  }, [reaction]);

  const start = (event: PointerEvent<HTMLButtonElement>) => {
    if (!event.isPrimary || event.button !== 0) return;
    origin.current = { id: event.pointerId, x: event.clientX, y: event.clientY };
    didDrag.current = false;
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const move = (event: PointerEvent<HTMLButtonElement>) => {
    const point = origin.current;
    if (!point) {
      if (event.pointerType === 'mouse' && event.buttons === 0 && !reduceMotion) {
        const { left, width } = event.currentTarget.getBoundingClientRect();
        const offset = Math.max(-1, Math.min(1, (event.clientX - left) / (width / 2) - 1));
        event.currentTarget.style.setProperty('--pet-tilt', `${offset * 4}deg`);
      }
      return;
    }
    if (event.pointerId !== point.id || (event.buttons & 1) === 0) return;
    if (Math.hypot(event.clientX - point.x, event.clientY - point.y) < DRAG_DISTANCE) return;
    origin.current = null;
    didDrag.current = true;
    event.currentTarget.releasePointerCapture(event.pointerId);
    event.currentTarget.style.removeProperty('--pet-tilt');
    setError('');
    void startDesktopPetDrag().catch(() => setError('无法拖动桌宠'));
  };

  return <main className="desktop-pet-window" data-reduced-motion={reduceMotion || undefined}
    data-reaction={reaction ? (reaction % 2 === 1 ? 'hop' : 'wiggle') : undefined}>
    <button type="button" className="desktop-pet-character" aria-label="打开 AngelBot 对话"
      title="点击打开对话，拖动移动桌宠"
      onPointerDown={start} onPointerMove={move}
      onPointerLeave={event => event.currentTarget.style.removeProperty('--pet-tilt')}
      onPointerUp={() => { origin.current = null; }}
      onLostPointerCapture={() => { origin.current = null; }}
      onPointerCancel={() => { origin.current = null; }}
      onClick={(event) => {
        if (event.detail !== 0 && didDrag.current) return;
        setError('');
        void openMainFromDesktopPet().catch(() => setError('无法打开对话'));
      }}>
      <span className="desktop-pet-pose"><img src={mascot} alt="" draggable={false} /></span>
    </button>
    {reaction > 0 && <span key={reaction} className="desktop-pet-hearts" aria-hidden="true"><span>♥</span><span>♥</span></span>}
    <div className="desktop-pet-actions">
      <button type="button" aria-label="摸摸头" title="摸摸头" onClick={() => setReaction(++pats.current)}>
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 13.4 3.1 8.6a2.8 2.8 0 0 1 4-4L8 5.5l.9-.9a2.8 2.8 0 0 1 4 4Z" /></svg>
      </button>
      <button type="button" aria-label="隐藏桌宠" title="隐藏桌宠" onClick={() => {
        setError('');
        void hideDesktopPet().catch(() => setError('无法隐藏桌宠'));
      }}>
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4 4 8 8m0-8-8 8" /></svg>
      </button>
    </div>
    {error && <p className="desktop-pet-error" role="alert">{error}</p>}
  </main>;
}

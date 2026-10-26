import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { isTauriRuntime } from './invoke';

/** One refresh contract shared by the scheduler and automation projections. */
export const AUTOMATIONS_UPDATED_EVENT = 'angelbot:automations-updated';

/** Refresh only; this subscription never schedules or retries an operation. */
export function subscribeAutomationUpdates(refresh: () => void): () => void {
  let disposed = false;
  let unlisten: UnlistenFn | undefined;
  const changed = () => { if (!disposed) refresh(); };
  window.addEventListener(AUTOMATIONS_UPDATED_EVENT, changed);

  if (isTauriRuntime()) {
    void listen(AUTOMATIONS_UPDATED_EVENT, changed).then(stop => {
      if (disposed) {
        stop();
      } else {
        unlisten = stop;
        // A receipt may have changed while native registration was pending.
        changed();
      }
    }).catch(() => {
      // Browser/focus refresh paths remain available; subscriptions never
      // own the desktop host's automation clock.
    });
  }

  return () => {
    if (disposed) return;
    disposed = true;
    window.removeEventListener(AUTOMATIONS_UPDATED_EVENT, changed);
    unlisten?.();
  };
}

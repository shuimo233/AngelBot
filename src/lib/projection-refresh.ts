/** Order snapshot reads within one scope; late results from a replaced scope cannot publish. */
export function createProjectionRefresh<Key, Snapshot>({
  read, onSuccess, onError, onStart, onIdle,
}: {
  read: (key: Key) => Promise<Snapshot>;
  onSuccess: (key: Key, snapshot: Snapshot) => void;
  onError: (error: unknown) => void;
  onStart?: (key: Key) => void;
  onIdle?: () => void;
}) {
  type Request = { key: Key; started: boolean; dirty: boolean; done: Promise<void> };
  let active: Request | null = null;

  return {
    request(key: Key): Promise<void> {
      if (active && active.key === key) {
        if (active.started) active.dirty = true;
        return active.done;
      }

      const next: Request = {
        key, started: false, dirty: false,
        done: Promise.resolve().then(async () => {
          if (active !== next) return;
          next.started = true;
          try {
            do {
              next.dirty = false;
              try {
                const snapshot = await read(key);
                if (active === next) onSuccess(key, snapshot);
              } catch (error) {
                if (active === next) onError(error);
              }
            } while (active === next && next.dirty);
          } finally {
            if (active === next) {
              active = null;
              onIdle?.();
            }
          }
        }),
      };
      active = next;
      onStart?.(key);
      return next.done;
    },
    isPending: () => active !== null,
    // Read-only work need not be cancelled; release it without publishing.
    invalidate() { active = null; },
  };
}

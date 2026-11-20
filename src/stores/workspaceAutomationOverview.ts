import { useCallback, useSyncExternalStore } from 'react';
import { subscribeAutomationUpdates } from '$lib/automation-events';
import { getWorkspaceAutomationOverview, type WorkspaceAutomationOverview } from '$lib/commands/automation';
import { createProjectionRefresh } from '$lib/projection-refresh';

type Snapshot = { overview: WorkspaceAutomationOverview | null; failed: boolean };
const empty: Snapshot = { overview: null, failed: false };

function observe(workspaceId: string) {
  let snapshot = empty;
  const listeners = new Set<() => void>();
  const publish = (value: Snapshot) => {
    snapshot = value;
    listeners.forEach(listener => listener());
  };
  const reads = createProjectionRefresh({
    read: getWorkspaceAutomationOverview,
    onSuccess: (_, overview) => publish({ overview, failed: false }),
    // Keep the last confirmed snapshot on a failed refresh.
    onError: () => publish({ ...snapshot, failed: true }),
  });
  const refresh = () => { void reads.request(workspaceId); };
  const unsubscribe = subscribeAutomationUpdates(refresh);
  window.addEventListener('focus', refresh);
  refresh();
  return {
    listeners, refresh, snapshot: () => snapshot,
    dispose: () => {
      reads.invalidate();
      unsubscribe();
      window.removeEventListener('focus', refresh);
    },
  };
}

const observations = new Map<string, ReturnType<typeof observe>>();

/** Share one event subscription and read for all mounted views of a workspace. */
export function useWorkspaceAutomationOverview(workspaceId: string | null | undefined) {
  const subscribe = useCallback((listener: () => void) => {
    if (!workspaceId) return () => {};
    let observation = observations.get(workspaceId);
    if (!observation) {
      observation = observe(workspaceId);
      observations.set(workspaceId, observation);
    }
    const owner = observation;
    owner.listeners.add(listener);
    return () => {
      owner.listeners.delete(listener);
      if (owner.listeners.size || observations.get(workspaceId) !== owner) return;
      observations.delete(workspaceId);
      owner.dispose();
    };
  }, [workspaceId]);
  const getSnapshot = useCallback(() => (
    workspaceId ? observations.get(workspaceId)?.snapshot() ?? empty : empty
  ), [workspaceId]);
  const refresh = () => { if (workspaceId) observations.get(workspaceId)?.refresh(); };
  return { ...useSyncExternalStore(subscribe, getSnapshot), refresh };
}

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { getWorkspaceActivity, type WorkspaceActivityProjection } from '$lib/commands/workspace-activity';
import { useWorkspaceActivityStore } from './workspaceActivity';
import type { AgentEvent } from '$lib/commands/agent-event';

const native = vi.hoisted(() => ({ listen: vi.fn(), stop: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: native.listen }));

vi.mock('$lib/commands/workspace-activity', () => ({ getWorkspaceActivity: vi.fn() }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function snapshot(workspaceId: string, cursor: number): WorkspaceActivityProjection {
  return { workspaceId, cursor, work: [], pendingDecisions: [], attention: null };
}

function work(workspaceId: string, cursor: number, status: 'running' | 'needsDecision' | 'completed'): WorkspaceActivityProjection {
  return { ...snapshot(workspaceId, cursor), work: [{ id: 'work-1', goal: '检查项目', status,
    summary: null, keyFacts: [], milestonesCompleted: 0, milestonesTotal: 0,
    verificationsPassed: 0, verificationsTotal: 0, evidenceCount: 0,
    activity: null, reviewerStatus: null, updatedAt: cursor }] };
}

describe('workspace activity ownership', () => {
  const listeners = new Map<string, (event: { payload: AgentEvent }) => void>();
  beforeEach(() => {
    vi.useFakeTimers();
    useWorkspaceActivityStore.getState().clear();
    vi.clearAllMocks();
    vi.mocked(getWorkspaceActivity).mockReset().mockImplementation(async (id) => snapshot(id, 0));
    listeners.clear();
    native.listen.mockReset().mockImplementation(async (name: string, callback: (event: { payload: AgentEvent }) => void) => {
      listeners.set(name, callback);
      return native.stop;
    });
  });
  afterEach(() => {
    useWorkspaceActivityStore.getState().clear();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it('does not let an old A response replace A after switching A → B → A', async () => {
    const oldA = deferred<WorkspaceActivityProjection>();
    vi.mocked(getWorkspaceActivity).mockReturnValueOnce(oldA.promise)
      .mockResolvedValueOnce(snapshot('B', 2)).mockResolvedValueOnce(snapshot('A', 3));
    const oldRead = useWorkspaceActivityStore.getState().refresh('A');
    await Promise.resolve(); // A is actually in flight before navigation.
    await useWorkspaceActivityStore.getState().refresh('B');
    await useWorkspaceActivityStore.getState().refresh('A');
    oldA.resolve(snapshot('A', 1));
    await oldRead;
    expect(useWorkspaceActivityStore.getState().projection?.cursor).toBe(3);
  });

  it('rejects a mismatched workspace response instead of displaying it as success', async () => {
    vi.mocked(getWorkspaceActivity).mockResolvedValue(snapshot('B', 1));
    await useWorkspaceActivityStore.getState().refresh('A');
    expect(useWorkspaceActivityStore.getState().projection).toBeNull();
    expect(useWorkspaceActivityStore.getState().error).toBeTruthy();
  });

  it('shares one read, transport and active-work timer across the capsule and workbench leases', async () => {
    vi.mocked(getWorkspaceActivity).mockResolvedValueOnce(work('A', 1, 'running'))
      .mockResolvedValueOnce(work('A', 2, 'completed'));
    const releaseCapsule = useWorkspaceActivityStore.getState().observe('A', 'main-A');
    const releasePanel = useWorkspaceActivityStore.getState().observe('A', 'main-A');
    await vi.advanceTimersByTimeAsync(0);
    expect(native.listen).toHaveBeenCalledTimes(1);
    expect(getWorkspaceActivity).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(3000);
    expect(getWorkspaceActivity).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(9000);
    expect(getWorkspaceActivity).toHaveBeenCalledTimes(2);
    releaseCapsule();
    expect(native.stop).not.toHaveBeenCalled();
    releasePanel();
    releasePanel();
    expect(native.stop).toHaveBeenCalledTimes(1);
    window.dispatchEvent(new Event('focus'));
    await vi.advanceTimersByTimeAsync(3000);
    expect(getWorkspaceActivity).toHaveBeenCalledTimes(2);
  });

  it('discovers new work from an initially empty native Main without an idle polling loop', async () => {
    useWorkspaceActivityStore.getState().observe('A', 'main-A');
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(9000);
    expect(getWorkspaceActivity).toHaveBeenCalledTimes(1);
    vi.mocked(getWorkspaceActivity).mockResolvedValueOnce(work('A', 1, 'running'));
    listeners.get('agent_event_main-A')?.({ payload: { type: 'ToolExecutionEnd', data: {} } });
    await vi.advanceTimersByTimeAsync(0);
    expect(useWorkspaceActivityStore.getState().projection?.work[0].status).toBe('running');
    expect(getWorkspaceActivity).toHaveBeenCalledTimes(2);
    // The existing Main bridge must not duplicate this native hint.
    useWorkspaceActivityStore.getState().invalidate('main-A');
    await vi.advanceTimersByTimeAsync(0);
    expect(getWorkspaceActivity).toHaveBeenCalledTimes(2);
  });

  it('uses existing Main fallback hints without trusting another session or creating idle polling', async () => {
    native.listen.mockRejectedValueOnce(new Error('No native bridge'));
    useWorkspaceActivityStore.getState().observe('A', 'main-A');
    await vi.advanceTimersByTimeAsync(9000);
    expect(getWorkspaceActivity).toHaveBeenCalledTimes(1);
    useWorkspaceActivityStore.getState().invalidate('main-B');
    await vi.advanceTimersByTimeAsync(0);
    expect(getWorkspaceActivity).toHaveBeenCalledTimes(1);
    vi.mocked(getWorkspaceActivity).mockResolvedValueOnce(work('A', 1, 'running'));
    useWorkspaceActivityStore.getState().invalidate('main-A');
    await vi.advanceTimersByTimeAsync(0);
    expect(useWorkspaceActivityStore.getState().projection?.work[0].status).toBe('running');
  });

  it('drops an old same-workspace Main response and isolates old cleanup from the new lease', async () => {
    const old = deferred<WorkspaceActivityProjection>();
    vi.mocked(getWorkspaceActivity).mockReturnValueOnce(old.promise).mockResolvedValueOnce(snapshot('A', 2));
    const releaseOld = useWorkspaceActivityStore.getState().observe('A', 'old-main');
    await vi.advanceTimersByTimeAsync(0);
    const releaseNew = useWorkspaceActivityStore.getState().observe('A', 'new-main');
    await vi.advanceTimersByTimeAsync(0);
    old.resolve(snapshot('A', 1));
    await vi.advanceTimersByTimeAsync(0);
    releaseOld();
    expect(useWorkspaceActivityStore.getState().sessionId).toBe('new-main');
    expect(useWorkspaceActivityStore.getState().projection?.cursor).toBe(2);
    expect(native.stop).toHaveBeenCalledTimes(1);
    releaseNew();
    expect(native.stop).toHaveBeenCalledTimes(2);
  });

  it('coalesces in-flight hints into one trailing read and retains the last good snapshot on failure', async () => {
    const first = deferred<WorkspaceActivityProjection>();
    vi.mocked(getWorkspaceActivity).mockReturnValueOnce(first.promise).mockResolvedValueOnce(snapshot('A', 2));
    useWorkspaceActivityStore.getState().observe('A', 'main-A');
    await vi.advanceTimersByTimeAsync(0);
    for (let i = 0; i < 3; i += 1) listeners.get('agent_event_main-A')?.({ payload: { type: 'ToolExecutionEnd', data: {} } });
    expect(getWorkspaceActivity).toHaveBeenCalledTimes(1);
    first.resolve(snapshot('A', 1));
    await vi.advanceTimersByTimeAsync(0);
    expect(getWorkspaceActivity).toHaveBeenCalledTimes(2);
    vi.mocked(getWorkspaceActivity).mockRejectedValueOnce(new Error('读取失败'));
    await useWorkspaceActivityStore.getState().refresh('A');
    expect(useWorkspaceActivityStore.getState().projection?.cursor).toBe(2);
    expect(useWorkspaceActivityStore.getState().error).toBe('读取失败');
  });

  it('does no background polling while hidden or waiting for a user decision, and refreshes on return', async () => {
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
    vi.mocked(getWorkspaceActivity).mockResolvedValueOnce(work('A', 1, 'running'))
      .mockResolvedValueOnce(work('A', 2, 'needsDecision'));
    useWorkspaceActivityStore.getState().observe('A', 'main-A');
    await vi.advanceTimersByTimeAsync(0);
    visibility.mockReturnValue('hidden');
    document.dispatchEvent(new Event('visibilitychange'));
    listeners.get('agent_event_main-A')?.({ payload: { type: 'AgentEnd', data: {} } });
    await vi.advanceTimersByTimeAsync(9000);
    expect(getWorkspaceActivity).toHaveBeenCalledTimes(1);
    visibility.mockReturnValue('visible');
    document.dispatchEvent(new Event('visibilitychange'));
    await vi.advanceTimersByTimeAsync(9000);
    expect(getWorkspaceActivity).toHaveBeenCalledTimes(2);
    window.dispatchEvent(new Event('focus'));
    await vi.advanceTimersByTimeAsync(0);
    expect(getWorkspaceActivity).toHaveBeenCalledTimes(3);
  });

  it('releases late native registration without reading after its last consumer closes', async () => {
    const registration = deferred<() => void>();
    native.listen.mockReturnValueOnce(registration.promise);
    const release = useWorkspaceActivityStore.getState().observe('A', 'main-A');
    release();
    registration.resolve(native.stop);
    await vi.advanceTimersByTimeAsync(0);
    expect(native.stop).toHaveBeenCalledTimes(1);
    expect(getWorkspaceActivity).not.toHaveBeenCalled();
  });
});

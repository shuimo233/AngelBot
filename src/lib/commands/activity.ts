import { invoke } from '../invoke';

export type ActivityKind = 'task_result' | 'memory_change';

export interface ActivityEntry {
  id: string;
  kind: ActivityKind;
  title: string;
  detail: string | null;
  status: string | null;
  occurred_at: number;
}

/** Server-side filtering excludes hidden adaptive-learning state. */
export function getActivity(limit = 80): Promise<ActivityEntry[]> {
  return invoke<ActivityEntry[]>('get_activity', { limit });
}

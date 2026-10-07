import { invoke } from '$lib/invoke';

export interface RuntimeHealth {
  status: 'ready' | 'degraded';
  delegationAvailable: boolean;
  issues: RuntimeHealthIssue[];
}

export interface RuntimeHealthIssue {
  code:
    | 'ephemeral_storage'
    | 'semantic_memory_unavailable'
    | 'settings_defaults'
    | 'delegation_disabled'
    | 'delegation_unavailable';
  severity: 'notice' | 'warning' | 'critical';
  title: string;
  detail: string;
  recoveryAction: 'none' | 'restart';
}

export function getRuntimeHealth(): Promise<RuntimeHealth> {
  return invoke<RuntimeHealth>('get_runtime_health');
}

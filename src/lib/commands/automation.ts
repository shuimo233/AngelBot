import { invoke } from '$lib/invoke';

export interface Automation { id:string; title:string; prompt:string; triggerKind:string; triggerValue:string; enabled:boolean; permissionSummary:string; executorKind:string; workspaceId?:string; scriptPath?:string; scriptArgs:string[]; workingDir?:string; timeoutSeconds:number; nextRunAt?:number; lastRunAt?:number; }
export type AutomationNotificationStatus = 'unconfirmed' | 'requested' | 'muted' | 'unavailable';
export interface AutomationRunSummary { id:string; status:string; summary:string; exitCode?:number|null; startedAt:number; stopControl?:'available'|'requested'; notificationStatus?: AutomationNotificationStatus; }
export interface AutomationRun extends AutomationRunSummary { output:string; }
export interface AutomationDashboardItem extends Automation { latestRun:AutomationRunSummary|null; lastSkippedAt:number|null; }
export interface AutomationDashboard { items:AutomationDashboardItem[]; }
export const getAutomationDashboard = () => invoke<AutomationDashboard>('get_automation_dashboard');
export const getAutomations = () => invoke<Automation[]>('get_automations');
export interface WorkspaceAutomationStatus {
  id: string;
  title: string;
  workspaceId: string;
  executorKind: string;
  triggerKind: string;
  triggerValue: string;
  enabled: boolean;
  nextRunAt: number | null;
  lastRunAt: number | null;
  latestRun: AutomationRunSummary | null;
  lastSkippedAt: number | null;
  maaConfig: {
    appId: string;
    appDisplayName: string | null;
    launchEmulator: boolean;
    timeoutSeconds: number;
  } | null;
}
export interface WorkspaceAutomationOverview { items: WorkspaceAutomationStatus[]; truncated: boolean; }
export const getWorkspaceAutomationOverview = (workspaceId: string) =>
  invoke<WorkspaceAutomationOverview>('get_workspace_automation_overview', { workspaceId });
export type CreateAutomationInput = Pick<Automation,'title'|'prompt'|'triggerKind'|'triggerValue'|'permissionSummary'|'executorKind'|'workspaceId'|'scriptPath'|'scriptArgs'|'workingDir'|'timeoutSeconds'>;
export const createAutomation = (input: CreateAutomationInput) => invoke<Automation>('create_automation', input);
export interface CreateMaaAutomationInput { title:string; triggerValue:string; workspaceId:string; appId:string; launchEmulator:boolean; timeoutSeconds?:number; }
export const createMaaAutomation = (input: CreateMaaAutomationInput) => invoke<Automation>('create_maa_automation', { ...input });
export const setAutomationEnabled = (id:string, enabled:boolean) => invoke<void>('set_automation_enabled',{id,enabled});
export const deleteAutomation = (id:string) => invoke<void>('delete_automation',{id});
export const runAutomationNow = (id:string) => invoke<void>('run_automation_now',{id});
export const stopAutomationRun = (automationId:string, runId:string) => invoke<void>('stop_automation_run',{id:automationId,runId});
/** Explicit scan command; periodic scans are owned by the desktop host. */
export const runDueAutomations = () => invoke<number>('run_due_automations');
export const getAutomationRuns = (automationId:string) => invoke<AutomationRun[]>('get_automation_runs',{automationId});

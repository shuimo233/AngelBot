import { invoke } from '$lib/invoke';

export interface Automation { id:string; title:string; prompt:string; triggerKind:string; triggerValue:string; enabled:boolean; permissionSummary:string; executorKind:string; workspaceId?:string; scriptPath?:string; scriptArgs:string[]; workingDir?:string; timeoutSeconds:number; nextRunAt?:number; lastRunAt?:number; }
export interface AutomationRun { id:string; status:string; summary:string; exitCode?:number; output:string; startedAt:number; }
export const getAutomations = () => invoke<Automation[]>('get_automations');
export type CreateAutomationInput = Pick<Automation,'title'|'prompt'|'triggerKind'|'triggerValue'|'permissionSummary'|'executorKind'|'workspaceId'|'scriptPath'|'scriptArgs'|'workingDir'|'timeoutSeconds'>;
export const createAutomation = (input: CreateAutomationInput) => invoke<Automation>('create_automation', input);
export const setAutomationEnabled = (id:string, enabled:boolean) => invoke<void>('set_automation_enabled',{id,enabled});
export const deleteAutomation = (id:string) => invoke<void>('delete_automation',{id});
export const runAutomationNow = (id:string) => invoke<void>('run_automation_now',{id});
/** Desktop-lifecycle heartbeat; due items are claimed only while AngelBot is running. */
export const runDueAutomations = () => invoke<number>('run_due_automations');
export const getAutomationRuns = (automationId:string) => invoke<AutomationRun[]>('get_automation_runs',{automationId});

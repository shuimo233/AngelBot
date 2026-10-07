/**
 * Settings commands - typed wrappers around Tauri IPC
 */
import { invoke } from '../invoke';

export interface ScheduledTask {
  id: string;
  type: string;
  trigger_at: string;
  content: string;
  enabled: boolean;
}

export interface EnvProviderInfo {
  provider: string;
  model: string;
  base_url: string;
  has_api_key: boolean;
  is_local: boolean;
  cost_tier: string;
}

export interface PersonalityMatchResult {
  direction_id: string;
  direction_name: string;
  direction_avatar: string;
  suggested_traits: string;
  suggested_description: string;
  suggested_greeting: string;
}

export interface NotificationSettings {
  chat_reply: boolean;
  reminder: boolean;
  task_complete: boolean;
  file_change: boolean;
  system: boolean;
}

export type AgentExecutionPermission = 'ask' | 'workspace_auto' | 'full_access';

export async function getAgentExecutionPermission(): Promise<AgentExecutionPermission> {
  return invoke<AgentExecutionPermission>('get_agent_execution_permission');
}

export async function setAgentExecutionPermission(
  permission: AgentExecutionPermission,
): Promise<AgentExecutionPermission> {
  return invoke<AgentExecutionPermission>('set_agent_execution_permission', { permission, permanent: true });
}

export async function getScheduledTasks(): Promise<ScheduledTask[]> {
  return invoke<ScheduledTask[]>('get_scheduled_tasks');
}

export async function saveScheduledTask(task: ScheduledTask): Promise<void> {
  return invoke<void>('save_scheduled_task', { task });
}

export async function deleteScheduledTask(id: string): Promise<void> {
  return invoke<void>('delete_scheduled_task', { id });
}

export async function getEnvConfig(): Promise<EnvProviderInfo | null> {
  return invoke<EnvProviderInfo | null>('get_env_config');
}

export async function matchPersonalityDirection(description: string): Promise<PersonalityMatchResult> {
  return invoke<PersonalityMatchResult>('match_personality_direction', { description });
}

export interface ApiProviderConfig {
  provider: string;
  model: string;
  base_url: string;
  api_key: string;
  max_tokens: number;
  temperature: number;
  has_api_key?: boolean;
  credential_source?: 'none' | 'keychain' | 'environment';
  protocol?: 'openai_chat_completions' | 'openai_responses' | 'anthropic_messages';
  auth_mode?: 'api_key' | 'none' | 'chatgpt_plan';
  credential_ref?: string | null;
  plan_connected?: boolean;
  plan_enabled?: boolean;
  plan_account_label?: string | null;
}

export interface ChatgptPlanStatus {
  connected: boolean;
  planEnabled: boolean;
  needsSignIn: boolean;
  accountLabel: string | null;
  credentialRef: string | null;
}

export interface ChatgptPlanModel { id: string; name: string }

export const chatgptPlanStatus = () => invoke<ChatgptPlanStatus>('chatgpt_plan_status');
export const chatgptPlanSignIn = () => invoke<ChatgptPlanStatus>('chatgpt_plan_sign_in');
export const chatgptPlanChangeAccount = () => invoke<ChatgptPlanStatus>('chatgpt_plan_change_account');
export const chatgptPlanCancelSignIn = () => invoke<void>('chatgpt_plan_cancel_sign_in');
export const chatgptPlanDisconnect = () => invoke<ChatgptPlanStatus>('chatgpt_plan_disconnect');
export const chatgptPlanModels = () => invoke<ChatgptPlanModel[]>('chatgpt_plan_models');
export const testModelConnection = (config: ApiProviderConfig) => invoke<boolean>('test_model_connection', { config });

export async function loadApiConfig(): Promise<ApiProviderConfig> {
  return invoke<ApiProviderConfig>('load_api_config');
}

export async function saveApiConfig(config: ApiProviderConfig): Promise<void> {
  return invoke<void>('save_api_config', { config });
}

export async function getNotificationSettings(): Promise<NotificationSettings> {
  return invoke<NotificationSettings>('get_notification_settings');
}

export async function saveNotificationSettings(settings: NotificationSettings): Promise<void> {
  return invoke<void>('save_notification_settings', { settings });
}

export async function sendNotificationChannel(
  channel: string,
  title: string,
  body: string
): Promise<boolean> {
  return invoke<boolean>('send_notification_channel', { channel, title, body });
}

export async function exportData(): Promise<string> {
  return invoke<string>('export_data');
}

export async function exportEncryptedData(password: string): Promise<string> {
  return invoke<string>('export_encrypted_data', { password });
}

export async function importData(data: string): Promise<void> {
  return invoke<void>('import_data', { data });
}

export async function importEncryptedData(data: string, password: string): Promise<void> {
  return invoke<void>('import_encrypted_data', { data, password });
}

export async function clearAllUserData(): Promise<void> {
  return invoke<void>('clear_all_user_data');
}

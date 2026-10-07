import { invoke } from '../invoke';

export type DesktopCapability = 'launch' | 'observe' | 'observeImage' | 'draft' | 'fill' | 'interact';
export type DesktopControlAction = 'invoke' | 'select' | 'expand' | 'collapse' | 'scrollup' | 'scrolldown';
export type DesktopActionOperation = 'draft' | 'field' | DesktopControlAction;

export interface TrustedDesktopApp {
  id: string;
  displayName: string;
  executablePath: string;
  capabilities: DesktopCapability[];
  draftSelector: string | null;
  enabled: boolean;
  createdAt: number;
  updatedAt: number;
}

export type TrustedDesktopAppRuntimeStatus = 'available' | 'disabled' | 'executableUnavailable';

export interface TrustedDesktopAppStatus {
  id: string;
  status: TrustedDesktopAppRuntimeStatus;
}

export interface DesktopDraftTargetCandidate {
  name: string;
  automationId?: string;
}

export interface DesktopDraftDiscovery {
  appId: string;
  candidates: DesktopDraftTargetCandidate[];
}

export const getDesktopTrustedApps = () => invoke<TrustedDesktopApp[]>('get_desktop_trusted_apps');
export const getDesktopTrustedAppStatuses = () => invoke<TrustedDesktopAppStatus[]>('get_desktop_trusted_app_statuses');
export const inspectDesktopDraftTargets = (appId: string) => invoke<DesktopDraftDiscovery>('inspect_desktop_draft_targets', { appId });
export const saveDesktopTrustedApp = (app: TrustedDesktopApp) => invoke<TrustedDesktopApp>('save_desktop_trusted_app', { app });
export const confirmDesktopObservationScope = (apps: Pick<TrustedDesktopApp, 'id' | 'executablePath'>[]) =>
  invoke<TrustedDesktopApp[]>('confirm_desktop_observation_scope', { apps });
export const deleteDesktopTrustedApp = (id: string) => invoke<void>('delete_desktop_trusted_app', { id });

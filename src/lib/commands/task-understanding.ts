import { invoke } from '$lib/invoke';

export type UserInformationAction = 'inspect' | 'ask' | 'act_with_assumption' | 'defer';

export interface TaskDecisionProjection {
  id: string;
  question: string;
  affects: string[];
}

export interface WorkspaceTaskUnderstandingView {
  workspaceId: string;
  action: UserInformationAction;
  decision: TaskDecisionProjection | null;
}

interface WireWorkspaceTaskUnderstandingView {
  workspaceId: string;
  action: UserInformationAction;
  decision: TaskDecisionProjection | null;
}

/** Read one bounded, user-safe decision projection for the active Workspace. */
export async function getWorkspaceTaskUnderstanding(
  workspaceId: string,
): Promise<WorkspaceTaskUnderstandingView> {
  return invoke<WireWorkspaceTaskUnderstandingView>('get_workspace_task_understanding', {
    workspaceId,
  });
}

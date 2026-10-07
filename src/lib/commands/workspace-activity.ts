import { invoke } from '$lib/invoke';

interface WireDelegationProjection {
  id: string;
  goal: string;
  status: string;
  summary: string | null;
  key_facts: string[];
  milestones_completed: number;
  milestones_total: number;
  verifications_passed: number;
  verifications_total: number;
  evidence_count: number;
  activity: string | null;
  reviewer_status: string | null;
  updated_at: number;
}

interface WirePendingDecisionProjection {
  delegation_id: string;
  goal: string;
  questions: string[];
  risks: string[];
  updated_at: number;
}

interface WireAttentionSummary {
  open_count: number;
  message: string;
}

interface WireWorkspaceActivityProjection {
  workspaceId: string;
  cursor: number;
  delegations: WireDelegationProjection[];
  pending_decisions: WirePendingDecisionProjection[];
  attention?: WireAttentionSummary | null;
}

export type WorkspaceActivityStatus =
  | 'queued'
  | 'running'
  | 'awaitingConfirmation'
  | 'awaitingSummary'
  | 'awaitingReview'
  | 'materializationPending'
  | 'needsDecision'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'unknown';

export interface WorkspaceActivityItem {
  id: string;
  goal: string;
  status: WorkspaceActivityStatus;
  summary: string | null;
  keyFacts: string[];
  milestonesCompleted: number;
  milestonesTotal: number;
  verificationsPassed: number;
  verificationsTotal: number;
  evidenceCount: number;
  activity: string | null;
  reviewerStatus: string | null;
  updatedAt: number;
}

export interface MainAgentDecision {
  workId: string;
  goal: string;
  questions: string[];
  risks: string[];
  updatedAt: number;
}

/** Safe, aggregate state for work that needs an explicit Main-Agent follow-up. */
export interface WorkspaceAttentionSummary {
  openCount: number;
  message: string;
}

export interface WorkspaceActivityProjection {
  workspaceId: string;
  cursor: number;
  work: WorkspaceActivityItem[];
  pendingDecisions: MainAgentDecision[];
  attention: WorkspaceAttentionSummary | null;
}

const statusByWire: Record<string, WorkspaceActivityStatus> = {
  queued: 'queued',
  running: 'running',
  awaiting_confirmation: 'awaitingConfirmation',
  awaiting_summary: 'awaitingSummary',
  awaiting_review: 'awaitingReview',
  materialization_pending: 'materializationPending',
  needs_decision: 'needsDecision',
  completed: 'completed',
  failed: 'failed',
  cancelled: 'cancelled',
};

function statusFromWire(status: string): WorkspaceActivityStatus {
  return statusByWire[status] ?? 'unknown';
}

export function mapWorkspaceActivityProjection(wire: WireWorkspaceActivityProjection): WorkspaceActivityProjection {
  return {
    workspaceId: wire.workspaceId,
    cursor: wire.cursor,
    work: wire.delegations.map((item) => ({
      id: item.id,
      goal: item.goal,
      status: statusFromWire(item.status),
      summary: item.summary,
      keyFacts: item.key_facts,
      milestonesCompleted: item.milestones_completed,
      milestonesTotal: item.milestones_total,
      verificationsPassed: item.verifications_passed,
      verificationsTotal: item.verifications_total,
      evidenceCount: item.evidence_count,
      activity: item.activity ?? null,
      reviewerStatus: item.reviewer_status ?? null,
      updatedAt: item.updated_at,
    })),
    pendingDecisions: wire.pending_decisions.map((item) => ({
      workId: item.delegation_id,
      goal: item.goal,
      questions: item.questions,
      risks: item.risks,
      updatedAt: item.updated_at,
    })),
    attention: wire.attention
      ? { openCount: wire.attention.open_count, message: wire.attention.message }
      : null,
  };
}

/** Read the bounded work projection owned by one Workspace's Main Agent. */
export async function getWorkspaceActivity(workspaceId: string): Promise<WorkspaceActivityProjection> {
  const wire = await invoke<WireWorkspaceActivityProjection>('get_workspace_activity_projection', { workspaceId });
  return mapWorkspaceActivityProjection(wire);
}

import { describe, expect, it, vi } from 'vitest';
import { invoke } from '$lib/invoke';
import { getWorkspaceActivity, mapWorkspaceActivityProjection } from './workspace-activity';

vi.mock('$lib/invoke', () => ({ invoke: vi.fn() }));

const projection = {
  workspaceId: 'workspace-1',
  cursor: 7,
  delegations: [{
    id: 'work-1',
    goal: '检查设计',
    status: 'awaiting_review',
    summary: '已完成初步检查',
    key_facts: ['发现一项风险'],
    milestones_completed: 2,
    milestones_total: 3,
    verifications_passed: 1,
    verifications_total: 2,
    evidence_count: 4,
    activity: '检查候选结果',
    reviewer_status: 'running',
    updated_at: 123,
  }],
  pending_decisions: [{
    delegation_id: 'work-1',
    goal: '检查设计',
    questions: ['是否继续？'],
    risks: ['范围会扩大'],
    updated_at: 124,
  }],
  attention: {
    open_count: 2,
    message: '有工作需要继续处理，进度已安全保留。请回到主对话说明下一步。',
  },
};

describe('Workspace activity command boundary', () => {
  it('maps the backend projection into the camelCase UI contract without child-agent controls', () => {
    expect(mapWorkspaceActivityProjection(projection)).toEqual({
      workspaceId: 'workspace-1',
      cursor: 7,
      work: [{
        id: 'work-1',
        goal: '检查设计',
        status: 'awaitingReview',
        summary: '已完成初步检查',
        keyFacts: ['发现一项风险'],
        milestonesCompleted: 2,
        milestonesTotal: 3,
        verificationsPassed: 1,
        verificationsTotal: 2,
        evidenceCount: 4,
        activity: '检查候选结果',
        reviewerStatus: 'running',
        updatedAt: 123,
      }],
      pendingDecisions: [{
        workId: 'work-1',
        goal: '检查设计',
        questions: ['是否继续？'],
        risks: ['范围会扩大'],
        updatedAt: 124,
      }],
      attention: {
        openCount: 2,
        message: '有工作需要继续处理，进度已安全保留。请回到主对话说明下一步。',
      },
    });
  });

  it('keeps the workbench compatible with an older empty Attention projection', () => {
    const { attention: _attention, ...withoutAttention } = projection;

    expect(mapWorkspaceActivityProjection(withoutAttention)).toMatchObject({
      workspaceId: 'workspace-1',
      attention: null,
    });
  });

  it('uses the registered projection command with the frontend camelCase argument', async () => {
    vi.mocked(invoke).mockResolvedValue(projection);

    await expect(getWorkspaceActivity('workspace-1')).resolves.toMatchObject({ workspaceId: 'workspace-1' });
    expect(invoke).toHaveBeenCalledWith('get_workspace_activity_projection', { workspaceId: 'workspace-1' });
  });
});

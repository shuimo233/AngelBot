import { describe, expect, it } from 'vitest';
import type { Message } from '$types';
import { normalizeMessageContract } from './message';

describe('normalizeMessageContract', () => {
  it('projects durable snake-case task facts into a visible pending confirmation', () => {
    const raw = {
      id: 'assistant-1',
      sessionId: 'session-1',
      role: 'assistant',
      content: 'Needs approval',
      createdAt: 1,
      toolCalls: [{
        id: 'call-1',
        name: 'open_windows_setting',
        arguments: JSON.stringify({ page: 'sound' }),
      }],
      taskFacts: {
        goal: 'Open sound settings',
        completed_steps: [],
        failed_steps: [],
        modified_files: ['notes/todo.md'],
        verification_evidence: [{
          command: 'verify', exit_code: 0, summary: 'passed', verified_at: 12,
        }],
        pending_confirmation: {
          call_id: 'call-1',
          tool_name: 'open_windows_setting',
          arguments: { page: 'sound' },
          requested_at: 10,
        },
      },
    } as unknown as Message;

    const message = normalizeMessageContract(raw);

    expect(message.taskFacts?.modifiedFiles).toEqual(['notes/todo.md']);
    expect(message.taskFacts?.verificationEvidence?.[0]).toEqual({
      command: 'verify', exitCode: 0, summary: 'passed', verifiedAt: 12,
    });
    expect(message.taskFacts?.pendingConfirmation).toEqual({
      callId: 'call-1',
      toolName: 'open_windows_setting',
      arguments: { page: 'sound' },
      requestedAt: 10,
    });
    expect(message.toolResults).toEqual([expect.objectContaining({
      callId: 'call-1',
      confirmationRequired: true,
      confirmationStatus: 'pending',
    })]);
  });

  it('does not duplicate a backend-provided result', () => {
    const message = normalizeMessageContract({
      id: 'assistant-1', sessionId: 'session-1', role: 'assistant', content: '', createdAt: 1,
      toolCalls: [{ id: 'call-1', name: 'write_file', arguments: '{}' }],
      toolResults: [{ callId: 'call-1', toolName: 'write_file', success: false, output: 'pending', confirmationRequired: true, confirmationStatus: 'pending' }],
      taskFacts: {
        goal: 'Write',
        pendingConfirmation: { callId: 'call-1', toolName: 'write_file', arguments: {}, requestedAt: 1 },
      },
    });

    expect(message.toolResults).toHaveLength(1);
  });

  it('projects a durable rejection when the backend stores no tool-result row', () => {
    const message = normalizeMessageContract({
      id: 'assistant-1', sessionId: 'session-1', role: 'assistant', content: '', createdAt: 1,
      toolCalls: [{ id: 'call-1', name: 'open_windows_setting', arguments: '{}' }],
      taskRun: {
        id: 'run-1', goal: 'Open settings', status: 'needs_attention', plan: ['open_windows_setting'],
        confirmationState: 'rejected', resumable: true, stepCount: 1, completedStepCount: 0,
      },
      taskFacts: {
        goal: 'Open settings',
        // Compatibility with pre-fix durable rows: the task-run verdict wins.
        pendingConfirmation: {
          callId: 'call-1', toolName: 'open_windows_setting', arguments: {}, requestedAt: 1,
        },
      },
    });

    expect(message.toolResults).toEqual([expect.objectContaining({
      callId: 'call-1', confirmationRequired: false, confirmationStatus: 'rejected',
    })]);
  });
});

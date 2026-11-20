import { invoke } from '$lib/invoke';

export const submitSteeringMessage = (sessionId: string, replyId: string, content: string) =>
  invoke<void>('submit_in_progress_command', { sessionId, replyId, content, mode: 'steer' });

export const pauseAgent = (sessionId: string) => invoke<void>('pause_agent', { sessionId });
export const resumeAgent = (sessionId: string) => invoke<void>('resume_agent', { sessionId });
export const interruptAgent = (sessionId: string) => invoke<void>('interrupt_agent', { sessionId });

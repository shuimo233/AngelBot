import { invoke } from '$lib/invoke';

export type SteeringMode = 'steer' | 'follow_up' | 'abort';

export const submitInProgressCommand = (sessionId: string, content: string, mode: SteeringMode) =>
  invoke<void>('submit_in_progress_command', { sessionId, content, mode });

export const pauseAgent = (sessionId: string) => invoke<void>('pause_agent', { sessionId });
export const resumeAgent = (sessionId: string) => invoke<void>('resume_agent', { sessionId });
export const interruptAgent = (sessionId: string) => invoke<void>('interrupt_agent', { sessionId });

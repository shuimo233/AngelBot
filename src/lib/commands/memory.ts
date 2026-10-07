/**
 * Memory commands with frequency decay system
 * 
 * Features:
 * - Vector similarity for memory activation
 * - Exponential decay for forgotten memories
 * - Three-stage forgetting: active -> summarized -> archived -> deleted
 */
import { invoke } from '../invoke';

// Rust 返回的 snake_case 类型
export interface RustMemory {
  id: string;
  scope: string;
  category: string;
  content: string;
  importance: number;
  source: string;
  frequency: number;
  last_mentioned: number | null;
  is_permanent: boolean;
  embedding: number[] | null;
  decay_factor: number;
  forget_stage: string;
  created_at: number;
  updated_at: number;
}

// ============================================
// Basic Memory Operations
// ============================================

export async function getMemories(): Promise<RustMemory[]> {
  return invoke<RustMemory[]>('get_memories');
}

export async function saveMemory(memory: MemoryInput): Promise<void> {
  return invoke<void>('save_memory', { memory });
}

export async function deleteMemory(id: string): Promise<void> {
  return invoke<void>('delete_memory', { id });
}

// ============================================
// Memory Types
// ============================================

export interface MemoryInput {
  id: string;
  scope: string;
  category: string;
  content: string;
  importance: number;
  source: string;
  isPermanent?: boolean;
  /** Required by the desktop backend before a direct memory write. */
  userConfirmed?: boolean;
}

export interface UpdateMemoryRequest {
  id: string;
  scope: string;
  category: string;
  content: string;
  importance: number;
  isPermanent: boolean;
}

export interface MemoryConflict {
  memory_id: string;
  content: string;
  category: string;
  similarity: number;
}

export interface MemoryHistoryEntry {
  id: string;
  memoryId: string;
  operation: string;
  previousContent: string | null;
  newContent: string | null;
  details: string | null;
  createdAt: number;
}

export interface MemoryConfig {
  similarity_threshold: number;
  base_decay_rate: number;
  forget_threshold: number;
  max_active_memories: number;
  enable_vector: boolean;
}

export const DEFAULT_MEMORY_CONFIG: MemoryConfig = {
  similarity_threshold: 0.7,
  base_decay_rate: 0.9,
  forget_threshold: 0.5,
  max_active_memories: 100,
  enable_vector: false,
};

export interface FrequencyUpdate {
  memory_id: string;
  similarity_score: number;
}

export interface SimilarityResult {
  memory_id: string;
  similarity: number;
  should_activate: boolean;
}

export interface ForgetAction {
  memory_id: string;
  stage: string;
  action: string;
  new_content: string | null;
}

export interface BatchForgetResult {
  decayed: number;
  summarized: number;
  archived: number;
  deleted: number;
}

export interface MemoryStats {
  total: number;
  active: number;
  summarized: number;
  archived: number;
  deleted: number;
  permanent: number;
  withEmbeddings: number;
}

export interface EvolutionMemory {
  category: string;
  content: string;
  importance: number;
}

export interface EvolutionReviewResult {
  new_memories: EvolutionMemory[];
  preferences_json: string | null;
  summary: string;
  total_stored: number;
  proposalsCreated: number;
}

export interface EvolutionProposal {
  id: string;
  proposalType: 'memory' | 'profile_preferences';
  sessionId: string | null;
  category: string | null;
  content: string | null;
  importance: number | null;
  preferencesJson: string | null;
  summary: string | null;
  source: string;
  status: 'pending' | 'accepted' | 'rejected';
  createdAt: number;
  reviewedAt: number | null;
}

export interface ReviewEvolutionProposalRequest {
  id: string;
  content?: string;
  importance?: number;
  permanent?: boolean;
}

// ============================================
// Frequency Decay Operations
// ============================================

/**
 * Update memory frequencies based on similarity scores
 * Only boosts frequency if similarity exceeds threshold
 */
export async function updateMemoryFrequency(
  updates: FrequencyUpdate[]
): Promise<void> {
  return invoke<void>('update_memory_frequency', { updates });
}

/**
 * Decay all active memories (batch operation)
 * Returns the number of memories updated
 */
export async function decayMemories(): Promise<number> {
  return invoke<number>('decay_memories');
}

/**
 * Get memories that should be forgotten based on score
 */
export async function getMemoriesToForget(
  config?: MemoryConfig
): Promise<RustMemory[]> {
  return invoke<RustMemory[]>('get_memories_to_forget', { config });
}

/**
 * Set a memory as permanent (never forgotten)
 */
export async function setMemoryPermanent(
  id: string,
  permanent: boolean
): Promise<void> {
  return invoke<void>('set_memory_permanent', { id, permanent });
}

export async function updateMemory(req: UpdateMemoryRequest): Promise<void> {
  return invoke<void>('update_memory', { req });
}

export async function detectMemoryConflicts(
  content: string,
  threshold?: number
): Promise<MemoryConflict[]> {
  return invoke<MemoryConflict[]>('detect_memory_conflicts', { content, threshold });
}

export async function getMemoryHistory(memoryId: string): Promise<MemoryHistoryEntry[]> {
  return invoke<MemoryHistoryEntry[]>('get_memory_history', { memory_id: memoryId });
}

export async function mergeMemories(sourceId: string, targetId: string): Promise<void> {
  return invoke<void>('merge_memories', { source_id: sourceId, target_id: targetId });
}

/**
 * Archive a memory (soft delete)
 */
export async function archiveMemory(id: string): Promise<void> {
  return invoke<void>('archive_memory', { id });
}

/**
 * Permanently delete a memory
 */
export async function permanentlyDeleteMemory(id: string): Promise<void> {
  return invoke<void>('permanently_delete_memory', { id });
}

/**
 * Cleanup archived memories older than specified days
 */
export async function cleanupArchivedMemories(
  olderThanDays: number = 90
): Promise<number> {
  return invoke<number>('cleanup_archived_memories', { olderThanDays });
}

// ============================================
// Vector-Based Operations
// ============================================

/**
 * Find relevant memories using vector similarity
 */
export async function findRelevantMemories(
  queryText: string,
  topK?: number,
  threshold?: number
): Promise<SimilarityResult[]> {
  return invoke<SimilarityResult[]>('find_relevant_memories', {
    query_text: queryText,
    top_k: topK,
    threshold,
  });
}

/**
 * Update embedding for a specific memory
 */
export async function updateMemoryEmbedding(
  memoryId: string,
  content: string
): Promise<void> {
  return invoke<void>('update_memory_embedding', { memory_id: memoryId, content });
}

/**
 * Batch update embeddings for all memories without embeddings
 */
export async function batchUpdateEmbeddings(): Promise<number> {
  return invoke<number>('batch_update_embeddings');
}

/**
 * Find relevant memories using time-based scoring (fallback when vectors disabled)
 */
export async function findRelevantMemoriesByTime(
  topK?: number
): Promise<[string, number][]> {
  return invoke<[string, number][]>('find_relevant_memories_by_time', { top_k: topK });
}

// ============================================
// Forget Stage Processing
// ============================================

/**
 * Generate a summary of memory content
 */
export async function summarizeMemoryContent(
  memoryId: string,
  currentContent: string
): Promise<ForgetAction> {
  return invoke<ForgetAction>('summarize_memory_content', {
    memory_id: memoryId,
    current_content: currentContent,
  });
}

/**
 * Truncate memory content to specified ratio
 */
export async function truncateMemoryContent(
  memoryId: string,
  currentContent: string,
  keepRatio?: number
): Promise<ForgetAction> {
  return invoke<ForgetAction>('truncate_memory_content', {
    memory_id: memoryId,
    current_content: currentContent,
    keep_ratio: keepRatio,
  });
}

/**
 * Process memory through forgetting stages
 */
export async function processForgettingStage(
  memoryId: string,
  targetStage: 'summarized' | 'archived' | 'deleted',
  useLlm?: boolean
): Promise<ForgetAction> {
  return invoke<ForgetAction>('process_forgetting_stage', {
    memory_id: memoryId,
    target_stage: targetStage,
    use_llm: useLlm,
  });
}

// ============================================
// Batch Operations
// ============================================

/**
 * Run full batch forgetting cycle
 * - Decay active memories
 * - Archive old summarized memories
 * - Delete old archived memories
 */
export async function runBatchForgetting(
  archiveThreshold?: number,
  deleteThreshold?: number
): Promise<BatchForgetResult> {
  return invoke<BatchForgetResult>('run_batch_forgetting', {
    archive_threshold: archiveThreshold,
    delete_threshold: deleteThreshold,
  });
}

/**
 * Get memory statistics
 */
export async function getMemoryStats(): Promise<MemoryStats> {
  return invoke<MemoryStats>('get_memory_stats');
}

// ============================================
// Evolution Proposal Operations
// ============================================

export async function runEvolutionReview(
  sessionId?: string,
  messageCount?: number
): Promise<EvolutionReviewResult> {
  return invoke<EvolutionReviewResult>('run_evolution_review', {
    session_id: sessionId,
    message_count: messageCount,
  });
}

export async function getEvolutionProposals(
  status: 'pending' | 'accepted' | 'rejected' | 'all' = 'pending'
): Promise<EvolutionProposal[]> {
  return invoke<EvolutionProposal[]>('get_evolution_proposals', { status });
}

export async function acceptEvolutionProposal(
  req: ReviewEvolutionProposalRequest
): Promise<EvolutionProposal> {
  return invoke<EvolutionProposal>('accept_evolution_proposal', { req });
}

export async function rejectEvolutionProposal(id: string): Promise<EvolutionProposal> {
  return invoke<EvolutionProposal>('reject_evolution_proposal', { id });
}

// ============================================
// Profile Operations
// ============================================

export interface Profile {
  name: string;
  preferences: string;
  habits: string;
  background: string;
}

export async function getProfile(): Promise<Profile> {
  return invoke<Profile>('get_profile');
}

export async function saveProfile(profile: Profile): Promise<void> {
  return invoke<void>('save_profile', { profile });
}

// ============================================
// Utility Functions
// ============================================

/**
 * Calculate memory score based on frequency and decay factor
 */
export function calculateMemoryScore(frequency: number, decayFactor: number): number {
  return frequency * decayFactor;
}

/**
 * Check if a memory should be forgotten
 */
export function shouldForget(
  frequency: number,
  decayFactor: number,
  isPermanent: boolean,
  threshold: number = 0.5
): boolean {
  if (isPermanent) return false;
  return calculateMemoryScore(frequency, decayFactor) < threshold;
}

/**
 * Format memory stats for display
 */
export function formatMemoryStats(stats: MemoryStats): string {
  const lines = [
    `Total memories: ${stats.total}`,
    `Active: ${stats.active}`,
    `Summarized: ${stats.summarized}`,
    `Archived: ${stats.archived}`,
    `Deleted: ${stats.deleted}`,
    `Permanent: ${stats.permanent}`,
    `With embeddings: ${stats.withEmbeddings}`,
  ];
  return lines.join('\n');
}

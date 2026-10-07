/**
 * Unit tests for memory commands API
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { invoke } from '../invoke';

// Mock the invoke function
vi.mock('../invoke', () => ({
  invoke: vi.fn(),
}));

import {
  getMemories,
  saveMemory,
  deleteMemory,
  updateMemoryFrequency,
  decayMemories,
  setMemoryPermanent,
  updateMemory,
  detectMemoryConflicts,
  getMemoryHistory,
  mergeMemories,
  findRelevantMemories,
  findRelevantMemoriesByTime,
  runBatchForgetting,
  getMemoryStats,
  runEvolutionReview,
  getEvolutionProposals,
  acceptEvolutionProposal,
  rejectEvolutionProposal,
  calculateMemoryScore,
  shouldForget,
  DEFAULT_MEMORY_CONFIG,
} from './memory';

describe('Memory Commands', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  describe('Basic Operations', () => {
    it('should get memories', async () => {
      const mockMemories = [
        {
          id: 'mem-1',
          scope: 'global',
          category: 'fact',
          content: 'Test memory',
          importance: 5,
          source: 'test',
          frequency: 3,
          lastMentioned: Date.now(),
          isPermanent: false,
          embedding: null,
          decayFactor: 1.0,
          createdAt: Date.now(),
          updatedAt: Date.now(),
        },
      ];

      (invoke as ReturnType<typeof vi.fn>).mockResolvedValue(mockMemories);

      const result = await getMemories();
      expect(result).toEqual(mockMemories);
      expect(invoke).toHaveBeenCalledWith('get_memories');
    });

    it('should save memory', async () => {
      (invoke as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);

      const memory = {
        id: 'mem-new',
        scope: 'global',
        category: 'fact',
        content: 'New memory',
        importance: 5,
        source: 'test',
      };

      await saveMemory(memory);
      expect(invoke).toHaveBeenCalledWith('save_memory', { memory });
    });

    it('should delete memory', async () => {
      (invoke as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);

      await deleteMemory('mem-1');
      expect(invoke).toHaveBeenCalledWith('delete_memory', { id: 'mem-1' });
    });
  });

  describe('Frequency Decay', () => {
    it('should update memory frequency', async () => {
      (invoke as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);

      const updates = [
        { memory_id: 'mem-1', similarity_score: 0.8 },
        { memory_id: 'mem-2', similarity_score: 0.5 },
      ];

      await updateMemoryFrequency(updates);
      expect(invoke).toHaveBeenCalledWith('update_memory_frequency', { updates });
    });

    it('should decay memories', async () => {
      (invoke as ReturnType<typeof vi.fn>).mockResolvedValue(5);

      const result = await decayMemories();
      expect(result).toBe(5);
      expect(invoke).toHaveBeenCalledWith('decay_memories');
    });

    it('should set memory permanent', async () => {
      (invoke as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);

      await setMemoryPermanent('mem-1', true);
      expect(invoke).toHaveBeenCalledWith('set_memory_permanent', { id: 'mem-1', permanent: true });
    });

    it('should update memory fields', async () => {
      (invoke as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);

      const req = {
        id: 'mem-1',
        scope: 'global',
        category: 'preference',
        content: 'Updated memory',
        importance: 8,
        isPermanent: true,
      };

      await updateMemory(req);
      expect(invoke).toHaveBeenCalledWith('update_memory', { req });
    });

    it('should expose memory history and conflict commands', async () => {
      (invoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
        { memory_id: 'mem-2', content: 'Similar', category: 'fact', similarity: 0.91 },
      ]);
      await detectMemoryConflicts('Similar memory', 0.8);
      expect(invoke).toHaveBeenCalledWith('detect_memory_conflicts', {
        content: 'Similar memory',
        threshold: 0.8,
      });

      (invoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
        { id: 'h1', memoryId: 'mem-1', operation: 'update', previousContent: 'a', newContent: 'b', details: null, createdAt: 1 },
      ]);
      await getMemoryHistory('mem-1');
      expect(invoke).toHaveBeenCalledWith('get_memory_history', { memory_id: 'mem-1' });

      (invoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(undefined);
      await mergeMemories('mem-2', 'mem-1');
      expect(invoke).toHaveBeenCalledWith('merge_memories', { source_id: 'mem-2', target_id: 'mem-1' });
    });
  });

  describe('Vector Operations', () => {
    it('should find relevant memories', async () => {
      const mockResults = [
        { memory_id: 'mem-1', similarity: 0.9, should_activate: true },
        { memory_id: 'mem-2', similarity: 0.6, should_activate: false },
      ];

      (invoke as ReturnType<typeof vi.fn>).mockResolvedValue(mockResults);

      const result = await findRelevantMemories('test query', 10, 0.7);
      expect(result).toEqual(mockResults);
      expect(invoke).toHaveBeenCalledWith('find_relevant_memories', {
        query_text: 'test query',
        top_k: 10,
        threshold: 0.7,
      });
    });

    it('should find relevant memories by time', async () => {
      const mockResults: [string, number][] = [['mem-1', 0.8], ['mem-2', 0.5]];

      (invoke as ReturnType<typeof vi.fn>).mockResolvedValue(mockResults);

      const result = await findRelevantMemoriesByTime(5);
      expect(result).toEqual(mockResults);
      expect(invoke).toHaveBeenCalledWith('find_relevant_memories_by_time', { top_k: 5 });
    });
  });

  describe('Batch Operations', () => {
    it('should run batch forgetting', async () => {
      const mockResult = {
        decayed: 10,
        summarized: 2,
        archived: 1,
        deleted: 0,
      };

      (invoke as ReturnType<typeof vi.fn>).mockResolvedValue(mockResult);

      const result = await runBatchForgetting(30, 90);
      expect(result).toEqual(mockResult);
      expect(invoke).toHaveBeenCalledWith('run_batch_forgetting', {
        archive_threshold: 30,
        delete_threshold: 90,
      });
    });

    it('should get memory stats', async () => {
      const mockStats = {
        total: 100,
        active: 80,
        summarized: 10,
        archived: 5,
        deleted: 3,
        permanent: 2,
        withEmbeddings: 50,
      };

      (invoke as ReturnType<typeof vi.fn>).mockResolvedValue(mockStats);

      const result = await getMemoryStats();
      expect(result).toEqual(mockStats);
      expect(invoke).toHaveBeenCalledWith('get_memory_stats');
    });
  });

  describe('Evolution Proposals', () => {
    it('should run evolution review into proposals', async () => {
      const mockResult = {
        new_memories: [{ category: 'preference', content: '用户喜欢 Rust', importance: 8 }],
        preferences_json: null,
        summary: '学到用户喜欢 Rust',
        total_stored: 0,
        proposalsCreated: 1,
      };
      (invoke as ReturnType<typeof vi.fn>).mockResolvedValue(mockResult);

      const result = await runEvolutionReview('session-1', 20);
      expect(result).toEqual(mockResult);
      expect(invoke).toHaveBeenCalledWith('run_evolution_review', {
        session_id: 'session-1',
        message_count: 20,
      });
    });

    it('should get pending evolution proposals by default', async () => {
      (invoke as ReturnType<typeof vi.fn>).mockResolvedValue([]);

      await getEvolutionProposals();
      expect(invoke).toHaveBeenCalledWith('get_evolution_proposals', { status: 'pending' });
    });

    it('should accept an evolution proposal', async () => {
      const proposal = {
        id: 'p1',
        proposalType: 'memory',
        sessionId: null,
        category: 'fact',
        content: '用户喜欢 Rust',
        importance: 8,
        preferencesJson: null,
        summary: null,
        source: 'test',
        status: 'accepted',
        createdAt: 1,
        reviewedAt: 2,
      };
      (invoke as ReturnType<typeof vi.fn>).mockResolvedValue(proposal);

      const req = { id: 'p1', content: '用户非常喜欢 Rust', importance: 9, permanent: true };
      const result = await acceptEvolutionProposal(req);
      expect(result).toEqual(proposal);
      expect(invoke).toHaveBeenCalledWith('accept_evolution_proposal', { req });
    });

    it('should reject an evolution proposal', async () => {
      (invoke as ReturnType<typeof vi.fn>).mockResolvedValue({ id: 'p1', status: 'rejected' });

      await rejectEvolutionProposal('p1');
      expect(invoke).toHaveBeenCalledWith('reject_evolution_proposal', { id: 'p1' });
    });
  });

  describe('Utility Functions', () => {
    it('should calculate memory score', () => {
      expect(calculateMemoryScore(10, 0.9)).toBe(9);
      expect(calculateMemoryScore(5, 0.5)).toBe(2.5);
      expect(calculateMemoryScore(0, 1.0)).toBe(0);
    });

    it('should check if should forget - permanent', () => {
      expect(shouldForget(1, 0.1, true)).toBe(false);
    });

    it('should check if should forget - high score', () => {
      expect(shouldForget(10, 0.9, false)).toBe(false);
    });

    it('should check if should forget - low score', () => {
      expect(shouldForget(1, 0.1, false)).toBe(true);
    });

    it('should have correct default config', () => {
      expect(DEFAULT_MEMORY_CONFIG.similarity_threshold).toBe(0.7);
      expect(DEFAULT_MEMORY_CONFIG.base_decay_rate).toBe(0.9);
      expect(DEFAULT_MEMORY_CONFIG.forget_threshold).toBe(0.5);
      expect(DEFAULT_MEMORY_CONFIG.enable_vector).toBe(false);
    });
  });
});

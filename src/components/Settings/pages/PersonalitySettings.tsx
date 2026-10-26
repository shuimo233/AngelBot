import { useState, useCallback, useEffect } from 'react';
import { SettingsSection } from '../components/SettingsSection';
import { TextField } from '../components/TextField';
import { TraitSlider } from '../components/TraitSlider';
import { LearnedHabits } from '../components/LearnedHabits';
import { useSettingsStore } from '$stores/settings';
import { usePreferencesStore } from '$stores/preferences';
import { TRAIT_LABELS, defaultTraits } from '$lib/presets';
import { describeUserPreferences } from '$lib/evolution';
import { applyTavernCard, exportTavernCard, parseTavernCardFile } from '$lib/tavern-card';
import { matchPersonalityDirection, type PersonalityMatchResult } from '$lib/commands';
import {
  acceptEvolutionProposal,
  getEvolutionProposals,
  rejectEvolutionProposal,
  setMemoryPermanent,
  getMemories,
  type EvolutionProposal,
  type RustMemory,
} from '$lib/commands/memory';
import type { TraitName, TraitConfig } from '$types';

const DownloadIcon = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
    <polyline points="7 10 12 15 17 10" />
    <line x1="12" y1="15" x2="12" y2="3" />
  </svg>
);

const UploadIcon = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
    <polyline points="17 8 12 3 7 8" />
    <line x1="12" y1="3" x2="12" y2="15" />
  </svg>
);

const PinIcon = ({ permanent }: { permanent: boolean }) =>
  permanent ? (
    <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor">
      <path d="M16 12V4h1V2H7v2h1v8l-2 2v2h5.2v6h1.6v-6H18v-2l-2-2z"/>
    </svg>
  ) : (
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
      <path d="M16 12V4h1V2H7v2h1v8l-2 2v2h5.2v6h1.6v-6H18v-2l-2-2z"/>
    </svg>
  );

const ShieldIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
    <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>
  </svg>
);

function applySuggestedTraits(suggestedTraitsJson: string): TraitConfig {
  try {
    const parsed = JSON.parse(suggestedTraitsJson);
    return {
      tone: parsed.tone ?? 0,
      verbosity: parsed.verbosity ?? 0,
      formality: parsed.formality ?? 0,
      humor: parsed.humor ?? 0,
      dependence: parsed.dependence ?? 0,
      intimacy: parsed.intimacy ?? 0,
      patience: parsed.patience ?? 5,
    };
  } catch {
    return { ...defaultTraits };
  }
}

export function PersonalitySettings() {
  const profile = useSettingsStore((s) => s.profile);
  const updateProfile = useSettingsStore((s) => s.updateProfile);

  const preferences = usePreferencesStore((s) => s.preferences);
  const memoryEnabled = preferences.habits.useLongTermMemory ?? true;
  const learningActive = preferences.evolutionEnabled && memoryEnabled;
  const learningHint = !preferences.evolutionEnabled
    ? '偏好学习已关闭。从下一次新对话请求起，不再自动复盘或沿用已学习的习惯；现有记录仍可查看和清理。'
    : !memoryEnabled
      ? '长期记忆已关闭，偏好学习暂停。可在「默认工作方式」开启长期记忆；现有记录仍可查看和清理。'
      : '完成对话后按数量和频率上限复盘新增表达，会使用当前模型额度。开关从下一次新对话请求起生效；已发送的内容不会撤回。';
  const setEvolutionEnabled = usePreferencesStore((s) => s.setEvolutionEnabled);
  const addInterest = usePreferencesStore((s) => s.addInterest);
  const removeInterest = usePreferencesStore((s) => s.removeInterest);
  const addAvoidTopic = usePreferencesStore((s) => s.addAvoidTopic);
  const removeAvoidTopic = usePreferencesStore((s) => s.removeAvoidTopic);

  const [descriptionInput, setDescriptionInput] = useState('');
  const traits: TraitConfig = {
    ...defaultTraits,
    ...profile.traits,
    // Preserve the legacy language-style setting as a fallback for profiles
    // created before fine-grained traits were introduced.
    formality: profile.traits?.formality
      ?? (profile.languageStyle === 'formal' ? 3 : profile.languageStyle === 'concise' ? 2 : 0),
  };

  // 推荐方向状态
  const [matchResult, setMatchResult] = useState<PersonalityMatchResult | null>(null);
  const [matching, setMatching] = useState(false);
  const [matchError, setMatchError] = useState<string | null>(null);

  const [showEvolutionDetails, setShowEvolutionDetails] = useState(false);
  const [newInterest, setNewInterest] = useState('');
  const [newAvoidTopic, setNewAvoidTopic] = useState('');
  const [permanentMemories, setPermanentMemories] = useState<Set<string>>(new Set());
  const [loadingPermanent, setLoadingPermanent] = useState(false);
  const [evolutionProposals, setEvolutionProposals] = useState<EvolutionProposal[]>([]);
  const [proposalDrafts, setProposalDrafts] = useState<Record<string, { content: string; importance: number; permanent: boolean }>>({});
  const [loadingProposals, setLoadingProposals] = useState(false);
  const [proposalError, setProposalError] = useState<string | null>(null);
  const [cardImportMessage, setCardImportMessage] = useState<string | null>(null);

  // 加载永久记忆列表
  const loadPermanentMemories = useCallback(async () => {
    setLoadingPermanent(true);
    try {
      const memories: RustMemory[] = await getMemories();
      const permanent = new Set(
        memories.filter((m) => m.is_permanent).map((m) => m.content)
      );
      setPermanentMemories(permanent);
    } catch {
      // ignore
    } finally {
      setLoadingPermanent(false);
    }
  }, []);

  const loadEvolutionProposals = useCallback(async () => {
    setLoadingProposals(true);
    setProposalError(null);
    try {
      const proposals = await getEvolutionProposals('pending');
      setEvolutionProposals(proposals);
      setProposalDrafts((prev) => {
        const next = { ...prev };
        for (const proposal of proposals) {
          if (proposal.proposalType !== 'memory') continue;
          if (!next[proposal.id]) {
            next[proposal.id] = {
              content: proposal.content ?? '',
              importance: proposal.importance ?? 5,
              permanent: false,
            };
          }
        }
        return next;
      });
    } catch (err) {
      setProposalError(err instanceof Error ? err.message : '加载学习候选失败');
    } finally {
      setLoadingProposals(false);
    }
  }, []);

  // 初始化时加载
  useEffect(() => {
    if (showEvolutionDetails) {
      loadPermanentMemories();
      loadEvolutionProposals();
    }
  }, [showEvolutionDetails, loadPermanentMemories, loadEvolutionProposals]);

  // 切换永久标记
  const togglePermanent = useCallback(async (topic: string, isInterest: boolean) => {
    try {
      const memories: RustMemory[] = await getMemories();
      const category = isInterest ? 'preference' : 'personality';
      const existing = memories.find(
        (m) => m.content === topic && m.category === category
      );
      if (existing) {
        await setMemoryPermanent(existing.id, !existing.is_permanent);
        setPermanentMemories((prev) => {
          const next = new Set(prev);
          if (existing.is_permanent) {
            next.delete(topic);
          } else {
            next.add(topic);
          }
          return next;
        });
      }
    } catch {
      // ignore
    }
  }, []);

  // 应用 L1 模板
  const handleDescriptionSubmit = useCallback(async () => {
    if (!descriptionInput.trim()) return;
    setMatching(true);
    setMatchError(null);
    setMatchResult(null);

    try {
      const result = await matchPersonalityDirection(descriptionInput.trim());
      setMatchResult(result);
    } catch (err) {
      setMatchError(err instanceof Error ? err.message : '推荐失败，请重试');
    } finally {
      setMatching(false);
    }
  }, [descriptionInput]);

  const handleApplyDirection = useCallback(() => {
    if (!matchResult) return;
    // Previewing a suggestion must not edit the draft or rename the assistant.
    updateProfile({
      bio: matchResult.suggested_description,
      traits: applySuggestedTraits(matchResult.suggested_traits),
    });
  }, [matchResult, updateProfile]);

  const handleTraitChange = useCallback((trait: TraitName, value: number) => {
    updateProfile({ traits: { ...traits, [trait]: value } });
  }, [traits, updateProfile]);

  const handleAddInterest = () => {
    if (newInterest.trim()) {
      addInterest(newInterest.trim());
      setNewInterest('');
    }
  };

  const handleAddAvoidTopic = () => {
    if (newAvoidTopic.trim()) {
      addAvoidTopic(newAvoidTopic.trim());
      setNewAvoidTopic('');
    }
  };

  const updateProposalDraft = useCallback((
    id: string,
    patch: Partial<{ content: string; importance: number; permanent: boolean }>
  ) => {
    setProposalDrafts((prev) => {
      const current = prev[id] ?? { content: '', importance: 5, permanent: false };
      return {
        ...prev,
        [id]: { ...current, ...patch },
      };
    });
  }, []);

  const handleAcceptProposal = useCallback(async (proposal: EvolutionProposal) => {
    try {
      setProposalError(null);
      const draft = proposalDrafts[proposal.id];
      await acceptEvolutionProposal({
        id: proposal.id,
        content: proposal.proposalType === 'memory' ? draft?.content ?? proposal.content ?? '' : undefined,
        importance: proposal.proposalType === 'memory' ? draft?.importance ?? proposal.importance ?? 5 : undefined,
        permanent: proposal.proposalType === 'memory' ? draft?.permanent ?? false : undefined,
      });
      await loadEvolutionProposals();
      await loadPermanentMemories();
    } catch (err) {
      setProposalError(err instanceof Error ? err.message : '接受学习候选失败');
    }
  }, [loadEvolutionProposals, loadPermanentMemories, proposalDrafts]);

  const handleRejectProposal = useCallback(async (id: string) => {
    try {
      setProposalError(null);
      await rejectEvolutionProposal(id);
      await loadEvolutionProposals();
    } catch (err) {
      setProposalError(err instanceof Error ? err.message : '拒绝学习候选失败');
    }
  }, [loadEvolutionProposals]);

  const handleExport = useCallback(() => {
    const exportData = {
      version: 1,
      profile: {
        name: profile.name,
        avatar: profile.avatar,
        bio: profile.bio,
        greeting: profile.greeting,
      },
      traits,
      exportedAt: new Date().toISOString(),
    };

    const blob = new Blob([JSON.stringify(exportData, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `angelbot-persona-${profile.name || 'custom'}-${Date.now()}.json`;
    a.click();
    URL.revokeObjectURL(url);
  }, [profile, traits]);

  const handleTavernExport = useCallback(() => {
    const blob = new Blob([JSON.stringify(exportTavernCard(profile, profile.bio), null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `angelbot-character-${profile.name || 'custom'}-${Date.now()}.json`;
    a.click();
    URL.revokeObjectURL(url);
  }, [profile]);

  const handleImport = useCallback(() => {
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = '.json,.png,application/json,image/png';
    input.onchange = async (e) => {
      const file = (e.target as HTMLInputElement).files?.[0];
      if (!file) return;

      try {
        if (file.name.toLowerCase().endsWith('.png') || file.type === 'image/png') {
          throw new Error('Tavern PNG');
        }
        const data = JSON.parse(await file.text());

        if (!data.profile) {
          throw new Error('Tavern JSON');
        }

        updateProfile({
          name: data.profile.name || '',
          avatar: data.profile.avatar || '',
          bio: data.profile.bio || '',
          greeting: data.profile.greeting || '',
          ...(data.traits ? { traits: data.traits } : {}),
        });

        setCardImportMessage('配置已导入，点击右上角「保存」后保留。');
      } catch {
        try {
          const card = await parseTavernCardFile(file);
          const nextProfile = applyTavernCard(card);
          updateProfile(nextProfile);
          setCardImportMessage(`角色卡「${card.name}」已导入，点击右上角「保存」后保留。`);
          return;
        } catch (cardError) {
          console.error('Failed to import persona:', cardError);
          setCardImportMessage(cardError instanceof Error ? cardError.message : '导入失败，请检查文件格式。');
          return;
        }
      }
    };
    input.click();
  }, [updateProfile]);

  return (
    <div className="settings-page-content personality-settings">
      <SettingsSection title="日常设置" description="用自己的话描述你喜欢的相处方式，不用编写提示词，也不需要调用模型。" defaultOpen>
        <div className="personality-basic-fields">
          <TextField
            label="名字"
            value={profile.name}
            placeholder="例如：AngelBot"
            onChange={(v) => updateProfile({ name: v })}
          />
          <TextField
            label="相处方式"
            value={profile.bio}
            placeholder="例如：温和自然，先说结论；我倾诉时先听我说，办事时简洁直接。"
            multiline
            rows={3}
            onChange={(bio) => updateProfile({ bio })}
          />
          <p className="personality-note">可留空，其他设定不受影响。修改后点击右上角「保存」。</p>
        </div>
      </SettingsSection>

      {/* 偏好学习设置 */}
      <SettingsSection
        title="偏好学习"
        description="逐渐了解你的日常习惯和相处偏好，让交流更舒适"
        defaultOpen
        actions={
          <button
            className="btn btn-secondary btn-sm"
            onClick={() => setShowEvolutionDetails(!showEvolutionDetails)}
          >
            {showEvolutionDetails ? '收起' : '查看详情'}
          </button>
        }
      >
        <div className="evolution-section">
          <div className="evolution-toggle">
            <label className="toggle-label">
              <input
                type="checkbox"
                checked={preferences.evolutionEnabled}
                onChange={(e) => setEvolutionEnabled(e.target.checked)}
              />
              <span>启用偏好学习</span>
            </label>
            <p className="evolution-hint" aria-live="polite">{learningHint}</p>
          </div>

          {showEvolutionDetails && (
            <div className="evolution-details">
              <LearnedHabits enabled={learningActive} />
              <div className="evolution-summary">
                <h4>当前设定的偏好</h4>
                <pre>{describeUserPreferences(preferences)}</pre>
              </div>

              <div className="evolution-proposals">
                <div className="evolution-proposals-header">
                  <div>
                    <h4>待确认的学习项</h4>
                    <p>手动复盘只产生候选，接受后才写入长期记忆。重复表达的轻微偏好可作为回应默认值，不能改变操作权限。</p>
                  </div>
                  <button
                    className="btn btn-secondary btn-sm"
                    onClick={loadEvolutionProposals}
                    disabled={loadingProposals}
                  >
                    {loadingProposals ? '刷新中...' : '刷新'}
                  </button>
                </div>

                {proposalError && <p className="proposal-error">{proposalError}</p>}

                {!loadingProposals && evolutionProposals.length === 0 && (
                  <div className="proposal-empty">暂无待确认学习项</div>
                )}

                <div className="proposal-list">
                  {evolutionProposals.map((proposal) => {
                    const draft = proposalDrafts[proposal.id] ?? {
                      content: proposal.content ?? '',
                      importance: proposal.importance ?? 5,
                      permanent: false,
                    };
                    const isMemory = proposal.proposalType === 'memory';
                    return (
                      <div className="proposal-item" key={proposal.id}>
                        <div className="proposal-meta">
                          <span className="proposal-type">
                            {isMemory ? '记忆候选' : '偏好候选'}
                          </span>
                          {proposal.category && (
                            <span className="proposal-category">{proposal.category}</span>
                          )}
                        </div>

                        {isMemory ? (
                          <>
                            <textarea
                              className="proposal-content-input"
                              value={draft.content}
                              rows={2}
                              onChange={(e) => updateProposalDraft(proposal.id, { content: e.target.value })}
                            />
                            <div className="proposal-controls">
                              <label>
                                重要性
                                <input
                                  type="number"
                                  min={1}
                                  max={10}
                                  value={draft.importance}
                                  onChange={(e) => updateProposalDraft(proposal.id, { importance: Number(e.target.value) })}
                                />
                              </label>
                              <label className="proposal-permanent">
                                <input
                                  type="checkbox"
                                  checked={draft.permanent}
                                  onChange={(e) => updateProposalDraft(proposal.id, { permanent: e.target.checked })}
                                />
                                不遗忘
                              </label>
                            </div>
                          </>
                        ) : (
                          <pre className="proposal-json">{proposal.preferencesJson}</pre>
                        )}

                        {proposal.summary && (
                          <p className="proposal-summary">{proposal.summary}</p>
                        )}

                        <div className="proposal-actions">
                          <button className="btn btn-primary btn-sm" onClick={() => handleAcceptProposal(proposal)}>
                            接受
                          </button>
                          <button className="btn btn-secondary btn-sm" onClick={() => handleRejectProposal(proposal.id)}>
                            拒绝
                          </button>
                        </div>
                      </div>
                    );
                  })}
                </div>
              </div>

              <div className="evolution-topics">
                <div className="topic-group">
                  <span className="field-label">
                    感兴趣的话题
                    {loadingPermanent && <span className="loading-dots">...</span>}
                  </span>
                  <div className="topic-tags">
                    {preferences.topics.interests.map((t) => {
                      const isPermanent = permanentMemories.has(t);
                      return (
                        <span key={t} className={`tag interest${isPermanent ? ' permanent' : ''}`}>
                          {isPermanent && <ShieldIcon />}
                          <span className="tag-text">{t}</span>
                          <button
                            className="tag-pin"
                            onClick={() => togglePermanent(t, true)}
                            title={isPermanent ? '取消永久保留' : '永久保留'}
                          >
                            <PinIcon permanent={isPermanent} />
                          </button>
                          <button className="tag-remove" onClick={() => removeInterest(t)}>×</button>
                        </span>
                      );
                    })}
                  </div>
                  <div className="topic-input-row">
                    <input
                      type="text"
                      value={newInterest}
                      onChange={(e) => setNewInterest(e.target.value)}
                      onKeyDown={(e) => e.key === 'Enter' && handleAddInterest()}
                      placeholder="添加话题..."
                    />
                    <button onClick={handleAddInterest}>+</button>
                  </div>
                </div>

                <div className="topic-group">
                  <span className="field-label">想避免的话题</span>
                  <div className="topic-tags">
                    {preferences.topics.avoidTopics.map((t) => {
                      const isPermanent = permanentMemories.has(t);
                      return (
                        <span key={t} className={`tag avoid${isPermanent ? ' permanent' : ''}`}>
                          {isPermanent && <ShieldIcon />}
                          <span className="tag-text">{t}</span>
                          <button
                            className="tag-pin"
                            onClick={() => togglePermanent(t, false)}
                            title={isPermanent ? '取消永久保留' : '永久保留'}
                          >
                            <PinIcon permanent={isPermanent} />
                          </button>
                          <button className="tag-remove" onClick={() => removeAvoidTopic(t)}>×</button>
                        </span>
                      );
                    })}
                  </div>
                  <div className="topic-input-row">
                    <input
                      type="text"
                      value={newAvoidTopic}
                      onChange={(e) => setNewAvoidTopic(e.target.value)}
                      onKeyDown={(e) => e.key === 'Enter' && handleAddAvoidTopic()}
                      placeholder="添加话题..."
                    />
                    <button onClick={handleAddAvoidTopic}>+</button>
                  </div>
                </div>

              </div>
            </div>
          )}
        </div>
      </SettingsSection>

      <SettingsSection
        title="高级设置"
        description="性格微调、开场白、模型辅助与导入导出。已有设定仍会生效，收起不会重置。"
        actions={profile.characterCard && <span className="personality-note">已导入角色卡</span>}
      >

        <SettingsSection title="性格微调" description="调整表达倾向；不修改操作权限或任务能力。">
          <div className="traits-grid">
            {(Object.keys(TRAIT_LABELS) as TraitName[]).map((trait) => (
              <TraitSlider
                key={trait}
                name={trait}
                value={traits[trait]}
                onChange={(v) => handleTraitChange(trait, v)}
              />
            ))}
          </div>
        </SettingsSection>

        <SettingsSection title="开场白" description="新对话开始时的问候语">
          <TextField
            label="打招呼"
            value={profile.greeting}
            placeholder="例如：欢迎回来，今天想聊些什么？"
            multiline
            rows={2}
            onChange={(greeting) => updateProfile({ greeting })}
          />
        </SettingsSection>

        <SettingsSection title="模型辅助设定" description="可选：调用当前模型生成建议，会使用模型额度；仅预览，不自动替换当前设置。">
          <div className="direction-input-group">
            <TextField
              label="希望的性格"
              value={descriptionInput}
              placeholder="例如：耐心的倾听者，轻松幽默但不过分热情。"
              multiline
              rows={3}
              onChange={setDescriptionInput}
            />
            <button
              className="btn btn-secondary btn-match-direction"
              onClick={handleDescriptionSubmit}
              disabled={matching || !descriptionInput.trim()}
            >
              {matching ? '生成中…' : '生成建议'}
            </button>
          </div>
          {matchError && <p className="match-error" role="alert">{matchError}</p>}
          {matchResult && (
            <div className="direction-match-card">
              <div className="direction-match-info">
                <h4>建议：{matchResult.direction_name}</h4>
                <p className="direction-match-desc">{matchResult.suggested_description}</p>
                <p className="personality-note">应用会替换相处方式和性格参数，不改名字和开场白。</p>
              </div>
              <button className="btn btn-secondary btn-sm" onClick={handleApplyDirection}>
                应用建议
              </button>
            </div>
          )}
        </SettingsSection>

        {profile.characterCard && (
          <SettingsSection title="角色卡设定" description="已导入的 SillyTavern 角色卡字段会在对话中生效" defaultOpen>
            <div className="form-grid">
              <TextField
                label="角色性格"
                value={profile.characterCard.personality}
                multiline
                rows={3}
                onChange={(personality) => updateProfile({ characterCard: { ...profile.characterCard!, personality } })}
              />
              <TextField
                label="场景"
                value={profile.characterCard.scenario}
                multiline
                rows={3}
                onChange={(scenario) => updateProfile({ characterCard: { ...profile.characterCard!, scenario } })}
              />
              <TextField
                label="系统提示"
                value={profile.characterCard.systemPrompt}
                multiline
                rows={3}
                onChange={(systemPrompt) => updateProfile({ characterCard: { ...profile.characterCard!, systemPrompt } })}
              />
              <TextField
                label="后续回复指引"
                value={profile.characterCard.postHistoryInstructions}
                multiline
                rows={3}
                onChange={(postHistoryInstructions) => updateProfile({ characterCard: { ...profile.characterCard!, postHistoryInstructions } })}
              />
              <TextField
                label="示例对话"
                value={profile.characterCard.mesExample}
                multiline
                rows={4}
                onChange={(mesExample) => updateProfile({ characterCard: { ...profile.characterCard!, mesExample } })}
              />
            </div>
          </SettingsSection>
        )}

        <SettingsSection title="导入/导出" description="备份或分享你的角色配置；导入后需点击右上角「保存」。">
          <div className="import-export-row">
            <button className="btn btn-secondary" onClick={handleExport}>
              <DownloadIcon />
              导出配置
            </button>
            <button className="btn btn-secondary" onClick={handleTavernExport}>
              <DownloadIcon />
              导出角色卡
            </button>
            <button className="btn btn-secondary" onClick={handleImport}>
              <UploadIcon />
              导入配置
            </button>
          </div>
          <p className="settings-section-desc">
            可导入 SillyTavern TavernCard V2 的 JSON 或 PNG 角色卡；导出角色卡为通用 V2 JSON。
          </p>
          {cardImportMessage && <p className="personality-note" role="status">{cardImportMessage}</p>}
        </SettingsSection>
      </SettingsSection>
    </div>
  );
}

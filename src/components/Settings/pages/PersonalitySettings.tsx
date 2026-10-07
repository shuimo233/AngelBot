import { useState, useCallback, useEffect } from 'react';
import { SettingsSection } from '../components/SettingsSection';
import { TextField } from '../components/TextField';
import { TraitSlider } from '../components/TraitSlider';
import { useSettingsStore } from '$stores/settings';
import { usePreferencesStore } from '$stores/preferences';
import { TRAIT_LABELS, defaultTraits } from '$lib/presets';
import { generateEvolutionSummary } from '$lib/evolution';
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
  const setEvolutionEnabled = usePreferencesStore((s) => s.setEvolutionEnabled);
  const addInterest = usePreferencesStore((s) => s.addInterest);
  const removeInterest = usePreferencesStore((s) => s.removeInterest);
  const addAvoidTopic = usePreferencesStore((s) => s.addAvoidTopic);
  const removeAvoidTopic = usePreferencesStore((s) => s.removeAvoidTopic);

  const [descriptionInput, setDescriptionInput] = useState('');
  const [localDescription, setLocalDescription] = useState(profile.bio);
  const [localTraits, setLocalTraits] = useState<TraitConfig>(() => ({
    ...defaultTraits,
    ...profile.traits,
    // Preserve the legacy language-style setting as a fallback for profiles
    // created before fine-grained traits were introduced.
    formality: profile.traits?.formality
      ?? (profile.languageStyle === 'formal' ? 3 : profile.languageStyle === 'concise' ? 2 : 0),
  }));

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
      // 自动将推荐的性格描述填入
      setLocalDescription(result.suggested_description);
      updateProfile({ bio: result.suggested_description });
    } catch (err) {
      setMatchError(err instanceof Error ? err.message : '推荐失败，请重试');
    } finally {
      setMatching(false);
    }
  }, [descriptionInput, updateProfile]);

  const handleApplyDirection = useCallback(() => {
    if (!matchResult) return;
    // 应用推荐的性格参数
    setLocalTraits(applySuggestedTraits(matchResult.suggested_traits));
    // 更新推荐的名称与性格描述；形象由应用统一呈现，不随人格方向替换。
    updateProfile({
      bio: matchResult.suggested_description,
      greeting: matchResult.suggested_greeting,
      name: matchResult.direction_name,
    });
  }, [matchResult, updateProfile]);

  const handleTraitChange = useCallback((trait: TraitName, value: number) => {
    setLocalTraits((prev) => {
      const traits = { ...prev, [trait]: value };
      // Keep the global settings draft in sync so the dialog's primary
      // “保存” action persists fine-grained personality changes as well.
      updateProfile({ traits });
      return traits;
    });
  }, [updateProfile]);

  const handleSaveAll = useCallback(() => {
    updateProfile({
      bio: localDescription,
      greeting: profile.greeting,
      name: profile.name,
      traits: localTraits,
    });
    useSettingsStore.getState().persistProfile();
  }, [localDescription, localTraits, profile, updateProfile]);

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
        bio: localDescription,
        greeting: profile.greeting,
      },
      traits: localTraits,
      exportedAt: new Date().toISOString(),
    };

    const blob = new Blob([JSON.stringify(exportData, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `angelbot-persona-${profile.name || 'custom'}-${Date.now()}.json`;
    a.click();
    URL.revokeObjectURL(url);
  }, [profile, localDescription, localTraits]);

  const handleTavernExport = useCallback(() => {
    const blob = new Blob([JSON.stringify(exportTavernCard(profile, localDescription), null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `angelbot-character-${profile.name || 'custom'}-${Date.now()}.json`;
    a.click();
    URL.revokeObjectURL(url);
  }, [profile, localDescription]);

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

        if (data.profile) {
          updateProfile({
            name: data.profile.name || '',
            avatar: data.profile.avatar || '',
            bio: data.profile.bio || '',
            greeting: data.profile.greeting || '',
          });
          setLocalDescription(data.profile.bio || '');
        }

        if (data.traits) {
          setLocalTraits(data.traits);
        }

        useSettingsStore.getState().persistProfile();
        setCardImportMessage('AngelBot profile imported.');
      } catch (err) {
        try {
          const card = await parseTavernCardFile(file);
          const nextProfile = applyTavernCard(card);
          updateProfile(nextProfile);
          setLocalDescription(nextProfile.bio || '');
          useSettingsStore.getState().persistProfile();
          setCardImportMessage(`Character card imported: ${card.name}`);
          return;
        } catch (cardError) {
          console.error('Failed to import persona:', cardError);
          setCardImportMessage(cardError instanceof Error ? cardError.message : 'Import failed. Please check the file format.');
          return;
        }
        console.error('Failed to import persona:', err);
        alert('导入失败，请检查文件格式');
      }
    };
    input.click();
  }, [updateProfile]);

  return (
    <div className="settings-page-content">
      {/* 性格描述 — 用户输入 → AI 推荐方向 */}
      <SettingsSection title="性格描述" description="描述你想要 Bot 的性格，AI 会推荐一个大致方向">
        <div className="direction-input-group">
          <TextField
            label="你的描述"
            value={descriptionInput}
            placeholder="例如：傲娇的学妹、温柔的倾听者、阳光开朗的大男孩..."
            multiline
            rows={3}
            onChange={setDescriptionInput}
          />
          <button
            className="btn btn-primary btn-match-direction"
            onClick={handleDescriptionSubmit}
            disabled={matching || !descriptionInput.trim()}
          >
            {matching ? (
              <>
                <span className="spinner"></span>
                分析中...
              </>
            ) : (
              'AI 推荐方向'
            )}
          </button>
        </div>

        {matchError && <p className="match-error">{matchError}</p>}

        {matchResult && (
          <div className="direction-match-card">
            <div className="direction-match-header">
              <span className="direction-avatar">{matchResult.direction_avatar}</span>
              <div className="direction-match-info">
                <h4>推荐方向：{matchResult.direction_name}</h4>
                <p className="direction-match-desc">{matchResult.suggested_description}</p>
              </div>
            </div>
            <button className="btn btn-secondary btn-sm" onClick={handleApplyDirection}>
              应用此方向并微调
            </button>
          </div>
        )}
      </SettingsSection>

      {/* 基础信息 */}
      <SettingsSection title="基础信息" description="设置 AngelBot 的称呼；形象由应用统一呈现。">
        <div className="avatar-section">
          <TextField
            label="名字"
            value={profile.name}
            placeholder="给你的 Bot 取个名字"
            onChange={(v) => updateProfile({ name: v })}
          />
        </div>
      </SettingsSection>

      {/* 自由描述 — 当前已应用的描述 */}
      <SettingsSection title="当前描述" description="当前生效的性格描述，可直接编辑">
        <TextField
          label="描述"
          value={localDescription}
          placeholder="描述你的 Bot 的性格特征..."
          multiline
          rows={3}
          onChange={(value) => {
            setLocalDescription(value);
            // The settings dialog owns persistence. Mirror this local field
            // into its draft so users do not need to find a second save button.
            updateProfile({ bio: value });
          }}
        />
      </SettingsSection>

      {/* 精细微调 */}
      <SettingsSection title="性格微调" description="基于推荐方向后，微调各项参数">
        <div className="traits-grid">
          {(Object.keys(TRAIT_LABELS) as TraitName[]).map((trait) => (
            <TraitSlider
              key={trait}
              name={trait}
              value={localTraits[trait]}
              onChange={(v) => handleTraitChange(trait, v)}
            />
          ))}
        </div>
      </SettingsSection>

      {/* 开场白 */}
      <SettingsSection title="开场白" description="新对话开始时的问候语">
        <TextField
          label="打招呼"
          value={profile.greeting}
          placeholder="你好呀~很高兴见到你！"
          multiline
          rows={2}
          onChange={(v) => updateProfile({ greeting: v })}
        />
      </SettingsSection>

      {/* 自进化设置 */}
      <SettingsSection
        title="自进化"
        description="AI 会学习你的偏好并调整行为"
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
              <span>启用自进化</span>
            </label>
            <p className="evolution-hint">开启后，AI 会自动学习你的轻微偏好</p>
          </div>

          {showEvolutionDetails && (
            <div className="evolution-details">
              <div className="evolution-summary">
                <h4>我学到的关于你</h4>
                <pre>{generateEvolutionSummary(preferences)}</pre>
              </div>

              <div className="evolution-proposals">
                <div className="evolution-proposals-header">
                  <div>
                    <h4>待确认的学习项</h4>
                    <p>自进化只会先提出候选，接受后才写入长期记忆。</p>
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

      {/* 导入/导出 */}
      <SettingsSection title="导入/导出" description="备份或分享你的角色配置">
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
          <button className="btn btn-primary" onClick={handleSaveAll}>
            保存所有设置
          </button>
        </div>
        <p className="settings-section-desc">
          可导入 SillyTavern TavernCard V2 的 JSON 或 PNG 角色卡；导出角色卡为通用 V2 JSON。
          {cardImportMessage ? ` ${cardImportMessage}` : ''}
        </p>
      </SettingsSection>
    </div>
  );
}

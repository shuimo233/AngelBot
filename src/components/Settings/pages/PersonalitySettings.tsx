import { useState, useCallback, useEffect, useId, useRef } from 'react';
import { PersonaNotesEditor } from './PersonaNotesEditor';
import { SettingsRow, SettingsSection } from '../components/SettingsSection';
import { TextField } from '../components/TextField';
import { Button, TextField as PreviewField } from '../../../ui';
import { TraitSlider } from '../components/TraitSlider';
import { LearnedHabits } from '../components/LearnedHabits';
import { useSettingsStore } from '$stores/settings';
import { usePreferencesStore } from '$stores/preferences';
import { TRAIT_LABELS, defaultTraits } from '$lib/presets';
import { describeUserPreferences } from '$lib/evolution';
import { applyTavernCard, exportTavernCard, parseTavernCardFile, PERSONA_NOTES_LIMIT } from '$lib/tavern-card';
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
import type { Persona, TraitName, TraitConfig } from '$types';

type PersonaImportPreview = { kind: 'profile' | 'card'; fileName: string; profile: Partial<Persona> };

const conversationExamples = [
  {
    label: '简洁协作',
    text: '先说结论和下一步，关键取舍再展开。遇到不确定的地方，直接告诉我。',
    reply: '先歇一会儿。今天只留一件必须做的事。',
    detail: '你把手头的安排发给我，我们一起挑。',
  },
  {
    label: '耐心陪伴',
    text: '聊天时自然、温和。我倾诉时先听我说，需要建议时再一起想办法。',
    reply: '辛苦了。想说的话，我听着。',
    detail: '今天发生了什么？慢慢说就好。',
  },
  {
    label: '一起推敲',
    text: '和我一起检查假设，给出具体例子。意见不同时直说原因，留出讨论的空间。',
    reply: '我们一起看看，是事情太多，还是有一件事格外耗力？',
    detail: '如果只是安排太满，可以先挑一件挪到明天。',
  },
];

function sampleReply(profile: Persona) {
  let lastExampleIndex = -1;
  let reply = '先歇一会儿吧。还有什么事是今天一定要做的？';
  let detail = '要是手头的事太多，我们可以把能往后放的先放一放。';
  for (const example of conversationExamples) {
    const index = profile.bio.lastIndexOf(example.text);
    if (index > lastExampleIndex) {
      lastExampleIndex = index;
      reply = example.reply;
      detail = example.detail;
    }
  }
  const verbosity = profile.traits?.verbosity ?? defaultTraits.verbosity;
  if (verbosity <= -2) return reply.split('。', 1)[0] + (reply.includes('。') ? '。' : '');
  return verbosity >= 2 ? `${reply}${detail}` : reply;
}

function ConversationPreview({ profile, draft, hasDraft }: { profile: Persona; draft: Persona; hasDraft: boolean }) {
  const headingId = useId();
  const greeting = profile.greeting.trim();
  const currentReply = sampleReply(profile);
  const draftReply = hasDraft ? sampleReply(draft) : null;
  const fields = [
    { label: '相处方式', value: profile.bio.trim() },
    { label: '人设补充', value: profile.personaNotes?.trim() ?? '' },
    ...(profile.characterCard ? [
      { label: '角色性格', value: profile.characterCard.personality.trim() },
      { label: '角色场景', value: profile.characterCard.scenario.trim() },
    ] : []),
  ].filter(({ value }) => value);
  return (
    <aside className="personality-preview" aria-labelledby={headingId}>
      <h3 id={headingId}>试聊示例</h3>
      <p className="personality-note">本地固定示例，随预设写法和回复长短变化。</p>
      <blockquote className="personality-preview-prompt">我今天有点累。</blockquote>
      <dl className="personality-preview-replies">
        <div>
          <dt>当前生效 · {profile.name.trim() || 'AngelBot'}</dt>
          <dd>{currentReply}</dd>
        </div>
        {draftReply !== null && draftReply !== currentReply && <div className="personality-preview-draft">
          <dt>未保存草稿 · {draft.name.trim() || 'AngelBot'}</dt>
          <dd>{draftReply}</dd>
        </div>}
      </dl>
      {draftReply === currentReply && <p className="personality-note">草稿的本地示例相同。</p>}
      {hasDraft && (greeting || fields.length > 0) && <details className="personality-preview-settings">
        <summary>查看当前设定</summary>
        <dl>
          {greeting && <div><dt>开场白</dt><dd>{greeting}</dd></div>}
          {fields.map(({ label, value }) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}
        </dl>
      </details>}
    </aside>
  );
}

function CardCompatibilityNote() {
  return <p className="personality-note">对话使用名字、描述、性格和场景。开场白仅供风格参考；高级字段和扩展数据仅保留供编辑或导出。仅替换 {'{{char}}'} 和 {'{{user}}'}，人物卡不会执行脚本或授予权限。</p>;
}

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
  const activeProfile = useSettingsStore((s) => s.activeProfile);
  const updateProfile = useSettingsStore((s) => s.updateProfile);
  const hasDraft = profile !== activeProfile
    && (Object.keys({ ...profile, ...activeProfile }) as (keyof Persona)[])
      .some((key) => profile[key] !== activeProfile[key]);

  const preferences = usePreferencesStore((s) => s.preferences);
  const memoryEnabled = preferences.habits.useLongTermMemory ?? true;
  const learningActive = preferences.evolutionEnabled && memoryEnabled;
  const learningHint = !preferences.evolutionEnabled
    ? '偏好学习已关闭。新对话请求、任务续接和定时任务不再自动复盘或沿用推测习惯。现有记录仍可查看和清理。'
    : !memoryEnabled
      ? '长期记忆已关闭，偏好学习暂停。可在「默认工作方式」开启长期记忆；现有记录仍可查看和清理。'
      : '完成对话后按数量和频率上限复盘新增表达，会使用当前模型额度。已发送的内容仍然保留。';
  const setMemoryControls = usePreferencesStore((s) => s.setMemoryControls);
  const memoryControlsBusy = usePreferencesStore((s) => s.memoryControlsBusy);
  const memoryControlsError = usePreferencesStore((s) => s.memoryControlsError);
  const addInterest = usePreferencesStore((s) => s.addInterest);
  const removeInterest = usePreferencesStore((s) => s.removeInterest);
  const addAvoidTopic = usePreferencesStore((s) => s.addAvoidTopic);
  const removeAvoidTopic = usePreferencesStore((s) => s.removeAvoidTopic);

  const [descriptionInput, setDescriptionInput] = useState('');
  const [showTraits, setShowTraits] = useState(false);
  const traitsId = useId();
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
  const [importPreview, setImportPreview] = useState<PersonaImportPreview | null>(null);
  const importVersion = useRef(0);
  useEffect(() => () => { importVersion.current += 1; }, []);

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
        personaNotes: profile.personaNotes ?? '',
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
      const version = ++importVersion.current;
      setImportPreview(null);
      setCardImportMessage('正在读取所选文件…');
      try {
        const png = file.name.toLowerCase().endsWith('.png') || file.type === 'image/png';
        const data = png ? null : JSON.parse(await file.text());
        let preview: PersonaImportPreview;
        if (data?.profile) {
          if (typeof data.profile !== 'object' || Array.isArray(data.profile)
            || ['name', 'avatar', 'bio', 'greeting', 'personaNotes'].some(key => data.profile[key] !== undefined && typeof data.profile[key] !== 'string')) {
            throw new Error('配置中的人设字段必须是文字。');
          }
          if ((data.profile.personaNotes?.length ?? 0) > PERSONA_NOTES_LIMIT) throw new Error('人设补充不能超过 4096 字。');
          preview = { kind: 'profile', fileName: file.name, profile: {
            name: data.profile.name || '', avatar: data.profile.avatar || '',
            bio: data.profile.bio || '', greeting: data.profile.greeting || '',
            characterCard: undefined, personaNotes: data.profile.personaNotes ?? '',
            ...(data.traits ? { traits: data.traits } : {}),
          } };
        } else {
          const card = await parseTavernCardFile(file);
          preview = { kind: 'card', fileName: file.name, profile: applyTavernCard(card) };
        }
        if (version !== importVersion.current) return;
        setImportPreview(preview);
        setCardImportMessage(null);
      } catch (cause) {
        if (version !== importVersion.current) return;
        setCardImportMessage(cause instanceof Error ? cause.message : '导入失败，请检查文件格式。');
      }
    };
    input.click();
  }, []);

  return (
    <div className="settings-page-content personality-settings">
      <div className="personality-workbench">
        <section className="personality-basic-fields" aria-labelledby="personality-daily-heading">
          <header>
            <h3 id="personality-daily-heading">日常相处</h3>
          </header>
          <TextField
            label="名字"
            value={profile.name}
            placeholder="例如：AngelBot"
            onChange={(v) => updateProfile({ name: v })}
          />
          <TextField
            label="相处方式"
            value={profile.bio}
            placeholder="例如：办事时先给结论；聊天时自然一点。"
            multiline
            rows={4}
            onChange={(bio) => updateProfile({ bio })}
          />
          <div className="personality-starters">
            <p>相处方式示例</p>
            <div className="personality-starter-actions">
              {conversationExamples.map(({ label, text }) => (
                <Button
                  key={label}
                  type="button"
                  variant="secondary"
                  size="small"
                  aria-pressed={profile.bio.includes(text)}
                  disabled={profile.bio.includes(text)}
                  onClick={() => updateProfile({ bio: profile.bio.trim() ? `${profile.bio.trimEnd()}\n${text}` : text })}
                >
                  {label}
                </Button>
              ))}
            </div>
          </div>
        </section>
        <ConversationPreview profile={activeProfile} draft={profile} hasDraft={hasDraft} />
      </div>

      <SettingsSection title="性格微调" action={
        <Button variant="secondary" size="small" aria-expanded={showTraits} aria-controls={traitsId}
          onClick={() => setShowTraits((visible) => !visible)}>
          {showTraits ? '收起滑杆' : '显示滑杆'}
        </Button>
      }>
        <div id={traitsId} hidden={!showTraits}>
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
        </div>
      </SettingsSection>

      <SettingsSection title="开场风格">
        <TextField
          label="开场白"
          value={profile.greeting}
          placeholder="例如：欢迎回来，今天想聊些什么？"
          multiline
          rows={2}
          onChange={(greeting) => updateProfile({ greeting })}
        />
      </SettingsSection>

      <SettingsSection title="人物卡与世界书" status={profile.characterCard ? '已导入角色卡' : undefined}>
        <div className="settings-rows">
          <SettingsRow title="导入" description="人物卡 V1–V3（JSON、含卡数据的 PNG）或 AngelBot 配置。">
            <Button onClick={handleImport}><UploadIcon />导入人物卡或配置</Button>
          </SettingsRow>
          <SettingsRow title="导出" description="角色卡导出为 JSON，V1 转为 V2。">
            <Button variant="ghost" onClick={handleExport}><DownloadIcon />导出配置</Button>
            <Button variant="ghost" onClick={handleTavernExport}><DownloadIcon />导出角色卡</Button>
          </SettingsRow>
        </div>
        {importPreview && <section className="settings-inline-review" aria-label="导入预览">
          <h4>{importPreview.kind === 'card' ? '人物卡预览' : '配置预览'}：{importPreview.fileName}</h4>
          <p className="personality-note">应用将替换名字、头像、相处方式、开场白和人设补充。缺少的字段会清空。</p>
          {importPreview.kind === 'card' ? <CardCompatibilityNote /> : <p className="personality-note">导入配置会移除原角色卡，并应用文件中的性格微调参数（若有）。</p>}
          <div className="settings-form-grid">
            <PreviewField label="预览名字" value={importPreview.profile.name ?? ''} readOnly />
            <PreviewField label="预览相处方式" value={importPreview.profile.bio ?? ''} readOnly multiline rows={3} />
            <PreviewField label="预览开场白（风格参考）" value={importPreview.profile.greeting ?? ''} readOnly multiline rows={2} />
            {importPreview.profile.characterCard && <>
              <PreviewField label="预览角色性格" value={importPreview.profile.characterCard.personality} readOnly multiline rows={2} />
              <PreviewField label="预览场景" value={importPreview.profile.characterCard.scenario} readOnly multiline rows={2} />
            </>}
          </div>
          <PersonaNotesEditor key={importPreview.fileName} card={importPreview.profile.characterCard}
            value={importPreview.profile.personaNotes ?? ''} preview onChange={personaNotes => setImportPreview(previous => previous ? { ...previous, profile: { ...previous.profile, personaNotes } } : previous)} />
          {importPreview.profile.avatar && <p className="personality-note">头像随草稿替换。预览不会加载外部图片。</p>}
          <div className="import-export-row">
            <button type="button" className="btn btn-primary" onClick={() => {
              importVersion.current += 1;
              updateProfile(importPreview.profile); setImportPreview(null);
              setCardImportMessage('已应用到草稿，点击右上角「保存」后保留。');
            }}>应用到草稿</button>
            <button type="button" className="btn btn-secondary" onClick={() => {
              importVersion.current += 1;
              setImportPreview(null); setCardImportMessage('已取消导入，原有草稿未改变。');
            }}>取消导入</button>
          </div>
        </section>}
        {cardImportMessage && <p className="personality-note" role="status">{cardImportMessage}</p>}

        <div className="personality-card-background">
          <PersonaNotesEditor card={profile.characterCard} value={profile.personaNotes ?? ''} onChange={personaNotes => updateProfile({ personaNotes })} />
        </div>
        {profile.characterCard && (
          <SettingsSection title="角色卡字段">
            {!importPreview && <CardCompatibilityNote />}
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
      </SettingsSection>

      <SettingsSection title="偏好学习">
        <div className="evolution-section">
          <div className="evolution-toggle">
            <label className="toggle-label">
              <input
                type="checkbox"
                checked={preferences.evolutionEnabled}
                disabled={memoryControlsBusy}
                onChange={(e) => void setMemoryControls({ evolutionEnabled: e.target.checked })}
              />
              <span>启用偏好学习</span>
            </label>
            <p className="evolution-hint" aria-live="polite">{learningHint}</p>
            {memoryControlsError && <p role="alert">偏好学习开关未能同步。{memoryControlsError}</p>}
            <button type="button" className="btn btn-secondary btn-sm" aria-expanded={showEvolutionDetails}
              onClick={() => setShowEvolutionDetails(!showEvolutionDetails)}>
              {showEvolutionDetails ? '收起' : '查看详情'}
            </button>
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

      <SettingsSection title="模型辅助设定" description="生成建议会使用当前模型的额度。">
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
    </div>
  );
}

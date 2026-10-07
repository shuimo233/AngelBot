/**
 * ChatToolbar — sits between ChatHeader and the message list.
 * Houses: thinking effort selector + model selector + branch switcher.
 * Freed from the ComposerInput stack, it now lives in a dedicated
 * strip so the composer bar stays focused on text input.
 */
import { useState, useEffect, useRef } from 'react';
import { useThinkingEffortStore, type ThinkingEffort } from '$stores/thinkingEffort';
import { useSettingsStore } from '$stores/settings';
import { useSessionsStore } from '$stores/sessions';
import { useMessagesStore } from '$stores/messages';
import { getReasoningCapability } from '$lib/modelCapabilities';
import { getBranchTree, switchToMessageBranch, type BranchTree } from '$lib/commands/session-tree';
import { getMessages } from '$lib/commands/message';
import { ALL_PROVIDERS } from '$lib/providers';
import './ChatToolbar.css';

const ClockIcon = () => (
  <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
    <circle cx="12" cy="12" r="10" />
    <polyline points="12 6 12 12 16 14" />
  </svg>
);

const ChevronIcon = ({ down }: { down?: boolean }) => (
  <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5"
    style={{ transform: down ? 'rotate(180deg)' : 'none', transition: 'transform 0.15s' }}>
    <polyline points="6 9 12 15 18 9" />
  </svg>
);

const ModelIcon = () => (
  <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
    <polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />
  </svg>
);

const BranchIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
    <line x1="6" y1="3" x2="6" y2="15" />
    <circle cx="18" cy="6" r="3" />
    <circle cx="6" cy="18" r="3" />
    <path d="M18 9a9 9 0 0 1-9 9" />
  </svg>
);

const CheckIcon = () => (
  <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3">
    <polyline points="20 6 9 17 4 12" />
  </svg>
);

const THINKING_OPTIONS: Array<{ value: ThinkingEffort; label: string; desc: string }> = [
  { value: 'low',    label: '低', desc: '快速回复，适合简单任务' },
  { value: 'medium', label: '中', desc: '速度与质量平衡' },
  { value: 'high',   label: '高', desc: '深度推理，适合复杂任务' },
];

function SmallDropdown({
  value,
  icon,
  options,
  onSelect,
  isOpen,
  onToggle,
  disabled = false,
}: {
  value: string;
  icon: React.ReactNode;
  options: Array<{ value: string; label: string; desc?: string }>;
  onSelect: (value: string) => void;
  isOpen: boolean;
  onToggle: () => void;
  disabled?: boolean;
}) {
  return (
    <div style={{ position: 'relative' }}>
      <button
        type="button"
        onClick={onToggle}
        disabled={disabled}
        style={{
          display: 'flex', alignItems: 'center', gap: 5,
          height: 28, padding: '0 9px',
          background: isOpen ? 'var(--color-accent-subtle)' : 'rgba(255,255,255,0.62)',
          border: `1px solid ${isOpen ? 'var(--color-accent-border)' : 'var(--color-border-muted)'}`,
          borderRadius: 7, color: isOpen ? 'var(--color-accent)' : 'var(--color-muted)',
          cursor: 'pointer', fontSize: 11, fontWeight: 500,
          transition: 'background 0.12s, color 0.12s, border-color 0.12s',
          whiteSpace: 'nowrap',
        }}
      >
        {icon}
        {value}
        <ChevronIcon down />
      </button>

      {isOpen && (
        <div style={{
          position: 'absolute', top: '100%', left: 0, marginTop: 5,
          minWidth: 200, background: 'rgba(251, 250, 247, 0.98)',
          backdropFilter: 'blur(14px)', border: '1px solid var(--color-border)',
          borderRadius: 8, padding: 4,
          boxShadow: '0 18px 32px var(--shadow)', zIndex: 500,
        }}>
          {options.map((opt) => (
            <button
              key={opt.value}
              type="button"
              onClick={() => { onSelect(opt.value); onToggle(); }}
              style={{
                display: 'flex', flexDirection: 'column', alignItems: 'flex-start', gap: 1,
                width: '100%', padding: '6px 10px',
                background: opt.value === value ? 'var(--color-accent-subtle)' : 'transparent',
                border: 'none', borderRadius: 6, cursor: 'pointer',
                fontSize: 11, fontWeight: opt.value === value ? 600 : 400,
                color: opt.value === value ? 'var(--color-accent)' : 'var(--color-text)',
                textAlign: 'left', transition: 'background 0.08s',
              }}
            >
              <span>{opt.label}</span>
              {opt.desc && (
                <span style={{ fontSize: 10, color: 'var(--color-muted)', fontWeight: 400 }}>
                  {opt.desc}
                </span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

interface BranchSwitcherProps {
  branchTree: BranchTree | null;
  isOpen: boolean;
  onToggle: () => void;
  onSwitch: (branchMessageId: string) => Promise<void>;
}

function BranchSwitcher({ branchTree, isOpen, onToggle, onSwitch }: BranchSwitcherProps) {
  const dropdownRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!isOpen) return;
    const handleClickOutside = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        onToggle();
      }
    };
    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, [isOpen, onToggle]);

  if (!branchTree || !Array.isArray(branchTree.branches) || branchTree.branches.length <= 1) {
    return null;
  }

  const activeBranch = branchTree.branches.find((b) => b.is_active);

  return (
    <div className="branch-switcher" ref={dropdownRef}>
      <button
        type="button"
        onClick={onToggle}
        className={`branch-switcher-button ${isOpen ? 'active' : ''}`}
      >
        <BranchIcon />
        <span>{activeBranch?.name ?? '分支'}</span>
        <span className="branch-count-badge">{branchTree.branches.length}</span>
        <ChevronIcon down />
      </button>

      {isOpen && (
        <div className="branch-dropdown">
          {branchTree.branches.map((branch) => (
            <button
              key={branch.message_id}
              type="button"
              onClick={() => onSwitch(branch.message_id)}
              className={`branch-dropdown-item ${branch.is_active ? 'active' : ''}`}
            >
              <BranchIcon />
              <span>{branch.name ?? '未命名分支'}</span>
              {branch.is_active && (
                <span className="check-icon">
                  <CheckIcon />
                </span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

export function ChatToolbar({ branchOnly = false }: { branchOnly?: boolean }) {
  const { effort, setEffort } = useThinkingEffortStore();
  const apiConfig = useSettingsStore((s) => s.activeApiConfig);
  const currentSession = useSessionsStore((s) => s.activeSession);
  const updateSessionModel = useSessionsStore((s) => s.updateSessionModel);
  const loadMessages = useMessagesStore((s) => s.loadMessages);

  const [thinkingOpen, setThinkingOpen] = useState(false);
  const [branchOpen, setBranchOpen] = useState(false);
  const [branchTree, setBranchTree] = useState<BranchTree | null>(null);
  const [branchSwitching, setBranchSwitching] = useState(false);
  const [modelOpen, setModelOpen] = useState(false);
  const [modelDropdownRef, setModelDropdownRef] = useState<HTMLDivElement | null>(null);

  const reasoning = getReasoningCapability(apiConfig);
  const availableThinkingOptions = THINKING_OPTIONS.filter((option) => reasoning.options.includes(option.value));

  const thinkingLabel = THINKING_OPTIONS.find((o) => o.value === effort)?.label ?? '中';
  const displayThinkingLabel = reasoning.supported ? thinkingLabel : 'Unavailable';

  // Session-level model: use session binding, fall back to global API config
  const sessionProvider = currentSession?.agentProvider ?? apiConfig?.provider ?? '';
  const sessionModel = currentSession?.agentModel ?? apiConfig?.model ?? '';
  const modelLabel = sessionModel.trim() || '选择模型';

  // Flatten all provider + model options
  const modelOptions = ALL_PROVIDERS.flatMap((p) =>
    p.models.map((m) => ({
      value: `${p.id}::${m.value}`,
      label: `${p.name} / ${m.label}`,
      provider: p.id,
      model: m.value,
    }))
  );

  const handleModelSelect = async (value: string) => {
    if (!currentSession?.id) return;
    const [provider, model] = value.split('::');
    try {
      await updateSessionModel(currentSession.id, provider, model);
    } catch (err) {
      console.error('[ChatToolbar] Failed to update session model:', err);
    }
    setModelOpen(false);
  };

  const currentModelOption = modelOptions.find(
    (o) => o.provider === sessionProvider && o.model === sessionModel
  );
  const displayedModelLabel = currentModelOption?.label ?? modelLabel;

  // Close model dropdown on outside click
  useEffect(() => {
    if (!modelOpen) return;
    const handleClick = (e: MouseEvent) => {
      if (modelDropdownRef && !modelDropdownRef.contains(e.target as Node)) {
        setModelOpen(false);
      }
    };
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [modelOpen, modelDropdownRef]);

  useEffect(() => {
    if (!currentSession?.id) {
      setBranchTree(null);
      return;
    }
    getBranchTree(currentSession.id)
      .then(setBranchTree)
      .catch((err) => {
        console.error('Failed to load branch tree:', err);
        setBranchTree(null);
      });
  }, [currentSession?.id]);

  const handleBranchSwitch = async (branchMessageId: string) => {
    if (!currentSession?.id || branchSwitching) return;
    setBranchSwitching(true);
    setBranchOpen(false);
    try {
      await switchToMessageBranch(currentSession.id, branchMessageId);
      loadMessages(await getMessages(currentSession.id));
      const newTree = await getBranchTree(currentSession.id);
      setBranchTree(newTree);
    } catch (err) {
      console.error('Failed to switch branch:', err);
    } finally {
      setBranchSwitching(false);
    }
  };

  // A workspace normally has one linear main conversation. Do not reserve a
  // toolbar strip until a real alternate branch exists; the rollback control
  // should appear only when it gives the user an actionable choice.
  if (branchOnly && (!Array.isArray(branchTree?.branches) || branchTree.branches.length <= 1)) {
    return null;
  }

  return (
    <div className="chat-toolbar">
      {!branchOnly && <div className="chat-toolbar-left">
        <span className="chat-toolbar-label">推理</span>
        <SmallDropdown
          value={displayThinkingLabel}
          icon={<ClockIcon />}
          options={availableThinkingOptions}
          onSelect={(v) => setEffort(v as ThinkingEffort)}
          isOpen={thinkingOpen}
          onToggle={() => reasoning.supported && setThinkingOpen((v) => !v)}
          disabled={!reasoning.supported}
        />
        <div style={{ position: 'relative' }} ref={setModelDropdownRef}>
          <button
            type="button"
            onClick={() => setModelOpen((v) => !v)}
            title={displayedModelLabel}
            style={{
              display: 'flex', alignItems: 'center', gap: 5,
              height: 28, padding: '0 9px',
              background: modelOpen ? 'var(--color-accent-subtle)' : 'rgba(255,255,255,0.62)',
              border: `1px solid ${modelOpen ? 'var(--color-accent-border)' : 'var(--color-border-muted)'}`,
              borderRadius: 7, color: modelOpen ? 'var(--color-accent)' : 'var(--color-muted)',
              cursor: 'pointer', fontSize: 11, fontWeight: 500,
              transition: 'background 0.12s, color 0.12s, border-color 0.12s',
              whiteSpace: 'nowrap',
            }}
          >
            <ModelIcon />
            <span style={{ maxWidth: 160, overflow: 'hidden', textOverflow: 'ellipsis' }}>
              {displayedModelLabel}
            </span>
            <ChevronIcon down />
          </button>
          {modelOpen && (
            <div style={{
              position: 'absolute', top: '100%', left: 0, marginTop: 5,
              minWidth: 260, maxHeight: 300, overflowY: 'auto',
              background: 'rgba(251, 250, 247, 0.98)',
              backdropFilter: 'blur(14px)',
              border: '1px solid var(--color-border)',
              borderRadius: 8, padding: 4,
              boxShadow: '0 18px 32px var(--shadow)', zIndex: 500,
            }}>
              {ALL_PROVIDERS.map((provider) => (
                <div key={provider.id}>
                  <div style={{
                    padding: '6px 10px 4px',
                    fontSize: 10, fontWeight: 600,
                    color: 'var(--color-muted)',
                    textTransform: 'uppercase', letterSpacing: '0.05em',
                  }}>
                    {provider.name}
                  </div>
                  {provider.models.map((model) => {
                    const key = `${provider.id}::${model.value}`;
                    const isSelected = currentModelOption?.value === key;
                    return (
                      <button
                        key={key}
                        type="button"
                        onClick={() => handleModelSelect(key)}
                        style={{
                          display: 'flex', alignItems: 'center', gap: 6,
                          width: '100%', padding: '5px 10px',
                          background: isSelected ? 'var(--color-accent-subtle)' : 'transparent',
                          border: 'none', borderRadius: 6, cursor: 'pointer',
                          fontSize: 11,
                          fontWeight: isSelected ? 600 : 400,
                          color: isSelected ? 'var(--color-accent)' : 'var(--color-text)',
                          textAlign: 'left', transition: 'background 0.08s',
                        }}
                      >
                        {isSelected && <CheckIcon />}
                        <span style={{ marginLeft: isSelected ? 0 : 18 }}>{model.label}</span>
                      </button>
                    );
                  })}
                </div>
              ))}
            </div>
          )}
        </div>
      </div>}
      <div className="chat-toolbar-right">
        <BranchSwitcher
          branchTree={branchTree}
          isOpen={branchOpen}
          onToggle={() => setBranchOpen((v) => !v)}
          onSwitch={handleBranchSwitch}
        />
      </div>
    </div>
  );
}

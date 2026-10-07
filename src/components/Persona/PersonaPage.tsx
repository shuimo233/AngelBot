import { useState } from 'react';
import { useSettingsStore } from '$stores/settings';
import { TraitSlider } from '../Settings/components/TraitSlider';
import { PRESET_TEMPLATES, TRAIT_LABELS } from '$lib/presets';
import type { TraitName } from '$types';
import './PersonaPage.css';

const TRAIT_NAMES: TraitName[] = ['tone', 'verbosity', 'formality', 'humor', 'dependence', 'intimacy', 'patience'];

export function PersonaPage() {
  const profile = useSettingsStore((s) => s.profile);
  const updateProfile = useSettingsStore((s) => s.updateProfile);
  const [selectedTrait, setSelectedTrait] = useState<TraitName>('tone');

  // traits from profile.tone -> map to slider values
  // For simplicity, we derive a trait value from tone + personality
  const traitValues: Record<TraitName, number> = {
    tone: profile.tone === 'friendly' ? -2 : profile.tone === 'gentle' ? -3 : profile.tone === 'professional' ? 2 : 0,
    verbosity: profile.languageStyle === 'detailed' ? 3 : profile.languageStyle === 'concise' ? -3 : 0,
    formality: profile.languageStyle === 'formal' ? 3 : profile.languageStyle === 'casual' ? -3 : 0,
    humor: profile.personality === 'cheerful' ? 3 : profile.personality === 'serious' ? -3 : 0,
    dependence: 0,
    intimacy: profile.personality === 'cute' ? 3 : profile.personality === 'cool' ? -2 : 0,
    patience: 5,
  };

  const handleTraitChange = (trait: TraitName, value: number) => {
    traitValues[trait] = value;
    // Sync back to profile for persistence
    const toneMap: Record<string, string> = {
      '-5': 'neutral', '-4': 'neutral', '-3': 'friendly', '-2': 'friendly',
      '-1': 'neutral', '0': 'neutral', '1': 'neutral',
      '2': 'professional', '3': 'professional', '4': 'professional', '5': 'professional',
    };
    updateProfile({
      tone: (toneMap[String(value)] ?? 'neutral') as typeof profile.tone,
    });
  };

  return (
    <div className="persona-page">
      <div className="persona-header">
        <h2 className="persona-page-title">Persona</h2>
      </div>

      <div className="persona-content">
        {/* Left: Editor */}
        <div className="persona-editor">
          <div className="persona-section">
            <h3 className="persona-section-title">当前人格</h3>
            <div className="persona-name-display">{profile.name || '未命名人格'}</div>
            <div className="persona-bio-display">
              {profile.bio || '尚无人设描述'}
            </div>
          </div>

          <div className="persona-section">
            <h3 className="persona-section-title">预设模板</h3>
            <div className="persona-presets">
              {PRESET_TEMPLATES.map((preset) => (
                <button
                  key={preset.id}
                  className="persona-preset-btn"
                  onClick={() => {
                    updateProfile({ name: preset.name, bio: preset.description });
                    // Reload to pick up trait changes
                  }}
                >
                  {preset.name}
                </button>
              ))}
            </div>
          </div>

          <div className="persona-section">
            <h3 className="persona-section-title">特质调节</h3>
            <div className="persona-trait-selector">
              {TRAIT_NAMES.map((trait) => (
                <button
                  key={trait}
                  className={`persona-trait-chip ${selectedTrait === trait ? 'active' : ''}`}
                  onClick={() => setSelectedTrait(trait)}
                >
                  {TRAIT_LABELS[trait].left}/{TRAIT_LABELS[trait].right}
                </button>
              ))}
            </div>
            <div className="persona-trait-slider-wrapper">
              <TraitSlider
                name={selectedTrait}
                value={traitValues[selectedTrait]}
                onChange={(v) => handleTraitChange(selectedTrait, v)}
              />
            </div>
          </div>
        </div>

        {/* Right: Preview */}
        <div className="persona-preview-panel">
          <h3 className="persona-section-title">实时预览</h3>
          <div className="persona-preview-response-box">
            <div className="persona-preview-label">当前 Persona 设定</div>
            <div className="persona-preview-traits-summary">
              {TRAIT_NAMES.map((trait) => {
                const val = traitValues[trait];
                const labels = TRAIT_LABELS[trait];
                const isLeft = val < 0;
                const isRight = val > 0;
                return (
                  <div key={trait} className="persona-trait-summary-row">
                    <span className="persona-trait-summary-name">{labels.left}</span>
                    <div className="persona-trait-summary-bar">
                      <div
                        className="persona-trait-summary-fill"
                        style={{
                          width: `${Math.abs(val) * 10}%`,
                          background: isLeft
                            ? 'var(--color-accent)'
                            : isRight
                            ? 'var(--color-purple)'
                            : 'var(--color-border)',
                          marginLeft: isLeft ? 'auto' : isRight ? '0' : '50%',
                          transform: isRight ? 'translateX(-100%)' : undefined,
                        }}
                      />
                    </div>
                    <span className="persona-trait-summary-name">{labels.right}</span>
                  </div>
                );
              })}
            </div>
          </div>
          <div className="persona-preview-tips">
            <div className="persona-preview-tip-title">人格预览说明</div>
            <p className="persona-preview-tip-text">
              当前设定会影响 AI 的回复风格。向左移动使该特质减弱，向右增强。
              完整的性格调节可在 Settings → Personality 中进行。
            </p>
          </div>
        </div>
      </div>
    </div>
  );
}

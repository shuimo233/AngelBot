import type { TraitName } from '$types';
import { TRAIT_LABELS } from '$lib/presets';

interface TraitSliderProps {
  name: TraitName;
  value: number;
  onChange: (value: number) => void;
}

export function TraitSlider({ name, value, onChange }: TraitSliderProps) {
  const labels = TRAIT_LABELS[name];

  return (
    <div className="trait-slider">
      <div className="trait-slider-labels">
        <span className="trait-label-left">{labels.left}</span>
        <span className="trait-label-center">
          {value > 0 ? `+${value}` : value}
        </span>
        <span className="trait-label-right">{labels.right}</span>
      </div>
      <div className="trait-slider-track-wrapper">
        <input
          type="range"
          min="-5"
          max="5"
          step="1"
          value={value}
          onChange={(e) => onChange(parseInt(e.target.value))}
          className="trait-slider-input"
        />
        <div className="trait-slider-markers">
          {[-5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5].map((v) => (
            <span key={v} className={`marker ${v === 0 ? 'zero' : ''} ${v === value ? 'active' : ''}`} />
          ))}
        </div>
      </div>
    </div>
  );
}

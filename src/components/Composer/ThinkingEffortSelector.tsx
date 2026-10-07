import { useState } from 'react';
import { useThinkingEffortStore, type ThinkingEffort } from '$stores/thinkingEffort';
import './ThinkingEffortSelector.css';

const EFFORT_OPTIONS: { value: ThinkingEffort; label: string; description: string }[] = [
  { value: 'low', label: '低', description: '快速响应' },
  { value: 'medium', label: '中', description: '平衡' },
  { value: 'high', label: '高', description: '深度思考' },
];

export function ThinkingEffortSelector() {
  const { effort, setEffort } = useThinkingEffortStore();
  const [expanded, setExpanded] = useState(false);

  const currentOption = EFFORT_OPTIONS.find(o => o.value === effort);

  return (
    <div className={`thinking-effort-container ${expanded ? 'expanded' : ''}`}>
      <button
        className="thinking-effort-trigger"
        onClick={() => setExpanded(!expanded)}
        title={currentOption?.description}
      >
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
          <circle cx="12" cy="12" r="10" />
          <path d="M12 6v6l4 2" />
        </svg>
        <span>思考: {currentOption?.label}</span>
        <svg
          width="10"
          height="10"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          style={{ transform: expanded ? 'rotate(180deg)' : 'rotate(0)', transition: 'transform 0.2s' }}
        >
          <polyline points="6 9 12 15 18 9" />
        </svg>
      </button>

      {expanded && (
        <div className="thinking-effort-dropdown">
          {EFFORT_OPTIONS.map((option) => (
            <button
              key={option.value}
              className={`thinking-effort-option ${effort === option.value ? 'active' : ''}`}
              onClick={() => {
                setEffort(option.value);
                setExpanded(false);
              }}
            >
              <span className="effort-label">{option.label}</span>
              <span className="effort-desc">{option.description}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

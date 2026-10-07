import { useEffect, useRef, useState } from 'react';
import { useSettingsStore } from '$stores/settings';
import { useThinkingEffortStore, type ThinkingEffort } from '$stores/thinkingEffort';
import './ComposerToolbar.css';

const ClockIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
    <circle cx="12" cy="12" r="10" />
    <polyline points="12 6 12 12 16 14" />
  </svg>
);

const ChevronIcon = ({ down = false }: { down?: boolean }) => (
  <svg
    width="10"
    height="10"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="2"
    style={{ transform: down ? 'rotate(180deg)' : 'rotate(0deg)', transition: 'transform 0.15s' }}
  >
    <polyline points={down ? '6 9 12 15 18 9' : '18 15 12 9 6 15'} />
  </svg>
);

const ModelIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
    <path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z" />
    <polyline points="3.27 6.96 12 12.01 20.73 6.96" />
    <line x1="12" y1="22.08" x2="12" y2="12" />
  </svg>
);

const CompactIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
    <line x1="4" y1="6" x2="20" y2="6" />
    <line x1="4" y1="12" x2="20" y2="12" />
    <line x1="4" y1="18" x2="20" y2="18" />
  </svg>
);

const THINKING_OPTIONS: Array<{ value: ThinkingEffort; label: string; desc: string }> = [
  { value: 'low', label: '\u4f4e', desc: '\u66f4\u5feb\u8fd4\u56de\uff0c\u9002\u5408\u8f7b\u91cf\u5bf9\u8bdd' },
  { value: 'medium', label: '\u4e2d', desc: '\u901f\u5ea6\u4e0e\u8d28\u91cf\u66f4\u5747\u8861' },
  { value: 'high', label: '\u9ad8', desc: '\u66f4\u6df1\u5165\u63a8\u7406\uff0c\u9002\u5408\u590d\u6742\u4efb\u52a1' },
];

interface DropdownProps {
  label: string;
  value: string;
  icon: React.ReactNode;
  options: Array<{ value: string; label: string; desc?: string }>;
  onSelect: (value: string) => void;
  isOpen: boolean;
  onToggle: () => void;
  onClose: () => void;
}

function Dropdown({
  label,
  value,
  icon,
  options,
  onSelect,
  isOpen,
  onToggle,
  onClose,
}: DropdownProps) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!isOpen) return;

    const handleClickOutside = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) {
        onClose();
      }
    };

    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, [isOpen, onClose]);

  return (
    <div className={`composer-dropdown ${isOpen ? 'open' : ''}`} ref={ref}>
      <button type="button" className="composer-dropdown-trigger" onClick={onToggle} title={label}>
        {icon}
        <span>{value}</span>
        <ChevronIcon down />
      </button>
      {isOpen && (
        <div className="composer-dropdown-menu">
          {options.map((option) => (
            <button
              key={option.value}
              type="button"
              className={`composer-dropdown-item ${option.label === value ? 'active' : ''}`}
              onClick={() => {
                onSelect(option.value);
                onClose();
              }}
            >
              <span className="dropdown-item-label">{option.label}</span>
              {option.desc && <span className="dropdown-item-desc">{option.desc}</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

interface ComposerToolbarProps {
  compact?: boolean;
  onCompactChange?: (compact: boolean) => void;
}

export function ComposerToolbar({ compact = false, onCompactChange }: ComposerToolbarProps) {
  const { effort, setEffort } = useThinkingEffortStore();
  const apiConfig = useSettingsStore((state) => state.activeApiConfig);
  const [openDropdown, setOpenDropdown] = useState<string | null>(null);

  const thinkingLabel =
    THINKING_OPTIONS.find((option) => option.value === effort)?.label ?? '\u4e2d';
  const modelLabel = apiConfig?.model?.trim() || '\u914d\u7f6e\u6a21\u578b';
  const providerLabel = apiConfig?.provider?.trim() || '\u672a\u9009\u62e9\u63d0\u4f9b\u5546';

  const openApiSettings = () => {
    window.dispatchEvent(new CustomEvent('open-settings', { detail: 'api' }));
  };

  return (
    <div className="composer-toolbar">
      <div className="composer-toolbar-left">
        <span className="composer-toolbar-label">{'\u601d\u8003'}</span>
        <Dropdown
          label={'\u601d\u8003\u5f3a\u5ea6'}
          value={thinkingLabel}
          icon={<ClockIcon />}
          options={THINKING_OPTIONS}
          onSelect={(value) => setEffort(value as ThinkingEffort)}
          isOpen={openDropdown === 'thinking'}
          onToggle={() => setOpenDropdown(openDropdown === 'thinking' ? null : 'thinking')}
          onClose={() => setOpenDropdown(null)}
        />
        <button
          type="button"
          className="composer-toolbar-btn"
          onClick={openApiSettings}
          title={`${providerLabel} / ${modelLabel}`}
        >
          <ModelIcon />
          <span>{modelLabel}</span>
        </button>
      </div>
      {onCompactChange && (
        <div className="composer-toolbar-right">
          <button
            type="button"
            className={`composer-toolbar-btn ${compact ? 'active' : ''}`}
            onClick={() => onCompactChange(!compact)}
            title={'\u7cbe\u7b80\u6a21\u5f0f'}
          >
            <CompactIcon />
            <span>{'\u7cbe\u7b80'}</span>
          </button>
        </div>
      )}
    </div>
  );
}

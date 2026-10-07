import type { PersonalityTemplate } from '$types';

interface TemplateCardProps {
  template: PersonalityTemplate;
  isSelected: boolean;
  onSelect: (template: PersonalityTemplate) => void;
}

export function TemplateCard({ template, isSelected, onSelect }: TemplateCardProps) {
  return (
    <button
      className={`template-card ${isSelected ? 'selected' : ''}`}
      onClick={() => onSelect(template)}
    >
      <span className="template-name">{template.name}</span>
      {template.description && (
        <span className="template-desc">{template.description}</span>
      )}
    </button>
  );
}

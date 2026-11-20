import { useId } from 'react';

export function RadioChoices<T extends string>({ label, value, options, onChange }: {
  label: string;
  value: T;
  options: readonly { value: T; label: string }[];
  onChange: (value: T) => void;
}) {
  const name = useId();
  return (
    <div className="ui-radio-choices" role="radiogroup" aria-label={label}>
      {options.map(option => (
        <label className="ui-radio-choice" key={option.value}>
          <input type="radio" name={name} value={option.value} checked={option.value === value}
            onChange={() => onChange(option.value)} />
          <span>{option.label}</span>
        </label>
      ))}
    </div>
  );
}

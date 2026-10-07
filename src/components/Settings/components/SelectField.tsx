import { SelectField as UISelectField } from '../../../ui';

interface SelectFieldProps {
  label: string;
  value: string;
  options: { value: string; label: string }[];
  onChange: (value: string) => void;
  disabled?: boolean;
}

export function SelectField({ label, value, options, onChange, disabled }: SelectFieldProps) {
  return (
    <UISelectField
      label={label}
      value={value}
      options={options}
      onChange={(event) => onChange(event.target.value)}
      disabled={disabled}
      wrapperClassName={`select-field ${disabled ? 'disabled' : ''}`}
    />
  );
}

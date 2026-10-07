import { Switch } from '../../../ui';

interface ToggleFieldProps {
  label: string;
  description?: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
}

export function ToggleField({ label, description, checked, disabled = false, onChange }: ToggleFieldProps) {
  return (
    <Switch
      label={label}
      hint={description}
      checked={checked}
      disabled={disabled}
      onChange={(event) => onChange(event.target.checked)}
      wrapperClassName="toggle-field"
    />
  );
}

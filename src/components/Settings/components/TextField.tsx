import { TextField as UITextField } from '../../../ui';

interface TextFieldProps {
  label: string;
  value: string;
  placeholder?: string;
  multiline?: boolean;
  rows?: number;
  onChange: (value: string) => void;
  disabled?: boolean;
}

export function TextField({ label, value, placeholder, multiline, rows = 3, onChange, disabled }: TextFieldProps) {
  const wrapperClassName = `field ${multiline ? 'textarea-field' : 'input-field'} ${disabled ? 'disabled' : ''}`;
  const sharedProps = { label, value, placeholder, disabled, wrapperClassName };
  return multiline ? (
    <UITextField {...sharedProps} multiline rows={rows} onChange={(event) => onChange(event.target.value)} />
  ) : (
    <UITextField {...sharedProps} onChange={(event) => onChange(event.target.value)} />
  );
}

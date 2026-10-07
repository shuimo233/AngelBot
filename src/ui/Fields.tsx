import {
  forwardRef,
  useId,
  type ForwardedRef,
  type InputHTMLAttributes,
  type ReactNode,
  type SelectHTMLAttributes,
  type TextareaHTMLAttributes,
} from 'react';
import { Input, Select } from './Input';

interface FieldDetails {
  label: ReactNode;
  hint?: ReactNode;
  error?: ReactNode;
  wrapperClassName?: string;
}

function useFieldIds(id: string | undefined, hint: ReactNode, error: ReactNode, describedBy?: string) {
  const generatedId = useId();
  const controlId = id ?? `ui-field-${generatedId}`;
  const hintId = hint ? `${controlId}-hint` : undefined;
  const errorId = error ? `${controlId}-error` : undefined;
  const descriptionIds = [describedBy, hintId, errorId].filter(Boolean).join(' ') || undefined;
  return { controlId, hintId, errorId, descriptionIds };
}

function FieldNotes({ hint, error, hintId, errorId }: {
  hint?: ReactNode;
  error?: ReactNode;
  hintId?: string;
  errorId?: string;
}) {
  return (
    <>
      {hint && <span className="ui-field-hint" id={hintId}>{hint}</span>}
      {error && <span className="ui-field-error" id={errorId} role="alert">{error}</span>}
    </>
  );
}

type SingleLineProps = FieldDetails & InputHTMLAttributes<HTMLInputElement> & {
  multiline?: false;
};
type MultilineProps = FieldDetails & TextareaHTMLAttributes<HTMLTextAreaElement> & {
  multiline: true;
};

export type TextFieldProps = SingleLineProps | MultilineProps;

export const TextField = forwardRef<HTMLInputElement | HTMLTextAreaElement, TextFieldProps>(function TextField({
  label,
  hint,
  error,
  wrapperClassName = '',
  id,
  className = '',
  ...props
}, ref) {
  const ids = useFieldIds(id, hint, error, props['aria-describedby']);
  const accessibility = {
    id: ids.controlId,
    'aria-describedby': ids.descriptionIds,
    'aria-invalid': error ? true : props['aria-invalid'],
  };

  let control;
  if (props.multiline) {
    const { multiline: _multiline, rows = 3, ...textareaProps } = props;
    control = (
      <textarea
        {...textareaProps}
        {...accessibility}
        ref={ref as ForwardedRef<HTMLTextAreaElement>}
        rows={rows}
        className={`ui-input ui-textarea ${className}`.trim()}
      />
    );
  } else {
    const { multiline: _multiline, type = 'text', ...inputProps } = props;
    control = (
      <Input
        {...inputProps}
        {...accessibility}
        ref={ref as ForwardedRef<HTMLInputElement>}
        type={type}
        className={className}
      />
    );
  }

  return (
    <div className={`ui-field ${wrapperClassName}`.trim()} data-disabled={props.disabled || undefined}>
      <label className="ui-field-label" htmlFor={ids.controlId}>{label}</label>
      {control}
      <FieldNotes hint={hint} error={error} hintId={ids.hintId} errorId={ids.errorId} />
    </div>
  );
});

export interface SelectFieldProps extends FieldDetails, SelectHTMLAttributes<HTMLSelectElement> {
  options?: readonly { value: string; label: string; disabled?: boolean }[];
}

export const SelectField = forwardRef<HTMLSelectElement, SelectFieldProps>(function SelectField({
  label,
  hint,
  error,
  wrapperClassName = '',
  id,
  options,
  children,
  ...props
}, ref) {
  const ids = useFieldIds(id, hint, error, props['aria-describedby']);
  return (
    <div className={`ui-field ${wrapperClassName}`.trim()} data-disabled={props.disabled || undefined}>
      <label className="ui-field-label" htmlFor={ids.controlId}>{label}</label>
      <Select
        {...props}
        ref={ref}
        id={ids.controlId}
        aria-describedby={ids.descriptionIds}
        aria-invalid={error ? true : props['aria-invalid']}
      >
        {options ? options.map((option) => (
          <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>
        )) : children}
      </Select>
      <FieldNotes hint={hint} error={error} hintId={ids.hintId} errorId={ids.errorId} />
    </div>
  );
});

export interface SwitchProps extends FieldDetails, Omit<InputHTMLAttributes<HTMLInputElement>, 'type'> {}

/** A native checkbox with a switch-shaped indicator; Space and forms work normally. */
export const Switch = forwardRef<HTMLInputElement, SwitchProps>(function Switch({
  label,
  hint,
  error,
  wrapperClassName = '',
  id,
  className = '',
  ...props
}, ref) {
  const ids = useFieldIds(id, hint, error, props['aria-describedby']);
  return (
    <div className={`ui-switch-field ${wrapperClassName}`.trim()} data-disabled={props.disabled || undefined}>
      <div className="ui-switch-copy">
        <label className="ui-field-label" htmlFor={ids.controlId}>{label}</label>
        <FieldNotes hint={hint} error={error} hintId={ids.hintId} errorId={ids.errorId} />
      </div>
      <span className="ui-switch-control">
        <input
          {...props}
          ref={ref}
          type="checkbox"
          id={ids.controlId}
          className={`ui-switch-input ${className}`.trim()}
          aria-describedby={ids.descriptionIds}
          aria-invalid={error ? true : props['aria-invalid']}
        />
        <span className="ui-switch-track" aria-hidden="true" />
      </span>
    </div>
  );
});

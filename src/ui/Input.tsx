import { forwardRef, type InputHTMLAttributes, type SelectHTMLAttributes } from 'react';

export interface InputProps extends InputHTMLAttributes<HTMLInputElement> {
  invalid?: boolean;
}

/** For controls whose label already lives in the surrounding page. */
export const Input = forwardRef<HTMLInputElement, InputProps>(function Input({
  className = '',
  invalid,
  ...props
}, ref) {
  return (
    <input
      {...props}
      ref={ref}
      className={`ui-input ${className}`.trim()}
      aria-invalid={invalid || props['aria-invalid']}
    />
  );
});

export interface SelectProps extends SelectHTMLAttributes<HTMLSelectElement> {
  invalid?: boolean;
}

export const Select = forwardRef<HTMLSelectElement, SelectProps>(function Select({
  className = '',
  invalid,
  ...props
}, ref) {
  return (
    <select
      {...props}
      ref={ref}
      className={`ui-select ${className}`.trim()}
      aria-invalid={invalid || props['aria-invalid']}
    />
  );
});

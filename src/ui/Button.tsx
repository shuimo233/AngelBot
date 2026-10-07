import { forwardRef, type ButtonHTMLAttributes } from 'react';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'primary' | 'secondary' | 'ghost' | 'danger';
  size?: 'normal' | 'small';
  /** Keeps the button's label visible while preventing duplicate actions. */
  busy?: boolean;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button({
  variant = 'secondary',
  size = 'normal',
  busy = false,
  disabled,
  type = 'button',
  className = '',
  ...props
}, ref) {
  return (
    <button
      {...props}
      ref={ref}
      type={type}
      className={`ui-button ui-button--${variant} ui-button--${size} ${className}`.trim()}
      disabled={disabled || busy}
      aria-busy={busy || props['aria-busy']}
    />
  );
});

export interface IconButtonProps extends ButtonProps {
  /** The action's accessible name; never rely on an icon or tooltip alone. */
  label: string;
}

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton({
  label,
  variant = 'ghost',
  className = '',
  title,
  children,
  ...props
}, ref) {
  return (
    <Button
      {...props}
      ref={ref}
      variant={variant}
      className={`ui-icon-button ${className}`.trim()}
      aria-label={label}
      title={title ?? label}
    >
      <span aria-hidden="true" className="ui-icon-button-glyph">{children}</span>
    </Button>
  );
});

import type { HTMLAttributes } from 'react';

export type NoticeTone = 'neutral' | 'success' | 'warning' | 'danger';

export function InlineNotice({ tone = 'neutral', className = '', ...props }: HTMLAttributes<HTMLDivElement> & {
  tone?: NoticeTone;
}) {
  const color = tone === 'neutral' ? '' : ` ui-notice--${tone}`;
  return <div role={tone === 'danger' ? 'alert' : 'status'}
    className={`ui-notice ui-notice--inline${color} ${className}`.trim()} {...props} />;
}

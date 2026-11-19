import { useId, type ReactNode } from 'react';

interface DisclosureCopy {
  title: string;
  description?: string;
  status?: string;
  tone?: 'danger';
}

function DisclosureLabel({ id, title, description, status }: DisclosureCopy & { id: string }) {
  return (
    <>
      <span className="settings-disclosure-label">
        <span id={id} className="settings-disclosure-title">{title}</span>
        {description && <span id={`${id}-description`} className="settings-disclosure-description">{description}</span>}
      </span>
      {status && <span id={`${id}-status`} className="settings-disclosure-status">{status}</span>}
      <svg className="settings-disclosure-chevron" viewBox="0 0 16 16" aria-hidden="true">
        <path d="m4 6 4 4 4-4" />
      </svg>
    </>
  );
}

function disclosureDescription(id: string, { description, status }: DisclosureCopy) {
  return [description && `${id}-description`, status && `${id}-status`].filter(Boolean).join(' ') || undefined;
}

export function SettingsDisclosureSummary(copy: DisclosureCopy) {
  const id = useId();
  return (
    <summary className="settings-disclosure-trigger" data-tone={copy.tone}
      aria-labelledby={id} aria-describedby={disclosureDescription(id, copy)}>
      <DisclosureLabel {...copy} id={id} />
    </summary>
  );
}

export function SettingsSection({ title, description, status, tone, children, action, className = '' }: DisclosureCopy & {
  children: ReactNode;
  action?: ReactNode;
  className?: string;
}) {
  const id = useId();
  return (
    <section className={`settings-section ${className}`.trim()} aria-labelledby={id} data-tone={tone}>
      <header className={action ? 'settings-section-header settings-section-header--action' : 'settings-section-header'}>
        <div>
          <h3 id={id}>{title}</h3>
          {description && <p>{description}</p>}
        </div>
        {(status || action) && <div className="settings-section-controls">
          {status && <span className="settings-section-status">{status}</span>}
          {action}
        </div>}
      </header>
      <div className="settings-section-content">{children}</div>
    </section>
  );
}

export function SettingsRow({ title, description, children }: {
  title: string;
  description?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="settings-row">
      <div className="settings-row-copy">
        <div className="settings-row-title">{title}</div>
        {typeof description === 'string' ? (description && <p className="settings-row-description">{description}</p>) : description}
      </div>
      <div className="settings-row-control">{children}</div>
    </div>
  );
}

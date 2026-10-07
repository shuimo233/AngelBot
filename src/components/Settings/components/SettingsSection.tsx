import { useId, useState } from 'react';

export function SettingsSection({
  title,
  description,
  children,
  actions,
  defaultOpen = false,
}: {
  title: string;
  description?: string;
  children: React.ReactNode;
  actions?: React.ReactNode;
  defaultOpen?: boolean;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const contentId = useId();
  const toggleOpen = () => setOpen((prev) => !prev);

  return (
    <div className={`settings-section ${open ? 'expanded' : 'collapsed'}`}>
      <div
        className="settings-section-header"
        role="button"
        tabIndex={0}
        aria-expanded={open}
        aria-controls={open ? contentId : undefined}
        onClick={toggleOpen}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            toggleOpen();
          }
        }}
      >
        <div className="settings-section-info">
          <h3 className="settings-section-title">{title}</h3>
          {description && open && (
            <p className="settings-section-desc">{description}</p>
          )}
        </div>
        <div className="settings-section-right">
          {actions && (
            <div className="settings-section-actions" onClick={(e) => e.stopPropagation()}>
              {actions}
            </div>
          )}
          <svg
            className="settings-section-chevron"
            width="16"
            height="16"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
            style={{ transform: open ? 'rotate(180deg)' : 'rotate(0deg)' }}
          >
            <polyline points="6 9 12 15 18 9" />
          </svg>
        </div>
      </div>
      {open && <div id={contentId} className="settings-section-content">{children}</div>}
    </div>
  );
}

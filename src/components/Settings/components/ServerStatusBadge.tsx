type ServerStatus = 'connected' | 'disconnected' | 'starting';

interface ServerStatusBadgeProps {
  status: ServerStatus;
  label?: string;
}

const statusConfig: Record<ServerStatus, { color: string; text: string }> = {
  connected: { color: 'var(--color-success, #22c55e)', text: '已连接' },
  disconnected: { color: 'var(--color-error, #ef4444)', text: '已断开' },
  starting: { color: 'var(--color-warning, #f59e0b)', text: '启动中' },
};

export function ServerStatusBadge({ status, label }: ServerStatusBadgeProps) {
  const config = statusConfig[status];
  return (
    <span className="server-status-badge" style={{ '--status-color': config.color } as React.CSSProperties}>
      <span className="status-dot" />
      <span className="status-text">{label || config.text}</span>
    </span>
  );
}

import { useEffect, useState } from 'react';
import { getActivity, type ActivityEntry } from '$lib/commands/activity';

const kindLabel: Record<ActivityEntry['kind'], string> = { task_result: '任务', memory_change: '记忆' };

function formatTime(timestamp: number): string {
  return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(new Date(timestamp * 1000));
}

export function ActivityPage() {
  const [entries, setEntries] = useState<ActivityEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let disposed = false;
    setError(null); setLoading(true);
    getActivity().then((items) => { if (!disposed) setEntries(items); })
      .catch(() => { if (!disposed) setError('暂时无法读取使用记录。'); })
      .finally(() => { if (!disposed) setLoading(false); });
    return () => { disposed = true; };
  }, [attempt]);
  if (loading) return <p role="status">正在读取使用记录…</p>;
  if (error) return <div className="library-activity-error" role="alert"><p>{error}</p><button type="button" onClick={() => setAttempt(value => value + 1)}>重新加载</button></div>;
  if (!entries.length) return <section className="library-activity-empty"><strong>暂时没有使用记录</strong><p>任务完成或失败，以及记忆的变更会显示在这里。这里用于回看，不需要提前设置。</p></section>;
  return <ol className="library-activity-list" aria-label="使用记录">{entries.map((entry) => <li key={entry.id} className="library-activity-entry">
    <span className={`library-activity-kind ${entry.kind}`}>{kindLabel[entry.kind]}</span>
    <div><strong>{entry.title}</strong>{entry.detail && <span>{entry.detail}</span>}</div>
    <time dateTime={new Date(entry.occurred_at * 1000).toISOString()}>{formatTime(entry.occurred_at)}</time>
  </li>)}</ol>;
}

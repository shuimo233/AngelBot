import { useEffect, useState } from 'react';
import { getActivity, type ActivityEntry } from '$lib/commands/activity';

const kindLabel: Record<ActivityEntry['kind'], string> = { task_result: '任务', memory_change: '记忆' };

function formatTime(timestamp: number): string {
  return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(new Date(timestamp * 1000));
}

export function ActivityPage() {
  const [entries, setEntries] = useState<ActivityEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let disposed = false;
    getActivity().then((items) => { if (!disposed) setEntries(items); })
      .catch(() => { if (!disposed) setError('Unable to load activity history.'); });
    return () => { disposed = true; };
  }, []);
  if (error) return <p className="library-activity-error" role="alert">{error}</p>;
  if (!entries.length) return <section className="library-activity-empty"><strong>暂时没有重要活动</strong><p>任务完成或失败，以及你明确保存的记忆会显示在这里。</p></section>;
  return <ol className="library-activity-list" aria-label="Activity history">{entries.map((entry) => <li key={entry.id} className="library-activity-entry">
    <span className={`library-activity-kind ${entry.kind}`}>{kindLabel[entry.kind]}</span>
    <div><strong>{entry.title}</strong>{entry.detail && <span>{entry.detail}</span>}</div>
    <time dateTime={new Date(entry.occurred_at * 1000).toISOString()}>{formatTime(entry.occurred_at)}</time>
  </li>)}</ol>;
}

import { useCallback, useEffect, useRef, useState } from 'react';
import {
  dismissLearnedPreference,
  getLearnedPreferences,
  type LearnedPreference,
} from '$lib/commands/memory';

export function LearnedHabits({ enabled }: { enabled: boolean }) {
  const [habits, setHabits] = useState<LearnedPreference[]>([]);
  const [loading, setLoading] = useState(true);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const revision = useRef(0);

  const load = useCallback(async () => {
    const request = ++revision.current;
    setLoading(true);
    setError(null);
    setNotice(null);
    try {
      const records = await getLearnedPreferences();
      if (request === revision.current) setHabits(records);
    } catch {
      if (request === revision.current) setError('暂时无法读取习惯记录，请重试。');
    } finally {
      if (request === revision.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
    return () => { revision.current += 1; };
  }, [load]);

  const dismiss = async (habit: LearnedPreference) => {
    const request = ++revision.current;
    setBusyId(habit.id);
    setError(null);
    setNotice(null);
    try {
      await dismissLearnedPreference(habit.id);
      if (request === revision.current) {
        setHabits((current) => current.filter((item) => item.id !== habit.id));
        setNotice('已停止把这条习惯作为回应参考。');
      }
    } catch {
      if (request === revision.current) setError('未能停止沿用，请重试。');
    } finally {
      if (request === revision.current) setBusyId(null);
    }
  };

  return (
    <div className="evolution-proposals">
      <div className="evolution-proposals-header">
        <div>
          <h4>逐渐了解的习惯</h4>
          <p>这里是近期了解到的日常习惯和相处偏好。不再沿用会移除这条自动记录，不会删除原对话或你主动保存的记忆。</p>
        </div>
        <button className="btn btn-secondary btn-sm" onClick={() => void load()} disabled={loading || busyId !== null}>
          {loading ? '读取中...' : '刷新习惯'}
        </button>
      </div>
      {error && <p className="proposal-error" role="alert">{error}</p>}
      {notice && <p role="status">{notice}</p>}
      {!loading && !error && habits.length === 0 && (
        <p className="proposal-empty">
          {enabled
            ? '暂时没有可显示的习惯记录。自然聊天就好，不必逐条填写。'
            : '暂时没有可显示的习惯记录；偏好学习已暂停。'}
        </p>
      )}
      <div className="proposal-list" role="list" aria-label="近期习惯记录">
        {habits.map((habit) => (
          <div className="proposal-item" role="listitem" key={habit.id}>
            <div className="proposal-meta">
              <span>{!enabled ? '已暂停沿用' : habit.ready ? '可作为回应参考' : '还在了解'}</span>
            </div>
            <p>{habit.content}</p>
            <div className="proposal-actions">
              <button
                className="btn btn-secondary btn-sm"
                aria-label={`不再沿用：${habit.content}`}
                onClick={() => void dismiss(habit)}
                disabled={loading || busyId !== null}
              >
                {busyId === habit.id ? '正在停止...' : '不再沿用'}
              </button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

import { useEffect, useState } from 'react';
import { getAutomationRuns, type AutomationRun } from '$lib/commands/automation';

/** Evidence is loaded only on demand; it never decides the card's current state. */
export function AutomationHistory({ automationId, revision }: { automationId: string; revision: string }) {
  const [open, setOpen] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [result, setResult] = useState<{ runs: AutomationRun[]; loading: boolean; error: boolean }>({ runs: [], loading: false, error: false });

  useEffect(() => {
    if (!open) return;
    let current = true;
    setResult({ runs: [], loading: true, error: false });
    void getAutomationRuns(automationId).then(runs => {
      if (current) setResult({ runs, loading: false, error: false });
    }).catch(() => {
      if (current) setResult({ runs: [], loading: false, error: true });
    });
    return () => { current = false; };
  }, [automationId, revision, open, attempt]);

  return <details className="automation-output" onToggle={event => setOpen(event.currentTarget.open)}>
    <summary>运行记录与输出</summary>
    {open && <>
      {result.loading ? <p role="status">正在读取运行记录…</p> : result.error ? <div>
        <p role="alert">无法读取运行记录，当前任务状态未改变。</p>
        <button type="button" onClick={() => setAttempt(value => value + 1)}>重试读取记录</button>
      </div> : <>
        <p className="automation-state-note">最多显示 20 条记录，优先保留当前和待处理回执；输出不代表业务目标已经完成。</p>
        {result.runs.length === 0 && <p>尚无运行记录</p>}
        {result.runs.map(run => <div className="automation-history-entry" key={run.id}>
          <p><time>{new Date(run.startedAt * 1000).toLocaleString()}</time>{run.exitCode != null && ` · 退出码 ${run.exitCode}`}</p>
          <p>{run.summary}</p>
          {run.output && <pre>{run.output}</pre>}
        </div>)}
      </>}
    </>}
  </details>;
}

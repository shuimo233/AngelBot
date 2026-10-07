import type { TaskDecisionProjection } from '$lib/commands/task-understanding';
import './TaskDecisionCard.css';

interface TaskDecisionCardProps {
  decision: TaskDecisionProjection | null;
}

/**
 * The only user-facing projection of task understanding.
 *
 * It intentionally has no action buttons: the answer belongs in the normal
 * composer so the main conversation remains the single source of truth.
 */
export function TaskDecisionCard({ decision }: TaskDecisionCardProps) {
  if (!decision) return null;

  return (
    <section className="task-decision-card" role="status" aria-live="polite">
      <div className="task-decision-card__rule" aria-hidden="true" />
      <div className="task-decision-card__body">
        <div className="task-decision-card__eyebrow">需要你的判断</div>
        <p className="task-decision-card__question">{decision.question}</p>
        {decision.affects.length > 0 && (
          <p className="task-decision-card__affects">
            会影响：{decision.affects.join('、')}
          </p>
        )}
        <p className="task-decision-card__hint">直接在下方消息中回复即可。</p>
      </div>
    </section>
  );
}

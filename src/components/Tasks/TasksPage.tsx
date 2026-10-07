import { useState } from 'react';
import { useTasksStore, type Task, type TaskStatus } from '$stores/tasks';
import './TasksPage.css';

const STATUS_ORDER: TaskStatus[] = ['in_progress', 'pending', 'done', 'cancelled'];

const STATUS_LABELS: Record<TaskStatus, string> = {
  in_progress: '进行中',
  pending: '待办',
  done: '已完成',
  cancelled: '已取消',
};

const PRIORITY_COLORS: Record<Task['priority'], string> = {
  high: 'var(--color-danger)',
  medium: '#9a6700',
  low: 'var(--color-success)',
};

function TaskCard({ task, onToggle, onDelete }: {
  task: Task;
  onToggle: () => void;
  onDelete: () => void;
}) {
  return (
    <div className={`task-card ${task.status}`}>
      <div className="task-card-main">
        <button
          className={`task-checkbox ${task.status === 'done' ? 'checked' : ''}`}
          onClick={onToggle}
          aria-label={task.status === 'done' ? '标记未完成' : '标记完成'}
        >
          {task.status === 'done' && <svg aria-hidden="true" width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="m3 8 3 3 7-7" /></svg>}
        </button>
        <div className="task-card-content">
          <div className="task-card-title">{task.title}</div>
          {task.description && (
            <div className="task-card-desc">{task.description}</div>
          )}
        </div>
        <span
          className="task-priority-dot"
          style={{ background: PRIORITY_COLORS[task.priority] }}
          title={`优先级: ${task.priority}`}
        />
        <button className="task-delete-btn" onClick={onDelete} aria-label="删除">×</button>
      </div>
    </div>
  );
}

export function TasksPage() {
  const { tasks, addTask, removeTask, toggleStatus } = useTasksStore();
  const [newTitle, setNewTitle] = useState('');
  const [newDesc, setNewDesc] = useState('');
  const [newPriority, setNewPriority] = useState<Task['priority']>('medium');
  const [filter, setFilter] = useState<TaskStatus | 'all'>('all');

  const handleAdd = () => {
    if (!newTitle.trim()) return;
    addTask(newTitle.trim(), newDesc.trim(), newPriority);
    setNewTitle('');
    setNewDesc('');
  };

  const filtered = filter === 'all' ? tasks : tasks.filter((t) => t.status === filter);

  const grouped = STATUS_ORDER.reduce<Record<TaskStatus, Task[]>>((acc, s) => {
    acc[s] = filtered.filter((t) => t.status === s);
    return acc;
  }, {} as Record<TaskStatus, Task[]>);

  const showGrouped = filter === 'all';

  return (
    <div className="tasks-page">
      <div className="tasks-header">
        <h2 className="tasks-page-title">Tasks</h2>
        <div className="tasks-filter-bar">
          <button
            className={`tasks-filter-btn ${filter === 'all' ? 'active' : ''}`}
            onClick={() => setFilter('all')}
          >
            全部 <span className="tasks-filter-count">{tasks.length}</span>
          </button>
          {STATUS_ORDER.map((s) => (
            <button
              key={s}
              className={`tasks-filter-btn ${filter === s ? 'active' : ''}`}
              onClick={() => setFilter(s)}
            >
              {STATUS_LABELS[s]} <span className="tasks-filter-count">{tasks.filter((t) => t.status === s).length}</span>
            </button>
          ))}
        </div>
      </div>

      <div className="tasks-add-form">
        <input
          className="tasks-add-input"
          value={newTitle}
          onChange={(e) => setNewTitle(e.target.value)}
          placeholder="添加新任务..."
          onKeyDown={(e) => e.key === 'Enter' && handleAdd()}
        />
        <input
          className="tasks-add-desc"
          value={newDesc}
          onChange={(e) => setNewDesc(e.target.value)}
          placeholder="描述（可选）"
        />
        <select
          className="tasks-add-priority"
          value={newPriority}
          onChange={(e) => setNewPriority(e.target.value as Task['priority'])}
        >
          <option value="low">低</option>
          <option value="medium">中</option>
          <option value="high">高</option>
        </select>
        <button className="btn btn-primary" onClick={handleAdd} disabled={!newTitle.trim()}>
          添加
        </button>
      </div>

      <div className="tasks-content">
        {showGrouped ? (
          <div className="tasks-columns">
            {STATUS_ORDER.map((status) => (
              <div key={status} className="tasks-column">
                <div className="tasks-column-header">
                  <span>{STATUS_LABELS[status]}</span>
                  <span className="tasks-column-count">{grouped[status].length}</span>
                </div>
                <div className="tasks-column-body">
                  {grouped[status].length === 0 ? (
                    <div className="tasks-column-empty">—</div>
                  ) : (
                    grouped[status].map((task) => (
                      <TaskCard
                        key={task.id}
                        task={task}
                        onToggle={() => toggleStatus(task.id)}
                        onDelete={() => removeTask(task.id)}
                      />
                    ))
                  )}
                </div>
              </div>
            ))}
          </div>
        ) : (
          <div className="tasks-list">
            {filtered.length === 0 ? (
              <div className="tasks-empty">没有任务</div>
            ) : (
              filtered.map((task) => (
                <TaskCard
                  key={task.id}
                  task={task}
                  onToggle={() => toggleStatus(task.id)}
                  onDelete={() => removeTask(task.id)}
                />
              ))
            )}
          </div>
        )}
      </div>
    </div>
  );
}

import { create } from 'zustand';
import { persist } from 'zustand/middleware';

export type TaskStatus = 'pending' | 'in_progress' | 'done' | 'cancelled';

export interface Task {
  id: string;
  title: string;
  description: string;
  status: TaskStatus;
  priority: 'low' | 'medium' | 'high';
  dueDate?: number;
  createdAt: number;
  updatedAt: number;
}

interface TasksState {
  tasks: Task[];
  addTask: (title: string, description?: string, priority?: Task['priority']) => void;
  updateTask: (id: string, patch: Partial<Omit<Task, 'id' | 'createdAt'>>) => void;
  removeTask: (id: string) => void;
  toggleStatus: (id: string) => void;
  getByStatus: (status: TaskStatus) => Task[];
}

export const useTasksStore = create<TasksState>()(
  persist(
    (set, get) => ({
      tasks: [],

      addTask: (title, description = '', priority = 'medium') => {
        const now = Date.now();
        const task: Task = {
          id: crypto.randomUUID(),
          title,
          description,
          status: 'pending',
          priority,
          createdAt: now,
          updatedAt: now,
        };
        set((s) => ({ tasks: [task, ...s.tasks] }));
      },

      updateTask: (id, patch) => {
        set((s) => ({
          tasks: s.tasks.map((t) =>
            t.id === id ? { ...t, ...patch, updatedAt: Date.now() } : t,
          ),
        }));
      },

      removeTask: (id) => {
        set((s) => ({ tasks: s.tasks.filter((t) => t.id !== id) }));
      },

      toggleStatus: (id) => {
        const task = get().tasks.find((t) => t.id === id);
        if (!task) return;
        const next: Record<TaskStatus, TaskStatus> = {
          pending: 'in_progress',
          in_progress: 'done',
          done: 'pending',
          cancelled: 'pending',
        };
        get().updateTask(id, { status: next[task.status] });
      },

      getByStatus: (status) => get().tasks.filter((t) => t.status === status),
    }),
    { name: 'angelbot-tasks' },
  ),
);

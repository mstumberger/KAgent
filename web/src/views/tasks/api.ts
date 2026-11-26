// The tasks routes (api.md section 6, automations.md section 5): the client and the types.
// Every route that changes the list answers the whole list.
import { api } from "../../api";

export type TaskKind = "agent" | "remind";

/** How a run ended, or that it is still going (api.md `TaskRun`). */
export type RunStatus = "running" | "ok" | "failed" | "missed" | "timeout" | "cancelled";

export type Schedule =
  | { type: "once"; at: string }
  | { type: "interval"; unit: "minute" | "hour" | "day" | "week" | "month"; n: number; at?: string; weekdays?: number[]; from?: string }
  | { type: "cron"; expr: string };

export interface TaskRun {
  /** When the run started; of a missed run, when it should have. */
  at: string;
  status: RunStatus;
  durationSec?: number;
  sessionId?: string;
  result?: string;
}

export interface Task {
  id: string;
  title: string;
  /** What KAgent should do, or the text of the reminder. */
  what: string;
  kind: TaskKind;
  /** Null: a to-do, run on demand. */
  schedule: Schedule | null;
  /** Where a reminder or a result goes: `desktop`, `session`, `telegram`. */
  deliver: string[];
  onApproval: "deny" | "wait";
  session: "new" | "reuse";
  timeoutMin: number;
  enabled: boolean;
  createdAt: string;
  nextRun: string | null;
  lastRun: string | null;
  lastStatus: RunStatus | null;
  lastResult: string | null;
  lastSession: string | null;
  /** The one session of a task with `session: reuse`. */
  sessionId: string | null;
  /** The last runs, oldest first. */
  runs: TaskRun[];
  /** Runs from before the history existed. */
  runsBefore?: number;
  /** A one-off that ran, or a to-do marked done. */
  done: boolean;
  /** The schedule in a few words, from the daemon. */
  scheduleText: string;
}

/** What adds a task, or changes one (then a field left out keeps its value). `what`
 * is required of a new task. */
export interface TaskInput {
  title?: string;
  what?: string;
  kind?: TaskKind;
  /** Words or cron; a schedule as the daemon stores it; null removes the schedule. */
  schedule?: string | Schedule | null;
  delayMinutes?: number;
  deliver?: string[];
  onApproval?: "deny" | "wait";
  session?: "new" | "reuse";
  timeoutMin?: number;
  enabled?: boolean;
  done?: boolean;
}

/** `GET /api/tasks/preview`: what a schedule text comes to. */
export interface SchedulePreview {
  schedule: Schedule | null;
  scheduleText: string;
  nextRun: string | null;
}

/** One of the three templates of the Tasks tab. Adding one gives a paused task. */
export interface TaskTemplate {
  id: string;
  title: string;
  what: string;
  kind: "agent";
  /** In words, as the owner could type it. */
  schedule: string;
  scheduleText: string;
}

const path = (id: string) => `/api/tasks/${encodeURIComponent(id)}`;

export const tasksApi = {
  /** A daemon that cannot answer gives an empty list. */
  list: async (): Promise<Task[]> => {
    const r = await api.get<{ tasks?: Task[] }>("/api/tasks");
    return Array.isArray(r?.tasks) ? r.tasks : [];
  },
  add: (input: TaskInput) => api.post<{ tasks: Task[] }>("/api/tasks", input),
  addTemplate: (id: string) => api.post<{ tasks: Task[] }>(`/api/tasks/templates/${encodeURIComponent(id)}`, {}),
  update: (id: string, patch: TaskInput) => api.put<{ tasks: Task[] }>(path(id), patch),
  remove: (id: string) => api.del<{ tasks: Task[] }>(path(id)),
  run: (id: string) => api.post<{ tasks: Task[] }>(`${path(id)}/run`, {}),
  runs: (id: string) => api.get<{ runs: TaskRun[] }>(`${path(id)}/runs`),
  cancel: (id: string) => api.post<{ cancelled: boolean }>(`${path(id)}/cancel`, {}),
  preview: (schedule: string) => api.get<SchedulePreview>(`/api/tasks/preview?schedule=${encodeURIComponent(schedule)}`),
  templates: async (): Promise<TaskTemplate[]> => {
    const r = await api.get<{ templates?: TaskTemplate[] }>("/api/tasks/templates");
    return Array.isArray(r?.templates) ? r.templates : [];
  },
};

/** A moment in words relative to now, with its clock time ("in 12 min (Tue 09:00)"). */
export function rel(iso?: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  const diff = (d.getTime() - Date.now()) / 60000;
  const abs = Math.abs(diff);
  const span = abs < 1 ? "now" : abs < 60 ? `${Math.round(abs)} min` : abs < 1440 ? `${Math.round(abs / 60)} h` : `${Math.round(abs / 1440)} d`;
  const when = span === "now" ? "now" : diff > 0 ? `in ${span}` : `${span} ago`;
  return `${when} (${d.toLocaleString(undefined, { weekday: "short", hour: "2-digit", minute: "2-digit", day: "numeric", month: "short" })})`;
}

/** A moment as a short date and time. */
export function moment(iso: string): string {
  return new Date(iso).toLocaleString(undefined, { weekday: "short", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

// The runs routes (workflows.md section 6, api.md section 13): the types and the client.
import { api, ApiError } from "../../api";
import type { Diff } from "../../components/diff/model";

export type RunState = "planning" | "awaiting_approval" | "working" | "paused" | "done" | "stopped" | "failed";
export type TaskState = "waiting" | "blocked" | "implementing" | "in_review" | "changes" | "needs_owner" | "done" | "dropped" | "failed" | "stopped";
export type Choice = "more_round" | "accept" | "drop";

/** A row of `GET /api/runs`. */
export interface RunRow {
  id: string;
  title: string;
  workflow: string;
  project: string;
  state: RunState;
  reason: string;
  done: number;
  total: number;
  needsOwner: boolean;
  ticket: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface Finding {
  severity: "must" | "should";
  file: string;
  line: number;
  what: string;
  why: string;
}

export interface Round {
  round: number;
  report: string;
  verdict: "pass" | "changes" | null;
  findings: Finding[];
}

export interface ProgressStep {
  title: string;
  status: "pending" | "in_progress" | "completed";
}

export interface WorkerRecord {
  role: string;
  session: string;
  round: number;
  startedAt: string;
  endedAt: string | null;
}

export interface Task {
  id: string;
  title: string;
  description: string;
  doneWhen: string[];
  dependsOn: string[];
  files: string[];
  ticket: string | null;
  kaTicket: string | null;
  state: TaskState;
  round: number;
  extraRounds: number;
  accepted: boolean;
  error: string;
  rounds: Round[];
  workers: WorkerRecord[];
  progress: ProgressStep[];
  changedFiles: string[];
  startedAt: string | null;
  endedAt: string | null;
}

export interface Question {
  id: string;
  task: string;
  text: string;
  options: string[];
  blocking: boolean;
  state: "open" | "escalated" | "answered";
  askedAt: string;
  answer: string | null;
  answeredBy: string | null;
}

export interface Message {
  id: string;
  task: string;
  text: string;
  state: "queued" | "delivered" | "expired";
  at: string;
  deliveredAt: string | null;
}

export interface Step {
  id: string;
  role: "planner" | "implementer" | "reviewer" | "orchestrator";
  worker: { kind: string; profile: string } | null;
  model: string | null;
  gate: string | null;
}

/** `GET /api/runs/:id`. */
export interface Run extends Omit<RunRow, "done" | "total" | "needsOwner"> {
  goal: string;
  definition: { name: string; description: string; steps: Step[] };
  originSession: string | null;
  runSession: string;
  planSummary: string;
  tasks: Task[];
  questions: Question[];
  messages: Message[];
  plannerSession: string | null;
  rejections: number;
  report: string;
}

export interface WorkflowInfo {
  name: string;
  description: string;
  shipped: boolean;
  steps: Step[];
}

export interface TaskEdit {
  id: string;
  title?: string;
}

export interface NewRun {
  workflow: string;
  project: string;
  goal: string;
  ticket?: string;
  models?: Record<string, string>;
}

const enc = encodeURIComponent;

export const runsApi = {
  workflows: () => api.get<{ workflows: WorkflowInfo[]; invalid: { file: string; reason: string }[] }>("/api/workflows"),
  list: () => api.get<{ runs: RunRow[] }>("/api/runs"),
  get: (id: string) => api.get<Run>(`/api/runs/${enc(id)}`),
  start: (body: NewRun) => api.post<Run>("/api/runs", body),
  stop: (id: string) => api.post<Run>(`/api/runs/${enc(id)}/stop`, {}),
  pause: (id: string) => api.post<Run>(`/api/runs/${enc(id)}/pause`, {}),
  resume: (id: string) => api.post<Run>(`/api/runs/${enc(id)}/continue`, {}),
  approve: (id: string, tasks?: TaskEdit[]) => api.post<Run>(`/api/runs/${enc(id)}/plan`, { action: "approve", ...(tasks ? { tasks } : {}) }),
  reject: (id: string, reason: string) => api.post<Run>(`/api/runs/${enc(id)}/plan`, { action: "reject", reason }),
  decide: (id: string, task: string, choice: Choice) => api.post<Run>(`/api/runs/${enc(id)}/tasks/${enc(task)}/decision`, { choice }),
  answer: (id: string, qid: string, text: string) => api.post<Run>(`/api/runs/${enc(id)}/questions/${enc(qid)}/answer`, { text }),
  diff: (id: string, task: string) => api.get<{ project: string; from: string; files: string[]; diff: Diff }>(`/api/runs/${enc(id)}/tasks/${enc(task)}/diff`),
};

export function errorMessage(e: unknown): string {
  return e instanceof ApiError ? e.message : e instanceof Error ? e.message : String(e);
}

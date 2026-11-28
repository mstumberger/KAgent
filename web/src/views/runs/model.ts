// What the board knows about runs and tasks (workflows.md section 8): the dot of a status, its words, the tree of the left
// column, the time of a worker. Nothing here reads the DOM.
import type { Run, RunRow, RunState, Step, Task, TaskState } from "./api";

export type Dot = "wait" | "run" | "owner" | "done" | "fail" | "off";

const RUN_WORD: Record<RunState, string> = { planning: "planning", awaiting_approval: "awaiting approval", working: "working", paused: "paused", done: "done", stopped: "stopped", failed: "failed" };
const TASK_WORD: Record<TaskState, string> = {
  waiting: "waiting",
  blocked: "blocked",
  implementing: "implementing",
  in_review: "in review",
  changes: "changes requested",
  needs_owner: "needs you",
  done: "done",
  dropped: "dropped",
  failed: "failed",
  stopped: "stopped",
};

export const runWord = (s: RunState) => RUN_WORD[s];
export const taskWord = (s: TaskState) => TASK_WORD[s];

/** The dot of a run: amber when something waits for the owner. */
export function runDot(r: Pick<RunRow, "state" | "needsOwner">): Dot {
  if (r.needsOwner && !(r.state === "done" || r.state === "failed" || r.state === "stopped")) return "owner";
  switch (r.state) {
    case "planning":
    case "working":
      return "run";
    case "awaiting_approval":
      return "owner";
    case "paused":
      return "wait";
    case "done":
      return "done";
    case "failed":
      return "fail";
    default:
      return "off";
  }
}

export function taskDot(s: TaskState): Dot {
  switch (s) {
    case "implementing":
    case "in_review":
    case "changes":
      return "run";
    case "needs_owner":
      return "owner";
    case "done":
      return "done";
    case "failed":
      return "fail";
    case "dropped":
    case "stopped":
      return "off";
    default:
      return "wait";
  }
}

export interface ProjectGroup {
  project: string;
  name: string;
  runs: RunRow[];
}

/** The runs grouped by project, the project with the newest run first, the runs newest first. */
export function groupRuns(rows: RunRow[]): ProjectGroup[] {
  const map = new Map<string, RunRow[]>();
  for (const r of [...rows].sort((a, b) => b.createdAt.localeCompare(a.createdAt))) map.set(r.project, [...(map.get(r.project) ?? []), r]);
  return [...map.entries()].map(([project, runs]) => ({ project, name: project.split("/").filter(Boolean).pop() ?? project, runs }));
}

/** "Running for 12m 9s". */
export function elapsed(startedAt: string | null | undefined, endedAt: string | null | undefined, now: number): string {
  const start = startedAt ? Date.parse(startedAt) : NaN;
  if (Number.isNaN(start)) return "";
  const end = endedAt ? Date.parse(endedAt) : now;
  const s = Math.max(0, Math.round((end - start) / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return h > 0 ? `${h}h ${m}m` : m > 0 ? `${m}m ${s % 60}s` : `${s}s`;
}

/** The first line of a report, for the list of ended workers. */
export function firstLine(text: string): string {
  return text.split("\n").map((l) => l.replace(/^[#>*\-\s]+/, "").trim()).find((l) => l !== "") ?? "";
}

export interface WorkerRow {
  session: string;
  task: string;
  role: string;
  title: string;
  round: number;
  startedAt: string;
  endedAt: string | null;
}

/** Every worker of a run in the order they started: the planner's (when it is known), then each task's. */
export function workersOf(run: Run): WorkerRow[] {
  const rows: WorkerRow[] = [];
  for (const t of run.tasks) for (const w of t.workers) rows.push({ session: w.session, task: t.id, role: w.role, title: `${w.role === "reviewer" ? "Review" : "Implement"} ${t.id}`, round: w.round, startedAt: w.startedAt, endedAt: w.endedAt });
  return rows.sort((a, b) => a.startedAt.localeCompare(b.startedAt));
}

/** The tab a task opens: its latest implementer, else its latest worker. */
export function workerOfTask(t: Task): string | null {
  const w = [...t.workers].reverse().find((x) => x.role === "implementer") ?? t.workers[t.workers.length - 1];
  return w?.session ?? null;
}

export function stateLine(run: Run): string {
  const done = run.tasks.filter((t) => t.state === "done").length;
  return `${runWord(run.state)}${run.reason ? ` (${run.reason})` : ""}, ${done} of ${run.tasks.length} tasks done`;
}

/** The step the run is at: the planner while it plans, the implementer or reviewer while a task is in them, the wrap at the end. */
export function activeStep(run: Run): string | null {
  const steps = run.definition.steps;
  const by = (role: Step["role"]) => steps.find((s) => s.role === role)?.id ?? null;
  if (run.state === "planning" || run.state === "awaiting_approval") return by("planner");
  if (run.state === "done") return by("orchestrator");
  if (run.state !== "working" && run.state !== "paused") return null;
  if (run.tasks.some((t) => t.state === "implementing" || t.state === "changes")) return by("implementer");
  if (run.tasks.some((t) => t.state === "in_review")) return by("reviewer");
  return null;
}

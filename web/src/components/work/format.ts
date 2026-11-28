// What the work panel says about a task and a plan: the pure parts of its lines.
import type { BackgroundTaskView } from "./state";

/** How long a task runs, or ran, in the short form of the cards: "42 s", "3 min 5 s". */
export function lasted(task: BackgroundTaskView, now: number): string {
  const start = Date.parse(task.startedAt);
  const seconds = task.status === "running" && !Number.isNaN(start) ? Math.max(0, Math.round((now - start) / 1000)) : task.seconds;
  return seconds < 60 ? `${seconds} s` : `${Math.floor(seconds / 60)} min ${seconds % 60} s`;
}

/** The state of a task as a line says it: the exit code in the colour of its outcome. */
export function stateOf(task: BackgroundTaskView): { text: string; tone: "ok" | "bad" | "signal" | "run" } {
  if (task.status === "running") return { text: "running", tone: "run" };
  if (task.status === "killed") return { text: "killed", tone: "signal" };
  return task.exitCode === null ? { text: task.status, tone: "bad" } : { text: `exit ${task.exitCode}`, tone: task.exitCode === 0 ? "ok" : "bad" };
}

/** The first line of a plan that says something, without its Markdown marks. */
export function firstLine(text: string): string {
  const line = text.split("\n").map((l) => l.replace(/^[#>*\-\s]+/, "").trim()).find((l) => l !== "");
  return line ?? "";
}

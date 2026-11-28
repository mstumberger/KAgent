// What the capsule counts for the open session (work-windows.md section 4.2) and the words it says.
import { useStore } from "../../store";
import { useWork, type BackgroundTaskView } from "./state";

const NO_TASKS: BackgroundTaskView[] = [];
const NO_CHILDREN: never[] = [];

/** What the capsule counts for a session (section 4.2). */
export interface Counts {
  agents: number;
  commands: number;
  plan: "planning" | "plan" | null;
  open: number;
  total: number;
}

export function useCounts(sessionId: string | null): Counts {
  const s = useStore();
  const tasks = useWork().tasks[sessionId ?? ""] ?? NO_TASKS;
  const children = (sessionId ? s.children[sessionId] : undefined) ?? NO_CHILDREN;
  const todo = (sessionId ? s.todos[sessionId] : undefined) ?? [];
  const planning = sessionId ? (s.planning[sessionId] ?? false) : false;
  const approved = sessionId ? !!s.metas[sessionId]?.approvedPlan : false;
  return {
    agents: children.filter((c) => c.status === "queued" || c.status === "running").length,
    commands: tasks.filter((t) => t.status === "running").length,
    plan: planning ? "planning" : approved ? "plan" : null,
    open: todo.filter((item) => item.status !== "completed").length,
    total: todo.length,
  };
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** The words the capsule's title says: "2 agents, 1 command, 3 of 7 to do". */
export function describe(c: Counts): string {
  const parts = [];
  if (c.agents > 0) parts.push(plural(c.agents, "agent"));
  if (c.commands > 0) parts.push(plural(c.commands, "command"));
  if (c.plan) parts.push(c.plan === "planning" ? "planning" : "plan approved");
  if (c.total > 0) parts.push(`${c.open} of ${c.total} to do`);
  return parts.join(", ");
}

export const hasWork = (c: Counts) => c.agents > 0 || c.commands > 0 || c.plan !== null || c.total > 0;

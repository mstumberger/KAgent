// The hooks (hooks.md; api.md section 6). The types and the client live here and not in
// `api.ts`, which is at its budget.
import { api } from "../../api";

export type HookEventName = "SessionStart" | "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "Stop" | "AlertRaised";

/** A record as the daemon keeps it (config.md section 11). `id` is the daemon's. */
export interface Hook {
  id?: string;
  event: HookEventName;
  /** Tool names; absent or empty = every tool (the two tool events only). */
  matcher?: string[];
  /** An executable path or name found through PATH; with a shell, one line for it. */
  command: string;
  /** Empty: no shell. Then `command` is the program and `args` its arguments. */
  shell?: string;
  args?: string[];
  /** 100 to 60000; 5000 when left out. */
  timeoutMs?: number;
  /** Started and not awaited; only its context is used, for the next turn. */
  async?: boolean;
  /** On unless it is `false`. */
  enabled?: boolean;
}

/** `GET /api/hooks`: the hooks, the events, and the payload of each event as a hook gets it. */
export interface HookList {
  hooks: Hook[];
  events: HookEventName[];
  samples: Partial<Record<HookEventName, object>>;
}

export type Decision = "allow" | "ask" | "deny";

/** `POST /api/hooks/:id/test`. */
export interface TestResult {
  exitCode: number | null;
  stdout: string;
  stderr: string;
  decision?: Decision;
  durationMs: number;
  timedOut: boolean;
  /** Why the run failed, when it did. */
  error?: string;
}

/** The events (hooks.md section 1): when each runs and what it may decide. */
export const EVENTS: { name: HookEventName; when: string; decides: string }[] = [
  { name: "SessionStart", when: "Before the first turn of a session.", decides: "Watches only." },
  { name: "UserPromptSubmit", when: "Before the first model request of a turn. Gets the prompt.", decides: "May add context to the turn." },
  { name: "PreToolUse", when: "After a tool call is checked and before it is allowed to run.", decides: "Allow, ask or deny; may replace the input." },
  { name: "PostToolUse", when: "After the tool returned. Gets the output.", decides: "May add context to the next request." },
  { name: "Stop", when: "After a turn ended. Gets how it ended.", decides: "Watches only." },
  { name: "AlertRaised", when: "When a watcher raises an alert. Gets the alert.", decides: "Watches only." },
];

export const takesMatcher = (event: HookEventName) => event === "PreToolUse" || event === "PostToolUse";

export const hooksApi = {
  list: () => api.get<HookList>("/api/hooks"),
  /** 201; 400 for an invalid record. */
  create: (body: Hook) => api.post<Hook>("/api/hooks", body),
  /** The whole record; 404 when there is no such hook. */
  update: (id: string, body: Hook) => api.put<Hook>(`/api/hooks/${encodeURIComponent(id)}`, body),
  remove: (id: string) => api.del<void>(`/api/hooks/${encodeURIComponent(id)}`),
  /** Runs the hook as it is stored, with the sample of `event`. Nothing of it reaches the agent. */
  test: (id: string, body: { event?: HookEventName; sample?: object }) => api.post<TestResult>(`/api/hooks/${encodeURIComponent(id)}/test`, body),
};

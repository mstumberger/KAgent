// The subagents (agents.md sections 1, 3 and 6; api.md sections 6 and 7). The types live
// here and not in `api.ts`, which is at its budget; the settings are saved through
// `PUT /api/settings` like every section.
import { api } from "../../api";
import type { SessionMeta } from "../../api";

/** `GET /api/agents` lists one per profile (api.md section 6). */
export interface AgentProfileView {
  name: string;
  description: string;
  builtIn: boolean;
  source: "builtin" | "user";
  /** Empty: the chat model. */
  model: string;
  /** Empty: every enabled group. */
  toolGroups: string[];
  maxIterations: number;
  prompt: string;
}

/** A file of `<config>/agents` that is not a valid profile: listed with its reason (agents.md section 1). */
export interface InvalidProfile {
  file: string;
  reason: string;
}

/** `GET /api/agents`. */
export interface AgentList {
  agents: AgentProfileView[];
  invalid: InvalidProfile[];
}

/** `PUT /api/agents/:name`: a field left out keeps its value. */
export interface ProfileBody {
  description?: string;
  model?: string;
  toolGroups?: string[];
  maxIterations?: number;
  prompt?: string;
}

export type ChildStatus = "queued" | "running" | "done" | "failed" | "cancelled";

/** A subagent as `subagentUpdated` tells of it (api.md section 7): `startedAt` once its turn started. */
export interface ChildView {
  id: string;
  title: string;
  profile: string;
  status: ChildStatus;
  startedAt?: string;
  endedAt?: string;
  /** The latest tool call of the worker (steering.md section 2). */
  lastAction?: { tool: string; summary: string; status: "running" | "done" | "error" | "refused"; at: string };
  /** The files its tool calls named. */
  files?: { path: string; changed: boolean }[];
  /** The owner's messages to it, with receipts (steering.md section 3). */
  steering?: Steering[];
}

export interface Steering {
  id: string;
  text: string;
  interrupt: boolean;
  state: "queued" | "delivered";
  at: string;
  deliveredAt: string | null;
}

/** `GET /api/sessions/:id/subagents/:child`: what the worker pane draws (steering.md section 2). */
export interface WorkerDetail {
  child: ChildView;
  job: string;
  todo: { content: string; status: "pending" | "in_progress" | "completed" }[];
  transcript: { role: "assistant" | "user" | "tool"; kind: "text" | "call" | "result" | "steer"; text: string; status?: string }[];
}

/** `settings.subagents` (config.md section 11). */
export interface SubagentsSettings {
  maxConcurrent?: number;
}

/** A session of a listing (`GET /api/sessions`): its meta, and `agents`, how many subagents it started. */
export type SessionRow = SessionMeta & { origin?: string; parent?: string | null; agents?: number };

/** The meta of one session: `origin` and `parent` tell the session of a subagent. */
export type SessionInfo = SessionMeta & { origin?: string; parent?: string | null };

export const agentsApi = {
  list: () => api.get<AgentList>("/api/agents"),
  /** 201 for a new profile, 200 for a change; 400 for an invalid one, 409 for a built-in name. */
  save: (name: string, body: ProfileBody) => api.put<AgentProfileView>(`/api/agents/${encodeURIComponent(name)}`, body),
  /** 204; 409 for a built-in name, 404 when there is no such file. */
  remove: (name: string) => api.del<void>(`/api/agents/${encodeURIComponent(name)}`),
  /** The Stop button of an Agents card: a running subagent is cancelled and a queued one never starts. */
  stop: (sessionId: string, childId: string) => api.post<{ status: ChildStatus }>(`/api/sessions/${sessionId}/subagents/${childId}/stop`, {}),
  /** The owner's message to a running worker (steering.md sections 3 and 4): 202; 409 when it ended or has not started. */
  tell: (childId: string, text: string, interrupt: boolean) => api.post<{ id: string; state: "queued" | "delivered" }>(`/api/agents/${childId}/message`, { text, interrupt }),
  /** What a worker is doing: its job, its todo list and the tail of its transcript. */
  detail: (sessionId: string, childId: string) => api.get<WorkerDetail>(`/api/sessions/${sessionId}/subagents/${childId}`),
};

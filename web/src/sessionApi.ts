// The session types and the calls that change one session (api.md sections 2, 6 and 8).
// `api.ts` re-exports this file, so no importer changes.
import { api, type Message } from "./api";

/** A compaction by hand (compaction.md section 4): everything before the last user message of the session is summarized. */
export function requestCompaction(id: string): Promise<CompactResult> {
  return api.post<CompactResult>(`/api/sessions/${id}/compact`, {});
}

export interface SessionMeta {
  id: string;
  title: string;
  createdAt: string;
  updatedAt: string;
  messageCount: number;
  /** From compaction (compaction.md section 4): the summary of the first `summaryUpto` stored messages. */
  summary?: string | null;
  summaryUpto?: number | null;
  /** The session's todo list (plan-todo.md section 1); empty while there is none. */
  todo?: TodoItem[];
  /** Plan mode (plan-todo.md section 2): on while the session plans; `approvedPlan` is the plan the owner approved last. */
  planning?: boolean;
  approvedPlan?: { path: string; text: string; approvedAt?: string } | null;
  /** The model and the effort this conversation chose (picker.md section 2); `null`: the default. */
  model?: string | null;
  reasoningLevel?: string | null;
  /** The folder attached to the conversation (sidebar.md section 2): an absolute canonical path, or none. */
  project?: string | null;
  /** The session stands in the section "Pinned" of the sidebar. */
  pinned?: boolean;
}

/** One item of a session's todo list (api.md section 6). */
export interface TodoItem {
  content: string;
  status: "pending" | "in_progress" | "completed";
  priority: "high" | "medium" | "low";
}

/** Replace the todo list of a session; an empty list clears it. The daemon normalises what it receives and answers with the stored list. */
export function putTodo(id: string, items: TodoItem[]): Promise<{ items: TodoItem[] }> {
  return api.put(`/api/sessions/${id}/todo`, { items });
}

/** Put a session in or out of plan mode ("Cancel plan" sends `{active: false}`); the answer is its flag. */
export function postPlan(id: string, active: boolean): Promise<{ planning: boolean }> {
  return api.post(`/api/sessions/${id}/plan`, { active });
}

/** The answer of `POST /api/sessions/:id/compact`: `keptMessages` stored messages follow the summary unchanged. */
export interface CompactResult {
  summary: string;
  keptMessages: number;
}

export interface Session {
  meta: SessionMeta;
  messages: Message[];
}


/** Attach a folder to a session, or detach it with `null`; the answer is the meta. A refusal is an `ApiError` with the daemon's message. */
export function putProject(id: string, path: string | null): Promise<SessionMeta> {
  return api.put(`/api/sessions/${id}/project`, { path });
}

/** Pin or unpin a session; the answer is the meta. */
export function putPin(id: string, pinned: boolean): Promise<SessionMeta> {
  return api.put(`/api/sessions/${id}/pin`, { pinned });
}

/** A folder of `GET /api/projects` (sidebar.md section 3). */
export interface ProjectView {
  path: string;
  name: string;
  agentsMd: boolean;
}

export function getProjects(): Promise<{ roots: string[]; projects: ProjectView[] }> {
  return api.get("/api/projects");
}

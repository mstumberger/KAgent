// The tickets routes (tickets.md section 4): the types and the client.
import { api, ApiError } from "../../api";

export type TState = "backlog" | "todo" | "in_progress" | "in_review" | "done" | "dropped";
export type Priority = "none" | "low" | "medium" | "high" | "urgent";
export type LinkKind = "conversation" | "commit" | "range" | "url";

export interface Link {
  kind: LinkKind;
  value: string;
  label: string;
}

export interface Ticket {
  key: string;
  /** The folder of the project, or null for the Inbox. */
  project: string | null;
  title: string;
  description: string;
  state: TState;
  priority: Priority;
  labels: string[];
  parent: string | null;
  links: Link[];
  assignee: string | null;
  doneWhen: string[];
  notes: { at: string; by: "owner" | "agent"; text: string }[];
  history: { at: string; by: "owner" | "agent"; what: string }[];
  createdAt: string;
  updatedAt: string;
}

export interface TProject {
  id: string;
  path: string | null;
  name: string;
  prefix: string;
  count: number;
}

export interface Listing {
  projects: TProject[];
  tickets: Ticket[];
}

/** What a PUT may carry (a field left out is kept; `parent` and `assignee` take null). */
export type Change = Partial<{
  title: string;
  description: string;
  state: TState;
  priority: Priority;
  labels: string[];
  parent: string | null;
  links: Link[];
  assignee: string | null;
  doneWhen: string[];
  note: string;
}>;

export interface NewTicket extends Change {
  title: string;
  project?: string | null;
}

export const ticketsApi = {
  list: () => api.get<Listing>("/api/tickets"),
  create: (body: NewTicket) => api.post<Ticket>("/api/tickets", body),
  change: (key: string, body: Change) => api.put<Ticket>(`/api/tickets/${encodeURIComponent(key)}`, body),
  remove: (key: string) => api.del<void>(`/api/tickets/${encodeURIComponent(key)}`),
};

export function errorMessage(e: unknown): string {
  return e instanceof ApiError ? e.message : e instanceof Error ? e.message : String(e);
}

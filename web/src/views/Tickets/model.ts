// What the tickets view knows about states, priorities and how a list is grouped (tickets.md section 6).
import type { Link, Priority, Ticket, TState } from "./api";

/** The columns of the board, in order. */
export const STATES: TState[] = ["backlog", "todo", "in_progress", "in_review", "done", "dropped"];
/** The sections of the list: what is being worked on first. */
export const LIST_ORDER: TState[] = ["in_progress", "in_review", "todo", "backlog", "done", "dropped"];
export const STATE_LABEL: Record<TState, string> = { backlog: "Backlog", todo: "To do", in_progress: "In progress", in_review: "In review", done: "Done", dropped: "Dropped" };
export const PRIORITIES: Priority[] = ["none", "low", "medium", "high", "urgent"];
export const PRIORITY_LABEL: Record<Priority, string> = { none: "No priority", low: "Low", medium: "Medium", high: "High", urgent: "Urgent" };
const PRIORITY_RANK: Record<Priority, number> = { urgent: 0, high: 1, medium: 2, low: 3, none: 4 };

export type Mode = "list" | "board" | "mine";
export const MODES: { id: Mode; label: string }[] = [
  { id: "list", label: "List" },
  { id: "board", label: "Board" },
  { id: "mine", label: "Mine now" },
];

export interface Filters {
  /** A folder path, `inbox`, or "" for every project. */
  project: string;
  label: string;
  text: string;
}

/** Urgent first, then the newest change. */
export function ordered(tickets: Ticket[]): Ticket[] {
  return [...tickets].sort((a, b) => PRIORITY_RANK[a.priority] - PRIORITY_RANK[b.priority] || b.updatedAt.localeCompare(a.updatedAt));
}

export function inProject(t: Ticket, project: string): boolean {
  return !project || (project === "inbox" ? t.project === null : t.project === project);
}

export function matches(t: Ticket, f: Filters): boolean {
  if (!inProject(t, f.project)) return false;
  if (f.label && !t.labels.some((l) => l.toLowerCase() === f.label.toLowerCase())) return false;
  const q = f.text.trim().toLowerCase();
  return !q || t.key.toLowerCase().includes(q) || t.title.toLowerCase().includes(q) || t.description.toLowerCase().includes(q);
}

/** In progress and in review, and not somebody else's. */
export function isMine(t: Ticket): boolean {
  return (t.state === "in_progress" || t.state === "in_review") && (t.assignee === null || t.assignee === "owner");
}

export function byState(tickets: Ticket[]): Record<TState, Ticket[]> {
  const out = { backlog: [], todo: [], in_progress: [], in_review: [], done: [], dropped: [] } as Record<TState, Ticket[]>;
  for (const t of ordered(tickets)) out[t.state].push(t);
  return out;
}

/** The project a quick add goes to: the one in the filter, else the Inbox (`null`). */
export function addTarget(project: string): string | null {
  return !project || project === "inbox" ? null : project;
}

const MODE_KEY = "kagent.tickets.mode";

export function readMode(): Mode {
  try {
    const v = localStorage.getItem(MODE_KEY);
    if (v === "list" || v === "board" || v === "mine") return v;
  } catch {
    // no storage: the list
  }
  return "list";
}

export function storeMode(mode: Mode) {
  try {
    localStorage.setItem(MODE_KEY, mode);
  } catch {
    // no storage: not remembered
  }
}

/** `from` and `to` of the first range or commit link; none for a ticket without one. */
export function rangeOf(links: Link[]): { from: string; to?: string } | null {
  const range = links.find((l) => l.kind === "range");
  if (range) {
    const [from, to] = range.value.split("..");
    return to === undefined ? { from } : { from, to };
  }
  const commit = links.find((l) => l.kind === "commit");
  return commit ? { from: `${commit.value}^`, to: commit.value } : null;
}

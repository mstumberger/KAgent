// Shared by the Memory page (memory.md section 7): the rules a new memory must follow
// (the daemon's, tools.md section 18, so a field says what is wrong before anything is
// sent), the filters and the order of the list.
import type { MemoryDraft, MemoryType, MemoryView } from "./client";

export const NAME_RULE = /^[a-z0-9][a-z0-9-]{0,63}$/;
export const MAX_DESCRIPTION = 1024;
export const MAX_CONTENT = 4000;
/** At most this many files, the two core files included. */
export const MAX_FILES = 200;
/** The names of the files KAgent keeps itself: no topic memory takes them. */
export const RESERVED: string[] = ["memory", "user", "machine"];

export const TYPES: MemoryType[] = ["user", "feedback", "machine", "project", "reference"];
export const TYPE_LABEL: Record<MemoryType, string> = { user: "user", feedback: "feedback", machine: "machine", project: "project", reference: "reference" };

/** What each filter shows; "Needs review" is the first order of the list. */
export type Filter = "All" | "Needs review" | "Feedback" | "User" | "Machine" | "Project" | "Reference";
export const FILTERS: Filter[] = ["All", "Needs review", "Feedback", "User", "Machine", "Project", "Reference"];

/** Today, as the daemon writes a day in a memory file: the local date. */
export function localDay(d = new Date()): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** Why a memory is out of recall (memory.md section 2), or `null` while it is recalled. */
export function state(m: Pick<MemoryView, "validUntil" | "supersededBy">, today = localDay()): "forgotten" | "replaced" | null {
  if (m.supersededBy) return "replaced";
  return m.validUntil && m.validUntil <= today ? "forgotten" : null;
}

/** Written by the agent and not yet seen by the owner: what waits for a review. */
export function needsReview(m: Pick<MemoryView, "source" | "reviewed" | "validUntil" | "supersededBy">, today = localDay()): boolean {
  return m.source === "agent" && !m.reviewed && state(m, today) === null;
}

export function inFilter(m: MemoryView, f: Filter): boolean {
  switch (f) {
    case "All":
      return true;
    case "Needs review":
      return needsReview(m);
    case "Feedback":
      return m.type === "feedback";
    case "User":
      return m.type === "user";
    case "Machine":
      return m.type === "machine";
    case "Project":
      return m.type === "project";
    case "Reference":
      return m.type === "reference";
  }
}

/** The order of the list (memory.md section 7): needs review first, then by updated time; what is out of recall comes last. */
export function sortMemories(list: MemoryView[]): MemoryView[] {
  const rank = (m: MemoryView) => (state(m) ? 2 : needsReview(m) ? 0 : 1);
  return [...list].sort((a, b) => rank(a) - rank(b) || b.updatedAt.localeCompare(a.updatedAt) || a.name.localeCompare(b.name));
}

/** "712 bytes", "2.4 KB", "1.1 MB". */
export function sizeText(bytes: number): string {
  if (bytes < 1024) return `${bytes} bytes`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

export type DraftErrors = Partial<Record<"name" | "description" | "content", string>>;

/** What is wrong with a memory before it is sent. The name of a memory being changed is fixed. */
export function draftErrors(d: MemoryDraft, existing: string[], editing: boolean): DraftErrors {
  const out: DraftErrors = {};
  const name = d.name.trim();
  if (!name) out.name = "A name is required.";
  else if (!NAME_RULE.test(name)) out.name = "Lowercase letters, digits and hyphens only; a letter or a digit first; at most 64 characters.";
  else if (RESERVED.includes(name)) out.name = `"${name}" is the name of a file KAgent keeps itself. Choose another name.`;
  else if (!editing && existing.includes(name)) out.name = `A memory named "${name}" exists already.`;
  if ([...d.description].length > MAX_DESCRIPTION) out.description = `At most ${MAX_DESCRIPTION} characters.`;
  const chars = [...d.content.trim()].length;
  if (!chars) out.content = "Write the fact to remember.";
  else if (chars > MAX_CONTENT) out.content = `The content is ${chars} characters; at most ${MAX_CONTENT}.`;
  return out;
}

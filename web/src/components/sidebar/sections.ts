// The pure grouping of the sidebar's list (sidebar.md section 5.3), over the sessions of the
// store. No React here: the order of the sections is what the tests read.
import type { SessionRow } from "../../settings/subagents/api";

export interface Section {
  /** A key that is stable across renders: "pinned", "machine", the folder, or the date bucket. */
  key: string;
  kind: "pinned" | "folder" | "machine" | "date";
  title: string;
  /** The folder of a "folder" section. */
  path?: string;
  /** Every session of the section, newest first, also when it is closed. */
  sessions: SessionRow[];
  closed: boolean;
}

const stamp = (s: SessionRow) => Date.parse(s.updatedAt) || 0;
const newestFirst = (sessions: SessionRow[]) => [...sessions].sort((a, b) => stamp(b) - stamp(a));

/** The last component of a folder path. */
export function folderName(path: string): string {
  const parts = path.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** Pinned, then a section per folder (in the order of the newest session of each), then this machine. */
export function projectSections(sessions: SessionRow[], closed: string[]): Section[] {
  const all = newestFirst(sessions);
  const out: Section[] = [];
  const pinned = all.filter((s) => s.pinned);
  if (pinned.length) out.push({ key: "pinned", kind: "pinned", title: "Pinned", sessions: pinned, closed: false });
  const folders = new Map<string, SessionRow[]>();
  for (const s of all) {
    if (s.pinned || !s.project) continue;
    folders.set(s.project, [...(folders.get(s.project) ?? []), s]);
  }
  for (const [path, list] of folders) out.push({ key: path, kind: "folder", title: folderName(path), path, sessions: list, closed: closed.includes(path) });
  const machine = all.filter((s) => !s.pinned && !s.project);
  if (machine.length) out.push({ key: "machine", kind: "machine", title: "This machine", sessions: machine, closed: false });
  return out;
}

const DAY = 86_400_000;
const midnight = (t: number) => new Date(new Date(t).getFullYear(), new Date(t).getMonth(), new Date(t).getDate()).getTime();

/** The bucket of a session: how many calendar days (local time) before `now` it was updated. */
function bucket(updatedAt: number, now: number): { key: string; title: string } {
  const days = Math.round((midnight(now) - midnight(updatedAt)) / DAY);
  if (days <= 0) return { key: "today", title: "Today" };
  if (days === 1) return { key: "yesterday", title: "Yesterday" };
  if (days <= 7) return { key: "week", title: "Previous 7 days" };
  if (days <= 30) return { key: "month", title: "Previous 30 days" };
  return { key: "older", title: "Older" };
}

/** Every session, newest first, under a heading wherever the bucket changes. */
export function recentSections(sessions: SessionRow[], now: number): Section[] {
  const out: Section[] = [];
  for (const s of newestFirst(sessions)) {
    const b = bucket(stamp(s), now);
    const last = out[out.length - 1];
    if (last && last.key === b.key) last.sessions.push(s);
    else out.push({ key: b.key, kind: "date", title: b.title, sessions: [s], closed: false });
  }
  return out;
}

/** The sessions whose title or folder contains `query` (either case), newest first. */
export function searchSessions(sessions: SessionRow[], query: string): SessionRow[] {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  return newestFirst(sessions).filter((s) => (s.title || "New session").toLowerCase().includes(q) || (s.project ?? "").toLowerCase().includes(q));
}

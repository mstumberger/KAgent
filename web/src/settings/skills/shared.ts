// Shared by the Skills page: names, filters, time, and the rules a draft must follow.
// The rules are the daemon's (tools.md section 17), so a field says what is wrong
// before anything is sent.
import type { SkillView } from "../../api";

/** Skills KAgent ships with whose names a new skill must not take. */
export const RESERVED: string[] = ["skill-creator", "widget-design"];
export const NAME_RULE = /^[a-z0-9][a-z0-9-]{0,63}$/;
export const MAX_DESCRIPTION = 1024;
export const MAX_BODY_BYTES = 64 * 1024;

export type Source = SkillView["source"];

/** The badge of a source. */
export const SOURCE_LABEL: Record<Source, string> = { bundled: "bundled", user: "mine", imported: "imported" };

/** Two skills of one name can be listed (one hides the other): name and source tell them apart. */
export const keyOf = (s: Pick<SkillView, "name" | "source">) => `${s.source}:${s.name}`;

export type Filter = "All" | "On" | "Off" | "Bundled" | "Mine" | "Imported";
export const FILTERS: Filter[] = ["All", "On", "Off", "Bundled", "Mine", "Imported"];

export function inFilter(s: SkillView, f: Filter): boolean {
  switch (f) {
    case "All":
      return true;
    case "On":
      return s.enabled && !s.shadowed;
    case "Off":
      return !s.enabled || s.shadowed;
    case "Bundled":
      return s.source === "bundled";
    case "Mine":
      return s.source === "user";
    case "Imported":
      return s.source === "imported";
  }
}

/** "today", "yesterday", "3 days ago", "2 weeks ago", "5 months ago"; empty without a date. */
export function ago(iso: string | undefined, now = Date.now()): string {
  if (!iso) return "";
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "";
  const days = Math.max(0, Math.floor((now - then) / 86_400_000));
  if (days === 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 14) return `${days} days ago`;
  if (days < 60) return `${Math.round(days / 7)} weeks ago`;
  const months = Math.round(days / 30);
  return months < 12 ? `${months} month${months === 1 ? "" : "s"} ago` : `${Math.round(days / 365)} years ago`;
}

export interface Draft {
  name: string;
  description: string;
  whenToUse: string;
  body: string;
}

export type DraftErrors = Partial<Record<"name" | "description" | "body", string>>;

export const emptyDraft = (): Draft => ({ name: "", description: "", whenToUse: "", body: "" });

/** What is wrong with a draft. A new skill must not take a name that exists; the name of a skill being changed is fixed. */
export function draftErrors(d: Draft, existing: string[], editing: boolean): DraftErrors {
  const out: DraftErrors = {};
  const name = d.name.trim();
  if (!name) out.name = "A name is required.";
  else if (!NAME_RULE.test(name)) out.name = "Lowercase letters, digits and hyphens only; a letter or a digit first; at most 64 characters.";
  else if (RESERVED.includes(name)) out.name = `${name} is the name of a skill KAgent ships with. Choose another name.`;
  else if (!editing) {
    const other = existing.find((n) => n.toLowerCase() === name.toLowerCase());
    if (other) out.name = `A skill named "${other}" exists already.`;
  }
  const chars = [...d.description].length;
  if (chars > MAX_DESCRIPTION) out.description = `At most ${MAX_DESCRIPTION} characters.`;
  const bytes = new TextEncoder().encode(d.body).length;
  if (!d.body.trim()) out.body = "Write what the agent should do.";
  else if (bytes > MAX_BODY_BYTES) out.body = `The body is ${bytes} bytes; at most ${MAX_BODY_BYTES}.`;
  return out;
}

/** `<name>-copy`, then `-copy-2`, ...: the first name no skill has, at most 64 characters. */
export function copyName(name: string, existing: string[]): string {
  const base = `${name.slice(0, 56)}-copy`;
  for (let i = 1; ; i++) {
    const candidate = i === 1 ? base : `${base}-${i}`;
    if (!existing.includes(candidate)) return candidate;
  }
}

// The slash commands of the chat composer (skills.md section 6). Typing `/` offers the
// enabled skills and the built-in commands. A skill goes to the daemon as
// `/<name> <request>`; a built-in command is done by the UI: `/new` starts a session,
// `/compact` calls the compaction route of the active session, `/plan <request>` sends the
// message with `plan: true` (plan-todo.md section 2) and `/plan` alone arms the Plan toggle.
import type { SkillView } from "./api";

export interface SlashCommand {
  name: string;
  description: string;
  /** The stage that gives the command its behaviour; until then the picker only names it. */
  comesWith?: string;
}

export const BUILTIN_COMMANDS: SlashCommand[] = [
  { name: "new", description: "Start a new, empty session" },
  { name: "compact", description: "Summarize the conversation so far" },
  { name: "plan", description: "Plan first: the agent researches and shows a plan before anything changes" },
];

/** One line of the picker: a skill or a built-in command. */
export interface SlashItem extends SlashCommand {
  skill: boolean;
}

/** What `/` offers: the built-in commands and the enabled skills, each alphabetical. */
export function slashItems(skills: SkillView[]): SlashItem[] {
  const list: SlashItem[] = skills
    .filter((s) => s.enabled && !s.shadowed)
    .map((s) => ({ name: s.name, description: s.description, skill: true }))
    .sort((a, b) => a.name.localeCompare(b.name));
  return [...BUILTIN_COMMANDS.map((c) => ({ ...c, skill: false })), ...list];
}

/** The slash word being typed: the whole text is `/` and one word (`/sl`). Null once a
 * space or anything but a name character is in the text. */
export function slashWord(text: string): string | null {
  const m = /^\/([a-z0-9-]*)$/.exec(text);
  return m ? m[1] : null;
}

/** The items for `query`: names that start with it first, then those whose description
 * mentions it. For a bare `/` the commands come first, then the skills. */
export function matchSlash(items: SlashItem[], query: string, limit = 8): SlashItem[] {
  const q = query.toLowerCase();
  const rank = (i: SlashItem) => {
    if (!q) return i.skill ? 1 : 0;
    if (i.name.toLowerCase().startsWith(q)) return 0;
    return i.description.toLowerCase().includes(q) ? 1 : 2;
  };
  return items
    .map((item) => ({ item, rank: rank(item) }))
    .filter((x) => x.rank < 2)
    .sort((a, b) => a.rank - b.rank || Number(a.item.skill) - Number(b.item.skill) || a.item.name.localeCompare(b.item.name))
    .slice(0, limit)
    .map((x) => x.item);
}

/** The built-in command a line is (`/new`, `/compact`), or null. */
export function builtinOf(text: string): SlashCommand | null {
  const word = text.trim();
  return BUILTIN_COMMANDS.find((c) => `/${c.name}` === word) ?? null;
}

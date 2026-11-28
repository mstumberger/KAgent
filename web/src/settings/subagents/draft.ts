// The draft behind the form of a subagent profile (agents.md section 1), the rules it is
// checked against before anything is sent, and what goes to the daemon. Here and not in
// `ProfileEditor.tsx`, which exports its component only (fast refresh).
import type { AgentProfileView, ProfileBody } from "./api";

/** `^[a-z0-9][a-z0-9-]{0,63}$`, the rule of a skill's name. */
export const NAME_OK = /^[a-z0-9][a-z0-9-]{0,63}$/;
export const MAX_DESCRIPTION = 500;
export const MAX_PROMPT = 8000;
export const MAX_ITERATIONS = 80;

export interface Draft {
  name: string;
  description: string;
  /** A model ref; empty: the chat model. */
  model: string;
  /** Empty: every enabled group. */
  groups: string[];
  iterations: string;
  prompt: string;
}

export const emptyDraft = (): Draft => ({ name: "", description: "", model: "", groups: [], iterations: "25", prompt: "" });

export const draftOf = (p: AgentProfileView): Draft => ({ name: p.name, description: p.description, model: p.model, groups: p.toolGroups, iterations: String(p.maxIterations), prompt: p.prompt });

/** What is wrong with the draft before it is sent: the rules of agents.md section 1, as the daemon checks them. `taken`: names that exist already. */
export function draftErrors(d: Draft, taken: string[], editing: boolean): Partial<Record<keyof Draft, string>> {
  const errors: Partial<Record<keyof Draft, string>> = {};
  const chars = (s: string) => [...s].length;
  const name = d.name.trim();
  if (!editing) {
    if (!name) errors.name = "The name is needed: the agent names it in agent_run.";
    else if (!NAME_OK.test(name)) errors.name = "Lowercase letters, digits and hyphens, starting with a letter or a digit; at most 64 characters.";
    else if (taken.includes(name)) errors.name = "A profile with this name exists already: a built-in one cannot be changed, another can (open it in the list).";
  }
  if (!d.description.trim()) errors.description = "Say in a few words what the profile is for.";
  else if (chars(d.description.trim()) > MAX_DESCRIPTION) errors.description = `At most ${MAX_DESCRIPTION} characters.`;
  const n = Number(d.iterations);
  if (d.iterations.trim() === "" || !Number.isInteger(n) || n < 1 || n > MAX_ITERATIONS) errors.iterations = `A whole number from 1 to ${MAX_ITERATIONS}.`;
  if (chars(d.prompt) > MAX_PROMPT) errors.prompt = `At most ${MAX_PROMPT} characters.`;
  return errors;
}

/** What goes to the daemon (the name is in the path). */
export const bodyOf = (d: Draft): ProfileBody => ({ description: d.description.trim(), model: d.model, toolGroups: d.groups, maxIterations: Number(d.iterations), prompt: d.prompt });

// The form of a hook: what is typed, what is sent, and what is wrong with it before
// anything is sent. The rules are the daemon's (config.md section 11), mirrored here so
// the form can say what is the matter at the field.
import { takesMatcher, type Hook, type HookEventName } from "./api";

export const MIN_TIMEOUT_MS = 100;
export const MAX_TIMEOUT_MS = 60000;
export const DEFAULT_TIMEOUT_MS = 5000;

export interface Draft {
  event: HookEventName;
  matcher: string[];
  command: string;
  /** One argument a line. */
  args: string;
  shell: string;
  timeoutMs: string;
  /** `async` of the record: started and not awaited. */
  background: boolean;
}

export const emptyDraft = (event: HookEventName = "PreToolUse"): Draft => ({ event, matcher: [], command: "", args: "", shell: "", timeoutMs: String(DEFAULT_TIMEOUT_MS), background: false });

export const draftOf = (hook: Hook): Draft => ({
  event: hook.event,
  matcher: [...(hook.matcher ?? [])],
  command: hook.command,
  args: (hook.args ?? []).join("\n"),
  shell: hook.shell ?? "",
  timeoutMs: String(hook.timeoutMs ?? DEFAULT_TIMEOUT_MS),
  background: hook.async ?? false,
});

/** A line may hold several words: they stay one argument. */
const argsOf = (text: string) =>
  text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);

/** What goes to the daemon: camelCase, as config.md section 11 has it. */
export const bodyOf = (draft: Draft, enabled: boolean): Hook => ({
  event: draft.event,
  ...(takesMatcher(draft.event) && draft.matcher.length > 0 ? { matcher: draft.matcher } : {}),
  command: draft.command.trim(),
  ...(argsOf(draft.args).length > 0 ? { args: argsOf(draft.args) } : {}),
  ...(draft.shell.trim() ? { shell: draft.shell.trim() } : {}),
  timeoutMs: Number(draft.timeoutMs),
  async: draft.background,
  enabled,
});

export type DraftErrors = Partial<Record<"matcher" | "command" | "args" | "shell" | "timeoutMs", string>>;

/** What is wrong with the draft, field by field; empty when it can be sent. */
export const draftErrors = (draft: Draft): DraftErrors => {
  const errors: DraftErrors = {};
  if (!takesMatcher(draft.event) && draft.matcher.length > 0) errors.matcher = "Only PreToolUse and PostToolUse call a tool, so only they have a tool to match.";
  if (!draft.command.trim()) errors.command = "The program to run is required.";
  else if (draft.command.includes("\n")) errors.command = "One line. A longer script goes in a file, and the command runs that.";
  if (draft.shell.trim() && argsOf(draft.args).length > 0) errors.args = "With a shell the whole line goes in Command; Arguments would not be used.";
  if (/\s/.test(draft.shell.trim())) errors.shell = "One word: the shell that runs the line, for example /bin/bash.";
  const timeout = Number(draft.timeoutMs);
  if (draft.timeoutMs.trim() === "" || !Number.isInteger(timeout) || timeout < MIN_TIMEOUT_MS || timeout > MAX_TIMEOUT_MS) {
    errors.timeoutMs = `A whole number of milliseconds from ${MIN_TIMEOUT_MS} to ${MAX_TIMEOUT_MS}.`;
  }
  return errors;
};

/** The command as the list shows it. */
export const lineOf = (hook: Hook): string => (hook.shell ? `${hook.shell} -c ${hook.command}` : [hook.command, ...(hook.args ?? [])].join(" "));

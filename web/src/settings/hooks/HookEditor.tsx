// The form of a new or a changed hook (hooks.md section 7): event, matcher as tool chips,
// command, arguments one a line, shell, timeout, run in the background. A field says what
// is wrong with it before anything is sent; what the daemon refuses on top of that is
// shown above the form.
import { useState, type ChangeEvent, type FormEvent } from "react";
import { Toggle } from "../ui";
import { EVENTS, takesMatcher, type Hook, type HookEventName } from "./api";
import { MAX_TIMEOUT_MS, MIN_TIMEOUT_MS, bodyOf, draftErrors, type Draft } from "./draft";
import Select from "../Select";

interface Props {
  initial: Draft;
  /** On for a new hook; for a change, what the hook is now. */
  enabled: boolean;
  /** The tool names the daemon has, for the matcher. */
  tools: string[];
  busy: boolean;
  serverError?: string;
  onSave: (body: Hook) => Promise<void>;
  onCancel: () => void;
}

export default function HookEditor({ initial, enabled, tools, busy, serverError, onSave, onCancel }: Props) {
  const [draft, setDraft] = useState<Draft>(initial);
  // Fields show their error once a save was tried, then as the user types.
  const [tried, setTried] = useState(false);
  const [find, setFind] = useState("");
  const errors = tried ? draftErrors(draft) : {};
  const set = (field: "command" | "args" | "shell" | "timeoutMs") => (e: ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => setDraft((d) => ({ ...d, [field]: e.target.value }));
  const changeEvent = (e: { target: { value: string } }) => {
    const event = e.target.value as HookEventName;
    // A tool matcher is for the two tool events only.
    setDraft((d) => ({ ...d, event, matcher: takesMatcher(event) ? d.matcher : [] }));
  };
  const toggle = (tool: string) => setDraft((d) => ({ ...d, matcher: d.matcher.includes(tool) ? d.matcher.filter((t) => t !== tool) : [...d.matcher, tool] }));

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setTried(true);
    if (Object.keys(draftErrors(draft)).length > 0) return;
    await onSave(bodyOf(draft, enabled));
  };
  const needle = find.trim().toLowerCase();
  const shown = tools.filter((t) => t.toLowerCase().includes(needle));
  const info = EVENTS.find((e) => e.name === draft.event);
  const bad = (text?: string) => (
    <span className="ui-inline-bad" role="alert">
      {text}
    </span>
  );

  return (
    <form className="ui-form hk-editor" onSubmit={(e) => void submit(e)} noValidate>
      {serverError && (
        <p className="hk-error" role="alert">
          {serverError}
        </p>
      )}
      <label className="ui-field">
        <span className="ui-field-label">Event</span>
        <Select value={draft.event} aria-label="Hook event" onChange={changeEvent}>
          {EVENTS.map((e) => (
            <option key={e.name} value={e.name}>
              {e.name}
            </option>
          ))}
        </Select>
        <span className="hint">
          {info?.when} {info?.decides}
        </span>
      </label>
      {takesMatcher(draft.event) ? (
        <div className="ui-field">
          <span className="ui-field-label">Tools it runs for</span>
          <input className="hk-find" value={find} aria-label="Find a tool" placeholder="Find a tool" onChange={(e) => setFind(e.target.value)} />
          <div className="chips hk-chips" role="group" aria-label="Matcher">
            {shown.map((tool) => (
              <label key={tool} className={`chip${draft.matcher.includes(tool) ? " on" : ""}`} data-tool={tool}>
                <input type="checkbox" checked={draft.matcher.includes(tool)} onChange={() => toggle(tool)} /> {tool}
              </label>
            ))}
            {shown.length === 0 && <span className="hint">No tool of that name.</span>}
          </div>
          <span className="hint">None picked: every tool call. The hook gets the tool's name and input.</span>
          {errors.matcher && bad(errors.matcher)}
        </div>
      ) : null}
      <label className="ui-field">
        <span className="ui-field-label">Command</span>
        <input className="hk-mono" value={draft.command} aria-label="Hook command" aria-invalid={!!errors.command} onChange={set("command")} />
        <span className="hint">An executable path or a name found through PATH. With a shell below: the one line that shell runs.</span>
        {errors.command && bad(errors.command)}
      </label>
      <label className="ui-field">
        <span className="ui-field-label">Arguments</span>
        <textarea className="hk-mono" rows={3} value={draft.args} aria-label="Hook arguments" aria-invalid={!!errors.args} onChange={set("args")} />
        <span className="hint">One argument a line; a line of several words stays one argument.</span>
        {errors.args && bad(errors.args)}
      </label>
      <label className="ui-field">
        <span className="ui-field-label">Shell</span>
        <input className="hk-mono" value={draft.shell} aria-label="Hook shell" aria-invalid={!!errors.shell} onChange={set("shell")} placeholder="/bin/bash" />
        <span className="hint">Leave empty to run the command itself. Else the program that runs the line, for example /bin/bash.</span>
        {errors.shell && bad(errors.shell)}
      </label>
      <label className="ui-field hk-timeout">
        <span className="ui-field-label">Timeout (ms)</span>
        <input type="number" min={MIN_TIMEOUT_MS} max={MAX_TIMEOUT_MS} step={1} value={draft.timeoutMs} aria-label="Hook timeout" aria-invalid={!!errors.timeoutMs} onChange={set("timeoutMs")} />
        <span className="hint">
          {MIN_TIMEOUT_MS} to {MAX_TIMEOUT_MS}. Past it the hook is killed and counts as failed.
        </span>
        {errors.timeoutMs && bad(errors.timeoutMs)}
      </label>
      <div className="ui-field">
        <Toggle checked={draft.background} onChange={(background) => setDraft((d) => ({ ...d, background }))} label="Run in the background" />
        <span className="hint">Started and not awaited; the agent does not wait for it. Only context it adds is used, for the next turn.</span>
      </div>
      <div className="row">
        <button className="primary" type="submit" disabled={busy}>
          Save
        </button>
        <button type="button" disabled={busy} onClick={onCancel}>
          Cancel
        </button>
      </div>
    </form>
  );
}

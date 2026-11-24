// Task list: scheduled jobs, reminders and to-dos KAgent runs for you. The same list the
// agent manages with its `tasks` tool ("remind me…", "every weekday at 9…").
import { useEffect, useState } from "react";
import { api } from "../api";
import Markdown from "../components/Markdown";
import { openSession, setView, useStore } from "../store";

interface Task {
  id: string;
  title: string;
  what: string;
  kind: "agent" | "remind";
  when: string;
  enabled: boolean;
  done: boolean;
  nextRun?: string | null;
  lastRun?: string | null;
  lastStatus?: string | null;
  lastResult?: string | null;
  lastSession?: string | null;
  runs: number;
}

const EXAMPLES = ["in 20m", "tomorrow 07:00", "daily 09:00", "weekdays 08:30", "every 2h"];

function rel(iso?: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  const diff = (d.getTime() - Date.now()) / 60000;
  const abs = Math.abs(diff);
  const span = abs < 1 ? "now" : abs < 60 ? `${Math.round(abs)} min` : abs < 1440 ? `${Math.round(abs / 60)} h` : `${Math.round(abs / 1440)} d`;
  const when = span === "now" ? "now" : diff > 0 ? `in ${span}` : `${span} ago`;
  return `${when} (${d.toLocaleString(undefined, { weekday: "short", hour: "2-digit", minute: "2-digit", day: "numeric", month: "short" })})`;
}

function stateOf(t: Task): { label: string; cls: string } {
  if (t.lastStatus === "running") return { label: "Running", cls: "run" };
  if (t.done) return { label: "Done", cls: "done" };
  if (!t.enabled) return { label: "Paused", cls: "paused" };
  if (!t.nextRun) return { label: "To-do", cls: "todo" };
  return { label: "Scheduled", cls: "sched" };
}

export default function Tasks() {
  const s = useStore();
  const [tasks, setTasks] = useState<Task[]>([]);
  const [form, setForm] = useState({ title: "", what: "", kind: "agent", when: "" });
  const [error, setError] = useState("");
  const [open, setOpen] = useState<string | null>(null);
  const [showDone, setShowDone] = useState(false);

  const load = () => void api.get<{ tasks: Task[] }>("/api/tasks").then((r) => setTasks(r.tasks));
  useEffect(load, [s.tasksTick]);
  // Relative times ("in 12 min") stay current.
  const [, setTick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => setTick((x) => x + 1), 30000);
    return () => clearInterval(t);
  }, []);

  const add = async (e: React.FormEvent) => {
    e.preventDefault();
    setError("");
    try {
      await api.post("/api/tasks", form);
      setForm({ title: "", what: "", kind: form.kind, when: "" });
      load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };
  const act = async (fn: () => Promise<unknown>) => {
    setError("");
    try {
      await fn();
      load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const active = tasks.filter((t) => !t.done).sort((a, b) => (a.nextRun ?? "9").localeCompare(b.nextRun ?? "9"));
  const finished = tasks.filter((t) => t.done).sort((a, b) => (b.lastRun ?? "").localeCompare(a.lastRun ?? ""));

  const row = (t: Task) => {
    const st = stateOf(t);
    const isOpen = open === t.id;
    return (
      <li key={t.id} className={`task task-${st.cls}`}>
        <button className="task-main" onClick={() => setOpen(isOpen ? null : t.id)} aria-expanded={isOpen}>
          <span className={`task-state ${st.cls}`}>{st.label}</span>
          <span className="task-title">
            {t.kind === "remind" && <span className="task-kind">Reminder</span>}
            {t.title}
          </span>
          <span className="task-when">
            {t.when}
            {t.nextRun && !t.done && t.enabled && <span className="task-next">next {rel(t.nextRun)}</span>}
          </span>
        </button>
        {isOpen && (
          <div className="task-detail">
            <p className="task-what">{t.what}</p>
            {t.lastRun && (
              <div className="task-last">
                <div className="hint">
                  Last run {rel(t.lastRun)}, {t.lastStatus === "ok" ? "finished" : t.lastStatus}, {t.runs} run{t.runs === 1 ? "" : "s"} so far
                </div>
                {t.lastResult && <Markdown text={t.lastResult} />}
              </div>
            )}
            <div className="row">
              <button className="primary" disabled={t.lastStatus === "running"} onClick={() => void act(() => api.post(`/api/tasks/${t.id}/run`, {}))}>
                Run now
              </button>
              {!t.done && t.nextRun !== undefined && (
                <button onClick={() => void act(() => api.put(`/api/tasks/${t.id}`, { enabled: !t.enabled }))}>{t.enabled ? "Pause" : "Resume"}</button>
              )}
              {!t.done && <button onClick={() => void act(() => api.put(`/api/tasks/${t.id}`, { done: true }))}>Mark done</button>}
              {t.lastSession && (
                <button
                  onClick={() => {
                    void openSession(t.lastSession!);
                    setView("chat");
                  }}
                >
                  Open last run
                </button>
              )}
              <button className="danger" onClick={() => void act(() => api.del(`/api/tasks/${t.id}`))}>
                Delete
              </button>
            </div>
          </div>
        )}
      </li>
    );
  };

  return (
    <div className="page tasks-page">
      <h2>Tasks</h2>
      <form className="card task-form" onSubmit={(e) => void add(e)}>
        <div className="task-form-row">
          <select aria-label="Kind" value={form.kind} onChange={(e) => setForm({ ...form, kind: e.target.value })}>
            <option value="agent">KAgent does</option>
            <option value="remind">Remind me</option>
          </select>
          <input
            className="grow"
            aria-label="What"
            placeholder={form.kind === "remind" ? "Call the electrician about the doorbell" : "Check for security updates and tell me what needs a reboot"}
            value={form.what}
            onChange={(e) => setForm({ ...form, what: e.target.value })}
          />
        </div>
        <div className="task-form-row">
          <input aria-label="Title (optional)" placeholder="Title (optional)" value={form.title} onChange={(e) => setForm({ ...form, title: e.target.value })} />
          <input aria-label="When" placeholder="When: empty = to-do" value={form.when} onChange={(e) => setForm({ ...form, when: e.target.value })} />
          <button className="primary" disabled={!form.what.trim()}>
            Add task
          </button>
        </div>
        <div className="task-examples">
          {EXAMPLES.map((x) => (
            <button type="button" key={x} className="chip" onClick={() => setForm({ ...form, when: x })}>
              {x}
            </button>
          ))}
        </div>
        {error && <div className="bad-text">{error}</div>}
      </form>

      {active.length === 0 ? (
        <p className="empty">No tasks yet. Add one above, or tell KAgent "every weekday at 9, check for updates".</p>
      ) : (
        <ul className="task-list">{active.map(row)}</ul>
      )}

      {finished.length > 0 && (
        <>
          <button className="link-btn" onClick={() => setShowDone(!showDone)}>
            {showDone ? "Hide" : "Show"} finished ({finished.length})
          </button>
          {showDone && <ul className="task-list finished">{finished.map(row)}</ul>}
        </>
      )}
    </div>
  );
}

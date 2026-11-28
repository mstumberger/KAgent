// Tasks: scheduled jobs, reminders and to-dos KAgent runs for you (automations.md
// section 6). The same list the agent manages with its `tasks` tool. The list is read
// again whenever the daemon says it changed (`tasksChanged`, `taskRun`).
import { useEffect, useState } from "react";
import Markdown from "../components/Markdown";
import { openSession, setView, useStore } from "../store";
import { errorMessage, rel, tasksApi, type Task, type TaskTemplate } from "./tasks/api";
import TaskForm from "./tasks/TaskForm";
import TaskHistory from "./tasks/TaskHistory";
import "./tasks/tasks.css";
import { Page, PageHeader } from "../components/page/Page";

/** The pill of a row: what the task is doing. */
function stateOf(t: Task): { label: string; cls: string } {
  if (t.lastStatus === "running") return { label: "Running", cls: "running" };
  if (t.done) return { label: "Done", cls: "done" };
  if (!t.enabled) return { label: "Paused", cls: "paused" };
  if (!t.nextRun) return { label: "To-do", cls: "todo" };
  return { label: "Scheduled", cls: "sched" };
}

/** The policy of an agent task in a line. */
function policy(t: Task): string {
  const parts = [`Deliver: ${t.deliver.length > 0 ? t.deliver.join(", ") : "nothing shown"}`];
  if (t.kind === "agent") {
    parts.push(t.onApproval === "wait" ? "a step that needs approval waits for me" : "a step that needs approval is skipped");
    parts.push(t.session === "reuse" ? "one session for the task" : "a new session for each run");
    parts.push(`at most ${t.timeoutMin} min a run`);
  }
  return parts.join(" | ");
}

export default function Tasks() {
  const s = useStore();
  const [tasks, setTasks] = useState<Task[]>([]);
  const [templates, setTemplates] = useState<TaskTemplate[]>([]);
  const [error, setError] = useState("");
  const [open, setOpen] = useState<string | null>(null);
  const [showDone, setShowDone] = useState(false);

  // Read again whenever the list changed; an answer that arrives after a newer one is dropped.
  useEffect(() => {
    let live = true;
    tasksApi.list().then(
      (list) => {
        if (live) setTasks(list);
      },
      (e) => {
        if (live) setError(errorMessage(e));
      },
    );
    return () => {
      live = false;
    };
  }, [s.tasksTick]);
  useEffect(() => {
    let live = true;
    tasksApi.templates().then(
      (list) => {
        if (live) setTemplates(list);
      },
      () => {
        // a daemon without the route: no templates
      },
    );
    return () => {
      live = false;
    };
  }, []);

  // Relative times ("in 12 min") stay current.
  const [, setTick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => setTick((x) => x + 1), 30000);
    return () => clearInterval(t);
  }, []);

  // Every route that changes the list answers it; one that does not (cancel) is followed
  // by reading the list again, and the event of the daemon brings what came of it.
  const act = async (fn: () => Promise<unknown>) => {
    setError("");
    try {
      const r = await fn();
      setTasks(r && Array.isArray((r as { tasks?: unknown }).tasks) ? (r as { tasks: Task[] }).tasks : await tasksApi.list());
    } catch (err) {
      setError(errorMessage(err));
    }
  };

  const active = tasks.filter((t) => !t.done).sort((a, b) => (a.nextRun ?? "9").localeCompare(b.nextRun ?? "9"));
  const finished = tasks.filter((t) => t.done).sort((a, b) => (b.lastRun ?? "").localeCompare(a.lastRun ?? ""));

  const row = (t: Task) => {
    const st = stateOf(t);
    const isOpen = open === t.id;
    const isRunning = t.lastStatus === "running";
    return (
      <li key={t.id} className={`task task-${st.cls}`} data-task={t.id}>
        <button className="task-main" onClick={() => setOpen(isOpen ? null : t.id)} aria-expanded={isOpen}>
          <span className={`task-state ${st.cls}`}>{st.label}</span>
          <span className="task-title">
            {t.kind === "remind" && <span className="task-kind">Reminder</span>}
            {t.title}
            {t.lastStatus && t.lastStatus !== "running" && <span className={`run-pill ${t.lastStatus}`}>{t.lastStatus}</span>}
          </span>
          <span className="task-when">
            {t.scheduleText}
            {t.nextRun && !t.done && t.enabled && <span className="task-next">next {rel(t.nextRun)}</span>}
          </span>
        </button>
        {isOpen && (
          <div className="task-detail">
            <p className="task-what">{t.what}</p>
            <div className="hint">{policy(t)}</div>
            <TaskHistory task={t} onCancel={() => void act(() => tasksApi.cancel(t.id))} />
            {t.lastResult && (
              <div className="task-last">
                <div className="hint">{t.kind === "remind" ? "Last" : "Last answer"}</div>
                <Markdown text={t.lastResult} />
              </div>
            )}
            <div className="row">
              <button className="primary" disabled={isRunning} onClick={() => void act(() => tasksApi.run(t.id))}>
                Run now
              </button>
              {!t.done && t.schedule && (
                <button onClick={() => void act(() => tasksApi.update(t.id, { enabled: !t.enabled }))}>{t.enabled ? "Pause" : "Resume"}</button>
              )}
              {!t.done && <button onClick={() => void act(() => tasksApi.update(t.id, { done: true }))}>Mark done</button>}
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
              <button className="danger" onClick={() => void act(() => tasksApi.remove(t.id))}>
                Delete
              </button>
            </div>
          </div>
        )}
      </li>
    );
  };

  return (
    <Page className="tasks-page">
      <PageHeader title="Tasks" />
      <TaskForm
        onAdded={(list) => {
          setTasks(list);
          setError("");
        }}
        onError={setError}
      />
      {error && <div className="bad-text" role="alert">{error}</div>}

      {templates.length > 0 && tasks.length < 3 && (
        <div className="task-templates" aria-label="Templates">
          {templates.map((template) => (
            <button key={template.id} className="task-template" data-template={template.id} onClick={() => void act(() => tasksApi.addTemplate(template.id))}>
              <span className="task-template-title">{template.title}</span>
              <span className="task-template-what">{template.what}</span>
              <span className="task-template-when">{template.scheduleText}. Added paused.</span>
            </button>
          ))}
        </div>
      )}

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
    </Page>
  );
}

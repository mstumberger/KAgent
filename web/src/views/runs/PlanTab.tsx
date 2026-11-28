// The Plan tab (workflows.md section 8): the tasks as a table, the approval card while the plan waits for the owner, the decision
// about a task that needs him, and the questions of the workers.
import { useState } from "react";
import Icon from "../../components/Icon";
import { errorMessage, runsApi, type Choice, type Run, type Task, type TaskEdit } from "./api";
import { StatusDot } from "./Tree";
import { taskDot, taskWord, workerOfTask } from "./model";

interface Props {
  run: Run;
  onChanged: (r: Run) => void;
  onOpenWorker: (session: string, title: string) => void;
  onOpenChanges: (task: string) => void;
  onError: (m: string) => void;
}

function Approval({ run, onError, onChanged }: { run: Run; onError: (m: string) => void; onChanged: (r: Run) => void }) {
  const [edit, setEdit] = useState(false);
  const [tasks, setTasks] = useState<TaskEdit[]>(() => run.tasks.map((t) => ({ id: t.id, title: t.title })));
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const act = (f: () => Promise<Run>) => {
    setBusy(true);
    f().then(
      (r) => {
        onChanged(r);
        onError("");
      },
      (e) => onError(errorMessage(e)),
    ).finally(() => setBusy(false));
  };
  const move = (i: number, d: number) => setTasks((ts) => {
    const out = [...ts];
    const j = i + d;
    if (j < 0 || j >= out.length) return ts;
    [out[i], out[j]] = [out[j], out[i]];
    return out;
  });
  return (
    <section className="rn-approval" aria-label="Plan approval">
      <h3>The plan waits for you</h3>
      <p>{run.planSummary || "The planner gave no summary."}</p>
      {edit ? (
        <ul className="rn-edit">
          {tasks.map((t, i) => (
            <li key={t.id}>
              <span className="rn-id">{t.id}</span>
              <input type="text" aria-label={`Title of ${t.id}`} value={t.title ?? ""} onChange={(e) => setTasks((ts) => ts.map((x, k) => (k === i ? { ...x, title: e.target.value } : x)))} />
              <button type="button" aria-label={`Move ${t.id} up`} disabled={i === 0} onClick={() => move(i, -1)}><Icon name="chevron" size={12} /></button>
              <button type="button" aria-label={`Move ${t.id} down`} disabled={i === tasks.length - 1} onClick={() => move(i, 1)}><Icon name="chevron" size={12} /></button>
              <button type="button" aria-label={`Remove ${t.id}`} onClick={() => setTasks((ts) => ts.filter((_, k) => k !== i))}><Icon name="close" size={12} /></button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="rn-none">{run.tasks.length} tasks. Edit them to rename, reorder or remove before you approve.</p>
      )}
      <div className="rn-actions">
        <button type="button" className="primary" disabled={busy || tasks.length === 0} onClick={() => act(() => runsApi.approve(run.id, edit ? tasks : undefined))}>Approve plan</button>
        <button type="button" aria-pressed={edit} onClick={() => setEdit(!edit)}>{edit ? "Stop editing" : "Edit tasks"}</button>
      </div>
      <div className="rn-reject">
        <label>
          <span>Reject with a reason</span>
          <textarea rows={2} value={reason} onChange={(e) => setReason(e.target.value)} placeholder="What the planner should change" />
        </label>
        <button type="button" disabled={busy || reason.trim() === ""} onClick={() => act(() => runsApi.reject(run.id, reason.trim()).then((r) => {
          setReason("");
          return r;
        }))}>Reject plan</button>
      </div>
    </section>
  );
}

function Decision({ run, task, onError, onChanged }: { run: Run; task: Task; onError: (m: string) => void; onChanged: (r: Run) => void }) {
  const [busy, setBusy] = useState(false);
  const last = task.rounds[task.rounds.length - 1];
  const choose = (c: Choice) => {
    setBusy(true);
    runsApi.decide(run.id, task.id, c).then(
      (r) => {
        onChanged(r);
        onError("");
      },
      (e) => onError(errorMessage(e)),
    ).finally(() => setBusy(false));
  };
  return (
    <section className="rn-decision" aria-label={`Decision about ${task.id}`}>
      <h4>{task.id} still has problems after {task.rounds.length} rounds</h4>
      <ul>
        {(last?.findings ?? []).map((f, i) => (
          <li key={i} data-severity={f.severity}>{f.severity === "must" ? "must" : "should"}: {f.what}{f.file ? ` (${f.file}${f.line ? `:${f.line}` : ""})` : ""}</li>
        ))}
      </ul>
      <div className="rn-actions">
        <button type="button" disabled={busy} onClick={() => choose("more_round")}>One more round</button>
        <button type="button" disabled={busy} onClick={() => choose("accept")}>Accept as is</button>
        <button type="button" className="danger" disabled={busy} onClick={() => choose("drop")}>Drop the task</button>
      </div>
    </section>
  );
}

function Questions({ run, onError, onChanged }: { run: Run; onError: (m: string) => void; onChanged: (r: Run) => void }) {
  const [text, setText] = useState<Record<string, string>>({});
  const open = run.questions.filter((q) => q.state !== "answered");
  if (open.length === 0) return null;
  const send = (id: string, answer: string) => {
    if (!answer.trim()) return;
    runsApi.answer(run.id, id, answer.trim()).then(
      (r) => {
        onChanged(r);
        onError("");
      },
      (e) => onError(errorMessage(e)),
    );
  };
  return (
    <section className="rn-questions" aria-label="Questions of the workers">
      <h3>Questions</h3>
      {open.map((q) => (
        <div key={q.id} className="rn-question" data-state={q.state}>
          <p><strong>{q.task || "planner"}</strong> asks: {q.text}</p>
          {q.state === "open" && <p className="rn-none">The conversation that started the run may answer first.</p>}
          {q.options.length > 0 && (
            <div className="rn-actions">
              {q.options.map((o) => (
                <button key={o} type="button" onClick={() => send(q.id, o)}>{o}</button>
              ))}
            </div>
          )}
          <div className="rn-answer">
            <input type="text" aria-label={`Answer to ${q.id}`} value={text[q.id] ?? ""} onChange={(e) => setText({ ...text, [q.id]: e.target.value })} onKeyDown={(e) => e.key === "Enter" && send(q.id, text[q.id] ?? "")} />
            <button type="button" onClick={() => send(q.id, text[q.id] ?? "")}>Answer</button>
          </div>
        </div>
      ))}
    </section>
  );
}

export default function PlanTab({ run, onChanged, onOpenWorker, onOpenChanges, onError }: Props) {
  return (
    <div className="rn-plan">
      {run.state === "awaiting_approval" && <Approval key={run.id + run.tasks.length} run={run} onError={onError} onChanged={onChanged} />}
      <Questions run={run} onError={onError} onChanged={onChanged} />
      {run.tasks.length === 0 ? (
        <p className="rn-none">No plan yet.</p>
      ) : (
        <table className="rn-table">
          <thead>
            <tr><th>Task</th><th>Status</th><th>Round</th><th>Worker</th><th>Ticket</th><th><span className="rn-sr">Actions</span></th></tr>
          </thead>
          <tbody>
            {run.tasks.map((t) => {
              const worker = workerOfTask(t);
              return (
                <tr key={t.id} data-task={t.id}>
                  <td><span className="rn-id">{t.id}</span> {t.title}</td>
                  <td><StatusDot dot={taskDot(t.state)} label="" /> {taskWord(t.state)}{t.error ? <div className="rn-none">{t.error}</div> : null}</td>
                  <td>{t.round > 0 ? t.round : ""}</td>
                  <td>{worker ? <button type="button" className="rn-link" onClick={() => onOpenWorker(worker, `${t.id}`)}>Open worker</button> : ""}</td>
                  <td>{t.kaTicket ?? ""}{t.ticket ? ` ${t.ticket}` : ""}</td>
                  <td>{t.changedFiles.length > 0 || t.files.length > 0 ? <button type="button" className="rn-link" onClick={() => onOpenChanges(t.id)}>Changes</button> : null}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
      {run.tasks.filter((t) => t.state === "needs_owner").map((t) => (
        <Decision key={t.id} run={run} task={t} onError={onError} onChanged={onChanged} />
      ))}
    </div>
  );
}

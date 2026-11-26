// The history of a task in its row (automations.md section 6): the last 10 runs, each
// with its time, status, duration, result and a link to its session; Cancel while a run
// is going.
import { openSession, setView } from "../../store";
import { moment, type Task, type TaskRun } from "./api";

const HOW_MANY = 10;

export default function TaskHistory({ task, onCancel }: { task: Task; onCancel: () => void }) {
  // Newest first.
  const runs: TaskRun[] = task.runs.slice(-HOW_MANY).reverse();
  // A run that is going does not count yet.
  const total = (task.runsBefore ?? 0) + task.runs.filter((run) => run.status !== "running").length;
  const running = task.lastStatus === "running";
  const finished = runs.filter((run) => run.status !== "running").length;
  return (
    <div className="task-history" aria-label="Run history">
      <div className="task-history-head">
        <span>
          Runs: {total} so far{total > finished ? ` (the last ${finished} shown)` : ""}
          {task.lastRun ? `, last ${moment(task.lastRun)}` : ""}
        </span>
        {running && <button onClick={onCancel}>Cancel run</button>}
      </div>
      {runs.length === 0 ? (
        <div className="hint">No run yet.</div>
      ) : (
        runs.map((run, i) => (
          <div className="task-run" key={`${run.at}-${i}`}>
            <span className="task-run-time">{moment(run.at)}</span>
            <span className={`run-pill ${run.status}`}>{run.status}</span>
            <span className="task-run-result">
              {run.result ?? ""}
              {run.durationSec !== undefined && <span className="hint"> ({run.durationSec} s)</span>}
            </span>
            {run.sessionId && (
              <button
                onClick={() => {
                  void openSession(run.sessionId!);
                  setView("chat");
                }}
              >
                Open session
              </button>
            )}
          </div>
        ))
      )}
    </div>
  );
}

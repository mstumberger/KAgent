// The Plan tab (workflows.md section 8): the tasks as a table, the approval card while the plan waits for the owner, the decision
// about a task that needs him, and the questions of the workers.
import type { Run } from "./api";
import { StatusDot } from "./Tree";
import { taskDot, taskWord, workerOfTask } from "./model";

interface Props {
  run: Run;
  onOpenWorker: (session: string, title: string) => void;
  onOpenChanges: (task: string) => void;
}

export default function PlanTab({ run, onOpenWorker, onOpenChanges }: Props) {
  return (
    <div className="rn-plan">
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
    </div>
  );
}

// New run (workflows.md section 8): pick a workflow, a project, type the goal. The workers and models of the roles show, and a
// model can be changed for this run.
import { useEffect, useState } from "react";
import { api } from "../../api";
import { useStore } from "../../store";
import { errorMessage, runsApi, type NewRun, type Run, type WorkflowInfo } from "./api";
import Select from "../../settings/Select";

interface Props {
  projects: string[];
  onStarted: (run: Run) => void;
  onCancel: () => void;
}

interface Known {
  path: string;
  name: string;
}

export default function NewRunForm({ projects, onStarted, onCancel }: Props) {
  const s = useStore();
  const [workflows, setWorkflows] = useState<WorkflowInfo[]>([]);
  const [known, setKnown] = useState<Known[]>([]);
  const [workflow, setWorkflow] = useState("");
  const [project, setProject] = useState("");
  const [goal, setGoal] = useState("");
  const [ticket, setTicket] = useState("");
  const [models, setModels] = useState<Record<string, string>>({});
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let live = true;
    runsApi.workflows().then(
      (r) => {
        if (!live) return;
        setWorkflows(r.workflows);
        setWorkflow((w) => w || r.workflows[0]?.name || "");
      },
      (e) => live && setError(errorMessage(e)),
    );
    api.get<{ projects: Known[] }>("/api/projects").then(
      (r) => live && setKnown(r.projects ?? []),
      () => {
        // a daemon without the route: the projects of the runs only
      },
    );
    return () => {
      live = false;
    };
  }, []);
  const open = s.activeSession ? s.metas[s.activeSession]?.project ?? "" : "";
  const options = [...new Set([...(open ? [open] : []), ...known.map((k) => k.path), ...projects])];
  useEffect(() => {
    setProject((p) => p || options[0] || "");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [known.length, projects.length, open]);
  const info = workflows.find((w) => w.name === workflow);
  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    if (busy || !workflow || !project || !goal.trim()) return;
    setBusy(true);
    const body: NewRun = { workflow, project, goal: goal.trim(), ...(ticket.trim() ? { ticket: ticket.trim() } : {}) };
    const changed = Object.fromEntries(Object.entries(models).filter(([, v]) => v.trim() !== ""));
    if (Object.keys(changed).length > 0) body.models = changed;
    runsApi.start(body).then(
      (run) => onStarted(run),
      (err) => setError(errorMessage(err)),
    ).finally(() => setBusy(false));
  };
  return (
    <form className="rn-new" onSubmit={submit} aria-label="New run" onKeyDown={(e) => e.key === "Escape" && onCancel()}>
      <h2>New run</h2>
      <label>
        <span>Workflow</span>
        <Select value={workflow} onChange={(e) => setWorkflow(e.target.value)}>
          {workflows.map((w) => (
            <option key={w.name} value={w.name}>{w.name}</option>
          ))}
        </Select>
      </label>
      {info && <p className="rn-none">{info.description}</p>}
      {info && (
        <ul className="rn-roles" aria-label="Workers of this workflow">
          {info.steps.filter((st) => st.worker).map((st) => (
            <li key={st.id}>
              <span className="rn-step-role">{st.role}</span>
              <span>{st.worker?.profile}</span>
              <input type="text" aria-label={`Model of the ${st.role}`} placeholder={st.model ?? "the profile's model, else the chat model"} value={models[st.role] ?? ""} onChange={(e) => setModels({ ...models, [st.role]: e.target.value })} />
            </li>
          ))}
        </ul>
      )}
      <label>
        <span>Project</span>
        <Select value={project} onChange={(e) => setProject(e.target.value)}>
          {options.length === 0 && <option value="">No project known</option>}
          {options.map((p) => (
            <option key={p} value={p}>{p.split("/").filter(Boolean).pop() ?? p} ({p})</option>
          ))}
        </Select>
      </label>
      <label>
        <span>Goal</span>
        <textarea rows={5} value={goal} onChange={(e) => setGoal(e.target.value)} placeholder="The whole job, as you would say it" />
      </label>
      <label>
        <span>Ticket (optional)</span>
        <input type="text" value={ticket} onChange={(e) => setTicket(e.target.value)} placeholder="A key like KA-12 to hang the tasks under" />
      </label>
      {error && <div className="rn-error" role="alert">{error}</div>}
      <div className="rn-actions">
        <button type="submit" className="primary" disabled={busy || !goal.trim() || !project || !workflow}>Start run</button>
        <button type="button" onClick={onCancel}>Cancel</button>
      </div>
    </form>
  );
}

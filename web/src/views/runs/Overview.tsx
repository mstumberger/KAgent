// The Overview tab (workflows.md section 8): the goal, the plan's summary, the steps of the workflow as a strip with the active
// one marked, the run's report when it ended, and what the owner can do to the run.
import { useState } from "react";
import type { Run } from "./api";
import { runsApi, errorMessage } from "./api";
import { activeStep, runWord } from "./model";

export default function Overview({ run, onError, onChanged }: { run: Run; onError: (m: string) => void; onChanged: (r: Run) => void }) {
  const [busy, setBusy] = useState(false);
  const active = activeStep(run);
  const act = (f: () => Promise<Run>) => {
    if (busy) return;
    setBusy(true);
    f().then(
      (r) => {
        onChanged(r);
        onError("");
      },
      (e) => onError(errorMessage(e)),
    ).finally(() => setBusy(false));
  };
  const live = run.state === "planning" || run.state === "working";
  return (
    <div className="rn-overview">
      <div className="rn-actions">
        {live && <button type="button" onClick={() => act(() => runsApi.pause(run.id))}>Pause</button>}
        {run.state === "paused" && <button type="button" className="primary" onClick={() => act(() => runsApi.resume(run.id))}>Continue</button>}
        {!(run.state === "done" || run.state === "failed" || run.state === "stopped") && (
          <button type="button" className="danger" onClick={() => act(() => runsApi.stop(run.id))}>Stop run</button>
        )}
      </div>
      <p className="rn-state">
        <strong>{runWord(run.state)}</strong>
        {run.reason && <span> ({run.reason})</span>}
      </p>
      <h3>Goal</h3>
      <p className="rn-goal">{run.goal}</p>
      {run.planSummary && (
        <>
          <h3>Plan</h3>
          <p>{run.planSummary}</p>
        </>
      )}
      <h3>Steps</h3>
      <ol className="rn-steps" aria-label="Steps of the workflow">
        {run.definition.steps.map((s) => (
          <li key={s.id} data-active={active === s.id} aria-current={active === s.id ? "step" : undefined}>
            <span className="rn-step-role">{s.role}</span>
            <span className="rn-step-worker">{s.worker ? s.worker.profile : "the engine"}{s.model ? `, ${s.model}` : ""}</span>
            {s.gate && <span className="rn-step-gate">you approve</span>}
          </li>
        ))}
      </ol>
      {run.report && (
        <>
          <h3>Report</h3>
          <pre className="rn-report">{run.report}</pre>
        </>
      )}
    </div>
  );
}

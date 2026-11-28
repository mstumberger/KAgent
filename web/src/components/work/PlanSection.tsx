// The Plan section (work-windows.md section 5.2): whether the session plans, with Cancel plan, and
// the approved plan or the one that waits for an answer, each with a way to open the Plan tab.
import { useState } from "react";
import { setPlan, useStore } from "../../store";
import { firstLine } from "./format";
import { openTab, planTab } from "./state";
import WorkSection from "./WorkSection";

export default function PlanSection({ sessionId, pane }: { sessionId: string; pane: boolean }) {
  const s = useStore();
  const [busy, setBusy] = useState(false);
  const planning = s.planning[sessionId] ?? false;
  const approved = s.metas[sessionId]?.approvedPlan ?? null;
  const waiting = s.approvals.some((a) => a.kind === "plan" && a.sessionId === sessionId);
  const cancel = async () => {
    setBusy(true);
    try {
      await setPlan(sessionId, false);
    } catch {
      // The store shows why, as a notice.
    } finally {
      setBusy(false);
    }
  };
  const open = () => openTab(planTab(sessionId));
  return (
    <WorkSection id="plan" title="Plan" count={planning ? "planning" : waiting ? "waits" : approved ? "approved" : undefined} hasSomething={planning || waiting || !!approved}>
      <div className="work-line plan-state">
        <span className="work-line-main static">
          <span className="work-line-title">Planning</span>
          <span className={`work-state ${planning ? "tone-ice" : "tone-dim"}`}>{planning ? "on" : "off"}</span>
        </span>
        {planning && (
          <button type="button" className="work-stop" disabled={busy} onClick={() => void cancel()}>
            Cancel plan
          </button>
        )}
      </div>
      {waiting && (
        <div className="work-line plan-waits">
          <span className="work-line-main static">
            <span className="work-line-title">A plan waits for your approval</span>
          </span>
          {pane && (
            <button type="button" className="work-stop" onClick={open}>
              Show
            </button>
          )}
        </div>
      )}
      {!waiting && approved && (
        <div className="work-line plan-approved">
          <span className="work-line-main static">
            <span className="work-line-title" title={approved.text}>
              {firstLine(approved.text) || "Approved plan"}
            </span>
          </span>
          {pane && (
            <button type="button" className="work-stop" onClick={open}>
              Open
            </button>
          )}
        </div>
      )}
      {!waiting && !approved && <div className="work-empty">No plan yet</div>}
    </WorkSection>
  );
}

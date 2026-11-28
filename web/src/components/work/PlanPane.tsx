// The Plan tab (work-windows.md section 5.4): the approved plan as Markdown, else the plan that
// waits for an answer, else a word.
import { useStore } from "../../store";
import Markdown from "../Markdown";
import type { SideTab } from "./state";

export default function PlanPane({ tab }: { tab: SideTab }) {
  const s = useStore();
  const approved = s.metas[tab.sessionId]?.approvedPlan;
  const waiting = s.approvals.find((a) => a.kind === "plan" && a.sessionId === tab.sessionId);
  return (
    <div className="pane-plan">
      <div className="pane-scroll">
        {waiting ? (
          <>
            <div className="work-note">Approve or reject it in the conversation</div>
            <Markdown text={waiting.preview ?? ""} />
          </>
        ) : approved ? (
          <Markdown text={approved.text} />
        ) : (
          <div className="work-empty">No plan yet</div>
        )}
      </div>
    </div>
  );
}

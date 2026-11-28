// Plan mode (plan-todo.md section 2): the card of a plan the agent wants approved, in the
// approvals area of Chat, Voice and the overlay. (The state of plan mode and Cancel plan are in the work panel.)
import { useState } from "react";
import "./plan.css";
import type { ApprovalRequest } from "../api";
import Markdown from "./Markdown";
import { decideApproval } from "../store";

/** Title "Plan", the plan as Markdown, Approve, and Reject with a reason field (sent as `reason`). */
export default function PlanCard({ request }: { request: ApprovalRequest }) {
  // Why the plan is rejected: sent as `reason`, and left out when empty.
  const [reason, setReason] = useState("");
  const reject = () => void decideApproval(request.requestId, "deny", reason.trim() || undefined);
  return (
    <section className="approval-card plan-card" aria-label="Plan">
      <div className="approval-title">Plan</div>
      <div className="plan-body">
        <Markdown text={request.preview ?? ""} />
      </div>
      <div className="approval-actions">
        <button className="primary" onClick={() => void decideApproval(request.requestId, "allowOnce")}>
          Approve
        </button>
        <input
          className="plan-reason"
          aria-label="Reason for rejecting the plan"
          placeholder="Why not (optional)"
          value={reason}
          onChange={(e) => setReason(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              reject();
            }
          }}
        />
        <button className="danger" onClick={reject}>
          Reject
        </button>
      </div>
    </section>
  );
}

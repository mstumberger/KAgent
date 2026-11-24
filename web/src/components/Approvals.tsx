// Approval cards: summary, risk, why it asks, the dry-run preview (apt simulation,
// old -> new value, diff) and the decision buttons. Shared by Chat and Voice.
import { decideApproval, useStore } from "../store";

export default function Approvals() {
  const s = useStore();
  if (s.approvals.length === 0) return null;
  return (
    <div className="approvals">
      {s.approvals.map((a) => (
        <div key={a.requestId} className={`approval-card ${a.privileged ? "privileged" : ""}`}>
          <div className="approval-title">
            {a.summary} <span className={`risk risk-${a.risk}`}>{a.risk}</span>
            {a.privileged && <span className="risk risk-critical">root</span>}
          </div>
          <div className="approval-reason">{a.reason}</div>
          {a.preview && <pre className="approval-preview">{a.preview}</pre>}
          <div className="approval-actions">
            <button className="primary" onClick={() => void decideApproval(a.requestId, "allowOnce")}>
              Allow once
            </button>
            {a.canRemember && (
              <>
                <button onClick={() => void decideApproval(a.requestId, "allowSession")}>This session</button>
                <button onClick={() => void decideApproval(a.requestId, "allowAlways")}>Always</button>
              </>
            )}
            <button className="danger" onClick={() => void decideApproval(a.requestId, "deny")}>
              Deny
            </button>
          </div>
        </div>
      ))}
    </div>
  );
}

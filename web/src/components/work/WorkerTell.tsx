// The message box under a worker's transcript (steering.md section 6): Send corrects it (it reads the message at its next step
// and keeps going), Interrupt cancels what it is doing now and resumes with the message first. The owner's messages stay
// below with their receipts. Never an approval: the cards of the worker are answered in the approvals area.
import { useState } from "react";
import { agentsApi, type ChildView } from "../../settings/subagents/api";

const errorMessage = (e: unknown) => (e instanceof Error ? e.message : String(e));
const RECEIPT = { queued: "queued", delivered: "delivered" } as const;

/** Why the box is off, or "" when the worker can read a message. */
function reason(child: ChildView | undefined): string {
  if (!child) return "";
  if (child.status === "queued") return "The worker has not started: it is waiting for a free slot.";
  if (child.status !== "running") return `The worker ended (${child.status}): it cannot read a message.`;
  return "";
}

export default function WorkerTell({ child, title }: { child: ChildView | undefined; title: string }) {
  const [text, setText] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const off = reason(child);
  const steering = child?.steering ?? [];
  const send = async (interrupt: boolean) => {
    const t = text.trim();
    if (!t || !child || busy) return;
    setBusy(true);
    try {
      await agentsApi.tell(child.id, t, interrupt);
      setText("");
      setError("");
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  const ended = child !== undefined && child.status !== "running" && child.status !== "queued";
  return (
    <div className="worker-tell-box">
      {steering.length > 0 && (
        <ul className="worker-receipts" aria-label="Your messages to the worker">
          {steering.map((s) => (
            <li key={s.id} data-state={s.state} data-interrupt={s.interrupt}>
              <span className="worker-receipt-text" title={s.text}>{s.text}</span>
              {s.interrupt && <span className="worker-receipt-tag">interrupt</span>}
              <span className={`worker-receipt state-${s.state}`}>
                {s.state === "queued" && ended ? "ended before it read this" : RECEIPT[s.state]}
              </span>
            </li>
          ))}
        </ul>
      )}
      <form
        className="worker-tell"
        onSubmit={(e) => {
          e.preventDefault();
          void send(false);
        }}
      >
        <textarea
          rows={2}
          aria-label={`Tell ${title}`}
          placeholder={off || `Tell ${title}...`}
          disabled={off !== ""}
          title={off || undefined}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              void send(false);
            }
          }}
        />
        <div className="worker-tell-buttons">
          <button type="submit" className="primary" disabled={off !== "" || !text.trim() || busy} title="It reads this at its next step and keeps going">
            Send
          </button>
          <button type="button" className="danger" disabled={off !== "" || !text.trim() || busy} title="Cancel what it is doing now and resume with this first" onClick={() => void send(true)}>
            Interrupt
          </button>
        </div>
      </form>
      {error && <div className="tk-error worker-tell-error" role="alert">{error}</div>}
    </div>
  );
}

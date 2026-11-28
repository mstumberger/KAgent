// An Agents card in the transcript (agents.md section 6): one subagent of the open session.
// Its profile, state and time; a preview of its answer; Stop while it runs or waits; a click
// opens the subagent in the side pane, read-only.
import { useEffect, useState } from "react";
import type { Message } from "../api";
import Markdown from "./Markdown";
import { stopSubagent } from "../store";
import { openTab, subagentTab } from "./work/state";
import { PREVIEW_CHARS, elapsedOf, reportsOf, type AgentCard } from "./agents";
import "./agents.css";

const LABEL = { queued: "queued", running: "running", done: "done", failed: "failed", cancelled: "cancelled" } as const;

export default function AgentsCard({ child, parentId }: { child: AgentCard; parentId: string }) {
  // The clock of a running card: it counts while the subagent works and stops when it ended.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (child.status !== "running" || !child.startedAt) return;
    const t = setInterval(() => {
      setNow(Date.now());
    }, 1000);
    return () => {
      clearInterval(t);
    };
  }, [child.status, child.startedAt]);

  const live = child.status === "running" || child.status === "queued";
  return (
    <section className="agent-card" data-status={child.status} data-child={child.id} aria-label={`Subagent ${child.title}`}>
      <div className="agent-line">
        <button type="button" className="agent-open" title="Open the subagent in the side pane (read-only)" onClick={() => openTab(subagentTab(parentId, child))}>
          <span className="agent-dot" aria-hidden="true" />
          <span className="agent-title">{child.title}</span>
          <span className="agent-profile">{child.profile}</span>
          <span className={`agent-status status-${child.status}`}>{LABEL[child.status]}</span>
          <span className="agent-time">{child.status === "queued" ? "waiting for a free slot" : elapsedOf(child, now)}</span>
        </button>
        {live && (
          <button type="button" className="danger agent-stop" aria-label={`Stop ${child.title}`} onClick={() => void stopSubagent(parentId, child.id)}>
            Stop
          </button>
        )}
      </div>
      {child.output.trim() !== "" && <pre className="agent-output">{[...child.output.trim()].slice(0, PREVIEW_CHARS).join("")}</pre>}
    </section>
  );
}

/** What a subagent or a background task reported to the session (agents.md sections 3 and 4): a note at the left, each report as a line and its output behind a toggle. The card above the note shows a preview; the whole output is in the toggle. Another message sent `Via::Agent` (the job a subagent was given) shows as it is. */
export function AgentNote({ m }: { m: Message }) {
  const reports = reportsOf(m.content);
  return (
    <div className="bubble assistant agent-note">
      <span className="via">agent</span>
      <div className="content">
        {reports.length === 0 ? (
          <Markdown text={m.content} />
        ) : (
          reports.map((report, i) => (
            <div className="agent-report" key={i}>
              <Markdown text={report.head} />
              {report.output !== "" && (
                <details className="agent-report-output">
                  <summary>output ({[...report.output].length.toLocaleString("en")} characters)</summary>
                  <pre>{report.output}</pre>
                </details>
              )}
            </div>
          ))
        )}
      </div>
    </div>
  );
}

// The Agents section (work-windows.md section 5.2): one line per child of the open session, in the
// order of the cards of the transcript; a click opens its Subagent tab (the widget has no pane).
import { useEffect, useMemo, useState } from "react";
import { stopSubagent, useStore } from "../../store";
import { agentCards, elapsedOf } from "../agents";
import { openTab, subagentTab } from "./state";
import WorkSection from "./WorkSection";

const NO_MESSAGES: never[] = [];
const NO_CHILDREN: never[] = [];

export default function AgentsSection({ sessionId, pane }: { sessionId: string; pane: boolean }) {
  const s = useStore();
  const messages = s.messages[sessionId] ?? NO_MESSAGES;
  const events = s.children[sessionId] ?? NO_CHILDREN;
  const cards = useMemo(() => agentCards(events, messages, s.running), [events, messages, s.running]);
  const running = cards.filter((c) => c.status === "running").length;
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (running === 0) return;
    const t = setInterval(() => {
      setNow(Date.now());
    }, 1000);
    return () => {
      clearInterval(t);
    };
  }, [running]);
  return (
    <WorkSection id="agents" title="Agents" count={cards.length === 0 ? undefined : running > 0 ? `${running} of ${cards.length} running` : String(cards.length)} hasSomething={cards.length > 0}>
      {cards.length === 0 && <div className="work-empty">No agents yet</div>}
      {cards.map((c) => {
        const live = c.status === "running" || c.status === "queued";
        return (
          <div className="work-line agent-row" key={c.id} data-status={c.status} data-child={c.id}>
            <button type="button" className="work-line-main" disabled={!pane} title={pane ? "Open the subagent in the side pane" : undefined} onClick={() => openTab(subagentTab(sessionId, c))}>
              <span className="agent-dot" aria-hidden="true" />
              <span className="work-line-title">{c.title}</span>
              <span className="work-line-note">{c.profile}</span>
              <span className={`agent-status status-${c.status}`}>{c.status}</span>
              <span className="work-line-note work-time">{c.status === "queued" ? "waiting" : elapsedOf(c, now)}</span>
            </button>
            {live && (
              <button type="button" className="danger work-stop" aria-label={`Stop ${c.title}`} onClick={() => void stopSubagent(sessionId, c.id)}>
                Stop
              </button>
            )}
          </div>
        );
      })}
    </WorkSection>
  );
}

// The Subagent tab (work-windows.md section 5.4): the session of a child, read-only. Its line
// (title, profile, state, time, Stop), then its messages as the transcript draws them. The pane
// reads, it never sends.
import { useEffect, useMemo, useState } from "react";
import type { Message } from "../../api";
import { stopSubagent, useStore } from "../../store";
import { agentCards, elapsedOf } from "../agents";
import Icon from "../Icon";
import { Bubble } from "../transcript";
import { readSession, type SideTab } from "./state";
import WorkerInfo from "./WorkerInfo";
import WorkerTell from "./WorkerTell";
import { useFollow } from "./useFollow";

const NO_MESSAGES: Message[] = [];
const NO_CHILDREN: never[] = [];

export default function SubagentPane({ tab }: { tab: SideTab }) {
  const s = useStore();
  const messages = s.messages[tab.sessionId] ?? NO_MESSAGES;
  const streaming = s.streaming[tab.sessionId];
  const running = s.running.has(tab.sessionId);
  const owner = s.messages[tab.owner] ?? NO_MESSAGES;
  const events = s.children[tab.owner] ?? NO_CHILDREN;
  const card = useMemo(() => agentCards(events, owner, s.running).find((c) => c.id === tab.sessionId), [events, owner, s.running, tab.sessionId]);
  // The first read: the child's messages, once; the events fill them after that.
  useEffect(() => {
    if (!(tab.sessionId in s.messages)) void readSession(tab.sessionId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.sessionId]);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (card?.status !== "running") return;
    const t = setInterval(() => {
      setNow(Date.now());
    }, 1000);
    return () => {
      clearInterval(t);
    };
  }, [card?.status]);
  const { ref, following, onScroll, resume } = useFollow<HTMLDivElement>(`${messages.length}|${streaming ?? ""}`);
  const live = card?.status === "running" || card?.status === "queued";
  return (
    <div className="pane-subagent">
      {card && (
        <div className="pane-line" data-status={card.status}>
          <span className="agent-dot" aria-hidden="true" />
          <span className="work-line-title">{card.title}</span>
          <span className="work-line-note">{card.profile}</span>
          <span className={`agent-status status-${card.status}`}>{card.status}</span>
          <span className="work-line-note work-time">{elapsedOf(card, now)}</span>
          {live && (
            <button type="button" className="danger work-stop" aria-label={`Stop ${card.title}`} onClick={() => void stopSubagent(tab.owner, card.id)}>
              Stop
            </button>
          )}
        </div>
      )}
      <WorkerInfo owner={tab.owner} childId={tab.sessionId} child={card} />
      <div className="pane-scroll messages" ref={ref} onScroll={onScroll} data-following={following}>
        {messages.length === 0 && <div className="work-empty">No messages yet</div>}
        {messages.map((m, i) => (
          <Bubble key={i} m={m} streaming={i === messages.length - 1 && running ? streaming : undefined} />
        ))}
        {running && streaming && messages[messages.length - 1]?.role === "user" && <Bubble m={{ role: "assistant", content: "" }} streaming={streaming} />}
      </div>
      <WorkerTell child={card} title={card?.title ?? tab.title} />
      {!following && (
        <button type="button" className="pane-jump" aria-label="Jump to the end" title="Jump to the end" onClick={resume}>
          <Icon name="arrowDown" size={14} />
        </button>
      )}
    </div>
  );
}

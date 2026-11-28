// The messages of a conversation as the transcript draws them: a bubble per message, a card per
// tool result. Chat and the side pane of a subagent (components/work/SubagentPane.tsx) both draw them.
import { useState } from "react";
import type { Message } from "../api";
import Markdown from "./Markdown";
import { AgentNote } from "./AgentsCard";
import MessageImages from "./MessageImages";
import { TOOL_RESULT_LABEL, toolResultKind } from "./toolResult";

export function ToolCard({ m }: { m: Message }) {
  const [open, setOpen] = useState(false);
  const kind = toolResultKind(m.content, m.isError);
  const isError = kind === "error";
  return (
    <div className={`tool-card ${isError ? "tool-error" : ""} tool-${kind} ${open ? "open" : ""}`}>
      <button className="tool-head" onClick={() => setOpen(!open)} aria-expanded={open}>
        <span className={`tool-mark ${kind === "ok" ? "ok" : kind === "error" ? "bad" : kind === "shown" ? "info" : "warn"}`} aria-hidden="true" />
        <span className="tool-name">{m.toolName}</span>
        <span className="tool-state">{TOOL_RESULT_LABEL[kind]}</span>
        <span className="tool-toggle">{open ? "hide output" : "output"}</span>
      </button>
      {open && <pre className="tool-body">{m.content.slice(0, 4000)}</pre>}
    </div>
  );
}

/** The first line of a stored note: the marker the daemon puts before the notes. */
const NOTE_HEADER = /^\[note from the daemon[^\]]*\]\n?/;

function DaemonNote({ m }: { m: Message }) {
  const lines = m.content.replace(NOTE_HEADER, "").split("\n").map((l) => l.replace(/^- /, "")).filter(Boolean);
  return (
    <div className="bubble note" role="note">
      <span className="via">daemon note</span>
      {lines.map((l, i) => (
        <div key={i} className="content">{l}</div>
      ))}
    </div>
  );
}

export function Bubble({ m, streaming, thinking, sessionId }: { m: Message; streaming?: string; thinking?: string; sessionId?: string | null }) {
  if (m.role === "tool") return <ToolCard m={m} />;
  const isUser = m.role === "user";
  const via = (m as { via?: string }).via;
  // What a subagent or a background task reported to the session (agents.md sections 3 and 4):
  // stored as a message of the user, shown as a report.
  // A report of a subagent or of a background task has a note of its own.
  if (isUser && via === "agent") return <AgentNote m={m} />;
  // A note of the daemon into a running turn (agent-loop.md C.1): its own dimmed line, never the owner's bubble.
  if (isUser && via === "note") return <DaemonNote m={m} />;
  const text = (streaming !== undefined ? streaming : m.content) || "";
  // A step that only calls tools has no words: its results show as tool chips.
  if (!isUser && !text && !thinking && m.toolCalls && m.toolCalls.length > 0) return null;
  const images = isUser ? (m.images ?? []) : [];
  return (
    <div className={`bubble ${isUser ? "user" : "assistant"}`}>
      {via === "voice" && <span className="via">voice</span>}
      {via === "telegram" && <span className="via">telegram</span>}
      {thinking && (
        <details className="thinking">
          <summary>thinking</summary>
          <pre>{thinking}</pre>
        </details>
      )}
      {images.length > 0 && <MessageImages sessionId={sessionId ?? null} images={images} />}
      {(text || images.length === 0) && <div className="content">{isUser ? text : text ? <Markdown text={text} /> : <span className="typing" aria-label="working" />}</div>}
    </div>
  );
}

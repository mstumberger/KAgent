// The messages of a conversation as the transcript draws them: a bubble per message, a card per
// tool result. Chat and the side pane of a subagent (components/work/SubagentPane.tsx) both draw them.
import { useState } from "react";
import type { Message } from "../api";
import Markdown from "./Markdown";
import { AgentNote } from "./AgentsCard";

export function ToolCard({ m }: { m: Message }) {
  const [open, setOpen] = useState(false);
  const isError = m.isError;
  return (
    <div className={`tool-card ${isError ? "tool-error" : ""} ${open ? "open" : ""}`}>
      <button className="tool-head" onClick={() => setOpen(!open)} aria-expanded={open}>
        <span className={`tool-mark ${isError ? "bad" : "ok"}`} aria-hidden="true" />
        <span className="tool-name">{m.toolName}</span>
        <span className="tool-state">{isError ? "failed" : "done"}</span>
        <span className="tool-toggle">{open ? "hide output" : "output"}</span>
      </button>
      {open && <pre className="tool-body">{m.content.slice(0, 4000)}</pre>}
    </div>
  );
}

export function Bubble({ m, streaming, thinking }: { m: Message; streaming?: string; thinking?: string }) {
  if (m.role === "tool") return <ToolCard m={m} />;
  const isUser = m.role === "user";
  const via = (m as { via?: string }).via;
  // What a subagent or a background task reported to the session (agents.md sections 3 and 4):
  // stored as a message of the user, shown as a report.
  // A report of a subagent or of a background task has a note of its own.
  if (isUser && via === "agent") return <AgentNote m={m} />;
  const text = (streaming !== undefined ? streaming : m.content) || "";
  // A step that only calls tools has no words: its results show as tool chips.
  if (!isUser && !text && !thinking && m.toolCalls && m.toolCalls.length > 0) return null;
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
      <div className="content">{isUser ? text : text ? <Markdown text={text} /> : <span className="typing" aria-label="working" />}</div>
    </div>
  );
}

import Approvals from "../components/Approvals";
import Markdown from "../components/Markdown";
import { useEffect, useRef, useState } from "react";
import type { Message } from "../api";
import { cancelSession, deleteSession, getStore, newSession, openSession, sendMessage, setView, useStore } from "../store";
import Icon from "../components/Icon";
import { useVoice } from "../voice/useVoice";
import { voice } from "../voice/controller";

/** Talk in this chat: opens it in voice mode and starts (or moves) voice into it. */
function talkHere(sessionId: string | null) {
  const liveVoice = (getStore().state?.settings.liveVoice as string) || undefined;
  if (voice.active && voice.sessionId === sessionId) {
    voice.stop();
    return;
  }
  setView("voice");
  if (voice.active) {
    voice.stop();
    setTimeout(() => voice.start(sessionId, liveVoice), 600);
    return;
  }
  voice.start(sessionId, liveVoice);
}

function ToolCard({ m }: { m: Message }) {
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

function Bubble({ m, streaming, thinking }: { m: Message; streaming?: string; thinking?: string }) {
  if (m.role === "tool") return <ToolCard m={m} />;
  const isUser = m.role === "user";
  const text = (streaming !== undefined ? streaming : m.content) || "";
  // A step that only calls tools has no words: its results show as tool chips.
  if (!isUser && !text && !thinking && m.toolCalls && m.toolCalls.length > 0) return null;
  return (
    <div className={`bubble ${isUser ? "user" : "assistant"}`}>
      {(m as { via?: string }).via === "voice" && <span className="via">voice</span>}
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

export default function Chat() {
  const s = useStore();
  const [text, setText] = useState("");
  const endRef = useRef<HTMLDivElement>(null);
  const messages = s.activeSession ? (s.messages[s.activeSession] ?? []) : [];
  const streaming = s.activeSession ? s.streaming[s.activeSession] : undefined;
  const running = s.activeSession ? s.running.has(s.activeSession) : false;
  const v = useVoice();
  const talking = v.active && v.sessionId === s.activeSession;
  const pendingText = v.pending.map((l) => l.text).join("|").length;

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages.length, streaming, pendingText]);

  return (
    <div className="chat-layout">
      <aside className="sessions">
        <button className="primary" onClick={() => void newSession()}>
          + New session
        </button>
        {s.sessions.map((sess) => (
          <div
            key={sess.id}
            className={`session-item ${sess.id === s.activeSession ? "active" : ""}`}
            onClick={() => void openSession(sess.id)}
          >
            <span className="session-title">{sess.title || "New session"}</span>
            <button
              className="icon"
              title="Delete session"
              aria-label="Delete session"
              onClick={(e) => {
                e.stopPropagation();
                void deleteSession(sess.id);
              }}
            >
              ×
            </button>
          </div>
        ))}
      </aside>

      <section className="chat-main">
        <Approvals />

        <div className="messages">
          {!s.activeSession && <div className="empty">Create a session to start.</div>}
          {messages.map((m, i) => (
            <Bubble key={i} m={m} streaming={i === messages.length - 1 && running ? streaming : undefined} />
          ))}
          {running && streaming && messages[messages.length - 1]?.role === "user" && (
            <Bubble m={{ role: "assistant", content: "" }} streaming={streaming} thinking={s.activeSession ? s.reasoning[s.activeSession] : undefined} />
          )}
          {talking &&
            v.pending.map((l, i) => (
              <div key={`live-${i}`} className={`bubble ${l.who === "user" ? "user" : "assistant"} live`}>
                <span className="via">voice, live</span>
                <div className="content">{l.text}</div>
              </div>
            ))}
          <div ref={endRef} />
        </div>

        <form
          className="composer"
          onSubmit={(e) => {
            e.preventDefault();
            void sendMessage(text);
            setText("");
          }}
        >
          <button
            type="button"
            className={`mic-btn${talking ? " on" : ""}${talking && v.muted ? " muted" : ""}`}
            title={talking ? "Stop talking" : "Talk in this chat"}
            aria-label={talking ? "Stop talking" : "Talk in this chat"}
            disabled={!s.activeSession || !s.state?.settings.liveModel}
            onClick={() => talkHere(s.activeSession)}
          >
            <Icon name={talking ? "stop" : "mic"} size={16} />
          </button>
          <input
            value={text}
            placeholder={talking ? "Listening... you can also type" : s.state?.chatModelValid ? "Ask about this machine..." : "Pick a chat model in Providers first"}
            onChange={(e) => setText(e.target.value)}
          />
          {running ? (
            <button type="button" className="danger" onClick={() => void cancelSession()}>
              Stop
            </button>
          ) : (
            <button className="primary" disabled={!text.trim() || !s.state?.chatModelValid}>
              Send
            </button>
          )}
        </form>
      </section>
    </div>
  );
}

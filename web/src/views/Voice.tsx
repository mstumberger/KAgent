// Voice: talk to KAgent through the daemon's live model. Uses the active chat session
// so spoken and typed turns are one conversation. Approval cards appear here too.
import { useEffect, useRef, useState } from "react";
import Approvals from "../components/Approvals";
import Orb, { normalizeTheme, type OrbTheme } from "../voice/Orb";
import { newSession, openSession, setView, showView, useStore } from "../store";
import { voice } from "../voice/controller";
import { useVoice } from "../voice/useVoice";

export default function Voice() {
  const s = useStore();
  const v = useVoice();
  const status = v.status;
  const detail = v.detail;
  const lines = v.lines;
  const [theme, setTheme] = useState<OrbTheme>(() => normalizeTheme(localStorage.getItem("kagent.orb")));
  const [typed, setTyped] = useState("");
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    localStorage.setItem("kagent.orb", theme);
  }, [theme]);

  useEffect(() => {
    const onOrb = () => setTheme(normalizeTheme(localStorage.getItem("kagent.orb")));
    window.addEventListener("kagent-orb", onOrb);
    // The session keeps running when the tab changes (tray / overlay / ui tool).
    return () => window.removeEventListener("kagent-orb", onOrb);
  }, []);

  // Voice mode of a chat session: the open session's history, then what is said live.
  const sid = v.active ? (v.sessionId ?? s.activeSession) : s.activeSession;
  const history = (sid ? (s.messages[sid] ?? []) : []).filter((m) => (m.role === "user" || m.role === "assistant") && m.content.trim());
  const pendingHere = v.active && v.sessionId === sid ? v.pending : [];
  const voiceName = () => (s.state?.settings.liveVoice as string) || undefined;
  const pendingLen = v.pending.length;
  const historyLen = history.length;

  // Braces matter: newer Chrome returns a Promise from scrollIntoView, and React would
  // call a returned value as the effect's cleanup ("destroy is not a function").
  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [lines, sid, historyLen, pendingLen]);

  // Make sure the conversation's history is loaded when the tab opens.
  useEffect(() => {
    if (sid && !s.messages[sid]) void openSession(sid);
  }, [sid, s.messages]);

  // Pick another conversation: open it, and move a running voice session there.
  const switchTo = async (id: string) => {
    const target = id === "__new" ? await newSession() : id;
    if (!target) return;
    if (id !== "__new") await openSession(target);
    if (v.active && v.sessionId !== target) {
      v.stop();
      setTimeout(() => voice.start(target, voiceName()), 600);
    }
  };

  const toggle = () => v.toggle(sid, voiceName());

  const liveModel = (s.state?.settings.liveModel as string) || "";
  const active = status !== "idle" && status !== "error";

  return (
    <div className="voice">
      <div className="voice-stage">
        <Orb mic={v.micAnalyser} play={v.playAnalyser} state={v.muted && active ? "muted" : status} theme={theme} />
        <div className={`voice-status st-${status}`}>
          {v.said || (status === "live" && v.thinking ? "Thinking" : ({ idle: "Ready", connecting: "Connecting", live: v.muted ? "Mic muted" : "Listening", rolling: "Reconnecting", error: "Voice error" } as Record<string, string>)[status] ?? status)}
          {!v.said && detail && `: ${detail}`}
        </div>
        <button className={active ? "danger big" : "primary big"} onClick={toggle} disabled={!liveModel && !active}>
          {active ? "Stop talking" : "Talk"}
        </button>
        {active && (
          <div className="voice-controls">
            <button className={v.muted ? "danger" : ""} onClick={() => v.setMuted(!v.muted)}>
              {v.muted ? "Unmute mic" : "Mute mic"}
            </button>
            <label className="vol">
              Volume {Math.round(v.volume * 100)}%
              <input type="range" min={0} max={100} value={Math.round(v.volume * 100)} onChange={(e) => v.setVolume(Number(e.target.value) / 100)} />
            </label>
          </div>
        )}
        {!liveModel && <div className="hint">Pick a live model in Voice settings (Gemini *-live; add a Gemini provider first).</div>}
        <div className="voice-settings">
          <button className="link-btn" onClick={() => showView("settings", "voice")}>
            Voice settings
          </button>
        </div>
      </div>
      <div className="voice-side">
        <Approvals />
        <div className="voice-convo-head">
          <select
            aria-label="Conversation"
            value={sid ?? ""}
            onChange={(e) => void switchTo(e.target.value)}
          >
            {!sid && <option value="">New conversation</option>}
            {s.sessions.map((x) => (
              <option key={x.id} value={x.id}>
                {x.title || "New session"}
              </option>
            ))}
            <option value="__new">+ New conversation</option>
          </select>
          <button className="link-btn" disabled={!sid} onClick={() => setView("chat")}>
            Open in chat
          </button>
        </div>
        <div className="transcript">
          {history.length === 0 && pendingHere.length === 0 && <div className="empty">Press Talk and speak. Everything goes into this conversation.</div>}
          {history.map((m, i) => (
            <div key={`h-${i}`} className={`tline ${m.role === "user" ? "user" : "assistant"}`}>
              <span className="who">
                {m.role === "user" ? "you" : (s.state?.settings.persona.name ?? "KAgent")}
                {m.via === "voice" && <span className="via-dot" title="spoken" />}
              </span>
              {m.content}
            </div>
          ))}
          {pendingHere.map((l, i) => (
            <div key={`p-${i}`} className={`tline ${l.who} live`}>
              <span className="who">{l.who === "assistant" ? s.state?.settings.persona.name ?? "KAgent" : l.who === "user" ? "you" : l.who}</span>
              {l.text}
            </div>
          ))}
          <div ref={endRef} />
        </div>
        <form
          className="composer"
          onSubmit={(e) => {
            e.preventDefault();
            if (typed.trim()) v.sendText(typed);
            setTyped("");
          }}
        >
          <input value={typed} disabled={!active} placeholder="Type into the live session..." onChange={(e) => setTyped(e.target.value)} />
          <button className="primary" disabled={!active || !typed.trim()}>
            Send
          </button>
        </form>
      </div>
    </div>
  );
}

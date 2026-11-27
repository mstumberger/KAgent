// Orb widget (?overlay): the compact always-on-top window opened from the tray ("Orb
// widget") or `scripts/kagent-overlay.sh`. A dark card that scales with the window:
// orb, status, the last exchange, and mic / volume / stop controls. Click the orb or
// press Space to talk, M to mute, Esc to stop; approvals pop up over it.
// ?autostart starts listening immediately.
import { useEffect, useRef, useState } from "react";
import Approvals from "../components/Approvals";
import PermissionPicker from "../components/PermissionPicker";
import Icon from "../components/Icon";
import { ModelBody } from "../components/ModelMenu";
import { useChoice } from "../components/useChoice";
import Sheet from "../components/Sheet";
import { startDrag } from "../frame";
import Orb, { ORB_THEMES, normalizeTheme, type OrbTheme } from "../voice/Orb";
import { label as modelLabel } from "../picker";
import { closeSheet, getStore, openSheet, refreshModels, useStore } from "../store";
import { useVoice } from "../voice/useVoice";
import { voice } from "../voice/controller";

const ORBS = ORB_THEMES;

function liveVoice(): string | undefined {
  // Read at call time so a voice changed in Settings applies to the next session.
  return (getStore().state?.settings.liveVoice as string) || undefined;
}

/** Open the sheet of the model picker; the list is read again when it is old. */
function openModels() {
  void refreshModels();
  openSheet("models");
}

interface Props {
  /** Switch this window to the full layout (KAgent's own window only). */
  onExpand?: () => void;
  onHide?: () => void;
}

export default function Overlay({ onExpand, onHide }: Props) {
  const s = useStore();
  const v = useVoice();
  const [theme, setTheme] = useState<OrbTheme>(() => normalizeTheme(localStorage.getItem("kagent.orb")));
  const stage = useRef<HTMLDivElement>(null);
  const [orbSize, setOrbSize] = useState(180);
  const name = s.state?.settings.persona.name ?? "KAgent";
  const choice = useChoice(s.activeSession);

  // The orb fills the free space of the window, whatever size the user drags it to.
  useEffect(() => {
    const el = stage.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => {
      const { width, height } = e.contentRect;
      setOrbSize(Math.max(80, Math.floor(Math.min(width, height) * 0.92)));
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    document.documentElement.classList.add("overlay-mode");
    document.title = "KAgent";
    if (new URLSearchParams(location.search).has("autostart")) voice.start(getStore().activeSession, liveVoice());
    const key = (e: KeyboardEvent) => {
      // While a sheet is open the keys of the widget do nothing: the sheet has its own.
      if (getStore().sheet) return;
      // Ctrl+M opens the model sheet and never mutes (picker.md section 5.4).
      if ((e.ctrlKey || e.metaKey) && e.code === "KeyM") {
        e.preventDefault();
        if (getStore().activeSession) openModels();
        return;
      }
      if (e.target instanceof HTMLInputElement) return;
      if (e.code === "Space") {
        e.preventDefault();
        voice.toggle(getStore().activeSession, liveVoice());
      } else if (e.code === "KeyM") {
        voice.setMuted(!voice.muted);
      } else if (e.code === "Escape") {
        voice.stop();
      }
    };
    const onOrb = () => setTheme(normalizeTheme(localStorage.getItem("kagent.orb")));
    window.addEventListener("keydown", key);
    window.addEventListener("kagent-orb", onOrb);
    return () => {
      window.removeEventListener("keydown", key);
      window.removeEventListener("kagent-orb", onOrb);
      // No voice.stop(): switching to the full layout keeps the conversation going.
      document.documentElement.classList.remove("overlay-mode");
    };
  }, []);

  useEffect(() => {
    localStorage.setItem("kagent.orb", theme);
  }, [theme]);

  const active = v.status !== "idle" && v.status !== "error";
  const state = v.muted && active ? "muted" : v.status;
  const label = v.status === "idle" ? "Tap the orb to talk" : v.status === "error" ? v.detail || "Voice error" : v.muted && active ? "Mic muted" : v.status === "live" ? "Listening" : v.status === "connecting" ? "Connecting" : v.status;
  // The last thing each side said, newest last.
  const lastUser = [...v.lines].reverse().find((l) => l.who === "user");
  const lastAgent = [...v.lines].reverse().find((l) => l.who !== "user");
  const pct = Math.round(v.volume * 100);
  // The todo list of the open session (plan-todo.md section 1): a chip "3/7" while it has items.
  const todo = s.activeSession ? (s.todos[s.activeSession] ?? []) : [];
  const todoDone = todo.filter((item) => item.status === "completed").length;

  return (
    <div className={`widget st-${state}`}>
      {(onExpand || onHide) && (
        <div className="widget-top" onPointerDown={startDrag} title="Drag to move">
          <span className="widget-name">{name}</span>
          <PermissionPicker compact />
          {onHide && (
            <button className="wicon" title="Hide to the tray (keeps listening)" aria-label="Hide to tray" onClick={onHide}>
              <Icon name="tray" size={14} />
            </button>
          )}
          {onExpand && (
            <button className="wicon" title="Expand: chat, transcript, settings" aria-label="Expand" onClick={onExpand}>
              <Icon name="expand" size={14} />
            </button>
          )}
        </div>
      )}
      <div className="widget-stage" ref={stage} onClick={() => voice.toggle(s.activeSession, liveVoice())} title="Click or Space: talk / stop. M: mute. Esc: stop.">
        <Orb mic={v.micAnalyser} play={v.playAnalyser} state={state} theme={theme} size={orbSize} />
      </div>

      <div className={`widget-status st-${state}`}>
        <span className="sdot" />
        {label}
        {todo.length > 0 && (
          <span className="widget-todo" title={`Todo: ${todoDone} of ${todo.length} done`} aria-label={`Todo: ${todoDone} of ${todo.length} done`}>
            {todoDone}/{todo.length}
          </span>
        )}
        {s.activeSession && (
          <button type="button" className={`widget-model${choice.none ? " none" : ""}`} title="Model and effort (Ctrl+M)" aria-label={`Model and effort: ${choice.none ? "no model" : modelLabel(choice.answering)}`} onClick={openModels}>
            {choice.none ? "No model" : modelLabel(choice.chosen ?? choice.chatModel)}
            {choice.level && !choice.none ? ` · ${choice.level}` : ""}
          </button>
        )}
      </div>

      {(lastUser || lastAgent) && (
        <div className="widget-lines">
          {lastUser && (
            <div className="wl user">
              <b>you</b> {lastUser.text}
            </div>
          )}
          {lastAgent && (
            <div className={`wl ${lastAgent.who}`}>
              <b>{lastAgent.who === "tool" ? "tool" : name}</b> {lastAgent.text}
            </div>
          )}
        </div>
      )}

      <div className="widget-bar">
        <button
          className={`wbtn ${active ? "on" : ""}`}
          title={active ? "Stop (Esc)" : "Talk (Space)"}
          aria-label={active ? "Stop" : "Talk"}
          onClick={() => voice.toggle(s.activeSession, liveVoice())}
        >
          <Icon name={active ? "stop" : "talk"} size={16} />
        </button>
        <button className={`wbtn ${v.muted ? "warn" : ""}`} disabled={!active} title={v.muted ? "Unmute microphone (M)" : "Mute microphone (M)"} aria-label="Mute microphone" onClick={() => voice.setMuted(!voice.muted)}>
          <Icon name={v.muted ? "micOff" : "mic"} size={16} />
        </button>
        <span className="wvol-icon" aria-hidden="true">
          <Icon name="volume" size={15} />
        </span>
        <input
          className="wvol"
          type="range"
          min={0}
          max={100}
          value={pct}
          title={`Voice volume ${pct}%`}
          onChange={(e) => voice.setVolume(Number(e.target.value) / 100)}
          onWheel={(e) => voice.setVolume(voice.volume + (e.deltaY < 0 ? 0.05 : -0.05))}
        />
        <button className="wbtn look" title={`Look: ${theme} (click for the next)`} aria-label="Change look" onClick={() => setTheme(ORBS[(ORBS.indexOf(theme) + 1) % ORBS.length])}>
          <Icon name="look" size={15} />
          <span>{theme}</span>
        </button>
      </div>

      {s.sheet === "models" && (
        <Sheet title="Model and effort" onClose={closeSheet}>
          <ModelBody sessionId={s.activeSession} />
        </Sheet>
      )}

      <div className="widget-approvals">
        <Approvals />
      </div>
    </div>
  );
}

// Voice: the live model, who thinks, the voice, the look of the orb and pausing media.
// The Voice tab keeps the live stage (orb, talk button, transcript) and links here.
import { useEffect, useState, useSyncExternalStore } from "react";
import { api, type StateView, type Template, type ToolView, type VoiceBrain } from "../../api";
import { voice as ctl } from "../../voice/controller";
import { ORB_THEMES, normalizeTheme, type OrbTheme } from "../../voice/Orb";
import { useStore } from "../../store";
import type { SectionProps } from "../registry";
import { GroupCard, Row, SectionPage, Toggle } from "../ui";
import { useSettingPatch } from "../useSettingPatch";
import { groupAbout, groupLabel } from "../groups";
import Select from "../Select";

const ORB = "kagent.orb";
/** The live model that thinks longer before it speaks (voice.md section 8). */
const EXTENDED = "extended-thinking";
const EXTENDED_HINT = "slower, more thorough";
/** The daemon's defaults, shown until the settings say otherwise. */
const DEFAULT_GROUPS = ["desktop", "tasks", "system"];
const THINKING: { id: VoiceBrain; label: string; hint: string }[] = [
  { id: "live", label: "Live model (fastest)", hint: "The live model thinks by itself, with the tools picked below." },
  { id: "auto", label: "Live model decides", hint: "The live model answers small talk and direct commands itself and hands everything else to the chat model." },
  { id: "routed", label: "Chat model (smarter, a little slower)", hint: "The chat model thinks, with the conversation, your memory and skills, and the live model reads its answer aloud. Without a chat model the live model thinks." },
];
/** Over this many ms of median wait for the Brain, the line becomes a warning (voice-control.md section 3). */
const SLOW_MS = 4000;

const readLook = (): OrbTheme => normalizeTheme(localStorage.getItem(ORB));

// The Voice tab and the `ui` tool change the look through this event.
function subscribeLook(notify: () => void) {
  window.addEventListener("kagent-orb", notify);
  return () => {
    window.removeEventListener("kagent-orb", notify);
  };
}

export default function VoiceSection({ title, description }: SectionProps) {
  const settings = useStore().state?.settings;
  const { save } = useSettingPatch();
  const [voices, setVoices] = useState<string[]>([]);
  const [liveModels, setLiveModels] = useState<{ ref: string }[]>([]);
  const [thinkMs, setThinkMs] = useState<number | null>(null);
  const [tools, setTools] = useState<ToolView[]>([]);
  // The slider moves at once (the page applies it live); the setting is saved when it is let go.
  const [dragging, setDragging] = useState<number | null>(null);
  const look = useSyncExternalStore(subscribeLook, readLook, readLook);

  useEffect(() => {
    void api.get<Template[] | { templates: Template[] }>("/api/providers/templates").then((t) => setVoices([...new Set((Array.isArray(t) ? t : t.templates).flatMap((x) => x.voices ?? []))]));
    void api.get<Record<string, { ref: string }[]>>("/api/models").then((m) => setLiveModels(m.live ?? []));
    void api
      .get<StateView>("/api/state")
      .then((st) => setThinkMs(st.voice?.thinkMedianMs ?? null))
      .catch(() => setThinkMs(null));
    // The groups the live model may be given: those of the registered tools.
    void api
      .get<{ tools: ToolView[] }>("/api/tools")
      .then((r) => setTools(r.tools ?? []))
      .catch(() => setTools([]));
  }, []);

  const liveModel = settings?.liveModel ?? "";
  const liveVoice = settings?.liveVoice ?? "Kore";
  const voice = settings?.voice;
  const brain = voice?.brain ?? "live";
  const liveGroups = voice?.toolGroups ?? DEFAULT_GROUPS;
  const setLook = (t: OrbTheme) => {
    localStorage.setItem(ORB, t);
    window.dispatchEvent(new Event("kagent-orb"));
  };
  // Every group a tool is in, and the groups the setting names that no tool has now.
  const groups = [...new Set([...tools.map((t) => t.group), ...liveGroups])].sort();
  // The live model only gets what chat may call too: a group that is off for chat is off here.
  const open = (g: string) => tools.some((t) => t.group === g && t.enabled);
  const toggle = (g: string) => void save("voice", { toolGroups: liveGroups.includes(g) ? liveGroups.filter((x) => x !== g) : [...liveGroups, g] });
  // The hint of a model is part of its line: a list has nothing beside it.
  const line = (ref: string) => (ref.endsWith(EXTENDED) ? `${ref} (${EXTENDED_HINT})` : ref);

  return (
    <SectionPage title={title} description={description}>
      <GroupCard title="Live session" description="What listens and speaks while you talk to KAgent.">
        <Row label="Live model" hint="A Gemini *-live model. Add a Gemini provider in Models first.">
          <Select aria-label="Live model" value={liveModel} onChange={(e) => void save("liveModel", e.target.value)}>
            <option value="">(none)</option>
            {liveModels.map((m) => (
              <option key={m.ref} value={m.ref}>
                {line(m.ref)}
              </option>
            ))}
          </Select>
        </Row>
        {liveModel.endsWith(EXTENDED) && <p className="hint ui-note">{liveModel}: {EXTENDED_HINT}.</p>}
        <Row label="Voice" hint="The voice that reads the answers.">
          <Select aria-label="Voice" value={liveVoice} onChange={(e) => void save("liveVoice", e.target.value)}>
            {[liveVoice, ...voices.filter((v) => v !== liveVoice)].map((v) => (
              <option key={v}>{v}</option>
            ))}
          </Select>
        </Row>
        <Row label="Volume" hint="How loud KAgent speaks. Not the system volume.">
          <input
            type="range"
            aria-label="Volume"
            min={0}
            max={100}
            step={5}
            value={dragging ?? voice?.volume ?? 100}
            onChange={(e) => {
              setDragging(Number(e.target.value));
              ctl.setSettingsVolume(Number(e.target.value));
            }}
            onPointerUp={(e) => {
              void save("voice", { volume: Number(e.currentTarget.value) });
              setDragging(null);
            }}
            onKeyUp={(e) => {
              void save("voice", { volume: Number(e.currentTarget.value) });
              setDragging(null);
            }}
          />
          <span className="hint">{dragging ?? voice?.volume ?? 100}%</span>
        </Row>
        <Row label="Look" hint="The orb on the Voice tab.">
          <Select aria-label="Look" value={look} onChange={(e) => setLook(e.target.value as OrbTheme)}>
            {ORB_THEMES.map((t) => (
              <option key={t}>{t}</option>
            ))}
          </Select>
        </Row>
        <Row label="Pause media" hint="Music and videos pause while a voice session is open.">
          <Toggle label="Pause media" checked={voice?.pauseMedia ?? true} onChange={(v) => void save("voice", { pauseMedia: v })} />
        </Row>
      </GroupCard>
      <GroupCard title="Thinking" description="Who understands what you say and acts on it. A change applies to the next voice session.">
        <Row label="Thinking" hint={THINKING.find((t) => t.id === brain)?.hint} stacked>
          <div className="row" role="radiogroup" aria-label="Thinking">
            {THINKING.map((t) => (
              <button key={t.id} type="button" role="radio" aria-checked={t.id === brain} className={t.id === brain ? "primary" : ""} onClick={() => void save("voice", { brain: t.id })}>
                {t.label}
              </button>
            ))}
          </div>
        </Row>
        {brain !== "live" && (
          <p className={thinkMs !== null && thinkMs > SLOW_MS ? "hint ui-note warn-text" : "hint ui-note"} role={thinkMs !== null && thinkMs > SLOW_MS ? "alert" : undefined}>
            {thinkMs !== null && thinkMs > SLOW_MS
              ? `The Brain took ${Math.round(thinkMs / 1000)} s on average: live answers faster.`
              : `Every request waits for the Brain (${(settings?.chatModel ?? "").split("::").pop() || "no chat model"}).`}
          </p>
        )}
        <Row label="Tools for the live model" hint="The groups the live model may call itself in the other two modes. A group that is off for chat is off here too; none picked means no tools." stacked>
          <div className="chips">
            {groups.map((g) => (
              <label key={g} className={`chip${liveGroups.includes(g) ? " on" : ""}`} title={open(g) ? groupAbout(g) : "Switch the group on for chat first (Tools page)"}>
                <input type="checkbox" checked={liveGroups.includes(g)} disabled={!open(g)} onChange={() => toggle(g)} /> {groupLabel(g)}
                {!open(g) && " (off for chat)"}
              </label>
            ))}
          </div>
        </Row>
        <Row label="Conversation context" hint="The live model hears what was discussed, your memory, the todo list and the open alerts when it connects (the other two modes).">
          <Toggle label="Conversation context" checked={voice?.context ?? true} onChange={(v) => void save("voice", { context: v })} />
        </Row>
        <Row label="Custom vocabulary" hint="Names of your projects, memories and skills are transcribed better.">
          <Toggle label="Custom vocabulary" checked={voice?.vocabulary ?? true} onChange={(v) => void save("voice", { vocabulary: v })} />
        </Row>
      </GroupCard>
    </SectionPage>
  );
}

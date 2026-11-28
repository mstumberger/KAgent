import Approvals from "../components/Approvals";
import CommandInput from "../components/CommandInput";
import ModelMenu from "../components/ModelMenu";
import { useChoice } from "../components/useChoice";
import { Bubble } from "../components/transcript";
import WorkPanel from "../components/WorkPanel";
import AgentsCard from "../components/AgentsCard";
import { agentCards, placeAgents } from "../components/agents";
import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import type { Message } from "../api";
import { cancelSession, getStore, openSession, sendMessage, setPlan, setView, takeComposerDraft, useStore } from "../store";
import Icon from "../components/Icon";
import { useVoice } from "../voice/useVoice";
import { voice } from "../voice/controller";

const EMPTY: never[] = [];
const NO_MESSAGES: Message[] = [];

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

export default function Chat() {
  const s = useStore();
  // Opens with the text another view left for the composer ("New skill with the agent" in Settings > Skills).
  const [text, setText] = useState(s.composerDraft);
  // The Plan toggle next to Send: armed for the next message until it is sent, and on while
  // the session plans (the chip above the composer says so, with Cancel plan).
  const [planNext, setPlanNext] = useState<{ session: string | null; on: boolean }>({ session: null, on: false });
  const [planBusy, setPlanBusy] = useState(false);
  // The model menu above the composer is open: the slash picker stays shut.
  const [menuOpen, setMenuOpen] = useState(false);
  const choice = useChoice(s.activeSession);
  // Something can answer: the default model, or the conversation's own choice (picker.md section 5.1).
  const canAnswer = !choice.none;
  const endRef = useRef<HTMLDivElement>(null);
  // One shared empty list: a fresh `[]` per render would defeat the memo of the cards.
  const messages = s.activeSession ? (s.messages[s.activeSession] ?? NO_MESSAGES) : NO_MESSAGES;
  const streaming = s.activeSession ? s.streaming[s.activeSession] : undefined;
  const running = s.activeSession ? s.running.has(s.activeSession) : false;
  const planning = s.activeSession ? (s.planning[s.activeSession] ?? false) : false;
  // The subagents of the open session (agents.md section 6): its cards sit in the transcript
  // under the message that reports each of them.
  const children = s.activeSession ? (s.children[s.activeSession] ?? EMPTY) : EMPTY;
  const cards = useMemo(() => agentCards(children, messages, s.running), [children, messages, s.running]);
  const placed = useMemo(() => placeAgents(messages, cards), [messages, cards]);
  // The session of a subagent opens read-only, with the way back to the one that started it.
  const info = s.activeSession ? s.metas[s.activeSession] : undefined;
  const subagent = info?.origin === "subagent";
  const parentId = s.activeSession ?? "";
  const armed = !planning && planNext.on && planNext.session === s.activeSession;
  const togglePlan = async () => {
    if (planning && s.activeSession) {
      setPlanBusy(true);
      try {
        await setPlan(s.activeSession, false);
      } catch {
        // The store shows why, as a notice in the header.
      } finally {
        setPlanBusy(false);
      }
      return;
    }
    setPlanNext({ session: s.activeSession, on: !armed });
  };
  const v = useVoice();
  const talking = v.active && v.sessionId === s.activeSession;
  const pendingText = v.pending.map((l) => l.text).join("|").length;

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages.length, streaming, pendingText]);

  useEffect(() => {
    const onComposer = (e: Event) => setText(String((e as CustomEvent).detail));
    window.addEventListener("kagent-composer", onComposer);
    // The text taken as the first one is consumed here.
    if (getStore().composerDraft) takeComposerDraft();
    return () => window.removeEventListener("kagent-composer", onComposer);
  }, []);

  return (
    <div className="chat-layout">
      <section className="chat-main">
        <Approvals />

        <div className="messages">
          {!s.activeSession && <div className="empty">Create a session to start.</div>}
          {messages.map((m, i) => (
            <Fragment key={i}>
              <Bubble m={m} streaming={i === messages.length - 1 && running ? streaming : undefined} />
              {(placed.under.get(i) ?? []).map((card) => (
                <AgentsCard key={card.id} child={card} parentId={parentId} />
              ))}
            </Fragment>
          ))}
          {placed.loose.map((card) => (
            <AgentsCard key={card.id} child={card} parentId={parentId} />
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

        <WorkPanel sessionId={s.activeSession} />

        {subagent ? (
          <div className="agent-readonly" role="note">
            <span>
              Read-only: this is the session of the subagent <strong>{info.title}</strong>. Its answer arrives in the session that started it.
            </span>
            {info.parent && (
              <button type="button" onClick={() => void openSession(info.parent as string)}>
                Back to the main session
              </button>
            )}
          </div>
        ) : (
        <form
          className="composer"
          onSubmit={(e) => {
            e.preventDefault();
            if (text.trim() === "/plan") {
              // The command alone: the next message starts in plan mode.
              setPlanNext({ session: s.activeSession, on: true });
              setText("");
              return;
            }
            void sendMessage(text, armed);
            setText("");
            setPlanNext({ session: null, on: false });
          }}
        >
          <CommandInput
            value={text}
            skills={s.skills}
            suspend={menuOpen}
            onChange={setText}
            onPick={setText}
            placeholder={talking ? "Listening... you can also type" : canAnswer ? "Ask about this machine... ( / for skills )" : "Pick a model in the menu below first"}
          />
          <div className="composer-bar">
            <ModelMenu sessionId={s.activeSession} onOpenChange={setMenuOpen} />
            <div className="composer-tools">
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
              <button
                type="button"
                className={`plan-toggle${planning || armed ? " on" : ""}`}
                aria-pressed={planning || armed}
                title={planning ? "Plan mode is on: click to cancel the plan" : armed ? "The next message starts in plan mode" : "Plan first: the agent researches and shows a plan before anything changes"}
                disabled={planBusy || !s.activeSession}
                onClick={() => void togglePlan()}
              >
                Plan
              </button>
              {running ? (
                <button type="button" className="danger" onClick={() => void cancelSession()}>
                  Stop
                </button>
              ) : (
                <button className="primary" disabled={!text.trim() || !canAnswer}>
                  Send
                </button>
              )}
            </div>
          </div>
        </form>
        )}
      </section>
    </div>
  );
}

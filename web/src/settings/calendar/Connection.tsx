// Card 1 of the Calendar page (calendar.md section 2): the OAuth client of the owner's own
// Google Cloud project, and the connection to the account.
import { useState, type FormEvent } from "react";
import { Badge, Field, GroupCard, type BadgeTone } from "../ui";
import { useDraft } from "../useDraft";
import type { CalendarState, CalendarView } from "./api";

const STATES: Record<CalendarState, { tone: BadgeTone; text: string }> = {
  off: { tone: "neutral", text: "Not connected" },
  connecting: { tone: "warn", text: "Waiting for Google" },
  connected: { tone: "ok", text: "Connected" },
  reconnect: { tone: "warn", text: "Reconnect needed" },
  error: { tone: "bad", text: "Error" },
};

const STEPS = [
  "In console.cloud.google.com create a project, and enable the \"Google Calendar API\" for it.",
  "OAuth consent screen: user type External, add your own address as a test user, then press \"Publish app\". No verification is needed for your own account.",
  "Credentials, Create credentials, OAuth client ID, application type \"Desktop app\".",
  "Paste its client ID and client secret into the two fields above and press Save client.",
  "Press Connect, choose your account in the Google window and allow the two permissions: events, and the list of calendars.",
];

export default function Connection({
  view,
  clientId,
  working,
  onSaveClient,
  onConnect,
  onDisconnect,
}: {
  view: CalendarView | null;
  clientId: string;
  working: boolean;
  onSaveClient: (id: string, secret: string) => Promise<void>;
  onConnect: () => void;
  onDisconnect: () => void;
}) {
  const [id, setId, idDirty] = useDraft(clientId);
  const [secret, setSecret] = useState("");
  const state = view ? STATES[view.state] : { tone: "neutral" as BadgeTone, text: "No answer from the daemon" };
  const hasClient = clientId !== "" && view?.hasClientSecret === true;
  const connected = view?.state === "connected" || view?.state === "reconnect";
  const save = (e: FormEvent) => {
    e.preventDefault();
    void onSaveClient(id.trim(), secret.trim()).then(() => setSecret(""));
  };
  return (
    <GroupCard title="Connection" description="KAgent talks to Google directly, with an OAuth client of your own: no third-party server sees your calendar.">
      <div className="cal-status" data-state={view?.state ?? "none"}>
        <div className="cal-line">
          <Badge tone={state.tone}>{state.text}</Badge>
          {view?.account && <span className="cal-account">{view.account}</span>}
        </div>
        {view?.state === "reconnect" && <p className="hint cal-warn">Google no longer accepts the saved connection. Press Reconnect; nothing was deleted.</p>}
        {view?.error && view.state !== "reconnect" && <p className="hint cal-bad">{view.error}</p>}
        {view?.state === "connecting" && <p className="hint">Finish in the Google window that opened. If it did not, press Connect again.</p>}
      </div>
      <form className="ui-form" onSubmit={save}>
        <Field label="Client ID" hint="Ends in .apps.googleusercontent.com.">
          <input type="text" value={id} autoComplete="off" spellCheck={false} placeholder="123456789-abc.apps.googleusercontent.com" onChange={(e) => setId(e.target.value)} />
        </Field>
        <Field label="Client secret" hint="Kept in your desktop keyring; this page only hears whether there is one.">
          <input type="password" value={secret} autoComplete="off" spellCheck={false} placeholder={view?.hasClientSecret ? "A secret is saved; leave empty to keep it" : "GOCSPX-..."} onChange={(e) => setSecret(e.target.value)} />
        </Field>
        <div className="row">
          <button type="submit" disabled={working || id.trim() === "" || (!idDirty && secret.trim() === "")}>
            Save client
          </button>
          {connected ? (
            <>
              {view?.state === "reconnect" && (
                <button type="button" className="primary" disabled={working || !hasClient} onClick={onConnect}>
                  Reconnect
                </button>
              )}
              <button type="button" className="danger" disabled={working} onClick={onDisconnect}>
                Disconnect
              </button>
            </>
          ) : (
            <button type="button" className="primary" disabled={working || !hasClient} onClick={onConnect}>
              Connect
            </button>
          )}
        </div>
      </form>
      <p className="hint ui-note">
        An OAuth consent screen left in "Testing" makes Google expire the saved connection after 7 days. Publish it ("In production"; your own account needs no verification) or reconnect every week.
      </p>
      <details className="cal-fold">
        <summary>How to create the client</summary>
        <ol>
          {STEPS.map((step) => (
            <li key={step}>{step}</li>
          ))}
        </ol>
      </details>
    </GroupCard>
  );
}

// Card 4 of the Calendar page (calendar.md section 6): the morning report, built by code
// (no model), sent to Telegram at its time and spoken in the first voice session after it.
import { useState } from "react";
import { GroupCard, Row, Toggle } from "../ui";
import type { CalendarView, ReportSettings } from "./api";

const when = (iso: string) => {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
};

const WHERE: Record<string, string> = { telegram: "Telegram", spoken: "the next voice session" };

/** A time of day, with a switch for "no report on these days". */
function At({ label, value, onChange }: { label: string; value: string; onChange: (v: string) => void }) {
  return (
    <Row label={label} hint={value === "" ? "No report on these days." : undefined}>
      <div className="cal-time">
        <Toggle title={`${label}: report on`} checked={value !== ""} onChange={(on) => onChange(on ? (label === "Weekdays" ? "07:45" : "09:00") : "")} />
        <input type="time" aria-label={`${label}: time`} value={value} disabled={value === ""} onChange={(e) => e.target.value && onChange(e.target.value)} />
      </div>
    </Row>
  );
}

export default function Report({
  view,
  report,
  telegramPaired,
  working,
  onSave,
  onSend,
}: {
  view: CalendarView | null;
  report: ReportSettings;
  telegramPaired: boolean;
  working: boolean;
  onSave: (patch: ReportSettings) => void;
  onSend: () => Promise<{ text: string; sent: string[] }>;
}) {
  const [shown, setShown] = useState<{ text: string; sent: string[] } | null>(null);
  const connected = view?.state === "connected";
  const last = shown ?? (view?.lastReport ? { text: view.lastReport.text, sent: view.lastReport.sent, at: view.lastReport.at } : null);
  const at = view?.lastReport?.at;
  const sendNow = () => void onSend().then(setShown);
  return (
    <GroupCard title="Morning report" description="Today's events, overlaps, when to leave and your reminders, made by KAgent without a model. A busy day, in your language.">
      <Row label="Morning report">
        <Toggle title="Morning report" checked={report.enabled !== false} onChange={(enabled) => onSave({ enabled })} />
      </Row>
      <At label="Weekdays" value={report.weekdays ?? "07:45"} onChange={(weekdays) => onSave({ weekdays })} />
      <At label="Weekends" value={report.weekends ?? "09:00"} onChange={(weekends) => onSave({ weekends })} />
      <Row label="Spoken" hint="Read aloud in the first voice session after the report time, once.">
        <Toggle title="Spoken" checked={report.spoken !== false} onChange={(spoken) => onSave({ spoken })} />
      </Row>
      <Row label="Telegram" hint={telegramPaired ? "Sent to the paired chat at the report time." : "Pair the bot first (Settings, Telegram)."}>
        <Toggle title="Telegram" checked={telegramPaired && report.telegram !== false} disabled={!telegramPaired} onChange={(telegram) => onSave({ telegram })} />
      </Row>
      <Row label="Tomorrow" hint="The first event of tomorrow, when it starts before 10:00 or is the only one.">
        <Toggle title="Tomorrow" checked={report.tomorrow !== false} onChange={(tomorrow) => onSave({ tomorrow })} />
      </Row>
      <Row label="Reminders" hint="KAgent's own reminders and tasks due today.">
        <Toggle title="Reminders" checked={report.tasks !== false} onChange={(tasks) => onSave({ tasks })} />
      </Row>
      <Row label="Leave by" hint="When to leave for the first event with a place. Needs a Maps key and a home address.">
        <Toggle title="Leave by" checked={report.leaveBy !== false} onChange={(leaveBy) => onSave({ leaveBy })} />
      </Row>
      <div className="row cal-send">
        <button type="button" className="primary" disabled={working || !connected} onClick={sendNow}>
          Send now
        </button>
        <span className="hint">Sends to the channels that are on, and shows the text.</span>
      </div>
      {last && (
        <div className="cal-last" aria-label="The last report">
          <p className="hint">
            {shown ? "Just now" : at ? when(at) : ""}
            {last.sent.length > 0 ? `, sent to ${last.sent.map((s) => WHERE[s] ?? s).join(" and ")}` : ", sent nowhere (no channel is on)"}
          </p>
          <pre>{last.text}</pre>
        </div>
      )}
    </GroupCard>
  );
}

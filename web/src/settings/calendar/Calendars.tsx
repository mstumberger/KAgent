// Card 2 of the Calendar page: which calendars of the account the agent and the report may
// see, and which one gets new events. A calendar that is off does not exist for them.
import { EmptyState, GroupCard, Row, Toggle, Badge } from "../ui";
import type { CalendarSettings, CalendarView } from "./api";

export default function Calendars({ view, settings, onSave }: { view: CalendarView | null; settings: CalendarSettings; onSave: (patch: CalendarSettings) => void }) {
  const list = view?.calendars ?? [];
  if (view?.state !== "connected" && view?.state !== "reconnect") {
    return (
      <GroupCard title="Calendars" description="Only the calendars you switch on exist for KAgent: it cannot read, add to or mention the others.">
        <EmptyState title="Connect an account first" hint="The calendars of the account appear here once it is connected." />
      </GroupCard>
    );
  }
  const isDefault = (c: CalendarView["calendars"][number]) => settings.defaultCalendar === c.id || (settings.defaultCalendar === "primary" && c.id === primaryId(list, view));
  const switchOn = (id: string, on: boolean) => {
    const calendars = list.filter((c) => (c.id === id ? on : c.on)).map((c) => c.id);
    if (calendars.length === 0) return;
    const current = list.find(isDefault);
    const keep = current && calendars.includes(current.id) ? current.id : (list.find((c) => c.writable && calendars.includes(c.id))?.id ?? calendars[0]);
    onSave({ calendars, defaultCalendar: keep });
  };
  const onCount = list.filter((c) => c.on).length;
  return (
    <GroupCard title="Calendars" description="Only the calendars you switch on exist for KAgent: it cannot read, add to or mention the others. The default one gets new events.">
      {list.length === 0 && <EmptyState title="No calendars" hint={view.error ?? "Google returned no calendars for this account."} />}
      <div className="cal-list" role="group" aria-label="Calendars of the account">
        {list.map((c) => (
          <div className="cal-item" key={c.id}>
            <span className="cal-dot" style={{ background: c.color || "var(--dim, #888)" }} aria-hidden="true" />
            <span className="cal-name">
              {c.name}
              {!c.writable && <Badge tone="neutral">read-only</Badge>}
            </span>
            <label className="cal-default" title={c.writable ? "New events go to this calendar" : "A read-only calendar cannot be the default"}>
              <input type="radio" name="default-calendar" checked={isDefault(c)} disabled={!c.writable || !c.on} onChange={() => onSave({ defaultCalendar: c.id })} />
              <span>default</span>
            </label>
            <Toggle title={`Switch ${c.name} on`} checked={c.on} disabled={c.on && onCount === 1} onChange={(on) => switchOn(c.id, on)} />
          </div>
        ))}
      </div>
      <Row label="New event length" hint="Minutes, when an event is added without an end.">
        <input type="number" min={5} max={1440} value={settings.defaultDurationMin ?? 60} onChange={(e) => e.target.valueAsNumber >= 5 && onSave({ defaultDurationMin: e.target.valueAsNumber })} />
      </Row>
    </GroupCard>
  );
}

function primaryId(list: CalendarView["calendars"], view: CalendarView): string | undefined {
  return list.find((c) => c.id === view.account)?.id;
}

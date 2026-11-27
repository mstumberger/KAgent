// Card 3 of the Calendar page (calendar.md section 5): the Google Maps key for places and
// travel times, the home address and the margin before leaving.
import { useState, type FormEvent } from "react";
import { Badge, Field, GroupCard } from "../ui";
import { useDraft } from "../useDraft";
import type { CalendarSettings, CalendarView, MapsTest } from "./api";

export default function Places({
  view,
  settings,
  working,
  onKey,
  onTest,
  onSave,
}: {
  view: CalendarView | null;
  settings: CalendarSettings;
  working: boolean;
  onKey: (key: string | null) => Promise<void>;
  onTest: () => Promise<MapsTest>;
  onSave: (patch: CalendarSettings) => void;
}) {
  const [key, setKey] = useState("");
  const [result, setResult] = useState<MapsTest | null>(null);
  const [home, setHome, homeDirty] = useDraft(settings.home ?? "");
  const [margin, setMargin, marginDirty] = useDraft(settings.travelMarginMin ?? 10);
  const marginOk = Number.isInteger(margin) && margin >= 0 && margin <= 120;
  const save = (e: FormEvent) => {
    e.preventDefault();
    void onKey(key.trim()).then(() => setKey(""));
  };
  const test = () => {
    setResult(null);
    void onTest().then(setResult);
  };
  return (
    <GroupCard title="Places and travel" description="With a Google Maps key an event's place is looked up on the map and the report can say when to leave. Without one the place search uses the web, and leave-by is off.">
      <form className="ui-form" onSubmit={save}>
        <Field label="Maps API key" hint="Needs Places API (New) and Routes API enabled. Kept in your desktop keyring; this page only hears whether there is one.">
          <input type="password" value={key} autoComplete="off" spellCheck={false} placeholder={view?.hasMapsKey ? "A key is saved; paste another to replace it" : "AIza..."} onChange={(e) => setKey(e.target.value)} />
        </Field>
        <div className="row">
          <button type="submit" disabled={working || key.trim() === ""}>
            Save key
          </button>
          <button type="button" disabled={working || !view?.hasMapsKey} onClick={test}>
            Test
          </button>
          {view?.hasMapsKey && (
            <button type="button" className="danger" disabled={working} onClick={() => void onKey(null)}>
              Delete key
            </button>
          )}
        </div>
        {result && (
          <p className="hint" role="status">
            {result.ok ? <Badge tone="ok">Works</Badge> : <Badge tone="bad">Failed</Badge>} {result.ok ? `Found ${result.place?.name}${result.place?.address ? `: ${result.place.address}` : ""}.` : result.error}
          </p>
        )}
      </form>
      <form className="ui-form" onSubmit={(e) => void (e.preventDefault(), homeDirty && onSave({ home }))}>
        <Field label="Home address" hint="Where you leave from. Needs a Maps key; without one it is kept as text and leave-by stays off.">
          <input type="text" value={home} placeholder="Street 1, 1000 Town" onChange={(e) => setHome(e.target.value)} />
        </Field>
        <div className="row">
          <button type="submit" disabled={!homeDirty}>
            Save home
          </button>
        </div>
      </form>
      <form className="ui-form" onSubmit={(e) => void (e.preventDefault(), marginDirty && marginOk && onSave({ travelMarginMin: margin }))}>
        <Field label="Margin before leaving" hint="Minutes added to the drive: 0 to 120.">
          <input type="number" min={0} max={120} value={Number.isNaN(margin) ? "" : margin} onChange={(e) => setMargin(e.target.valueAsNumber)} onBlur={() => marginDirty && marginOk && onSave({ travelMarginMin: margin })} />
        </Field>
      </form>
    </GroupCard>
  );
}

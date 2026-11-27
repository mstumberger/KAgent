// Calendar (calendar.md section 2): the connection to Google, the calendars KAgent may see,
// places and travel, and the morning report. The client secret, the token and the Maps key stay
// in the daemon and your desktop keyring; this page only hears whether they exist.
import { useEffect, useState } from "react";
import { events } from "../../events";
import { applySettings, useStore } from "../../store";
import type { SectionProps } from "../registry";
import { SectionPage } from "../ui";
import { useSettingPatch } from "../useSettingPatch";
import { calendarApi, type CalendarSettings, type CalendarView, type MapsTest, type ReportSettings } from "../calendar/api";
import { telegramApi } from "../telegram/api";
import Calendars from "../calendar/Calendars";
import Connection from "../calendar/Connection";
import Places from "../calendar/Places";
import Report from "../calendar/Report";
import "../calendar/calendar.css";

const reason = (e: unknown) => (e instanceof Error ? e.message : String(e));

export default function CalendarSection({ title, description }: SectionProps) {
  const settings = useStore().state?.settings;
  const { save, report, busy } = useSettingPatch();
  const [view, setView] = useState<CalendarView | null>(null);
  const [paired, setPaired] = useState(false);
  const [working, setWorking] = useState(false);
  const calendar: CalendarSettings = settings?.calendar ?? {};

  // The connection when the page opens, and again after every calendarChanged event.
  useEffect(() => {
    let open = true;
    const read = () =>
      calendarApi.view().then(
        (next) => {
          if (open) setView(next);
        },
        () => {},
      );
    void read();
    telegramApi.status().then(
      (s) => {
        if (open) setPaired(s.paired);
      },
      () => {},
    );
    const off = events.on((ev) => {
      if (ev.type === "calendarChanged") void read();
    });
    return () => {
      open = false;
      off();
    };
  }, []);

  /** One action of the page: its result goes to the header, a failure too. */
  const act = async <T,>(what: string, run: () => Promise<T>, done?: string): Promise<T | undefined> => {
    setWorking(true);
    try {
      const out = await run();
      if (done) report(done);
      return out;
    } catch (e) {
      report(`${what} failed: ${reason(e)}`, true);
      return undefined;
    } finally {
      setWorking(false);
    }
  };
  const refresh = async () => setView(await calendarApi.view());
  const saveCalendar = async (patch: CalendarSettings) => {
    await save("calendar", patch);
    await refresh().catch(() => {});
  };
  const saveReport = (patch: ReportSettings) => void save("calendar", { report: patch });

  const connect = () =>
    void act("Connecting", async () => {
      const { url } = await calendarApi.connect();
      // The sign-in happens in the owner's browser; the daemon's callback tells this page when it is done.
      window.open(url, "_blank", "noopener");
      await refresh();
    }, "Google's sign-in opened in your browser.");
  const disconnect = () => void act("Disconnecting", async () => void (await calendarApi.disconnect(), await refresh()), "Disconnected. The token is revoked and deleted; your settings stay.");
  const saveClient = async (id: string, secret: string) => {
    await act("Saving the client", async () => {
      await calendarApi.setClient(id, secret);
      if (settings) applySettings({ ...settings, calendar: { ...calendar, clientId: id } });
      await refresh();
    }, "Client saved.");
  };
  const setKey = async (key: string | null) => {
    await act("Maps key", async () => void (await calendarApi.setMapsKey(key), await refresh()), key === null ? "Maps key deleted." : "Maps key saved.");
  };
  const testKey = async (): Promise<MapsTest> => (await act("Testing the key", calendarApi.testMaps)) ?? { ok: false, error: "No answer from the daemon" };
  const send = async () => (await act("Sending the report", () => calendarApi.report(true), "Report sent to the channels that are on.")) ?? { text: "", sent: [] };

  return (
    <SectionPage title={title} description={description}>
      <Connection view={view} clientId={calendar.clientId ?? ""} working={working || busy} onSaveClient={saveClient} onConnect={connect} onDisconnect={disconnect} />
      <Calendars view={view} settings={calendar} onSave={(patch) => void saveCalendar(patch)} />
      <Places view={view} settings={calendar} working={working || busy} onKey={setKey} onTest={testKey} onSave={(patch) => void saveCalendar(patch)} />
      <Report view={view} report={calendar.report ?? {}} telegramPaired={paired} working={working || busy} onSave={saveReport} onSend={send} />
    </SectionPage>
  );
}

// The calendar routes (calendar.md section 7) and what they answer. The settings themselves
// are in `Settings.calendar` (api.ts); the tokens and keys stay in the daemon.
import { api, type Settings } from "../../api";

/** `settings.calendar` (calendar.md section 2). */
export type CalendarSettings = NonNullable<Settings["calendar"]>;
export type ReportSettings = NonNullable<CalendarSettings["report"]>;

/** The connection (`calendarChanged` events carry the same names). */
export type CalendarState = "off" | "connecting" | "connected" | "reconnect" | "error";

export interface AccountCalendar {
  id: string;
  name: string;
  color: string;
  writable: boolean;
  /** Switched on by the owner: the agent and the report may see it. */
  on: boolean;
}

export interface LastReport {
  /** RFC 3339. */
  at: string;
  text: string;
  /** `telegram`, `spoken`. */
  sent: string[];
}

/** `GET /api/calendar`. It says whether a secret is stored, never the secret. */
export interface CalendarView {
  state: CalendarState;
  account: string | null;
  calendars: AccountCalendar[];
  hasClientSecret: boolean;
  hasMapsKey: boolean;
  lastReport: LastReport | null;
  error: string | null;
}

export interface MapsTest {
  ok: boolean;
  place?: { name: string; address: string };
  error?: string;
}

export const calendarApi = {
  view: () => api.get<CalendarView>("/api/calendar"),
  /** An empty secret keeps the one stored. */
  setClient: (clientId: string, clientSecret: string) => api.put<void>("/api/calendar/client", { clientId, clientSecret }),
  /** The address the owner opens at Google. */
  connect: () => api.post<{ url: string }>("/api/calendar/connect", {}),
  disconnect: () => api.post<void>("/api/calendar/disconnect", {}),
  /** `null` forgets the key. */
  setMapsKey: (key: string | null) => api.put<void>("/api/calendar/maps-key", { key }),
  testMaps: () => api.post<MapsTest>("/api/calendar/maps-test", {}),
  report: (deliver: boolean) => api.post<{ text: string; sent: string[] }>("/api/calendar/report", { deliver }),
};

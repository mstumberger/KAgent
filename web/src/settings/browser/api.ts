// The browser routes (api.md section 6) and what they answer. The settings themselves
// are in `Settings.browser` (api.ts).
import { api } from "../../api";

/** `browser.backend`: KAgent's own window (the default), or the owner's running browser. */
export type BrowserBackend = "own" | "attach";
/** `browser.attachTrust` (browser.md section 4): `strict` asks for every action in the owner's own tabs. */
export type AttachTrust = "strict" | "normal";

/** `attach` of the browser status. */
export interface AttachStatus {
  /** The profile folder; empty when none was found. */
  profile: string;
  /** The browser's product name (its name from the profile folder until it is connected). */
  browser: string;
  port: number | null;
  /** `on`: the browser wrote its port file; `off`: it did not; `unknown`: no profile folder. */
  debugging: "on" | "off" | "unknown";
  connected: boolean;
  tabs: number;
  detail: string;
}

/** `GET /api/browser/status`. */
export interface BrowserStatus {
  /** The backend the daemon runs with; a change of the setting applies after a restart. */
  backend: BrowserBackend;
  own: { running: boolean; port: number; tabs: number };
  attach: AttachStatus;
}

/** What the owner opens in the browser once (browser.md section 6). */
export const INSTRUCTION_URL = "chrome://inspect/#remote-debugging";

export const browserApi = {
  /** Reads the port file of the profile folder; the browser itself is not contacted. */
  status: () => api.get<BrowserStatus>("/api/browser/status"),
  /** One connect attempt, which waits up to 20 s for the browser's "Allow"; the answer is the status after it. */
  connect: () => api.post<BrowserStatus>("/api/browser/attach/connect", {}),
  /** Closes the connection; the browser stops showing it as controlled. */
  disconnect: () => api.post<void>("/api/browser/attach/disconnect", {}),
};

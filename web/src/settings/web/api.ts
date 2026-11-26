// The web tools (web.md sections 3 and 5): `settings.search` and the route of the Brave
// key. The types live here and not in `api.ts`, which is at its budget; the settings are
// saved through `PUT /api/settings` like every section.
import { api } from "../../api";

/** `search.engine` (web.md section 3): the engine the `web_search` tool asks. */
export type SearchEngine = "duckduckgo" | "brave" | "searxng" | "none";

/** `settings.search` (config.md section 11). */
export interface SearchSettings {
  engine?: SearchEngine;
  /** The base address of a SearXNG instance; empty = none. */
  searxUrl?: string;
}

/** `GET /api/web/brave-key`: whether a key is stored. Never the key itself. */
export interface KeyStatus {
  configured: boolean;
}

export const webApi = {
  keyStatus: () => api.get<KeyStatus>("/api/web/brave-key"),
  /** The key is kept in the daemon (400 for one of another shape: 16 to 128 characters, no space). */
  setKey: (key: string) => api.put<KeyStatus>("/api/web/brave-key", { key }),
  clearKey: () => api.del<void>("/api/web/brave-key"),
};

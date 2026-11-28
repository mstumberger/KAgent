// The web tools (web.md sections 3 and 5): `settings.search` and the route of the Brave
// key. The types live here and not in `api.ts`, which is at its budget; the settings are
// saved through `PUT /api/settings` like every section.
import { api } from "../../api";

/** An engine of `web_search` (web.md section 3): `none` is only the old single setting, `reader` only a chain entry. */
export type SearchEngine = "duckduckgo" | "brave" | "searxng" | "reader" | "none";
/** The engines of the chain, in the order a list of them starts. */
export const CHAIN_ENGINES: Exclude<SearchEngine, "none">[] = ["duckduckgo", "brave", "searxng", "reader"];

/** `settings.search` (config.md section 11). */
export interface SearchSettings {
  /** The old single engine: the chain is this one and the fallbacks while `engines` is not set. */
  engine?: SearchEngine;
  /** The base address of a SearXNG instance; empty = none. */
  searxUrl?: string;
  /** The ordered chain (web.md section 3.1); null or absent: derived from `engine`. */
  engines?: SearchEngine[] | null;
}

/** `settings.web` (web.md section 8): the reader's switch and the sites it is always used for. */
export interface WebSettings {
  reader?: boolean;
  readerSites?: string[];
}

/** The chain as the daemon reads it (`SearchSection::chain`). */
export function chainOf(search?: SearchSettings): Exclude<SearchEngine, "none">[] {
  if (Array.isArray(search?.engines)) return search.engines.filter((e): e is Exclude<SearchEngine, "none"> => e !== "none");
  const first = search?.engine ?? "duckduckgo";
  if (first === "none") return [];
  const chain: Exclude<SearchEngine, "none">[] = [first];
  for (const fallback of ["duckduckgo", "reader"] as const) if (!chain.includes(fallback)) chain.push(fallback);
  return chain;
}

/** `GET /api/web/reader` (web.md section 9). */
export interface ReaderStatus {
  enabled: boolean;
  /** The browser the reader would run; null when none is installed. */
  browser: string | null;
  running: boolean;
  windowOpen: boolean;
  profile: string;
  warmed: { host: string; at: number }[];
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
  reader: () => api.get<ReaderStatus>("/api/web/reader"),
  /** Forget a warmed site: its cookies leave the reader's profile (409 while a window is open or a page is read). */
  forget: (host: string) => api.del<void>(`/api/web/reader/sites/${encodeURIComponent(host)}`),
  clearProfile: () => api.del<void>("/api/web/reader/profile"),
};

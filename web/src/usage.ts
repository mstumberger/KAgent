// Usage and limits as the daemon reports them (usage.md): the types of `GET /api/usage`, the read of it
// (at the start, when a card or the chooser opens, and on `usageChanged`) and the words of a limit.
import { useSyncExternalStore } from "react";
import { api } from "./api";

export type Level = "fine" | "close" | "nearlyOut" | "out";

/** One limit (usage.md section 3): `used` and `total` are null for a limit that is known to be out and not how big it is. */
export interface Limit {
  id: string;
  name: string;
  used: number | null;
  total: number | null;
  unit: "requests" | "tokens";
  resetsAt: string | null;
  /** `provider`: the provider said it. `kagent`: KAgent's own count against a limit the owner typed. */
  source: "provider" | "kagent";
  level: Level;
}

export interface Totals {
  requests: number;
  inputTokens: number;
  outputTokens: number;
}

export interface KnownLimit {
  window: "minute" | "day" | "month";
  unit: "requests" | "tokens";
  total: number;
}

export interface UsageProvider {
  id: string;
  name: string;
  local: boolean;
  limits: Limit[];
  tightest: Limit | null;
  today: Totals;
  month: Totals;
  models: { model: string; today: Totals; month: Totals }[];
  knownLimits: KnownLimit[];
  checkedAt: string | null;
}

export interface UsageView {
  providers: UsageProvider[];
}

let current: UsageView | null = null;
const listeners = new Set<() => void>();

function set(next: UsageView) {
  current = next;
  listeners.forEach((l) => l());
}

/** The limits and counters read again. A daemon that cannot answer leaves the last read. */
export async function refreshUsage() {
  try {
    set(await api.get<UsageView>("/api/usage"));
  } catch {
    // keep what was read last
  }
}

export function getUsage(): UsageView | null {
  return current;
}

/** The usage of every provider, as read last (null before the first read). */
export function useUsage(): UsageView | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    getUsage,
  );
}

export function usageOf(view: UsageView | null, providerId: string): UsageProvider | undefined {
  return view?.providers.find((p) => p.id === providerId);
}

/** `used / total` of 1; a limit with no size is full. */
export function fraction(l: Limit): number {
  return l.used === null || l.total === null || l.total <= 0 ? 1 : Math.min(l.used / l.total, 1);
}

/** `47`, `12k`, `1.2M`. */
export function count(n: number): string {
  const round = (x: number) => String(Number(x.toFixed(1)));
  if (n >= 1_000_000) return `${round(n / 1_000_000)}M`;
  if (n >= 10_000) return `${round(n / 1_000)}k`;
  return String(n);
}

/** `94 %`, `out`, or `used` when the size is not known. */
export function percent(l: Limit): string {
  if (l.level === "out") return "out";
  return `${Math.round(fraction(l) * 100)} %`;
}

/** `47 / 50`, or nothing when the size is not known. */
export function amount(l: Limit): string {
  return l.used === null || l.total === null ? "" : `${count(l.used)} / ${count(l.total)}`;
}

/** `09:00`, or `Nov 29, 09:00` when it is not today. */
export function resetText(iso: string | null, now = new Date()): string {
  if (!iso) return "";
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return "";
  const time = t.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
  return t.toDateString() === now.toDateString() ? time : `${t.toLocaleDateString([], { month: "short", day: "numeric" })}, ${time}`;
}

/** Where a limit's number comes from, in words. */
export function sourceText(l: Limit): string {
  return l.source === "kagent" ? "counted by KAgent: other apps using this key are not included" : "from the provider";
}

export function providerOfRef(ref: string): string {
  return ref.split("::")[0];
}

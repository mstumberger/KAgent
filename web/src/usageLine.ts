// The line above the composer when a limit passes 90 % (usage.md section 7): once per limit, shown again only after it
// dropped below 90 % and passed it again. Kept outside React so that announcing it is not a state set in an effect.
import { useSyncExternalStore } from "react";
import { fraction, getUsage, refreshUsage, resetText, type Limit, type UsageProvider } from "./usage";

export interface Line {
  key: string;
  text: string;
}

const announced = new Set<string>();
let line: Line | null = null;
const listeners = new Set<() => void>();

function emit(next: Line | null) {
  line = next;
  listeners.forEach((l) => l());
}

export const isOver = (l: Limit) => l.level === "nearlyOut" || l.level === "out";

/** A limit that went back under 90 % may be announced again; call with every provider after each read. */
export function settle(providers: UsageProvider[]) {
  for (const p of providers) {
    for (const key of [...announced]) {
      if (!key.startsWith(`${p.id}:`)) continue;
      const l = p.limits.find((x) => `${p.id}:${x.id}` === key);
      if (!l || !isOver(l)) announced.delete(key);
    }
  }
  if (line && !announced.has(line.key)) emit(null);
}

/** The words of the line for `limit` of provider `name`. */
export function words(name: string, l: Limit): string {
  const reset = l.resetsAt ? `, resets ${resetText(l.resetsAt)}` : "";
  return l.level === "out" ? `${name} is out of ${l.name}${reset}.` : `${name} is at ${Math.round(fraction(l) * 100)} % of ${l.name}${reset}.`;
}

/** The answering model's provider is over 90 % on `limit`: announce it if it was not yet. */
export function announce(provider: UsageProvider, limit: Limit | null) {
  if (!limit || !isOver(limit)) return;
  const key = `${provider.id}:${limit.id}`;
  if (announced.has(key)) return;
  announced.add(key);
  emit({ key, text: words(provider.name, limit) });
}

export function dismiss() {
  emit(null);
}

export function useLine(): Line | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => line,
  );
}

/** For the tests of the page: forget what was announced. */
export function reset() {
  announced.clear();
  emit(null);
}

/** `refreshUsage`, then the lines that are due. What the window calls at the start and on `usageChanged`. */
export async function readUsage() {
  await refreshUsage();
  const view = getUsage();
  if (view) settle(view.providers);
}

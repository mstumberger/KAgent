// The data side of the model chooser (chooser.md section 3): the chips, the search, the grouping by provider and
// the words a row shows. Pure functions, so the component files hold only the screen.
import type { PickerModel } from "../../picker";
import { bareId, contextLabel } from "../../settings/sections/models/present";

export type ChooserModel = PickerModel;

export const CHIPS = [
  { key: "free", label: "Free" },
  { key: "local", label: "Local" },
  { key: "vision", label: "Vision" },
  { key: "tools", label: "Tools" },
  { key: "reasoning", label: "Reasoning" },
] as const;
export type ChipKey = (typeof CHIPS)[number]["key"];

/** The id of the "Recent" entry of the provider column. */
export const RECENT = "__recent";

/** What a chip asks of a model. */
export function hasChip(m: ChooserModel, chip: ChipKey): boolean {
  switch (chip) {
    case "free":
      return !!m.free;
    case "local":
      return !!m.local;
    case "vision":
      return m.config.supportsImage;
    case "tools":
      return m.config.supportsToolCall;
    case "reasoning":
      return m.config.reasoningLevels.length > 0;
  }
}

/** The words a search term may hit: the model, its provider, and the properties it has. */
function haystack(m: ChooserModel): string {
  const words = [bareId(m.id), m.name, m.providerName, m.providerId];
  if (m.free) words.push("free");
  if (m.local) words.push("local");
  if (m.config.supportsImage) words.push("vision", "image", "images");
  if (m.config.supportsToolCall) words.push("tools");
  if (m.config.reasoningLevels.length) words.push("reasoning", ...m.config.reasoningLevels);
  return words.join(" ").toLowerCase();
}

/** Every space-separated term is in the model's words, and every chip that is on fits. */
export function matches(m: ChooserModel, query: string, chips: ReadonlySet<ChipKey>): boolean {
  for (const chip of chips) if (!hasChip(m, chip)) return false;
  const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (terms.length === 0) return true;
  const hay = haystack(m);
  return terms.every((t) => hay.includes(t));
}

export interface Group {
  id: string;
  name: string;
  local: boolean;
  /** Every model shown is free. */
  free: boolean;
  models: ChooserModel[];
}

/** The provider column: "Recent" first when it has models, then the providers in the order of the list; only models that match. */
export function groupsOf(models: ChooserModel[], query: string, chips: ReadonlySet<ChipKey>, recent: string[]): Group[] {
  const shown = models.filter((m) => matches(m, query, chips));
  const out: Group[] = [];
  for (const m of shown) {
    let g = out.find((x) => x.id === m.providerId);
    if (!g) {
      g = { id: m.providerId, name: m.providerName, local: !!m.local, free: true, models: [] };
      out.push(g);
    }
    g.models.push(m);
    if (!m.free) g.free = false;
  }
  const kept = recent.map((ref) => shown.find((m) => m.ref === ref)).filter((m): m is ChooserModel => !!m);
  if (kept.length) out.unshift({ id: RECENT, name: "Recent", local: false, free: false, models: kept });
  return out;
}

/** The id as the owner reads it. */
export const modelLabel = (m: ChooserModel): string => bareId(m.id);

export { contextLabel };

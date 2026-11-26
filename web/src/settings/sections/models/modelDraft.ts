// The draft of the model dialog (models.md section 7.5): what its fields hold, how the
// Smart switch fills and empties them, the checks of the server in its order (section
// 4), and the one request of Save. A .ts file, so the component files export components only.
import type { ModelConfigPatch, ModelRow, ResolvedModelConfig } from "../../../api";
import { bareId } from "./present";

export type Field = "id" | "contextWindow" | "maxOutputTokens" | "reasoningLevels" | "reasoningPatches";

export interface DraftError {
  field: Field;
  message: string;
}

/** What a toggle or the list of levels holds: a value that differs from the built-in one. */
export type Held = "supportsImage" | "supportsToolCall" | "reasoningLevels";

export interface ModelDraft {
  id: string;
  smart: boolean;
  /** Empty: inherit (Smart on). */
  contextWindow: string;
  maxOutputTokens: string;
  supportsImage: boolean;
  supportsToolCall: boolean;
  roles: string[];
  levels: string[];
  /** JSON text; empty: inherit. */
  mapping: string;
  held: Held[];
}

/** What `PUT /api/providers/:id` carries for the model. */
export interface SaveBody {
  addModels?: { id: string }[];
  removeModels?: string[];
  models: { id: string; enabled?: boolean; smart: boolean; roles: string[]; config: ModelConfigPatch }[];
}

export interface SavePlan {
  error?: DraftError;
  body: SaveBody;
}

/** The defaults of config.md section 3, before the rules: what a model that is not saved yet shows. */
export const DEFAULTS: ResolvedModelConfig = {
  enabled: true,
  contextWindow: 128_000,
  maxOutputTokens: 8_192,
  supportsToolCall: true,
  supportsImage: false,
  reasoningLevels: [],
  reasoningPatches: {},
  supportsLive: false,
  liveProtocol: null,
  requestPatch: {},
  roles: ["chat"],
};

export const baseOf = (inherited: ResolvedModelConfig | null): ResolvedModelConfig => inherited ?? DEFAULTS;

const HELD: Held[] = ["supportsImage", "supportsToolCall", "reasoningLevels"];
const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
const mappingText = (config: ResolvedModelConfig) => (Object.keys(config.reasoningPatches).length ? JSON.stringify(config.reasoningPatches, null, 2) : "");

/** The empty form of "Add model". */
export function emptyDraft(id = ""): ModelDraft {
  return { id, smart: true, contextWindow: "", maxOutputTokens: "", supportsImage: DEFAULTS.supportsImage, supportsToolCall: DEFAULTS.supportsToolCall, roles: [], levels: [], mapping: "", held: [] };
}

/** The stored values. Smart: the fields hold what the patch holds, the rest shows what is effective. */
export function draftOf(model: ModelRow): ModelDraft {
  const p = model.config;
  const manual = !model.smart;
  return {
    id: model.id,
    smart: model.smart,
    contextWindow: p.contextWindow !== undefined ? String(p.contextWindow) : manual ? String(model.effective.contextWindow) : "",
    maxOutputTokens: p.maxOutputTokens !== undefined ? String(p.maxOutputTokens) : manual ? String(model.effective.maxOutputTokens) : "",
    supportsImage: model.effective.supportsImage,
    supportsToolCall: model.effective.supportsToolCall,
    roles: [...model.roles],
    levels: [...model.effective.reasoningLevels],
    mapping: p.reasoningPatches ? JSON.stringify(p.reasoningPatches, null, 2) : "",
    held: HELD.filter((field) => manual || p[field] !== undefined),
  };
}

/** The Smart switch (models.md section 7.5). Off: the built-in values become the values of the fields. On: every field that equals the built-in value stops being an override. */
export function setSmart(draft: ModelDraft, smart: boolean, inherited: ResolvedModelConfig | null): ModelDraft {
  const base = baseOf(inherited);
  if (smart === draft.smart) return draft;
  if (!smart) {
    return {
      ...draft,
      smart,
      contextWindow: draft.contextWindow || String(base.contextWindow),
      maxOutputTokens: draft.maxOutputTokens || String(base.maxOutputTokens),
      mapping: draft.mapping || mappingText(base),
      held: HELD,
    };
  }
  let parsed: unknown;
  try {
    parsed = draft.mapping.trim() ? JSON.parse(draft.mapping) : undefined;
  } catch {
    parsed = undefined;
  }
  return {
    ...draft,
    smart,
    contextWindow: Number(draft.contextWindow) === base.contextWindow ? "" : draft.contextWindow,
    maxOutputTokens: Number(draft.maxOutputTokens) === base.maxOutputTokens ? "" : draft.maxOutputTokens,
    mapping: parsed !== undefined && same(parsed, base.reasoningPatches) ? "" : draft.mapping,
    held: HELD.filter((field) => !same(field === "reasoningLevels" ? draft.levels : draft[field], base[field])),
  };
}

/** One of the two toggles: an override while it differs from the built-in value. */
export function toggleOf(draft: ModelDraft, field: "supportsImage" | "supportsToolCall", inherited: ResolvedModelConfig | null): ModelDraft {
  const value = !draft[field];
  const held = value === baseOf(inherited)[field] ? draft.held.filter((f) => f !== field) : draft.held.includes(field) ? draft.held : [...draft.held, field];
  return { ...draft, [field]: value, held };
}

/** The list of levels after an edit: an override while it differs from the built-in list. */
export function levelsOf(draft: ModelDraft, levels: string[], inherited: ResolvedModelConfig | null): ModelDraft {
  const differs = !same(levels, baseOf(inherited).reasoningLevels);
  const held: Held[] = draft.held.includes("reasoningLevels") ? draft.held : [...draft.held, "reasoningLevels"];
  return { ...draft, levels, held: differs ? held : draft.held.filter((f) => f !== "reasoningLevels") };
}

const whole = (text: string): number | null => (/^\d+$/.test(text.trim()) && Number.isSafeInteger(Number(text)) && Number(text) > 0 ? Number(text) : null);
const fail = (field: Field, message: string): SavePlan => ({ error: { field, message }, body: { models: [] } });

/**
 * The checks of the server, in its order (models.md section 4), then the request. A
 * model whose id changes is replaced in the same request: the API has no other way to
 * name a new id (`addModels`, then `removeModels`).
 */
export function planSave(draft: ModelDraft, opts: { model?: ModelRow; ids: string[]; inherited: ResolvedModelConfig | null }): SavePlan {
  const id = draft.id.trim();
  const replace = !!opts.model && id !== opts.model.id;
  if (!opts.model || replace) {
    if (!id || id.length > 200 || /\s/.test(id)) return fail("id", "1 to 200 characters, without whitespace.");
    if (opts.ids.some((other) => other !== opts.model?.id && bareId(other) === bareId(id))) return fail("id", "The provider already has this model.");
  }
  const contextWindow = whole(draft.contextWindow);
  if (!draft.smart && contextWindow === null) return fail("contextWindow", "Required: a whole number above 0.");
  if (contextWindow === null && draft.contextWindow.trim() !== "") return fail("contextWindow", "A whole number above 0.");
  const maxOutputTokens = whole(draft.maxOutputTokens);
  if (!draft.smart && maxOutputTokens === null) return fail("maxOutputTokens", "Required: a whole number above 0.");
  if (maxOutputTokens === null && draft.maxOutputTokens.trim() !== "") return fail("maxOutputTokens", "A whole number above 0.");
  const levels = draft.levels.map((l) => l.trim());
  if (levels.some((l) => l === "")) return fail("reasoningLevels", "No blank level.");
  if (new Set(levels).size !== levels.length) return fail("reasoningLevels", "No repeated level.");
  // The levels the model ends with: its own list, or the built-in one.
  const effective = draft.smart && !draft.held.includes("reasoningLevels") ? baseOf(opts.inherited).reasoningLevels : levels;
  let patches: Record<string, unknown> | null = null;
  if (draft.mapping.trim() !== "") {
    let parsed: unknown;
    try {
      parsed = JSON.parse(draft.mapping);
    } catch {
      return fail("reasoningPatches", "Not JSON. Write an object: a level and the patch for it.");
    }
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return fail("reasoningPatches", "An object: a level and the patch for it.");
    patches = parsed as Record<string, unknown>;
    const unknown = Object.keys(patches).find((level) => !effective.includes(level));
    if (unknown !== undefined) return fail("reasoningPatches", `"${unknown}" is not one of the levels (${effective.join(", ") || "none"}).`);
    if (Object.values(patches).some((v) => typeof v !== "object" || v === null || Array.isArray(v))) return fail("reasoningPatches", "Every value is an object: the patch of its level.");
  }
  const held = (field: Held) => draft.held.includes(field);
  const config: ModelConfigPatch = draft.smart
    ? {
        ...(contextWindow !== null ? { contextWindow } : {}),
        ...(maxOutputTokens !== null ? { maxOutputTokens } : {}),
        ...(held("supportsToolCall") ? { supportsToolCall: draft.supportsToolCall } : {}),
        ...(held("supportsImage") ? { supportsImage: draft.supportsImage } : {}),
        ...(held("reasoningLevels") ? { reasoningLevels: levels } : {}),
        ...(patches ? { reasoningPatches: patches } : {}),
      }
    : {
        contextWindow: contextWindow ?? 0,
        maxOutputTokens: maxOutputTokens ?? 0,
        reasoningLevels: levels,
        supportsToolCall: draft.supportsToolCall,
        supportsImage: draft.supportsImage,
        ...(patches ? { reasoningPatches: patches } : {}),
      };
  return {
    body: {
      ...(replace && opts.model ? { addModels: [{ id }], removeModels: [opts.model.id] } : !opts.model ? { addModels: [{ id }] } : {}),
      models: [{ id, ...(replace && opts.model ? { enabled: opts.model.enabled } : {}), smart: draft.smart, roles: draft.roles, config }],
    },
  };
}

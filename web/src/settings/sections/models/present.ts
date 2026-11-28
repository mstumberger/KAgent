// How a model is shown in the lists (models.md sections 7.4, 7.6 and 7.8): the label
// of its context window, its id as the owner reads it, why one cannot be used, and the
// options of a picker. A .ts file, so the component files export components only.
import type { ProviderView, Settings } from "../../../api";
import type { DropdownOption } from "../../Dropdown";
import { API_FORMATS } from "./shared";

/** What the detail of a provider asks of its section: run something on a provider, then load the lists again. */
export type Run = (label: string, fn: () => Promise<string | void>) => Promise<void>;

/** The message formats, in the order of the picker: OpenAI chat completions first. */
export const API_TYPE_ORDER = ["openai-chat-completions", "anthropic-messages", "openai-responses", "ollama-native"] as const;

export function apiFormatOptions(): DropdownOption[] {
  return API_TYPE_ORDER.map((type) => ({ value: type, label: API_FORMATS[type] ?? type }));
}

/** `1M`, `128k`, `4096`. */
export function contextLabel(tokens: number): string {
  const round = (n: number) => String(Number(n.toFixed(1)));
  if (tokens >= 1_000_000) return `${round(tokens / 1_000_000)}M`;
  if (tokens >= 10_000) return `${round(tokens / 1_000)}k`;
  return String(tokens);
}

/** A model id as the owner reads it: Gemini's API answers `models/<id>`. */
export function bareId(id: string): string {
  return id.replace(/^models\//, "");
}

/** `models/x` and `x` are one model (models.md section 3). */
export function sameModel(a: string, b: string): boolean {
  return bareId(a) === bareId(b);
}

// The jobs a model id says it has (models.md section 3, first match wins): the table of `discovery.rs`.
const LIVE_IDS = ["-live", "native-audio", "realtime"];
const NO_JOB_IDS = ["embed", "tts", "transcribe", "audio", "whisper", "lyria", "imagen", "image", "banana", "veo", "robotics", "computer-use", "deep-research", "antigravity", "aqa"];

/** The roles a new row gets from its id. */
export function rolesFromId(id: string): string[] {
  const lower = id.toLowerCase();
  if (LIVE_IDS.some((k) => lower.includes(k))) return ["live"];
  if (NO_JOB_IDS.some((k) => lower.includes(k))) return [];
  return ["chat"];
}

const ROLE_KEYS = [
  ["chat", "chatModel"],
  ["fallback", "fallbackModel"],
  ["fast", "fastModel"],
  ["vision", "visionModel"],
  ["voice", "liveModel"],
] as const;

/** The jobs a model has in the settings, as the badges of its row. */
export function rolesInUse(ref: string, settings?: Settings): string[] {
  return ROLE_KEYS.filter(([, key]) => settings?.[key] === ref).map(([role]) => role);
}

/** Why a model cannot take a text job, or null when it can: what the daemon decides for `usable` (models.md section 2). */
export function unusableWhy(ref: string, providers: ProviderView[]): string | null {
  const [id, ...rest] = ref.split("::");
  const provider = providers.find((p) => p.id === id);
  const model = provider?.models.find((m) => m.id === rest.join("::"));
  if (!provider || !model) return "unknown model";
  if (!provider.enabled) return "provider switched off";
  if (!model.enabled) return "model switched off";
  if (!model.roles.includes("chat")) return "not a chat model";
  return null;
}

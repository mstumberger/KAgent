// The local models routes (local-models.md sections 1 and 3) and what they answer.
import { api } from "../../api";

export type Job = "chat" | "vision" | "embedding" | "stt";
export type Fit = "fits" | "tight" | "tooBig" | "slow";
/** The settings that can name a model. */
export type UsedFor = "chatModel" | "fastModel" | "fallbackModel" | "visionModel" | "liveModel";

export interface Pull {
  id: string;
  state: "queued" | "pulling" | "failed";
  doneMb: number;
  totalMb: number;
  error?: string;
}

export interface Gpu {
  name: string;
  vramTotalMb: number;
  vramFreeMb: number;
  source: string;
}

/** One entry of the shipped list with what this machine says about it. */
export interface Recommended {
  id: string;
  name: string;
  jobs: Job[];
  sizeMb: number;
  vramMb: number;
  context: number;
  tools: boolean;
  languages: string[];
  note: string;
  engine: "ollama" | "whisper";
  downloadable: boolean;
  fit: Fit;
  /** "<n> MB are in use by other programs at the moment". */
  fitNote: string | null;
  /** The jobs this entry is the Recommended pick of. */
  recommendedFor: Job[];
  installed: boolean;
  installedId: string | null;
  /** `provider::model`, when the Models page has a row for it. */
  ref: string | null;
  usedFor: UsedFor[];
}

export interface Installed {
  id: string;
  sizeMb: number;
  jobs: Job[];
  listed: boolean;
  ref: string | null;
  usedFor: UsedFor[];
}

/** `GET /api/local-models`. */
export interface LocalModelsView {
  ollama: { reachable: boolean; version: string | null; baseUrl: string };
  gpu: Gpu | null;
  ramTotalMb: number;
  installed: Installed[];
  recommended: Recommended[];
  pulls: Pull[];
}

export const localModelsApi = {
  view: () => api.get<LocalModelsView>("/api/local-models"),
  pull: (id: string) => api.post<{ id: string; pulls: Pull[] }>("/api/local-models/pull", { id }),
  cancel: (id: string) => api.del<void>(`/api/local-models/pull/${encodeURIComponent(id)}`),
  remove: (id: string) => api.del<void>(`/api/local-models/${encodeURIComponent(id)}`),
};

/** 3300 -> "3.2 GB", 274 -> "274 MB". */
export function size(mb: number): string {
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${Math.round(mb)} MB`;
}

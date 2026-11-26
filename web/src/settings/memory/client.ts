// The memory routes (api.md section 6) for the Memory page.
import { api } from "../../api";

export type MemoryType = "user" | "feedback" | "machine" | "project" | "reference";

/** One memory file as the daemon shows it (`MemoryView`). The core files are `user` and `machine`. */
export interface MemoryView {
  name: string;
  description: string;
  type: MemoryType;
  path: string;
  bytes: number;
  /** `YYYY-MM-DD`. */
  createdAt: string;
  updatedAt: string;
  source: "user" | "agent";
  reviewed: boolean;
  /** The last day the memory counts: `memory_forget` and `replaces` set it. */
  validUntil?: string;
  /** The memory that replaced this one. */
  supersededBy?: string;
  /** The text of the file; a search does not send it. */
  content?: string;
}

/** `GET /api/memory`. */
export interface MemoryOverview {
  /** settings.memory.enabled. */
  enabled: boolean;
  /** `user.md` and `machine.md`. */
  core: MemoryView[];
  /** The topic memories, the forgotten and replaced ones among them. */
  memories: MemoryView[];
  stats: { files: number; bytes: number; unreviewed: number };
}

export interface MemoryDraft {
  name: string;
  description: string;
  type: MemoryType;
  content: string;
}

/** What `PUT /api/memory/:name` changes; a field left out keeps its value. */
export type MemoryPatch = Partial<Pick<MemoryDraft, "description" | "type" | "content">> & { reviewed?: boolean };

export const memoryApi = {
  overview: () => api.get<MemoryOverview>("/api/memory"),
  /** A memory of the owner's: reviewed from the start. A name the owner has already: 409. */
  create: (draft: MemoryDraft) => api.post<MemoryView>("/api/memory", draft),
  /** An edit makes the memory the owner's; `reviewed` alone only marks it seen. */
  update: (name: string, patch: MemoryPatch) => api.put<MemoryView>(`/api/memory/${encodeURIComponent(name)}`, patch),
  /** Out of recall from today on; the file stays. */
  forget: (name: string) => api.post<MemoryView>(`/api/memory/${encodeURIComponent(name)}/forget`, {}),
  /** Removes the file. A core file is refused (409). */
  remove: (name: string) => api.del(`/api/memory/${encodeURIComponent(name)}`),
};

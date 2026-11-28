// The model picker's data (picker.md section 5.6): the list of the models a conversation
// may choose, the name shown for a ref, and the last choice, which a new conversation starts with.
import { api, type ModelView, type SessionMeta } from "./api";

/** A model of `GET /api/models` (`chat`): with the provider it belongs to. */
export type PickerModel = ModelView & { providerId: string; providerName: string; /** The provider is on this machine or network (the daemon decides). */ local?: boolean; /** The id ends in `-free`. */ free?: boolean };

/** What a conversation chose: a model ref and an effort, each `null` for the default. */
export interface Choice {
  model: string | null;
  reasoningLevel: string | null;
}

const KEY = "kagent.picker";
/** The list is read again when it is older than this. */
export const LIST_TTL_MS = 10_000;

let fetchedAt = 0;

/** The list, read again when it is older than 10 s (or when `force`). */
export async function list(force = false): Promise<PickerModel[] | null> {
  if (!force && Date.now() - fetchedAt < LIST_TTL_MS) return null;
  const lists = await api.get<{ chat: PickerModel[] }>("/api/models");
  fetchedAt = Date.now();
  return lists.chat;
}

/** The id of a ref, without the provider and without a leading `models/` (models.md section 7.8). */
export function label(ref: string | null | undefined): string {
  const id = (ref ?? "").split("::").slice(1).join("::") || (ref ?? "");
  return id.replace(/^models\//, "");
}

/** The last choice of the owner, or nothing chosen. */
export function last(): Choice {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "null") as Partial<Choice> | null;
    return { model: typeof v?.model === "string" ? v.model : null, reasoningLevel: typeof v?.reasoningLevel === "string" ? v.reasoningLevel : null };
  } catch {
    return { model: null, reasoningLevel: null };
  }
}

export function remember(choice: Choice) {
  try {
    localStorage.setItem(KEY, JSON.stringify(choice));
  } catch {
    // private window: the choice is not kept
  }
}

/** `PUT /api/sessions/:id/model`: a key left out keeps its value, `null` clears it. */
export function putModel(id: string, body: Partial<Choice>): Promise<SessionMeta> {
  return api.put<SessionMeta>(`/api/sessions/${id}/model`, body);
}

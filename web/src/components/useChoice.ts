// What a conversation has chosen and what answers it (picker.md section 5): read by the menu, the sheet, the composer and the widget.
import { useStore } from "../store";

/** What a conversation has chosen, and what answers it: shared by the menu, the sheet and the widget's chip. */
export function useChoice(sessionId: string | null) {
  const s = useStore();
  const meta = sessionId ? s.metas[sessionId] : undefined;
  const chatModel = s.state?.settings.chatModel ?? "";
  const chosen = meta?.model ?? null;
  const inList = !!chosen && !!s.models?.some((m) => m.ref === chosen);
  // A chosen model that cannot answer leaves the default to do it (the daemon's rule).
  const answering = chosen && inList ? chosen : chatModel;
  const levels = s.models?.find((m) => m.ref === answering)?.config.reasoningLevels ?? [];
  const level = meta?.reasoningLevel ?? null;
  const pos = level && levels.includes(level) ? levels.indexOf(level) + 1 : 0;
  const missing = !!chosen && !!s.models && !inList;
  // Nothing can answer: the default is not usable and the choice is not in the list.
  const none = !s.state?.chatModelValid && !(chosen && inList);
  return { models: s.models, chatModel, chosen, inList, answering, levels, level, pos, missing, none };
}

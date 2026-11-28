// What the sidebar does to a conversation (shell.md section 5): rename, move to a project, archive, and delete with ten
// seconds of undo. Each writes through the store's `applyMeta` and answers a refusal of the daemon with a notice.
import { sendOnExit } from "../../api";
import { putArchive, putTitle } from "../../sessionApi";
import { applyMeta, deleteSession, getStore, openSession, patch } from "../../store";
import { quiet } from "../../router/quiet";
import { pushNotice, notify } from "../toasts";
import { setProject } from "./store";
import { folderName } from "./sections";

const reason = (e: unknown) => (e instanceof Error ? e.message : String(e));
const running = (id: string) => getStore().running.has(id);

/** Give a conversation the title the owner typed (the field of the row). A refusal is a notice; the answer is whether it was saved. */
export async function renameSession(id: string, title: string): Promise<boolean> {
  const next = title.trim().replace(/\s+/g, " ");
  const row = getStore().sessions.find((s) => s.id === id);
  if (!next || next === (row?.title ?? "")) return false;
  try {
    applyMeta(id, await putTitle(id, next));
    return true;
  } catch (e) {
    notify(`Not renamed: ${reason(e)}`);
    return false;
  }
}

/** Put a conversation away or bring it back. A running one is refused here and by the daemon alike (409). */
export async function archiveSession(id: string, archived: boolean) {
  if (archived && running(id)) {
    notify("The conversation is running: stop it first.");
    return;
  }
  try {
    applyMeta(id, await putArchive(id, archived));
  } catch (e) {
    notify(`${archived ? "Not archived" : "Not restored"}: ${reason(e)}`);
  }
}

/** Move a conversation to a project (a folder), or to none (`null`). The next turn works there, which the notice says when there is history. */
export async function moveSession(id: string, path: string | null) {
  const row = getStore().sessions.find((s) => s.id === id);
  if ((row?.project ?? null) === path) return;
  try {
    const meta = await setProject(id, path);
    if ((row?.messageCount ?? 0) > 0) notify(meta.project ? `The next turn works in ${folderName(meta.project)}.` : "The next turn works with no project.", "info");
  } catch (e) {
    notify(`Not moved: ${reason(e)}`);
  }
}

// ---------------------------------------------------------------- delete with undo

/** How long the undo toast stays, and so how long a delete waits. */
export const UNDO_MS = 10_000;

/** `UNDO_MS`, unless a suite shortened it with the setting `kagent.undo-ms` of the page (the tests of the other behaviours do not wait 10 s). */
function undoMs(): number {
  try {
    return Number(localStorage.getItem("kagent.undo-ms")) || UNDO_MS;
  } catch {
    return UNDO_MS;
  }
}

/** The deletes waiting for their toast to end, by conversation. */
const waiting = new Map<string, { title: string; done: boolean }>();

const unhide = (id: string) => patch({ hiddenSessions: getStore().hiddenSessions.filter((x) => x !== id) });

/** The conversation the window shows when the open one goes: the newest of the rest. */
function nextOf(id: string): string | null {
  const s = getStore();
  return s.sessions.filter((x) => x.id !== id && !s.hiddenSessions.includes(x.id) && !x.archived && x.origin !== "subagent")[0]?.id ?? null;
}

/** The toast ended: the daemon deletes it (a refusal brings the row back with the reason). */
async function commit(id: string) {
  const w = waiting.get(id);
  if (!w || w.done) return;
  w.done = true;
  try {
    await deleteSession(id);
  } catch (e) {
    notify(`Not deleted: ${reason(e)}`);
  } finally {
    waiting.delete(id);
    unhide(id);
  }
}

/** Delete a conversation: it is hidden at once, an Undo toast stays for 10 s, and the daemon deletes it when the toast ends. */
export function deleteWithUndo(id: string) {
  const s = getStore();
  const row = s.sessions.find((x) => x.id === id);
  if (!row || waiting.has(id)) return;
  if (running(id)) {
    notify("The conversation is running: stop it first.");
    return;
  }
  const title = row.title || "New session";
  waiting.set(id, { title, done: false });
  const wasOpen = s.activeSession === id;
  const next = wasOpen ? nextOf(id) : null;
  patch({ hiddenSessions: [...s.hiddenSessions, id] });
  if (wasOpen) {
    quiet(() => patch({ activeSession: next }));
    if (next) void openSession(next);
  }
  pushNotice({
    key: null,
    level: "info",
    kind: "info",
    text: `Deleted "${title}".`,
    ms: undoMs(),
    local: {
      label: "Undo",
      run: () => {
        waiting.delete(id);
        unhide(id);
        // The one that was open is open again.
        if (wasOpen) {
          quiet(() => patch({ activeSession: id }));
          void openSession(id);
        }
      },
    },
    onEnd: () => void commit(id),
  });
}

/** The window is closing during the 10 s: the deletes are done, not lost (shell.md section 5). */
function onPageHide() {
  for (const [id, w] of waiting) {
    if (w.done) continue;
    w.done = true;
    sendOnExit("DELETE", `/api/sessions/${id}`);
  }
}

if (typeof window !== "undefined") window.addEventListener("pagehide", onPageHide);

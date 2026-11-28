// The addresses of the window (shell.md section 2): a path is a place, and a place is a path. Pure:
// no store, no history here, so the table can be read in one screen.
import { resolveView, type View } from "../nav";

/** Where the window is: a view, and what it has open. */
export interface Place {
  view: View;
  /** The settings page (`view` is `settings`). */
  section: string | null;
  /** A settings item, a ticket key or a run id, by view. */
  id: string | null;
  /** A task of the run. */
  sub: string | null;
  /** The open conversation (`view` is `chat`). */
  session: string | null;
}

export const place = (view: View, rest: Partial<Place> = {}): Place => ({ view, section: null, id: null, sub: null, session: null, ...rest });

const SAFE = /^[\w.:-]{1,128}$/;

function decode(part: string): string | null {
  try {
    const v = decodeURIComponent(part);
    return SAFE.test(v) ? v : null;
  } catch {
    return null;
  }
}

/** The place of a path; `null` for an address that is not one (the caller replaces it by the chat). */
export function parsePath(pathname: string): Place | null {
  const parts = pathname.split("/").filter(Boolean);
  const [head, a, b, extra] = parts;
  if (!head) return place("chat");
  if (parts.length > 3 || extra !== undefined) return null;
  const x = a === undefined ? null : decode(a);
  const y = b === undefined ? null : decode(b);
  if ((a !== undefined && x === null) || (b !== undefined && y === null)) return null;
  if (head === "chat") return b === undefined ? place("chat", { session: x }) : null;
  if (head === "settings") return place("settings", { section: x, id: y });
  if (head === "tickets") return b === undefined ? place("tickets", { id: x }) : null;
  if (head === "runs") return place("runs", { id: x, sub: y });
  // `/providers` and `/tools` were tabs before the settings screen: their pages are settings pages.
  const target = parts.length === 1 ? resolveView(head) : null;
  if (!target) return null;
  return target.view === "settings" ? place("settings", { section: target.section }) : place(target.view);
}

const enc = encodeURIComponent;

/** The path of a place. */
export function buildPath(p: Place): string {
  switch (p.view) {
    case "chat":
      return p.session ? `/chat/${enc(p.session)}` : "/chat";
    case "settings":
      return p.section ? `/settings/${enc(p.section)}${p.id ? `/${enc(p.id)}` : ""}` : "/settings";
    case "tickets":
      return p.id ? `/tickets/${enc(p.id)}` : "/tickets";
    case "runs":
      return p.id ? `/runs/${enc(p.id)}${p.sub ? `/${enc(p.sub)}` : ""}` : "/runs";
    default:
      return `/${p.view}`;
  }
}

/** The place of an old link: `#view=..&section=..&session=..` (KRunner and `ui show` still make them). `null` when it names nothing. */
export function fromLegacy(view: string | null, section: string | null, session: string | null): Place | null {
  const target = view ? resolveView(view, section) : null;
  if (target) return place(target.view, { section: target.section, session: target.view === "chat" ? session : null });
  return session ? place("chat", { session }) : null;
}

const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1).replace(/-/g, " ");

/** What the Back and Forward buttons call a place ("Settings: Models"). `title` is the conversation's title when known. */
export function labelOf(p: Place, title?: string): string {
  if (p.view === "chat") return title ? `Chat: ${title}` : "Chat";
  if (p.view === "settings") return p.section ? `Settings: ${cap(p.section)}` : "Settings";
  if (p.view === "tickets" && p.id) return `Tickets: ${p.id}`;
  if (p.view === "runs" && p.id) return `Runs: ${p.id}`;
  return cap(p.view);
}

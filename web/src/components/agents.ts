// The pure parts of the Agents cards (agents.md section 6): what the transcript of a
// session says about its subagents (the results of `agent_run` and `agent_wait`, the
// messages that report how one ended) and where a card sits in it. The events of the open
// page know more (the times); they win where they exist.
import type { Message } from "../api";
import type { ChildStatus, ChildView } from "../settings/subagents/api";

/** An output preview shows at most this many characters. */
export const PREVIEW_CHARS = 1500;

/** What a message says of one ended subagent: `[Agent "<title>" <status> in N s]`, `agent <id>, profile <p>`, then its output. */
interface AgentNote {
  id: string;
  title: string;
  profile: string;
  status: ChildStatus;
  seconds: number;
  output: string;
}

/** One card: the subagent and the answer the transcript has. */
export interface AgentCard extends ChildView {
  output: string;
  /** How long it ran, when only a report of its end says so. */
  seconds?: number;
}

const NOTE = /^\[Agent "(.*)" (done|failed|cancelled) in (\d+) s\]\nagent (\S+), profile (\S+)\n?([\s\S]*)$/;

/** The subagents a text reports on: one per `[Agent ...]` block (`agent_wait` answers several). */
export function agentNotes(text: string): AgentNote[] {
  return text.split(/\n(?=\[Agent ")/).flatMap((part) => {
    const m = NOTE.exec(part);
    return m ? [{ id: m[4], title: m[1], profile: m[5], status: m[2] as ChildStatus, seconds: Number(m[3]), output: m[6] }] : [];
  });
}

/** What `agent_run` answered with `background: true`: `{agentId, status, title, profile}`. */
function startedAgent(text: string): Pick<ChildView, "id" | "title" | "profile"> | null {
  try {
    const v = JSON.parse(text) as { agentId?: unknown; title?: unknown; profile?: unknown };
    return typeof v.agentId === "string" ? { id: v.agentId, title: String(v.title ?? ""), profile: String(v.profile ?? "general") } : null;
  } catch {
    return null;
  }
}

/** The subagents the transcript tells of, in the order they were started: each result of an `agent_run` call, with the state its last report gives; one without a report is `running` while the daemon says so, else `cancelled` (stopped, or cut off). */
export function agentsIn(messages: Message[], running: Set<string>): ChildView[] {
  const out: ChildView[] = [];
  const latest = new Map<string, AgentNote>();
  for (const m of messages) {
    if (m.role === "assistant") continue;
    if (m.toolName === "agent_run") {
      const note = agentNotes(m.content)[0];
      const started = note ?? startedAgent(m.content);
      if (started && !out.some((c) => c.id === started.id)) {
        out.push({ id: started.id, title: started.title, profile: started.profile, status: note ? note.status : running.has(started.id) ? "running" : "cancelled" });
      }
    }
    for (const note of agentNotes(m.content)) latest.set(note.id, note);
  }
  return out.map((c) => (latest.has(c.id) ? { ...c, status: latest.get(c.id)!.status } : c));
}

/** The cards of a session: the subagents its events told of (their times and states win), then those only its transcript tells of. Each with the answer the transcript has. */
export function agentCards(events: ChildView[], messages: Message[], running: Set<string>): AgentCard[] {
  const cards = new Map<string, AgentCard>();
  const last = new Map<string, AgentNote>();
  for (const m of messages) if (m.role !== "assistant") for (const note of agentNotes(m.content)) last.set(note.id, note);
  for (const child of agentsIn(messages, running)) {
    const note = last.get(child.id);
    cards.set(child.id, { ...child, seconds: note?.seconds, output: note?.output ?? "" });
  }
  for (const child of events) {
    const was = cards.get(child.id);
    cards.set(child.id, { output: was?.output ?? last.get(child.id)?.output ?? "", seconds: was?.seconds, ...child });
  }
  return [...cards.values()];
}

/** Where a card sits: under the message that reports the subagent (its result), else under the answer whose `agent_run` call has no result yet; `undefined`: at the end of the transcript. */
export function anchorOf(messages: Message[], child: ChildView): number | undefined {
  const reported = messages.findIndex((m) => m.role === "tool" && m.content.includes(child.id));
  if (reported >= 0) return reported;
  const pending = messages.findIndex(
    (m) => m.role === "assistant" && (m.toolCalls ?? []).some((c) => c.name === "agent_run" && !messages.some((r) => r.role === "tool" && r.toolCallId === c.id) && String((c.input as { description?: unknown } | null)?.description ?? "") === child.title),
  );
  return pending >= 0 ? pending : undefined;
}

/** The cards under each message, and those that wait at the end of the transcript. */
export function placeAgents(messages: Message[], cards: AgentCard[]): { under: Map<number, AgentCard[]>; loose: AgentCard[] } {
  const under = new Map<number, AgentCard[]>();
  const loose: AgentCard[] = [];
  for (const card of cards) {
    const at = anchorOf(messages, card);
    if (at === undefined) loose.push(card);
    else under.set(at, [...(under.get(at) ?? []), card]);
  }
  return { under, loose };
}

/** How long a subagent ran (or has been running at `now`): from the times the daemon gave, else from the report of its end. */
export function elapsedOf(child: AgentCard, now: number): string {
  const start = child.startedAt ? Date.parse(child.startedAt) : NaN;
  const end = child.endedAt ? Date.parse(child.endedAt) : NaN;
  if (!Number.isNaN(start)) {
    const until = Number.isNaN(end) || child.status === "running" ? now : end;
    return `${Math.max(0, Math.round((until - start) / 1000))} s`;
  }
  return child.seconds === undefined ? "" : `${child.seconds} s`;
}

const TASK = /^\[Task (\S+) finished, exit (\S+)\]\n?([\s\S]*)$/;

/** One report: its line as Markdown, and the output that followed it. */
export interface Report {
  head: string;
  output: string;
}

/** The reports a message holds (`agent_wait` answers several), or none when it is another message sent `Via::Agent`. The line with the id is left out: the card above shows it. */
export function reportsOf(text: string): Report[] {
  const notes = agentNotes(text);
  if (notes.length > 0) return notes.map((n) => ({ head: `**Agent "${n.title}" ${n.status} in ${n.seconds} s** (${n.profile})`, output: n.output.trim() }));
  const task = TASK.exec(text);
  if (task) return [{ head: `**Task ${task[1]} finished, exit ${task[2]}**`, output: task[3].trim() }];
  return [];
}

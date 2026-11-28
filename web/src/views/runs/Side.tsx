// The right column (workflows.md section 8): the progress of the run's tasks, or of the open worker's own steps, as a checklist,
// and the workers: the running ones with how long they run, and the ended ones behind "Ended (n)".
import { useEffect, useState } from "react";
import { useStore } from "../../store";
import type { ChildView } from "../../settings/subagents/api";
import type { ProgressStep, Run } from "./api";
import { elapsed, firstLine, workersOf, type WorkerRow } from "./model";

const NONE: ChildView[] = [];

function Checklist({ items, label }: { items: { key: string; title: string; status: ProgressStep["status"] }[]; label: string }) {
  if (items.length === 0) return <p className="rn-none">Nothing reported yet.</p>;
  return (
    <ol className="rn-check" aria-label={label}>
      {items.map((i) => (
        <li key={i.key} data-status={i.status} aria-current={i.status === "in_progress" ? "step" : undefined}>
          <span className="rn-mark" aria-hidden="true">{i.status === "completed" ? "✓" : i.status === "in_progress" ? "→" : "·"}</span>
          <span className={i.status === "completed" ? "rn-struck" : ""}>{i.title}</span>
        </li>
      ))}
    </ol>
  );
}

/** How an ended worker ended, in the words of the list. */
function endedWord(w: WorkerRow, view: ChildView | undefined, run: Run): string {
  if (view?.status === "failed") return "Failed";
  if (view?.status === "cancelled") return "Stopped";
  const t = run.tasks.find((x) => x.id === w.task);
  const round = t?.rounds.find((r) => r.round === w.round);
  if (w.role === "reviewer" && round?.verdict === "changes") return "Changes requested";
  return "Completed";
}

interface Props {
  run: Run;
  /** The task whose worker is open: its own steps are the progress. */
  task: string | null;
  onOpenWorker: (session: string, title: string) => void;
}

export default function Side({ run, task, onOpenWorker }: Props) {
  const s = useStore();
  const views = s.children[run.runSession] ?? NONE;
  const rows = workersOf(run);
  const state = (w: WorkerRow) => views.find((v) => v.id === w.session);
  const isRunning = (w: WorkerRow) => (state(w) ? state(w)?.status === "running" || state(w)?.status === "queued" : w.endedAt === null);
  const running = rows.filter(isRunning);
  const ended = rows.filter((w) => !isRunning(w)).reverse();
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (running.length === 0) return;
    const t = setInterval(() => {
      setNow(Date.now());
    }, 1000);
    return () => {
      clearInterval(t);
    };
  }, [running.length]);
  const opened = task ? run.tasks.find((t) => t.id === task) : undefined;
  const own = opened && opened.progress.length > 0;
  const done = (own ? opened.progress : run.tasks).filter((x) => ("status" in x ? x.status === "completed" : x.state === "done")).length;
  const total = own ? opened.progress.length : run.tasks.length;
  const items = own
    ? opened.progress.map((p, i) => ({ key: String(i), title: p.title, status: p.status }))
    : run.tasks.map((t) => ({ key: t.id, title: `${t.id} ${t.title}`, status: (t.state === "done" ? "completed" : t.state === "implementing" || t.state === "in_review" || t.state === "changes" ? "in_progress" : "pending") as ProgressStep["status"] }));
  return (
    <aside className="rn-side" aria-label="Progress and agents">
      <section>
        <h3>{own ? `Progress ${opened.id}` : "Tasks"} <span className="rn-count">{done}/{total}</span></h3>
        <Checklist items={items} label={own ? `Steps of ${opened.id}` : "Tasks of the run"} />
      </section>
      <section>
        <h3>Agents <span className="rn-count">{running.length} running</span></h3>
        {running.length === 0 && <p className="rn-none">No worker is running.</p>}
        {running.map((w) => (
          <button key={w.session} type="button" className="rn-agent" data-session={w.session} onClick={() => onOpenWorker(w.session, w.task || w.title)}>
            <span className="rn-dot" data-dot="run" aria-hidden="true" />
            <span className="rn-agent-title">{w.title}</span>
            <span className="rn-agent-note">Running for {elapsed(state(w)?.startedAt ?? w.startedAt, null, now)}</span>
          </button>
        ))}
        <details className="rn-ended">
          <summary>Ended ({ended.length})</summary>
          {ended.map((w) => {
            const word = endedWord(w, state(w), run);
            const report = run.tasks.find((t) => t.id === w.task)?.rounds.find((r) => r.round === w.round)?.report ?? "";
            return (
              <button key={w.session} type="button" className="rn-agent" data-session={w.session} data-ended={word} onClick={() => onOpenWorker(w.session, w.task || w.title)}>
                <span className="rn-agent-title">{w.title}</span>
                <span className="rn-agent-word">{word}</span>
                {w.role === "implementer" && report && <span className="rn-agent-note">{firstLine(report)}</span>}
                <span className="rn-agent-note">{elapsed(w.startedAt, w.endedAt, now)}</span>
              </button>
            );
          })}
        </details>
      </section>
    </aside>
  );
}

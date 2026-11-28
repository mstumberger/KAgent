// The left column (workflows.md section 8): projects, under each its runs, under the open run its tasks, each with a status dot.
// The arrow keys move between the items, Enter opens one, Left and Right fold a project.
import { useState } from "react";
import Icon from "../../components/Icon";
import type { Run } from "./api";
import { runDot, runWord, taskDot, taskWord, type Dot, type ProjectGroup } from "./model";

export function StatusDot({ dot, label }: { dot: Dot; label: string }) {
  return (
    <>
      <span className="rn-dot" data-dot={dot} aria-hidden="true" />
      <span className="rn-sr">{label}</span>
    </>
  );
}

interface Props {
  groups: ProjectGroup[];
  run: Run | null;
  selectedRun: string | null;
  selectedTask: string | null;
  onRun: (id: string) => void;
  onTask: (task: string) => void;
}

/** Move the focus among the items of the tree. */
function onKeys(e: React.KeyboardEvent<HTMLElement>) {
  const items = [...e.currentTarget.querySelectorAll<HTMLElement>("[data-ti]")];
  const at = items.indexOf(document.activeElement as HTMLElement);
  const go = (i: number) => {
    e.preventDefault();
    items[Math.max(0, Math.min(items.length - 1, i))]?.focus();
  };
  if (e.key === "ArrowDown") go(at + 1);
  else if (e.key === "ArrowUp") go(at - 1);
  else if (e.key === "Home") go(0);
  else if (e.key === "End") go(items.length - 1);
}

export default function Tree({ groups, run, selectedRun, selectedTask, onRun, onTask }: Props) {
  const [folded, setFolded] = useState<string[]>([]);
  const toggle = (p: string) => setFolded((f) => (f.includes(p) ? f.filter((x) => x !== p) : [...f, p]));
  if (groups.length === 0) return <div className="rn-none">No runs yet. Start one with New run.</div>;
  return (
    <div className="rn-tree" role="tree" aria-label="Projects, runs and tasks" onKeyDown={onKeys}>
      {groups.map((g) => {
        const open = !folded.includes(g.project);
        return (
          <div key={g.project} role="group">
            <button
              type="button"
              role="treeitem"
              aria-level={1}
              aria-expanded={open}
              data-ti=""
              className="rn-project"
              title={g.project}
              onClick={() => toggle(g.project)}
              onKeyDown={(e) => {
                if (e.key === "ArrowLeft" && open) toggle(g.project);
                if (e.key === "ArrowRight" && !open) toggle(g.project);
              }}
            >
              <Icon name="chevron" size={12} />
              <Icon name="folder" size={13} />
              <span className="rn-name">{g.name}</span>
              <span className="rn-count">{g.runs.length}</span>
            </button>
            {open &&
              g.runs.map((r) => (
                <div key={r.id} role="group">
                  <button type="button" role="treeitem" aria-level={2} aria-selected={selectedRun === r.id} data-ti="" data-run={r.id} className={`rn-run${selectedRun === r.id ? " selected" : ""}`} onClick={() => onRun(r.id)}>
                    <StatusDot dot={runDot(r)} label={r.needsOwner ? "needs you" : runWord(r.state)} />
                    <span className="rn-name">{r.title}</span>
                    <span className="rn-count">{r.done}/{r.total}</span>
                  </button>
                  {run && run.id === r.id &&
                    run.tasks.map((t) => (
                      <button key={t.id} type="button" role="treeitem" aria-level={3} aria-selected={selectedTask === t.id} data-ti="" data-task={t.id} className={`rn-task${selectedTask === t.id ? " selected" : ""}`} onClick={() => onTask(t.id)}>
                        <StatusDot dot={taskDot(t.state)} label={taskWord(t.state)} />
                        <span className="rn-id">{t.id}</span>
                        <span className="rn-name">{t.title}</span>
                        {(t.ticket ?? t.kaTicket) && <span className="rn-key">{t.ticket ?? t.kaTicket}</span>}
                      </button>
                    ))}
                </div>
              ))}
          </div>
        );
      })}
    </div>
  );
}

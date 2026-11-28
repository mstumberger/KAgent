// What the worker is doing (steering.md section 6), from the daemon's detail of it: its job, its last action, the files its
// calls named, its own todo list and the tail of its transcript. Read again when its view changes.
import { useEffect, useState } from "react";
import { agentsApi, type ChildView, type WorkerDetail } from "../../settings/subagents/api";
import { useStore } from "../../store";

/** What changes the detail: the view of the worker (its action, files, receipts, state) and its todo list. */
function signature(child: ChildView | undefined, todo: unknown): string {
  return JSON.stringify([child?.status, child?.lastAction, child?.files?.length, child?.steering?.map((s) => s.state), todo]);
}

export default function WorkerInfo({ owner, childId, child }: { owner: string; childId: string; child: ChildView | undefined }) {
  const s = useStore();
  const [detail, setDetail] = useState<WorkerDetail | null>(null);
  const sig = signature(child, s.todos[childId]);
  useEffect(() => {
    let live = true;
    agentsApi.detail(owner, childId).then(
      (d) => {
        if (live) setDetail(d);
      },
      () => {
        // a daemon without the route, or a child that is gone: the transcript below still shows
      },
    );
    return () => {
      live = false;
    };
  }, [owner, childId, sig]);
  const files = child?.files ?? detail?.child.files ?? [];
  const action = child?.lastAction ?? detail?.child.lastAction;
  const todo = detail?.todo ?? [];
  const done = todo.filter((t) => t.status === "completed").length;
  if (!detail && !action) return null;
  return (
    <div className="worker-info" aria-label="What the worker is doing">
      <div className="worker-job" title={detail?.job}>{detail?.job ?? child?.title}</div>
      {action && <div className="worker-doing" data-action-status={action.status}>{action.summary}</div>}
      <details>
        <summary>Files ({files.length})</summary>
        <ul className="worker-files">
          {files.map((f) => (
            <li key={f.path} className={f.changed ? "changed" : ""}>{f.path}</li>
          ))}
          {files.length === 0 && <li>None yet</li>}
        </ul>
      </details>
      <details>
        <summary>Todo ({todo.length === 0 ? "none" : `${done} of ${todo.length}`})</summary>
        <ul className="worker-todo">
          {todo.map((t, i) => (
            <li key={i} className={t.status === "completed" ? "done" : ""} data-status={t.status}>{t.content}</li>
          ))}
          {todo.length === 0 && <li>No list</li>}
        </ul>
      </details>
      <details>
        <summary>Recent steps ({detail?.transcript.length ?? 0})</summary>
        <ul className="worker-tail">
          {(detail?.transcript ?? []).slice(-12).map((t, i) => (
            <li key={i} data-kind={t.kind}>{t.kind === "steer" ? "[you] " : t.kind === "call" ? "> " : t.kind === "result" ? "= " : ""}{t.text}</li>
          ))}
        </ul>
      </details>
    </div>
  );
}

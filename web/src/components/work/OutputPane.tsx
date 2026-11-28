// The Output tab (work-windows.md section 5.4): a background command, its state and the tail of its
// output. While the tab is shown and the task runs it is read again every second and follows the
// end; one last read follows the end of the task. Scrolled up, following pauses and a floating
// button resumes it.
import { useEffect, useState } from "react";
import { api } from "../../api";
import Icon from "../Icon";
import { lasted, stateOf } from "./format";
import { stopTask, useWork, type BackgroundTaskView, type SideTab } from "./state";
import { useFollow } from "./useFollow";

const TAIL = 8000;
const EVERY_MS = 1000;

export default function OutputPane({ tab }: { tab: SideTab }) {
  const listed = useWork().tasks[tab.sessionId]?.find((t) => t.id === tab.taskId);
  const [task, setTask] = useState<BackgroundTaskView | null>(listed ?? null);
  const [output, setOutput] = useState("");
  const [gone, setGone] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const listedStatus = listed?.status;
  useEffect(() => {
    let alive = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const read = async () => {
      try {
        const r = await api.get<{ task: BackgroundTaskView; output: string }>(`/api/background/${tab.taskId}/output?tail=${TAIL}`);
        if (!alive) return;
        setTask(r.task);
        setOutput(r.output);
        setGone(false);
        if (r.task.status === "running") timer = setTimeout(() => void read(), EVERY_MS);
      } catch {
        // Forgotten (the registry keeps the latest 200): what was read stays.
        if (alive) setGone(true);
      }
    };
    void read();
    return () => {
      alive = false;
      if (timer) clearTimeout(timer);
    };
    // The end event of the task (the list says it ended) is one more read.
  }, [tab.taskId, listedStatus]);
  const running = task?.status === "running";
  useEffect(() => {
    if (!running) return;
    const t = setInterval(() => {
      setNow(Date.now());
    }, 1000);
    return () => {
      clearInterval(t);
    };
  }, [running]);
  const { ref, following, onScroll, resume } = useFollow<HTMLPreElement>(output);
  const state = task ? stateOf(task) : null;
  return (
    <div className="pane-output">
      <div className="pane-line" data-status={task?.status}>
        <span className="work-command" title={task?.command ?? tab.title}>
          {task?.command ?? tab.title}
        </span>
        {state && <span className={`work-state tone-${state.tone}`}>{state.text}</span>}
        {task && <span className="work-line-note work-time">{lasted(task, now)}</span>}
        {running && task && (
          <button type="button" className="danger work-stop" aria-label={`Stop ${task.command}`} onClick={() => void stopTask(task)}>
            Stop
          </button>
        )}
      </div>
      {gone && !task && <div className="work-empty">The task is gone: the daemon keeps the latest 200</div>}
      <pre className="pane-scroll pane-log" ref={ref} onScroll={onScroll} data-following={following} tabIndex={0} aria-label="Output">
        {output === "" && task ? "(no output yet)" : output}
      </pre>
      {!following && (
        <button type="button" className="pane-jump" aria-label="Jump to the end" title="Jump to the end" onClick={resume}>
          <Icon name="arrowDown" size={14} />
        </button>
      )}
    </div>
  );
}

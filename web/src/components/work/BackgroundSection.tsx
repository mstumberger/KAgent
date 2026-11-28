// The Background section (work-windows.md section 5.2): one line per background shell task of the
// open session. A click opens its Output tab (the widget has no pane); Stop ends a running one.
import { useEffect, useState } from "react";
import Icon from "../Icon";
import { lasted, stateOf } from "./format";
import { openTab, outputTab, stopTask, useWork, type BackgroundTaskView } from "./state";
import WorkSection from "./WorkSection";

const NO_TASKS: BackgroundTaskView[] = [];

export default function BackgroundSection({ sessionId, pane }: { sessionId: string; pane: boolean }) {
  const tasks = useWork().tasks[sessionId] ?? NO_TASKS;
  const running = tasks.filter((t) => t.status === "running").length;
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (running === 0) return;
    const t = setInterval(() => {
      setNow(Date.now());
    }, 1000);
    return () => {
      clearInterval(t);
    };
  }, [running]);
  return (
    <WorkSection id="background" title="Background" count={tasks.length === 0 ? undefined : running > 0 ? `${running} running` : "none running"} hasSomething={tasks.length > 0}>
      {tasks.length === 0 && <div className="work-empty">No commands in the background</div>}
      {tasks.map((t) => {
        const state = stateOf(t);
        return (
          <div className="work-line task-row" key={t.id} data-status={t.status} data-task={t.id}>
            <button type="button" className="work-line-main" disabled={!pane} title={pane ? undefined : t.command} onClick={() => openTab(outputTab(sessionId, t))}>
              <span className="work-line-icon" aria-hidden="true">
                <Icon name="terminal" size={13} />
              </span>
              <span className="work-command" title={t.command}>
                {t.command}
              </span>
              {t.status === "running" && <span className="agent-dot pulse" aria-hidden="true" />}
              <span className={`work-state tone-${state.tone}`}>{state.text}</span>
              <span className="work-line-note work-time">{lasted(t, now)}</span>
            </button>
            {t.status === "running" && (
              <button type="button" className="danger work-stop" aria-label={`Stop ${t.command}`} onClick={() => void stopTask(t)}>
                Stop
              </button>
            )}
          </div>
        );
      })}
    </WorkSection>
  );
}

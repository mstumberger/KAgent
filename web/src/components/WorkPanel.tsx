// The work panel (work-windows.md section 5.1): a capsule above the composer that counts what goes
// on in the open session (agents, commands, the plan, the todo list) and expands into sections.
// The widget draws the same capsule and, behind it, the sections in a sheet.
import { useEffect, useId, useMemo, useState } from "react";
import Icon from "./Icon";
import ActivitySection from "./work/ActivitySection";
import { useActivityCount } from "./toasts";
import AgentsSection from "./work/AgentsSection";
import BackgroundSection from "./work/BackgroundSection";
import PlanSection from "./work/PlanSection";
import ProgressSection from "./work/ProgressSection";
import { describe, hasWork, useCounts, type Counts } from "./work/counts";
import { loadTasks } from "./work/state";
import "./work.css";

interface CapsuleProps {
  counts: Counts;
  /** The notices of the conversation in the Activity section. */
  notices?: number;
  open?: boolean;
  controls?: string;
  onClick: () => void;
  className?: string;
}

/** The button: one item per kind that has something. The first two carry the pulsing dot while something runs. */
export function Capsule({ counts: c, notices = 0, open, controls, onClick, className = "" }: CapsuleProps) {
  const running = c.agents > 0 || c.commands > 0;
  const text = describe(c);
  return (
    <button type="button" className={`work-capsule ${className}`} aria-expanded={open} aria-controls={controls} aria-label={`Work: ${text}`} title={text} onClick={onClick}>
      {c.agents > 0 && (
        <span className="cap-item cap-agents">
          <Icon name="bot" size={13} />
          <b>{c.agents}</b>
          {running && <span className="agent-dot pulse" aria-hidden="true" />}
        </span>
      )}
      {c.commands > 0 && (
        <span className="cap-item cap-commands">
          <Icon name="terminal" size={13} />
          <b>{c.commands}</b>
          {running && <span className="agent-dot pulse" aria-hidden="true" />}
        </span>
      )}
      {notices > 0 && (
        <span className="cap-item cap-activity">
          <Icon name="bell" size={13} />
          <b>{notices}</b>
        </span>
      )}
      {c.plan && <span className={`cap-item cap-plan ${c.plan}`}>{c.plan}</span>}
      {c.total > 0 && (
        <span className="cap-item cap-todo">
          <Icon name="list" size={13} />
          <b>
            {c.open}/{c.total}
          </b>
        </span>
      )}
      {open !== undefined && (
        <span className={`work-chevron${open ? " open" : ""}`} aria-hidden="true">
          <Icon name="chevron" size={12} />
        </span>
      )}
    </button>
  );
}

/** The sections, for the panel and for the widget's sheet. `pane`: a line may open a tab (the full window only). */
export function WorkSections({ sessionId, pane }: { sessionId: string; pane: boolean }) {
  return (
    <>
      <AgentsSection sessionId={sessionId} pane={pane} />
      <BackgroundSection sessionId={sessionId} pane={pane} />
      <PlanSection sessionId={sessionId} pane={pane} />
      <ProgressSection sessionId={sessionId} />
      <ActivitySection sessionId={sessionId} />
    </>
  );
}

/** Above the composer of Chat. */
export default function WorkPanel({ sessionId }: { sessionId: string | null }) {
  const counts = useCounts(sessionId);
  const [open, setOpen] = useState(false);
  const id = useId();
  // The tasks of a session are read when it opens and when the panel opens (section 5.3).
  useEffect(() => {
    if (sessionId) void loadTasks(sessionId);
  }, [sessionId]);
  useEffect(() => {
    if (open && sessionId) void loadTasks(sessionId);
  }, [open, sessionId]);
  const notices = useActivityCount(sessionId);
  const visible = useMemo(() => hasWork(counts) || notices > 0, [counts, notices]);
  if (!sessionId || !visible) return null;
  return (
    <div className="work-panel-wrap">
      <Capsule counts={counts} notices={notices} open={open} controls={id} onClick={() => setOpen(!open)} />
      {open && (
        <div className="work-panel" id={id} role="region" aria-label="Work">
          <WorkSections sessionId={sessionId} pane />
        </div>
      )}
    </div>
  );
}

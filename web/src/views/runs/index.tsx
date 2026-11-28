// Runs: the board of workflow runs (workflows.md section 8). Left the projects, their runs and the tasks of the open run;
// in the middle the run in tabs; right the progress and the workers. The word "Work" is the work panel of a conversation
// (work-windows.md), so this view is called Runs. It is read again whenever the daemon says a run changed (`runUpdated`).
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import Icon from "../../components/Icon";
import SubagentPane from "../../components/work/SubagentPane";
import type { SideTab } from "../../components/work/state";
import { getStore, patch, useStore } from "../../store";
import type { ChildView } from "../../settings/subagents/api";
import { errorMessage, runsApi, type Run, type RunRow } from "./api";
import ChangesTab from "./ChangesTab";
import { groupRuns, runWord, stateLine, workerOfTask } from "./model";
import Overview from "./Overview";
import PlanTab from "./PlanTab";
import Side from "./Side";
import Tree from "./Tree";
import "./runs.css";

interface Tab {
  key: string;
  label: string;
  kind: "overview" | "plan" | "worker" | "changes";
  session?: string;
  task?: string;
}

const BASE: Tab[] = [
  { key: "overview", label: "Overview", kind: "overview" },
  { key: "plan", label: "Plan", kind: "plan" },
];

export default function Runs() {
  const s = useStore();
  const [rows, setRows] = useState<RunRow[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [run, setRun] = useState<Run | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [task, setTask] = useState<string | null>(null);
  const [tabs, setTabs] = useState<Tab[]>(BASE);
  const [active, setActive] = useState("overview");
  
  const [error, setError] = useState("");
  const selectedRef = useRef<string | null>(null);
  useEffect(() => {
    selectedRef.current = selected;
  }, [selected]);

  const readRun = useCallback((id: string) => {
    runsApi.get(id).then(
      (r) => {
        if (selectedRef.current !== id) return;
        setRun(r);
        // The workers of the run that this page did not see start (after a reload).
        api.get<{ children: ChildView[] }>(`/api/sessions/${r.runSession}/subagents`).then(
          ({ children }) => {
            const mine = getStore().children[r.runSession] ?? [];
            patch({ children: { ...getStore().children, [r.runSession]: [...children.filter((c) => !mine.some((m) => m.id === c.id)), ...mine] } });
          },
          () => {
            // a daemon without the route: the events still fill it
          },
        );
      },
      (e) => selectedRef.current === id && setError(errorMessage(e)),
    );
  }, []);

  // Read the list (and the open run) again, a moment after the last event of a burst.
  useEffect(() => {
    const t = setTimeout(() => {
      runsApi.list().then(
        (r) => {
          setRows(r.runs);
          setLoaded(true);
        },
        (e) => setError(errorMessage(e)),
      );
      if (selectedRef.current) readRun(selectedRef.current);
    }, loaded ? 80 : 0);
    return () => {
      clearTimeout(t);
    };
  }, [s.runsTick, loaded, readRun]);

  const open = (id: string) => {
    setSelected(id);
    selectedRef.current = id;
    setTask(null);
    setRun((r) => (r?.id === id ? r : null));
    const row = rows.find((x) => x.id === id);
    setTabs(BASE);
    setActive(row?.state === "awaiting_approval" ? "plan" : "overview");
    readRun(id);
  };
  const openTab = (tab: Tab) => {
    setTabs((ts) => (ts.some((t) => t.key === tab.key) ? ts : [...ts, tab]));
    setActive(tab.key);
  };
  const openWorker = (session: string, label: string) => {
    const owner = run?.tasks.find((t) => t.workers.some((w) => w.session === session));
    if (owner) setTask(owner.id);
    openTab({ key: `worker:${session}`, label: `Worker ${label}`, kind: "worker", session });
  };
  const close = (key: string) => {
    setTabs((ts) => ts.filter((t) => t.key !== key));
    if (active === key) setActive("overview");
  };
  const pickTask = (id: string) => {
    setTask(id);
    const t = run?.tasks.find((x) => x.id === id);
    const w = t ? workerOfTask(t) : null;
    if (w) openWorker(w, id);
    else setActive("plan");
  };

  // What an action of the owner answered is shown at once; the event that follows reads it again.
  const changed = (r: Run) => {
    if (selectedRef.current === r.id) setRun(r);
    const needsOwner = r.state === "awaiting_approval" || r.tasks.some((t) => t.state === "needs_owner") || r.questions.some((q) => q.state === "escalated");
    setRows((x) => x.map((y) => (y.id === r.id ? { ...y, state: r.state, reason: r.reason, done: r.tasks.filter((t) => t.state === "done").length, total: r.tasks.length, needsOwner, updatedAt: r.updatedAt } : y)));
  };

  const groups = useMemo(() => groupRuns(rows), [rows]);
  const tab = tabs.find((t) => t.key === active) ?? tabs[0];
  const sideTab = (t: Tab): SideTab => ({ key: t.key, kind: "subagent", owner: run?.runSession ?? "", sessionId: t.session ?? "", title: t.label });
  const onTab = (e: React.KeyboardEvent) => {
    if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
    const at = tabs.findIndex((t) => t.key === active);
    const next = tabs[(at + (e.key === "ArrowRight" ? 1 : tabs.length - 1)) % tabs.length];
    setActive(next.key);
    requestAnimationFrame(() => document.getElementById(`rn-tab-${next.key}`)?.focus());
  };

  return (
    <div className={`runs${selected ? " has-run" : ""}`}>
      <div className="rn-left">
        <div className="rn-head">
          <h2>Runs</h2>
        </div>
        {loaded && <Tree groups={groups} run={run} selectedRun={selected} selectedTask={task} onRun={open} onTask={pickTask} />}
      </div>
      <div className="rn-centre">
        <button type="button" className="rn-back" onClick={() => { setSelected(null); setRun(null); }}>
          <Icon name="chevron" size={12} /> Back to runs
        </button>
        {!selected ? (
          <div className="rn-empty">
            <h2>No run open</h2>
            <p className="rn-none">Open a run on the left, or start one with New run.</p>
          </div>
        ) : !run ? (
          <div className="rn-empty">{error ? <div className="rn-error" role="alert">{error}</div> : <p className="rn-none">Loading the run.</p>}</div>
        ) : (
          <>
            <div className="rn-run-head">
              <h2>{run.title}</h2>
              <span className="rn-state-badge" data-state={run.state}>{runWord(run.state)}</span>
              <span className="rn-none">{run.workflow}. {stateLine(run)}</span>
            </div>
            {error && <div className="rn-error" role="alert">{error}</div>}
            <div className="rn-tabs" role="tablist" aria-label="The run" onKeyDown={onTab}>
              {tabs.map((t) => (
                <span key={t.key} className="rn-tabwrap" data-active={t.key === active}>
                  <button type="button" role="tab" id={`rn-tab-${t.key}`} aria-selected={t.key === active} aria-controls="rn-panel" tabIndex={t.key === active ? 0 : -1} onClick={() => setActive(t.key)}>{t.label}</button>
                  {t.kind !== "overview" && t.kind !== "plan" && (
                    <button type="button" className="rn-tabx" aria-label={`Close ${t.label}`} onClick={() => close(t.key)}>
                      <Icon name="close" size={11} />
                    </button>
                  )}
                </span>
              ))}
            </div>
            <div className="rn-panel" id="rn-panel" role="tabpanel" aria-labelledby={`rn-tab-${tab.key}`}>
              {tab.kind === "overview" && <Overview run={run} onError={setError} onChanged={changed} />}
              {tab.kind === "plan" && <PlanTab run={run} onOpenWorker={openWorker} onOpenChanges={(t) => openTab({ key: `changes:${t}`, label: `Changes ${t}`, kind: "changes", task: t })} />}
              {tab.kind === "worker" && <SubagentPane key={tab.session} tab={sideTab(tab)} />}
              {tab.kind === "changes" && tab.task && <ChangesTab runId={run.id} task={tab.task} stamp={run.updatedAt} />}
            </div>
          </>
        )}
      </div>
      {run && selected && <Side run={run} task={task} onOpenWorker={openWorker} />}
    </div>
  );
}

// Runs: the board of workflow runs (workflows.md section 8). Left the projects, their runs and the tasks of the open run;
// in the middle the run in tabs; right the progress and the workers. The word "Work" is the work panel of a conversation
// (work-windows.md), so this view is called Runs. It is read again whenever the daemon says a run changed (`runUpdated`).
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import Icon from "../../components/Icon";
import SubagentPane from "../../components/work/SubagentPane";
import type { SideTab } from "../../components/work/state";
import { getStore, patch, setPlace, useStore } from "../../store";
import type { ChildView } from "../../settings/subagents/api";
import { errorMessage, runsApi, type Run, type RunRow } from "./api";
import ChangesTab from "./ChangesTab";
import { groupRuns, runWord, stateLine, workerOfTask } from "./model";
import NewRunForm from "./NewRun";
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
  // The open run and its task are the address (`/runs/<run>/<task>`, shell.md section 2): a Back or a pasted link opens them too.
  const selected = s.placeId;
  const task = s.placeSub;
  const [tabs, setTabs] = useState<Tab[]>(BASE);
  const [active, setActive] = useState("overview");
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState("");
  const selectedRef = useRef<string | null>(selected);

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

  // A different run is open (a click, a Back, an address): its tabs start again and it is read. The reset is made while rendering (the
  // state a page derives from its address), the read in the effect.
  const [seen, setSeen] = useState<string | null>(null);
  if (seen !== selected) {
    setSeen(selected);
    setRun((r) => (r?.id === selected ? r : null));
    setTabs(BASE);
    setError("");
    if (selected) {
      setCreating(false);
      setActive(rows.find((x) => x.id === selected)?.state === "awaiting_approval" ? "plan" : "overview");
    }
  }
  useEffect(() => {
    selectedRef.current = selected;
    if (selected) readRun(selected);
  }, [selected, readRun]);
  // A task named by the address (or picked in the tree) shows its worker, or the plan when it has none: once per address, so a worker
  // tab the owner closed stays closed while the run is read again.
  const [shown, setShown] = useState("");
  const taskKey = run && run.id === selected && task ? `${selected}/${task}` : "";
  if (!task && shown) setShown("");
  if (taskKey && shown !== taskKey && run) {
    const t = run.tasks.find((x) => x.id === task);
    if (t) {
      setShown(taskKey);
      const w = workerOfTask(t);
      if (!w) setActive("plan");
      else {
        const key = `worker:${w}`;
        setTabs((ts) => (ts.some((x) => x.key === key || (x.kind === "worker" && t.workers.some((z) => z.session === x.session))) ? ts : [...ts, { key, label: `Worker ${task}`, kind: "worker" as const, session: w }]));
        setActive((a) => (a.startsWith("worker:") && t.workers.some((z) => a === `worker:${z.session}`) ? a : key));
      }
    }
  }

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
    setCreating(false);
    setPlace(id, null);
  };
  const openTab = (tab: Tab) => {
    setTabs((ts) => (ts.some((t) => t.key === tab.key) ? ts : [...ts, tab]));
    setActive(tab.key);
  };
  const openWorker = (session: string, label: string) => {
    const owner = run?.tasks.find((t) => t.workers.some((w) => w.session === session));
    if (owner) setPlace(selected, owner.id);
    openTab({ key: `worker:${session}`, label: `Worker ${label}`, kind: "worker", session });
  };
  const close = (key: string) => {
    setTabs((ts) => ts.filter((t) => t.key !== key));
    if (active === key) setActive("overview");
  };
  const pickTask = (id: string) => {
    if (id !== task) return setPlace(selected, id);
    // The task is the open one already: a click shows its worker again (the tab may have been closed).
    setShown("");
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
    <div className={`runs${selected || creating ? " has-run" : ""}`}>
      <div className="rn-left">
        <div className="rn-head">
          <h2>Runs</h2>
          <button type="button" className="primary" onClick={() => setCreating(true)}>New run</button>
        </div>
        {loaded && <Tree groups={groups} run={run} selectedRun={selected} selectedTask={task} onRun={open} onTask={pickTask} />}
      </div>
      <div className="rn-centre">
        <button type="button" className="rn-back" onClick={() => { setCreating(false); setPlace(null); }}>
          <Icon name="chevron" size={12} /> Back to runs
        </button>
        {creating ? (
          <NewRunForm
            projects={[...new Set(rows.map((r) => r.project))]}
            onCancel={() => setCreating(false)}
            onStarted={(r) => {
              setCreating(false);
              setRows((x) => [{ id: r.id, title: r.title, workflow: r.workflow, project: r.project, state: r.state, reason: r.reason, done: 0, total: r.tasks.length, needsOwner: false, ticket: r.ticket, createdAt: r.createdAt, updatedAt: r.updatedAt }, ...x.filter((y) => y.id !== r.id)]);
              open(r.id);
            }}
          />
        ) : !selected ? (
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
              {tab.kind === "plan" && <PlanTab run={run} onChanged={changed} onOpenWorker={openWorker} onOpenChanges={(t) => openTab({ key: `changes:${t}`, label: `Changes ${t}`, kind: "changes", task: t })} onError={setError} />}
              {tab.kind === "worker" && <SubagentPane key={tab.session} tab={sideTab(tab)} />}
              {tab.kind === "changes" && tab.task && <ChangesTab runId={run.id} task={tab.task} stamp={run.updatedAt} />}
            </div>
          </>
        )}
      </div>
      {run && selected && !creating && <Side run={run} task={task} onOpenWorker={openWorker} />}
    </div>
  );
}

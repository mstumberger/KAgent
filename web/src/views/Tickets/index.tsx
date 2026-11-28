// Tickets: a project's list and board (tickets.md section 6). The same list the agent manages with its `tickets` tool.
// It is read again whenever the daemon says a ticket changed (`ticketChanged`).
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import { setPlace, useStore } from "../../store";
import { errorMessage, ticketsApi, type Change, type Listing, type Ticket, type TState } from "./api";
import Board from "./Board";
import Detail from "./Detail";
import List from "./List";
import { MODES, STATE_LABEL, addTarget, inProject, isMine, matches, readMode, storeMode, type Filters, type Mode } from "./model";
import "./tickets.css";

interface KnownProject {
  path: string;
  name: string;
}

/** The key `c` makes a ticket from anywhere in the view, unless a text is being typed. */
function typing(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

export default function Tickets() {
  const s = useStore();
  const [data, setData] = useState<Listing>({ projects: [], tickets: [] });
  const [known, setKnown] = useState<KnownProject[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState("");
  const [mode, setMode] = useState<Mode>(readMode);
  const [filters, setFilters] = useState<Filters>({ project: "", label: "", text: "" });
  // The open ticket is the address (`/tickets/<KEY>`, shell.md section 2).
  const selected = s.placeId;
  const setSelected = (key: string | null) => setPlace(key);
  const [adding, setAdding] = useState(false);
  const [announce, setAnnounce] = useState("");
  const newInput = useRef<HTMLInputElement>(null);

  // Read again whenever a ticket changed; an answer that arrives after a newer one is dropped.
  useEffect(() => {
    let live = true;
    ticketsApi.list().then(
      (list) => {
        if (!live) return;
        setData(list);
        setLoaded(true);
      },
      (e) => {
        if (live) setError(errorMessage(e));
      },
    );
    return () => {
      live = false;
    };
  }, [s.ticketsTick]);
  useEffect(() => {
    let live = true;
    api.get<{ projects: KnownProject[] }>("/api/projects").then(
      (r) => {
        if (live) setKnown(r.projects ?? []);
      },
      () => {
        // a daemon without the route: only the projects that have tickets
      },
    );
    return () => {
      live = false;
    };
  }, []);

  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.key !== "c" || e.ctrlKey || e.metaKey || e.altKey || e.defaultPrevented || typing(e.target)) return;
      e.preventDefault();
      setAdding(true);
    };
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("keydown", key);
    };
  }, []);
  useEffect(() => {
    if (adding) newInput.current?.focus();
  }, [adding]);

  const projects = useMemo(() => {
    const out: { value: string; label: string }[] = [{ value: "", label: "All projects" }, { value: "inbox", label: "Inbox" }];
    const seen = new Set<string>();
    for (const p of [...data.projects.filter((p) => p.path).map((p) => ({ path: p.path as string, name: p.name })), ...known]) {
      if (seen.has(p.path)) continue;
      seen.add(p.path);
      out.push({ value: p.path, label: p.name });
    }
    return out;
  }, [data.projects, known]);
  const nameOf = useCallback((t: Ticket) => (filters.project ? undefined : data.projects.find((p) => p.path === t.project)?.name ?? "Inbox"), [data.projects, filters.project]);
  const labels = useMemo(() => [...new Set(data.tickets.flatMap((t) => t.labels))].sort(), [data.tickets]);
  const shown = useMemo(() => data.tickets.filter((t) => matches(t, filters) && (mode !== "mine" || isMine(t))), [data.tickets, filters, mode]);
  const open = data.tickets.find((t) => t.key === selected) ?? null;

  const apply = (t: Ticket) => setData((d) => ({ ...d, tickets: d.tickets.map((x) => (x.key === t.key ? t : x)) }));
  const change = async (key: string, body: Change) => {
    apply(await ticketsApi.change(key, body));
  };
  const move = (key: string, state: TState) => {
    void ticketsApi.change(key, { state }).then(
      (t) => {
        apply(t);
        setAnnounce(`${key} moved to ${STATE_LABEL[state]}`);
        setError("");
      },
      (e) => setError(errorMessage(e)),
    );
  };
  const add = async (title: string, state: TState) => {
    const t = await ticketsApi.create({ title, state, project: addTarget(filters.project) });
    setData((d) => ({ ...d, tickets: [...d.tickets, t] }));
    setAnnounce(`${t.key} added to ${STATE_LABEL[state]}`);
  };
  const remove = async (key: string) => {
    await ticketsApi.remove(key);
    setData((d) => ({ ...d, tickets: d.tickets.filter((t) => t.key !== key) }));
    setSelected(null);
  };
  const choose = (m: Mode) => {
    setMode(m);
    storeMode(m);
  };
  const siblings = open ? data.tickets.filter((t) => t.project === open.project) : [];
  const projectLabel = (t: Ticket) => data.projects.find((p) => p.path === t.project)?.name ?? "Inbox";

  return (
    <div className={`tickets${open ? " with-detail" : ""}`}>
      <div className="tk-main">
        <div className="tk-head">
          <h2>Tickets</h2>
          <div className="tk-switch" role="group" aria-label="View">
            {MODES.map((m) => (
              <button key={m.id} type="button" aria-pressed={mode === m.id} onClick={() => choose(m.id)}>
                {m.label}
              </button>
            ))}
          </div>
          <span className="spacer" />
          <button type="button" className="primary" onClick={() => setAdding(true)} title="New ticket (c)">
            New ticket
          </button>
        </div>
        <div className="tk-filters">
          <select aria-label="Project" value={filters.project} onChange={(e) => setFilters({ ...filters, project: e.target.value })}>
            {projects.map((p) => (
              <option key={p.value} value={p.value}>{p.label}</option>
            ))}
          </select>
          <select aria-label="Label" value={filters.label} onChange={(e) => setFilters({ ...filters, label: e.target.value })}>
            <option value="">All labels</option>
            {labels.map((l) => (
              <option key={l} value={l}>{l}</option>
            ))}
          </select>
          <input type="search" placeholder="Filter tickets" aria-label="Filter tickets" value={filters.text} onChange={(e) => setFilters({ ...filters, text: e.target.value })} />
        </div>
        {adding && (
          <form
            className="tk-new"
            onSubmit={(e) => {
              e.preventDefault();
              const input = newInput.current;
              const title = input?.value.trim() ?? "";
              if (!title) return;
              void add(title, "todo").then(
                () => {
                  setAdding(false);
                  setError("");
                },
                (err) => setError(errorMessage(err)),
              );
            }}
            onKeyDown={(e) => {
              if (e.key === "Escape") setAdding(false);
            }}
          >
            <input ref={newInput} type="text" aria-label="New ticket title" placeholder={`New ticket in ${projects.find((p) => p.value === filters.project)?.label ?? "Inbox"}`} />
            <button type="submit" className="primary">Add</button>
            <button type="button" onClick={() => setAdding(false)}>Cancel</button>
          </form>
        )}
        {error && <div className="tk-error" role="alert">{error}</div>}
        {loaded && data.tickets.filter((t) => inProject(t, filters.project)).length === 0 && <div className="tk-none">No tickets yet. Press c to add one.</div>}
        {mode === "board" ? (
          <Board tickets={shown} selected={selected} projectName={nameOf} onOpen={setSelected} onMove={move} onAdd={add} />
        ) : (
          <List tickets={shown} only={mode === "mine" ? ["in_progress", "in_review"] : undefined} selected={selected} projectName={nameOf} onOpen={setSelected} onMove={move} onAdd={add} />
        )}
        <div className="tk-live" role="status" aria-live="polite">{announce}</div>
      </div>
      {open && <Detail key={open.key} ticket={open} siblings={siblings} projectName={projectLabel(open)} onChange={change} onDelete={remove} onClose={() => setSelected(null)} />}
    </div>
  );
}

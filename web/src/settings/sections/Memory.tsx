// Memory (memory.md section 7): the switch, the stats line, the two core files as
// editors, and the topic memories: the list on the left with search and a type filter,
// the one picked on the right.
import { useEffect, useRef, useState } from "react";
import { events } from "../../events";
import CoreEditor from "../memory/CoreEditor";
import { memoryApi, type MemoryDraft, type MemoryOverview, type MemoryView } from "../memory/client";
import MemoryDetail from "../memory/MemoryDetail";
import MemoryEditor from "../memory/MemoryEditor";
import "../memory/memory.css";
import { FILTERS, MAX_FILES, TYPE_LABEL, count, inFilter, needsReview, sizeText, sortMemories, state } from "../memory/shared";
import { ago } from "../skills/shared";
import type { SectionProps } from "../registry";
import { Badge, EmptyState, GroupCard, Row, SectionPage, Toggle } from "../ui";
import { useSettingPatch } from "../useSettingPatch";

const why = (e: unknown) => (e instanceof Error ? e.message : String(e));

const CORE_TITLE: Record<string, string> = { user: "About me", machine: "This machine" };
const CORE_HINT: Record<string, string> = {
  user: "Who you are: your preferences, the people and projects you mention, how you like your answers.",
  machine: "What no tool can find out: devices, addresses, services you run, pinned versions and why, past fixes and decisions.",
};

export default function Memory({ title, description }: SectionProps) {
  const { save, report } = useSettingPatch();
  const [overview, setOverview] = useState<MemoryOverview | null>(null);
  const [loadError, setLoadError] = useState("");
  const [round, setRound] = useState(0);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<(typeof FILTERS)[number]>("All");
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [busy, setBusy] = useState(false);
  const [serverError, setServerError] = useState<string | undefined>();
  const detailRef = useRef<HTMLElement>(null);

  // Read on opening, after a change made here (`round`), and when the daemon says a
  // memory changed (`memoryChanged`): the agent saved or forgot one, another window wrote.
  // An answer that comes late, after another read was asked for, is dropped.
  useEffect(() => {
    let live = true;
    memoryApi
      .overview()
      .then((o) => {
        if (!live) return;
        setOverview(o);
        setLoadError("");
      })
      .catch((e: unknown) => {
        if (live) setLoadError(why(e));
      });
    return () => {
      live = false;
    };
  }, [round]);

  useEffect(() => {
    const off = events.on((ev) => {
      if (ev.type === "memoryChanged") setRound((n) => n + 1);
    });
    return () => {
      off();
    };
  }, []);

  const reload = () => setRound((n) => n + 1);
  /** A write answered with the memory: its row shows the answer before the list is read again. */
  const patch = (m: MemoryView) =>
    setOverview((o) => (o ? { ...o, core: o.core.map((x) => (x.name === m.name ? m : x)), memories: o.memories.map((x) => (x.name === m.name ? m : x)) } : o));

  const memories = overview?.memories ?? [];
  const q = query.trim().toLowerCase();
  const rows = sortMemories(memories).filter((m) => inFilter(m, filter) && (!q || [m.name, m.description, m.content ?? ""].some((t) => t.toLowerCase().includes(q))));
  // Nothing is picked by itself: opening a memory the agent wrote marks it seen
  // (memory.md section 3), and that must be the owner's doing.
  const selected = creating ? null : (rows.find((m) => m.name === selectedKey) ?? null);
  const unreviewed = overview?.stats.unreviewed ?? 0;

  const switchMemory = async (on: boolean) => {
    // The switch shows the daemon's answer; a refused save leaves it where it was.
    if (await save("memory", { enabled: on })) setOverview((o) => (o ? { ...o, enabled: on } : o));
    reload();
  };

  const saveCore = (m: MemoryView) => async (content: string) => {
    const saved = await memoryApi.update(m.name, { content });
    patch(saved);
    report(`Saved ${CORE_TITLE[m.name] ?? m.name}`);
    reload();
    return saved;
  };

  /** On a narrow window the detail sits beside the top of the list: bring it into view when something is opened. */
  const showDetail = () => requestAnimationFrame(() => detailRef.current?.scrollIntoView({ block: "nearest" }));

  const startNew = () => {
    setServerError(undefined);
    setCreating(true);
    showDetail();
    // The result of an earlier action does not belong above an empty form.
    report("");
  };

  const create = async (draft: MemoryDraft) => {
    setServerError(undefined);
    setBusy(true);
    try {
      const view = await memoryApi.create(draft);
      setCreating(false);
      setSelectedKey(view.name);
      // The new memory stays in the list under the owner's eyes, whatever was looked for.
      setFilter("All");
      setQuery("");
      report(`Saved ${view.name}`);
    } catch (e) {
      setServerError(why(e));
    } finally {
      setBusy(false);
      reload();
    }
  };

  const open = (m: MemoryView) => {
    setCreating(false);
    setSelectedKey(m.name);
    showDetail();
    // Opening a memory the agent wrote marks it reviewed; an edit makes it the owner's.
    if (needsReview(m)) {
      memoryApi.update(m.name, { reviewed: true }).then(patch, (e: unknown) => report(`${m.name} could not be marked reviewed: ${why(e)}`, true));
    }
  };

  const stats = overview
    ? `${count(overview.memories.length, "memory", "memories")} in ${count(overview.stats.files, "file", "files")} (${MAX_FILES} at most), ${sizeText(overview.stats.bytes)}. ` +
      (unreviewed ? `${unreviewed} written by the agent ${unreviewed === 1 ? "is" : "are"} not reviewed.` : "Nothing written by the agent waits for review.")
    : "";

  return (
    <SectionPage
      title={title}
      description={description}
      actions={
        <button className="primary" disabled={creating} onClick={startNew}>
          New memory
        </button>
      }
    >
      <GroupCard>
        <Row label="Remember across sessions" hint="Off: nothing is recalled, and the agent's memory tools answer 'memory is off'. The files stay editable.">
          <Toggle checked={overview?.enabled ?? true} title="Remember across sessions" onChange={(v) => void switchMemory(v)} />
        </Row>
        {overview && (
          <p className="hint mem-stats" data-testid="memory-stats">
            {stats}
          </p>
        )}
        {loadError && (
          <p className="mem-error" role="alert">
            {loadError}
          </p>
        )}
      </GroupCard>

      {overview && (
        <div className="mem-core">
          {overview.core.map((m) => (
            <CoreEditor key={m.name} title={CORE_TITLE[m.name] ?? m.name} hint={CORE_HINT[m.name] ?? ""} view={m} onSave={saveCore(m)} />
          ))}
        </div>
      )}

      <div className="pv-layout ld mem-layout">
        <nav className="pv-nav" aria-label="Topic memories">
          <div className="mem-toolbar">
            <input aria-label="Search memories" placeholder="Search" value={query} onChange={(e) => setQuery(e.target.value)} />
            <select aria-label="Filter" value={filter} onChange={(e) => setFilter(e.target.value as (typeof FILTERS)[number])}>
              {FILTERS.map((f) => (
                <option key={f} value={f}>
                  {f}
                </option>
              ))}
            </select>
          </div>
          {rows.length === 0 && <div className="hint pv-nav-empty">{memories.length === 0 ? "No memories yet" : "Nothing matches"}</div>}
          {rows.map((m) => {
            const at = selected?.name === m.name;
            const gone = state(m);
            const updated = ago(m.updatedAt);
            return (
              <button
                key={m.name}
                className={`pv-nav-item mem-row${at ? " active" : ""}${gone ? " retired" : ""}`}
                aria-current={at ? "page" : undefined}
                title={m.description}
                onClick={() => open(m)}
              >
                <span className="mem-name">
                  <span className="mem-n">{m.name}</span>
                  <Badge tone="neutral">{TYPE_LABEL[m.type]}</Badge>
                  {m.source === "agent" && <Badge tone="accent">agent</Badge>}
                  {needsReview(m) && <Badge tone="warn">not reviewed</Badge>}
                  {gone && <Badge tone="neutral">{gone}</Badge>}
                </span>
                <span className="mem-desc">{m.description || "no description"}</span>
                <span className="mem-meta">updated {updated || m.updatedAt}</span>
              </button>
            );
          })}
        </nav>
        <section className="pv-detail" ref={detailRef}>
          {creating ? (
            <MemoryEditor busy={busy} existing={memories.map((m) => m.name)} serverError={serverError} onSave={create} onCancel={() => setCreating(false)} />
          ) : selected ? (
            <MemoryDetail key={selected.name} memory={selected} onChanged={reload} onPatch={patch} report={report} />
          ) : (
            <EmptyState
              title={memories.length === 0 ? "No memories yet" : "No memory picked"}
              hint={
                memories.length === 0
                  ? "Tell KAgent in chat: \"Remember that ...\". Each save asks for your approval. Or write one here."
                  : "Pick one on the left to read it, change it, forget it or delete it."
              }
              actions={
                <button className="primary" onClick={startNew}>
                  New memory
                </button>
              }
            />
          )}
        </section>
      </div>
    </SectionPage>
  );
}

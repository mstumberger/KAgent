// Skills (skills.md section 9): the library of the agent. The list on the left with a
// switch per skill, the selected skill on the right. Import and sources open dialogs.
import { useEffect, useState } from "react";
import { skillsApi, type SkillDraft, type SkillView } from "../../skillsApi";
import { prefillComposer, useStore } from "../../store";
import type { SectionProps } from "../registry";
import ImportDialog from "../skills/ImportDialog";
import SkillDetail from "../skills/SkillDetail";
import SkillEditor from "../skills/SkillEditor";
import SourcesDialog from "../skills/SourcesDialog";
import { FILTERS, SOURCE_LABEL, ago, inFilter, keyOf } from "../skills/shared";
import "../skills/skills.css";
import { Badge, EmptyState, SectionPage, Toggle } from "../ui";
import { useSettingPatch } from "../useSettingPatch";
import Select from "../Select";

const why = (e: unknown) => (e instanceof Error ? e.message : String(e));

export default function Skills({ title, description }: SectionProps) {
  const tick = useStore().skillsTick;
  const { report } = useSettingPatch();
  const [skills, setSkills] = useState<SkillView[]>([]);
  const [loadError, setLoadError] = useState("");
  const [round, setRound] = useState(0);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<(typeof FILTERS)[number]>("All");
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [dialog, setDialog] = useState<"import" | "sources" | null>(null);
  const [busy, setBusy] = useState(false);
  const [serverError, setServerError] = useState<string | undefined>();

  // Read on opening, after a change made here (`round`), and when the daemon says the
  // library changed (`skillsTick`): the agent saved a skill, a file was edited.
  useEffect(() => {
    skillsApi
      .list()
      .then((list) => {
        setSkills(list);
        setLoadError("");
      })
      .catch((e: unknown) => setLoadError(why(e)));
  }, [tick, round]);

  const reload = () => setRound((n) => n + 1);
  /** A write answered with the skill: its row shows the answer before the list is read again. */
  const patch = (skill: SkillView) => setSkills((list) => list.map((x) => (keyOf(x) === keyOf(skill) ? { ...x, ...skill } : x)));
  const existing = skills.filter((s) => !s.shadowed).map((s) => s.name);
  const q = query.trim().toLowerCase();
  const rows = skills.filter((s) => inFilter(s, filter) && (!q || s.name.toLowerCase().includes(q) || s.description.toLowerCase().includes(q)));
  // The row that was picked, else the first of the list. Nothing while a new skill is written.
  const selected = creating ? null : (rows.find((s) => keyOf(s) === selectedKey) ?? rows[0] ?? null);
  const activeKey = selected ? keyOf(selected) : "";

  const startNew = () => {
    setServerError(undefined);
    setCreating(true);
    // The result of an earlier action does not belong above an empty form.
    report("");
  };

  const create = async (draft: SkillDraft) => {
    setServerError(undefined);
    setBusy(true);
    try {
      const view = await skillsApi.create(draft);
      setCreating(false);
      setSelectedKey(keyOf(view));
      report(`Saved ${view.name}`);
    } catch (e) {
      setServerError(why(e));
    } finally {
      setBusy(false);
      reload();
    }
  };

  const switchOne = async (s: SkillView, enabled: boolean) => {
    try {
      const view = await skillsApi.setEnabled(s.name, enabled);
      setSkills((list) => list.map((x) => (keyOf(x) === keyOf(view) ? { ...x, enabled: view.enabled } : x)));
      report(`${s.name} is ${view.enabled ? "on" : "off"}`);
    } catch (e) {
      report(`${s.name} could not be switched: ${why(e)}`, true);
    }
  };

  const rescan = async () => {
    try {
      report(`Folders read again: ${(await skillsApi.rescan()).count} skills on`);
    } catch (e) {
      report(`Rescan failed: ${why(e)}`, true);
    }
    reload();
  };

  // "New skill with the agent": the chat with `/skill-creator` in the composer.
  const withAgent = () => prefillComposer("/skill-creator ").catch((e: unknown) => report(`The chat could not be opened: ${why(e)}`, true));

  // With no skill of the owner's yet (written or imported) the empty state offers to write one or to import.
  const own = skills.some((s) => s.source !== "bundled");
  const emptyTitle = !skills.length ? "No skills found" : own ? "Nothing matches" : "No skills of your own yet";
  const emptyHint = !skills.length ? "Read the folders again, write a skill, or import the skills of other tools." : own ? "Another filter, or another word, shows them again." : "Write one, or import the skills of other tools.";

  return (
    <SectionPage
      title={title}
      description={description}
      wide
      actions={
        <>
          <button disabled={creating} onClick={() => void withAgent()}>
            New skill with the agent
          </button>
          <button onClick={() => setDialog("import")}>Import</button>
          <button onClick={() => setDialog("sources")}>Add source</button>
          <button onClick={() => void rescan()}>Rescan</button>
          <button className="primary" onClick={startNew}>
            New skill
          </button>
        </>
      }
    >
      <div className="pv-layout ld sk-layout">
        <div className="pv-nav" role="listbox" aria-label="Skills">
          <div className="sk-toolbar">
            <input aria-label="Search skills" placeholder="Search skills" value={query} onChange={(e) => setQuery(e.target.value)} />
            <Select aria-label="Filter" value={filter} onChange={(e) => setFilter(e.target.value as (typeof FILTERS)[number])}>
              {FILTERS.map((f) => (
                <option key={f} value={f}>
                  {f}
                </option>
              ))}
            </Select>
          </div>
          {loadError && (
            <p className="sk-error" role="alert">
              {loadError}
            </p>
          )}
          {!loadError && rows.length === 0 && <div className="hint pv-nav-empty">{emptyTitle}</div>}
          {rows.map((s) => {
            const on = s.enabled && !s.shadowed;
            const at = keyOf(s) === activeKey;
            const updated = ago(s.updatedAt);
            return (
              <div key={keyOf(s)} className={`sk-row${at ? " active" : ""}${on ? "" : " off"}`}>
                <Toggle checked={s.enabled} disabled={s.shadowed} title={`${s.enabled ? "Switch off" : "Switch on"} ${s.name}`} onChange={(v) => void switchOne(s, v)} />
                <button
                  className="sk-select"
                  role="option"
                  aria-selected={at}
                  title={s.description}
                  onClick={() => {
                    setCreating(false);
                    setSelectedKey(keyOf(s));
                  }}
                >
                  <span className="sk-name">
                    <span className="sk-n">{s.name}</span>
                    <Badge tone={s.source === "user" ? "accent" : "neutral"}>{SOURCE_LABEL[s.source]}</Badge>
                    {s.source === "imported" && !s.enabled && <Badge tone="warn">off</Badge>}
                    {s.shadowed && <Badge tone="neutral">hidden</Badge>}
                  </span>
                  <span className="sk-desc">{s.description || "no description"}</span>
                  <span className="sk-meta">
                    {updated && <span>updated {updated}</span>}
                    <span>
                      used {s.uses} {s.uses === 1 ? "time" : "times"}
                    </span>
                  </span>
                </button>
              </div>
            );
          })}
        </div>
        <section className="pv-detail">
          {creating ? (
            <SkillEditor busy={busy} existing={existing} serverError={serverError} onSave={create} onCancel={() => setCreating(false)} />
          ) : selected ? (
            <SkillDetail key={keyOf(selected)} skill={selected} existing={existing} onChanged={reload} onPatch={patch} onSelect={setSelectedKey} report={report} />
          ) : (
            <EmptyState
              title={emptyTitle}
              hint={emptyHint}
              actions={
                own ? (
                  <>
                    <button className="primary" onClick={startNew}>
                      New skill
                    </button>
                    {skills.length > 0 && <button onClick={() => void withAgent()}>New skill with the agent</button>}
                  </>
                ) : (
                  <>
                    <button className="primary" onClick={startNew}>
                      New skill
                    </button>
                    <button onClick={() => setDialog("import")}>Import</button>
                  </>
                )
              }
            />
          )}
        </section>
      </div>
      {dialog === "import" && <ImportDialog onClose={() => setDialog(null)} onImported={reload} report={report} />}
      {dialog === "sources" && <SourcesDialog onClose={() => setDialog(null)} onChanged={reload} report={report} />}
    </SectionPage>
  );
}

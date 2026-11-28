// Subagents (agents.md section 6): the profiles a subagent runs with, and how many work at
// once. The list on the left (the built-ins marked, the files that are not valid profiles
// with their reason), the selected profile on the right: a form, or read-only for a built-in.
import { api } from "../../api";
import { useStore } from "../../store";
import { useEffect, useState, type FormEvent } from "react";
import type { SectionProps } from "../registry";
import { Badge, Confirm, EmptyState, Field, GroupCard, ListDetail, SectionPage } from "../ui";
import { useDraft } from "../useDraft";
import { useSettingPatch } from "../useSettingPatch";
import { agentsApi, type AgentList, type AgentProfileView, type InvalidProfile, type ProfileBody, type SubagentsSettings } from "../subagents/api";
import { draftOf, emptyDraft } from "../subagents/draft";
import type { PickerModel } from "../../picker";
import ProfileEditor from "../subagents/ProfileEditor";
import "../subagents/subagents.css";

/** One row of the list: a profile, or a file that is not one. */
type Row = { key: string; profile?: AgentProfileView; broken?: InvalidProfile };

const why = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** A built-in profile as the daemon defines it: read-only here. */
const BUILTIN: Record<string, { groups: string; note: string }> = {
  general: { groups: "Every enabled group", note: "Every tool the main agent has." },
  explore: { groups: "Every enabled group, read-only calls only", note: "Read-only research: any call whose assessment is not read-only is refused with \"explore is read-only\"." },
};

function BuiltIn({ profile }: { profile: AgentProfileView }) {
  const def = BUILTIN[profile.name];
  return (
    <div className="sb-detail" data-profile={profile.name}>
      <div className="sb-detail-head">
        <h3>{profile.name}</h3>
        <Badge tone="neutral">Built-in</Badge>
      </div>
      <p className="hint">{def?.note ?? profile.description} It cannot be changed or deleted.</p>
      <dl className="sb-facts">
        <dt>Description</dt>
        <dd>{profile.description}</dd>
        <dt>Model</dt>
        <dd>The chat model</dd>
        <dt>Tool groups</dt>
        <dd>{def?.groups ?? "Every enabled group"}</dd>
        <dt>Max iterations</dt>
        <dd>{profile.maxIterations}</dd>
      </dl>
    </div>
  );
}

/** A file of the folder that is not a valid profile: the reason, and the way out. */
function Broken({ file, onDelete }: { file: InvalidProfile; onDelete: () => void }) {
  return (
    <div className="sb-detail" data-broken={file.file}>
      <div className="sb-detail-head">
        <h3>{file.file}</h3>
        <Badge tone="bad">Not used</Badge>
      </div>
      <p className="hint" role="alert">
        The daemon skips this file: {file.reason}
      </p>
      <p className="hint">Fix the file in the agents folder of KAgent's config (it is read again on every request), or delete it here.</p>
      <div className="row">
        <button className="danger" onClick={onDelete}>
          Delete file
        </button>
      </div>
    </div>
  );
}

export default function Subagents({ title, description }: SectionProps) {
  const { save, busy, report } = useSettingPatch();
  const settings = useStore().state?.settings;
  const section = settings?.subagents as SubagentsSettings | undefined;
  const [max, setMax, maxDirty] = useDraft(String(section?.maxConcurrent ?? 4));
  const maxOk = Number.isInteger(Number(max)) && Number(max) >= 1 && Number(max) <= 8;
  const [list, setList] = useState<AgentList | null>(null);
  const [loadError, setLoadError] = useState("");
  const [round, setRound] = useState(0);
  const [selected, setSelected] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [working, setWorking] = useState(false);
  const [serverError, setServerError] = useState<string | undefined>();
  const [confirmDelete, setConfirmDelete] = useState<{ name: string; label: string } | null>(null);
  const [models, setModels] = useState<PickerModel[]>([]);
  const [groups, setGroups] = useState<string[]>([]);

  // Read on opening, after a change made here (`round`).
  useEffect(() => {
    let open = true;
    agentsApi.list().then(
      (next) => {
        if (open) {
          setList(next);
          setLoadError("");
        }
      },
      (e: unknown) => {
        if (open) setLoadError(why(e));
      },
    );
    return () => {
      open = false;
    };
  }, [round]);

  // The models and the tool groups the form offers.
  useEffect(() => {
    let open = true;
    void api.get<Record<string, PickerModel[]>>("/api/models").then(
      (m) => {
        if (open) setModels(m.chat ?? []);
      },
      () => {},
    );
    void api.get<{ tools: { group: string }[] }>("/api/tools").then(
      (r) => {
        if (open) setGroups([...new Set(r.tools.map((t) => t.group))].filter((g) => g !== "other").sort());
      },
      () => {},
    );
    return () => {
      open = false;
    };
  }, []);

  const agents = list?.agents ?? [];
  const invalid = list?.invalid ?? [];
  const rows: Row[] = [...agents.map((profile) => ({ key: profile.name, profile })), ...invalid.map((broken) => ({ key: `file:${broken.file}`, broken }))];
  // The row that was picked, else the first of the list. Nothing while a new profile is written.
  const picked = creating ? null : (rows.find((r) => r.key === selected) ?? rows[0] ?? null);
  const pickedGroups = [...new Set([...groups, ...(picked?.profile?.toolGroups ?? [])])].sort();

  const startNew = () => {
    setServerError(undefined);
    setCreating(true);
    report("");
  };
  const cancelNew = () => {
    setCreating(false);
    setServerError(undefined);
  };

  /** A write: its result goes to the header, then the list is read again. Why it failed, or null when it worked. */
  const act = async (what: string, run: () => Promise<string>): Promise<string | null> => {
    setWorking(true);
    try {
      report(await run());
      return null;
    } catch (e) {
      report(`${what} failed: ${why(e)}`, true);
      return why(e);
    } finally {
      setWorking(false);
      setRound((n) => n + 1);
    }
  };
  const saveProfile = async (name: string, body: ProfileBody) => {
    setServerError(undefined);
    // What the daemon refuses shows above the form too (the first browser pass found it missing).
    const failed = await act("Saving", async () => {
      await agentsApi.save(name, body);
      return `Saved ${name}`;
    });
    if (failed === null) {
      setCreating(false);
      setSelected(name);
    } else {
      setServerError(failed);
    }
  };
  const remove = async () => {
    const target = confirmDelete;
    setConfirmDelete(null);
    if (!target) return;
    const failed = await act(`Deleting ${target.label}`, async () => {
      await agentsApi.remove(target.name);
      return `Deleted ${target.label}`;
    });
    if (failed === null) setSelected(null);
  };

  const saveMax = (e: FormEvent) => {
    e.preventDefault();
    if (maxDirty && maxOk) void save("subagents", { maxConcurrent: Number(max) });
  };

  const renderRow = (row: Row) =>
    row.broken ? (
      <span className="sb-row broken">
        <span className="sb-name">
          {row.broken.file}
          <Badge tone="bad">not valid</Badge>
        </span>
        <span className="sb-desc">{row.broken.reason}</span>
      </span>
    ) : (
      <span className="sb-row">
        <span className="sb-name">
          {row.profile?.name}
          {row.profile?.builtIn && <Badge tone="neutral">built-in</Badge>}
        </span>
        <span className="sb-desc">{row.profile?.description}</span>
      </span>
    );

  const detail = creating ? (
    <ProfileEditor busy={working} groups={pickedGroups} initial={emptyDraft()} models={models} onSave={saveProfile} onCancel={cancelNew} serverError={serverError} taken={agents.map((a) => a.name)} />
  ) : picked?.profile ? (
    picked.profile.builtIn ? (
      <BuiltIn profile={picked.profile} />
    ) : (
      <ProfileEditor
        key={picked.profile.name}
        busy={working}
        editing
        groups={pickedGroups}
        initial={draftOf(picked.profile)}
        models={models}
        onDelete={() => setConfirmDelete({ name: picked.profile?.name ?? "", label: `profile '${picked.profile?.name}'` })}
        onSave={saveProfile}
        onCancel={() => setSelected(null)}
        serverError={serverError}
        taken={[]}
      />
    )
  ) : picked?.broken ? (
    <Broken file={picked.broken} onDelete={() => setConfirmDelete({ name: picked.broken?.file.slice(0, -3) ?? "", label: picked.broken?.file ?? "" })} />
  ) : (
    <EmptyState title="No profiles" hint="The built-ins are always there. A new profile is a file in the agents folder of KAgent's config." />
  );

  return (
    <SectionPage title={title} description={description}>
      <GroupCard title="Running at once" description="How many subagents work at the same time. More wait for a free slot and are shown as queued.">
        <form className="ui-form sb-max" onSubmit={saveMax}>
          <Field label="At most" hint="1 to 8. The default is 4.">
            <input type="number" min={1} max={8} step={1} aria-label="Subagents at once" value={max} disabled={!settings} onChange={(e) => setMax(e.target.value)} />
          </Field>
          <div className="row">
            <button className="primary" type="submit" disabled={!maxDirty || !maxOk || busy || !settings}>
              Save
            </button>
            {maxDirty && !maxOk && <span className="ui-inline-bad">A whole number from 1 to 8.</span>}
          </div>
        </form>
      </GroupCard>
      <ListDetail
        actions={
          <button className="primary" disabled={creating} onClick={startNew}>
            New profile
          </button>
        }
        detail={detail}
        empty={loadError ? "The daemon did not answer." : "No profiles found."}
        items={rows}
        keyOf={(row) => row.key}
        label="Subagent profiles"
        onSelect={(row) => {
          setCreating(false);
          setServerError(undefined);
          setSelected(row.key);
        }}
        renderRow={renderRow}
        selectedKey={picked?.key ?? null}
      />
      {loadError && (
        <p className="sb-error" role="alert">
          {loadError}
        </p>
      )}
      {confirmDelete && (
        <Confirm
          body={confirmDelete.label === confirmDelete.name ? `The profile '${confirmDelete.name}' is deleted from the agents folder. A subagent that runs with it now goes on.` : `The file ${confirmDelete.label} is deleted from the agents folder.`}
          confirmLabel="Delete"
          danger
          onCancel={() => setConfirmDelete(null)}
          onConfirm={() => void remove()}
          title="Delete this profile?"
        />
      )}
    </SectionPage>
  );
}

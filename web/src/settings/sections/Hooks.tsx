// Hooks (hooks.md section 7): the commands of the owner that run at the points of the
// agent's life, grouped by event. Each row: the command, what it runs for, whether it runs
// in the background, a switch, Test, Edit, Delete. The form and the test panel open above
// the list.
import { api } from "../../api";
import { useEffect, useState } from "react";
import type { SectionProps } from "../registry";
import { Badge, Confirm, EmptyState, GroupCard, SectionPage, Toggle } from "../ui";
import { useSettingPatch } from "../useSettingPatch";
import { EVENTS, hooksApi, type Hook, type HookEventName, type HookList } from "../hooks/api";
import { draftOf, emptyDraft, lineOf } from "../hooks/draft";
import HookEditor from "../hooks/HookEditor";
import TestPanel from "../hooks/TestPanel";
import "../hooks/hooks.css";

const why = (e: unknown) => (e instanceof Error ? e.message : String(e));
/** On unless the record says `false`. */
const on = (hook: Hook) => hook.enabled !== false;
const idOf = (hook: Hook) => hook.id ?? lineOf(hook);

type Editing = { mode: "new" } | { mode: "change"; hook: Hook };

/** The events of the list in the order of the page; one the page does not know comes last. */
const groupsOf = (hooks: Hook[]): { event: HookEventName; when: string; hooks: Hook[] }[] => [
  ...EVENTS.map((info) => ({ event: info.name, when: info.when, hooks: hooks.filter((h) => h.event === info.name) })),
  ...hooks.filter((h) => !EVENTS.some((info) => info.name === h.event)).map((h) => ({ event: h.event, when: "", hooks: [h] })),
].filter((group) => group.hooks.length > 0);

export default function Hooks({ title, description }: SectionProps) {
  const { report } = useSettingPatch();
  const [list, setList] = useState<HookList | null>(null);
  const [loadError, setLoadError] = useState("");
  const [round, setRound] = useState(0);
  const [tools, setTools] = useState<string[]>([]);
  const [editing, setEditing] = useState<Editing | null>(null);
  const [testing, setTesting] = useState<Hook | null>(null);
  const [working, setWorking] = useState(false);
  const [serverError, setServerError] = useState<string | undefined>(undefined);
  const [confirmDelete, setConfirmDelete] = useState<Hook | null>(null);

  // Read on opening, and after a change made here (`round`).
  useEffect(() => {
    let open = true;
    hooksApi.list().then(
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

  // The tools the matcher offers.
  useEffect(() => {
    let open = true;
    void api
      .get<{ tools: { contract: { name: string } }[] }>("/api/tools")
      .then((r) => {
        if (open) setTools([...new Set(r.tools.map((t) => t.contract.name))].sort());
      })
      .catch(() => {});
    return () => {
      open = false;
    };
  }, []);

  const hooks = list?.hooks ?? [];
  const groups = groupsOf(hooks);

  const startNew = () => {
    setServerError(undefined);
    setTesting(null);
    setEditing({ mode: "new" });
    report("");
  };
  const startEdit = (hook: Hook) => {
    setServerError(undefined);
    setTesting(null);
    setEditing({ mode: "change", hook });
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
  const save = async (body: Hook) => {
    setServerError(undefined);
    const failed = await act("Saving", async () => {
      if (editing?.mode === "change") {
        await hooksApi.update(editing.hook.id ?? "", body);
        return `Saved ${lineOf(body)}`;
      }
      await hooksApi.create(body);
      return `Added ${lineOf(body)}`;
    });
    if (failed === null) {
      setEditing(null);
    } else {
      // What the daemon refuses shows above the form too.
      setServerError(failed);
    }
  };
  const toggle = (hook: Hook) =>
    act(`Switching ${idOf(hook)}`, async () => {
      await hooksApi.update(hook.id ?? "", { ...hook, enabled: !on(hook) });
      return `${on(hook) ? "Switched off" : "Switched on"} ${idOf(hook)}`;
    });
  const remove = async () => {
    const target = confirmDelete;
    setConfirmDelete(null);
    if (!target) return;
    await act(`Deleting ${idOf(target)}`, async () => {
      await hooksApi.remove(target.id ?? "");
      return `Deleted ${idOf(target)}`;
    });
  };

  const startTest = (hook: Hook) => {
    setServerError(undefined);
    setEditing(null);
    setTesting(hook);
  };

  const row = (hook: Hook) => (
    <div className="hk-row" key={idOf(hook)} data-hook-id={hook.id} data-enabled={on(hook)}>
      <div className="hk-main">
        <code className="hk-cmd">{lineOf(hook)}</code>
        <div className="hk-meta">
          {hook.matcher && hook.matcher.length > 0 ? (
            <>
              <span>for</span>
              {hook.matcher.map((tool) => (
                <code key={tool}>{tool}</code>
              ))}
            </>
          ) : (
            <span>{hook.event === "PreToolUse" || hook.event === "PostToolUse" ? "every tool" : ""}</span>
          )}
          {hook.async ? <Badge tone="accent">background</Badge> : null}
          {!on(hook) ? <Badge tone="neutral">off</Badge> : null}
        </div>
      </div>
      <div className="hk-actions">
        <Toggle checked={on(hook)} title={`${idOf(hook)} on or off`} disabled={working} onChange={() => void toggle(hook)} />
        <button type="button" className="hk-test-button" disabled={working} onClick={() => startTest(hook)}>
          Test
        </button>
        <button type="button" disabled={working} onClick={() => startEdit(hook)}>
          Edit
        </button>
        <button type="button" className="danger" disabled={working} onClick={() => setConfirmDelete(hook)}>
          Delete
        </button>
      </div>
    </div>
  );
  return (
    <SectionPage
      title={title}
      description={description}
      actions={
        <button className="primary" type="button" disabled={editing !== null} onClick={startNew}>
          New hook
        </button>
      }
    >
      <p className="hk-warning" role="note">
        Hooks run with your user's privileges while the agent works. Changes apply from the next turn.
      </p>
      {editing && (
        <GroupCard title={editing.mode === "new" ? "New hook" : `Change ${idOf(editing.hook)}`}>
          <HookEditor
            key={editing.mode === "new" ? "new" : idOf(editing.hook)}
            busy={working}
            enabled={editing.mode === "new" ? true : on(editing.hook)}
            initial={editing.mode === "new" ? emptyDraft() : draftOf(editing.hook)}
            serverError={serverError}
            tools={tools}
            onCancel={() => setEditing(null)}
            onSave={save}
          />
        </GroupCard>
      )}
      {testing && <GroupCard title={`Test ${idOf(testing)}`}>{list && <TestPanel hook={testing} samples={list.samples} onClose={() => setTesting(null)} />}</GroupCard>}
      {!editing && !testing && hooks.length === 0 ? (
        <EmptyState
          actions={
            <button className="primary" type="button" onClick={startNew}>
              New hook
            </button>
          }
          hint="A hook is a command of yours that runs when the agent starts a session, gets a message, calls a tool, answers or raises an alert. It gets a JSON description of the moment on stdin and may answer with a decision."
          title="No hooks"
        />
      ) : (
        groups.map((group) => (
          <GroupCard description={group.when} key={group.event} title={group.event}>
            {group.hooks.map(row)}
          </GroupCard>
        ))
      )}
      {loadError && (
        <p className="hk-error" role="alert">
          {loadError}
        </p>
      )}
      {confirmDelete && (
        <Confirm
          body={`The hook ${idOf(confirmDelete)} is deleted from the settings. The agent goes on without it.`}
          confirmLabel="Delete"
          danger
          onCancel={() => setConfirmDelete(null)}
          onConfirm={() => void remove()}
          title="Delete this hook?"
        />
      )}
    </SectionPage>
  );
}

// Local models (local-models.md section 4): what this machine can run, the recommended models
// per job with a fit verdict, and downloads through Ollama. KAgent installs nothing but models,
// and only when you press Download.
import { useCallback, useEffect, useRef, useState } from "react";
import { events } from "../../events";
import type { SectionProps } from "../registry";
import { Confirm, EmptyState, GroupCard, SectionPage } from "../ui";
import { useSettingPatch } from "../useSettingPatch";
import { localModelsApi, type LocalModelsView, type Recommended } from "../local-models/api";
import Group from "../local-models/Group";
import { JOBS } from "../local-models/jobs";
import Machine from "../local-models/Machine";
import type { Actions } from "../local-models/Row";
import "../local-models/local-models.css";

const reason = (e: unknown) => (e instanceof Error ? e.message : String(e));

export default function LocalModels({ title, description }: SectionProps) {
  const { save, report } = useSettingPatch();
  const [view, setView] = useState<LocalModelsView | null>(null);
  const [busy, setBusy] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [removing, setRemoving] = useState<Recommended | null>(null);
  const known = useRef<string[]>([]);

  const read = useCallback(async () => {
    try {
      const next = await localModelsApi.view();
      known.current = next.pulls.map((p) => p.id);
      setView(next);
      setLoadError(null);
    } catch (e) {
      setLoadError(reason(e));
    }
  }, []);

  // The machine when the page opens; the downloads as the daemon reports them, and the whole
  // answer again when a download is over (the installed models changed).
  useEffect(() => {
    let open = true;
    const load = () =>
      localModelsApi.view().then(
        (next) => {
          if (!open) return;
          known.current = next.pulls.map((p) => p.id);
          setView(next);
          setLoadError(null);
        },
        (e) => {
          if (open) setLoadError(reason(e));
        },
      );
    void load();
    const off = events.on((ev) => {
      if (ev.type !== "localModelsChanged" || !open) return;
      const ids = ev.pulls.map((p) => p.id);
      const ended = known.current.some((id) => !ids.includes(id));
      known.current = ids;
      setView((v) => (v ? { ...v, pulls: ev.pulls } : v));
      if (ended) void load();
    });
    return () => {
      open = false;
      off();
    };
  }, []);

  /** One action of the page: a failure goes to the header. */
  const act = async (what: string, run: () => Promise<unknown>, done?: string) => {
    setBusy(true);
    try {
      await run();
      if (done) report(done);
    } catch (e) {
      report(`${what} failed: ${reason(e)}`, true);
    } finally {
      setBusy(false);
      await read();
    }
  };

  const actions: Actions = {
    busy,
    download: (id) => void act("Download", () => localModelsApi.pull(id)),
    cancel: (id) => void act("Cancel", () => localModelsApi.cancel(id)),
    remove: (m) => setRemoving(m),
    use: (setting, ref) => void act("Saving", async () => void (await save(setting, ref)), setting === "visionModel" ? "Vision model set" : setting === "chatModel" ? "Brain set" : "Fallback set"),
  };

  const refresh = (
    <button type="button" disabled={busy} onClick={() => void read()}>
      Refresh
    </button>
  );

  return (
    <SectionPage title={title} description={description} actions={refresh}>
      {loadError && !view && <EmptyState title="Could not read this machine" hint={loadError} actions={refresh} />}
      {view && !view.ollama.reachable && (
        <GroupCard title="Ollama is not running" description="The models run in Ollama. KAgent does not install it.">
          <div className="lm-help">
            <p>
              Nothing answered at <code>{view.ollama.baseUrl}</code>. Start Ollama, then press Refresh:
            </p>
            <pre>systemctl start ollama{"\n"}# or, in a terminal:{"\n"}ollama serve</pre>
            <p className="hint">Not installed yet? Get it from ollama.com and follow its steps for your system. KAgent only downloads models, and only when you press Download.</p>
          </div>
        </GroupCard>
      )}
      {view?.ollama.reachable && (
        <>
          <Machine view={view} />
          {JOBS.map((j) => (
            <Group key={j.job} {...j} view={view} actions={actions} />
          ))}
          <GroupCard title="Other installed models" description="Managed in Settings -> Models.">
            {view.installed.filter((i) => !i.listed).length === 0 ? (
              <p className="hint lm-none">No other models are installed in Ollama.</p>
            ) : (
              <ul className="lm-others">
                {view.installed
                  .filter((i) => !i.listed)
                  .map((i) => (
                    <li key={i.id}>
                      <code>{i.id}</code>
                      <span className="hint">{i.sizeMb >= 1024 ? `${(i.sizeMb / 1024).toFixed(1)} GB` : `${i.sizeMb} MB`}</span>
                    </li>
                  ))}
              </ul>
            )}
          </GroupCard>
        </>
      )}
      {removing && (
        <Confirm
          title={`Remove ${removing.name}?`}
          body={`${removing.name} is deleted from this machine (${removing.id}). You can download it again later.`}
          confirmLabel="Remove"
          danger
          onCancel={() => setRemoving(null)}
          onConfirm={() => {
            const id = removing.id;
            setRemoving(null);
            void act("Remove", () => localModelsApi.remove(id), `${removing.name} removed`);
          }}
        />
      )}
    </SectionPage>
  );
}

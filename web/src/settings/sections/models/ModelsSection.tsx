// Models: the providers (list, catalog, connection, models) and the two cards for what
// the models do: their roles and the advanced options.
import { useEffect, useState } from "react";
import Icon from "../../../components/Icon";
import { api, type ProviderView, type Template } from "../../../api";
import { useStore } from "../../../store";
import type { SectionProps } from "../../registry";
import { EmptyState, ListDetail, SectionPage } from "../../ui";
import { useSettingPatch } from "../../useSettingPatch";
import AddProvider from "./AddProvider";
import { Logo } from "./Catalog";
import ProviderDetail from "./ProviderDetail";
import Advanced from "./Advanced";
import Roles from "./Roles";
import { STATUS_LABEL, statusOf } from "./shared";

export default function ModelsSection({ title, description }: SectionProps) {
  const settings = useStore().state?.settings;
  const { report } = useSettingPatch();
  const [templates, setTemplates] = useState<Template[]>([]);
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [models, setModels] = useState<{ ref: string; config?: { reasoningLevels?: string[] } }[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [working, setWorking] = useState(false);
  const [round, setRound] = useState(0);

  // Everything is read again when `round` moves: after a provider changed.
  useEffect(() => {
    void api.get<{ templates: Template[] }>("/api/providers/templates").then((t) => setTemplates(t.templates));
    void api.get<Record<string, { ref: string }[]>>("/api/models").then((m) => setModels(m.chat ?? []));
    void api.get<{ providers: ProviderView[] }>("/api/providers").then((p) => {
      setProviders(p.providers);
      setSelectedId((cur) => (p.providers.some((x) => x.id === cur) ? cur : (p.providers[0]?.id ?? null)));
      setAdding((cur) => cur || p.providers.length === 0);
    });
  }, [round]);

  /** Run something on a provider, then load again and show the result in the header. */
  const run = async (label: string, fn: () => Promise<string | void>) => {
    setWorking(true);
    try {
      report((await fn()) || `${label} done`);
    } catch (e) {
      report(`${label} failed: ${e instanceof Error ? e.message : String(e)}`, true);
    } finally {
      setRound((n) => n + 1);
      setWorking(false);
    }
  };

  const add = (templateId: string, apiKey: string) =>
    void run("Add provider", async () => {
      const r = await api.post<{ id: string }>("/api/providers", { templateId, apiKey: apiKey || undefined });
      setAdding(false);
      setSelectedId(r.id);
      if (apiKey || templateId === "ollama") await api.post(`/api/providers/${r.id}/fetch-models`).catch(() => undefined);
      return "Provider added";
    });

  const selected = providers.find((p) => p.id === selectedId) ?? null;

  return (
    <SectionPage
      title={title}
      description={description}
      actions={
        <button disabled={working} onClick={() => setRound((n) => n + 1)}>
          Refresh
        </button>
      }
    >
      <ListDetail
        label="Providers"
        items={providers}
        keyOf={(p) => p.id}
        selectedKey={adding ? null : (selected?.id ?? null)}
        onSelect={(p) => {
          setSelectedId(p.id);
          setAdding(false);
        }}
        renderRow={(p) => (
          <>
            <Logo name={p.name} />
            <span className="pv-nav-name">{p.name}</span>
            <span className={`pv-dot ${statusOf(p)}`} title={STATUS_LABEL[statusOf(p)]} />
          </>
        )}
        empty="No providers yet"
        actions={
          adding ? undefined : (
            <div className="ld-actions">
              <button className="ld-add" onClick={() => setAdding(true)}>
                <Icon name="plus" size={14} />
                Add provider
              </button>
            </div>
          )
        }
        detail={
          adding ? (
            <AddProvider templates={templates} existing={providers} busy={working} onAdd={add} onCancel={providers.length ? () => setAdding(false) : undefined} />
          ) : selected ? (
            <ProviderDetail key={selected.id} p={selected} template={templates.find((t) => t.id === selected.templateId)} settings={settings} busy={working} run={run} />
          ) : (
            <EmptyState title="No provider" hint="Add one to give KAgent a model." />
          )
        }
      />
      <Roles models={models} />
      <Advanced models={models} />
    </SectionPage>
  );
}

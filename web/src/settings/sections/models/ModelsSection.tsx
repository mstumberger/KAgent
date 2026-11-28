// Models (models.md section 7): the owner's providers on the left and what is done to
// one of them on the right, in one card; the defaults under it.
import { useEffect, useState } from "react";
import Icon from "../../../components/Icon";
import { api, type NewProvider, type ProviderView, type Template } from "../../../api";
import type { PickerModel } from "../../../picker";
import { useStore } from "../../../store";
import type { SectionProps } from "../../registry";
import { EmptyState, SectionPage } from "../../ui";
import { useSettingPatch } from "../../useSettingPatch";
import AddProvider from "./AddProvider";
import Defaults from "./Defaults";
import ProviderCard from "./ProviderCard";
import ProviderList from "./ProviderList";
import type { Run } from "./present";

export default function ModelsSection({ title, description }: SectionProps) {
  const settings = useStore().state?.settings;
  const { report } = useSettingPatch();
  const [templates, setTemplates] = useState<Template[]>([]);
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [chat, setChat] = useState<PickerModel[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [working, setWorking] = useState(false);
  const [round, setRound] = useState(0);

  // Everything is read again when `round` moves: after a provider changed.
  useEffect(() => {
    void api.get<{ templates: Template[] }>("/api/providers/templates").then((t) => setTemplates(t.templates));
    void api.get<Record<string, PickerModel[]>>("/api/models").then((m) => setChat(m.chat ?? []));
    void api.get<{ providers: ProviderView[] }>("/api/providers").then((p) => {
      setProviders(p.providers);
      setSelectedId((cur) => (p.providers.some((x) => x.id === cur) ? cur : (p.providers[0]?.id ?? null)));
      setAdding((cur) => cur || p.providers.length === 0);
    });
  }, [round]);
  const reload = () => setRound((n) => n + 1);

  /** Run something on a provider, then load again and show the result in the header. */
  const run: Run = async (label, fn) => {
    setWorking(true);
    try {
      report((await fn()) || `${label} done`);
    } catch (e) {
      report(`${label} failed: ${e instanceof Error ? e.message : String(e)}`, true);
    } finally {
      reload();
      setWorking(false);
    }
  };

  const add = (request: NewProvider) =>
    void run("Add provider", async () => {
      const r = await api.post<ProviderView>("/api/providers", request);
      setAdding(false);
      setSelectedId(r.id);
      if (request.apiKey || request.templateId === "ollama") await api.post(`/api/providers/${r.id}/fetch-models`).catch(() => undefined);
      return "Provider added";
    });

  const selected = providers.find((p) => p.id === selectedId) ?? null;

  return (
    <SectionPage
      title={title}
      description={description}
      wide
      actions={
        <>
          <button disabled={working} onClick={reload}>
            Refresh
          </button>
          <button disabled={working || adding} onClick={() => setAdding(true)}>
            <Icon name="plus" size={14} />
            Add provider
          </button>
        </>
      }
    >
      <div className="pv-layout ld">
        <ProviderList
          providers={providers}
          selectedId={adding ? null : selectedId}
          busy={working}
          adding={adding}
          onSelect={(id) => {
            setSelectedId(id);
            setAdding(false);
          }}
          onAdd={() => setAdding(true)}
          onReorder={(order) => void run("Reorder providers", async () => void (await api.put("/api/providers/order", { order })))}
        />
        <section className="pv-detail">
          {adding ? (
            <AddProvider templates={templates} existing={providers} busy={working} onAdd={add} onCancel={providers.length ? () => setAdding(false) : undefined} />
          ) : selected ? (
            <ProviderCard key={selected.id} p={selected} template={templates.find((t) => t.id === selected.templateId)} settings={settings} busy={working} run={run} reload={reload} />
          ) : (
            <EmptyState title="No provider" hint="Add one to give KAgent a model." />
          )}
        </section>
      </div>
      <Defaults chat={chat} providers={providers} />
    </SectionPage>
  );
}

// The models of a provider (models.md section 7.4): a row each, with what it is used
// for, its badges, Test, the pencil for the dialog, and the trash. A drag on the
// handle reorders the rows; "Add model" opens the dialog for a model that is new.
import { useState } from "react";
import { ApiError, api, type ModelRow, type ProviderView, type Settings, type Template } from "../../../api";
import Icon from "../../../components/Icon";
import { Badge, Confirm, Toggle } from "../../ui";
import { useDragList } from "../../dragList";
import ModelDialog from "./ModelDialog";
import { bareId, contextLabel, rolesInUse, type Run } from "./present";

type TestResult = { ok: boolean; latencyMs: number; error?: string; reply?: string };

export default function ModelList({
  p,
  template,
  settings,
  busy,
  run,
  reload,
}: {
  p: ProviderView;
  template?: Template;
  settings?: Settings;
  busy: boolean;
  run: Run;
  reload: () => void;
}) {
  const [filter, setFilter] = useState("");
  const [onlyEnabled, setOnlyEnabled] = useState(p.models.length > 15);
  const [askDelete, setAskDelete] = useState<ModelRow | null>(null);
  const [notRemoved, setNotRemoved] = useState("");
  // The open dialog: a model's, or `null` for "Add model".
  const [dialog, setDialog] = useState<{ model?: ModelRow } | null>(null);
  const ids = p.models.map((m) => m.id);
  const shown = p.models.filter((m) => (!onlyEnabled || m.enabled) && m.id.toLowerCase().includes(filter.toLowerCase()));
  // Rows are dragged only when every row is on the page.
  const reorderable = filter === "" && shown.length === p.models.length;
  const drag = useDragList(
    () => ids,
    (order) => void run("Reorder models", async () => void (await api.put(`/api/providers/${p.id}`, { modelOrder: order }))),
  );

  const remove = async (m: ModelRow) => {
    try {
      await api.put(`/api/providers/${p.id}`, { removeModels: [m.id] });
      setAskDelete(null);
      reload();
    } catch (e) {
      // A 409 says which setting still names the model.
      setNotRemoved(e instanceof ApiError ? e.message : String(e));
    }
  };

  return (
    <div className="pv-models">
      <div className="pv-models-head">
        <h4>
          Models <span className="hint">{p.models.filter((m) => m.enabled).length} of {p.models.length} enabled</span>
        </h4>
        <input className="pv-search small" placeholder="Filter models" aria-label="Filter models" value={filter} onChange={(e) => setFilter(e.target.value)} />
        <label className="check-inline">
          <input type="checkbox" checked={onlyEnabled} onChange={(e) => setOnlyEnabled(e.target.checked)} /> Enabled only
        </label>
        <button className="pv-mini" disabled={busy} onClick={() => setDialog({})}>
          <Icon name="plus" size={14} />
          Add model
        </button>
      </div>
      {p.models.length === 0 && <p className="hint">No models yet: Fetch models, or add one.</p>}
      <ul className="pv-model-list">
        {shown.map((m) => {
          const ref = `${p.id}::${m.id}`;
          const liveOnly = m.roles.length > 0 && m.roles.every((r) => r === "live");
          return (
            <li key={m.id} {...drag.row(m.id, `pv-model${m.enabled ? "" : " off"}`, reorderable)}>
              {reorderable ? (
                <button {...drag.handle(m.id, bareId(m.id))}>
                  <Icon name="handle" size={14} />
                </button>
              ) : (
                <span className="pv-grip-space" />
              )}
              <Toggle
                checked={m.enabled}
                disabled={busy}
                title={m.enabled ? "Disable model" : "Enable model"}
                onChange={(v) => void run("Model", async () => void (await api.put(`/api/providers/${p.id}`, { models: [{ id: m.id, enabled: v }] })))}
              />
              <span className="pv-model-id" title={m.id}>
                {bareId(m.id)}
              </span>
              <Badge tone="neutral">{contextLabel(m.effective.contextWindow)}</Badge>
              {m.effective.supportsImage && (
                <span className="pv-icon" title="Reads images">
                  <Icon name="image" size={14} />
                </span>
              )}
              {rolesInUse(ref, settings).map((role) => (
                <Badge key={role} tone="accent">
                  {role}
                </Badge>
              ))}
              <span className="spacer" />
              {!liveOnly && (
                <button
                  className="pv-mini"
                  disabled={busy}
                  onClick={() =>
                    void run("Test", async () => {
                      const r = await api.post<TestResult>(`/api/providers/${p.id}/test`, { modelId: m.id });
                      if (!r.ok) throw new Error(r.error ?? "no reply");
                      return `${bareId(m.id)}: ${r.latencyMs} ms, "${(r.reply ?? "").slice(0, 60)}"`;
                    })
                  }
                >
                  Test
                </button>
              )}
              <button className="pv-mini pv-iconbtn" disabled={busy} title="Edit model" aria-label={`Edit ${bareId(m.id)}`} onClick={() => setDialog({ model: m })}>
                <Icon name="pencil" size={14} />
              </button>
              <button
                className="pv-mini pv-iconbtn danger"
                disabled={busy}
                title="Remove model"
                aria-label={`Remove ${bareId(m.id)}`}
                onClick={() => {
                  setNotRemoved("");
                  setAskDelete(m);
                }}
              >
                <Icon name="trash" size={14} />
              </button>
            </li>
          );
        })}
        {shown.length === 0 && p.models.length > 0 && <li className="hint">No model matches.</li>}
      </ul>

      {dialog && (
        <ModelDialog
          mode={dialog.model ? "edit" : "new"}
          model={dialog.model}
          provider={p}
          template={template}
          onClose={() => setDialog(null)}
          onSaved={() => {
            setDialog(null);
            reload();
          }}
        />
      )}
      {askDelete && (
        <Confirm
          title={`Remove ${bareId(askDelete.id)}?`}
          body={`The model is removed from ${p.name}. Its settings go with it.${notRemoved ? ` Not removed: ${notRemoved}` : ""}`}
          confirmLabel="Remove"
          danger
          onCancel={() => setAskDelete(null)}
          onConfirm={() => void remove(askDelete)}
        />
      )}
    </div>
  );
}

// The model dialog (models.md section 7.5): the Smart switch, the model's id, its
// context window and output limit; under "Advanced settings" what it reads, its jobs,
// its reasoning levels and their mapping. With Smart on a field is empty unless the
// owner overrode it, and the placeholder is the built-in value.
import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import "./model-dialog.css";
import { api, type ModelRow, type ProviderView, type Template } from "../../../api";
import Dialog from "../../dialog";
import { Toggle } from "../../ui";
import LevelsEditor from "./LevelsEditor";
import { draftOf, emptyDraft, levelsOf, planSave, setSmart, toggleOf, type DraftError, type Field, type ModelDraft } from "./modelDraft";
import { rolesFromId } from "./present";

// The fields under the block that opens by itself when one of them is wrong.
const ADVANCED: Field[] = ["reasoningLevels", "reasoningPatches"];

export default function ModelDialog({
  mode,
  model,
  provider,
  template,
  onClose,
  onSaved,
}: {
  mode: "edit" | "new";
  /** Left out for "Add model". */
  model?: ModelRow;
  provider: ProviderView;
  template?: Template;
  onClose: () => void;
  onSaved: () => void;
}) {
  const isNew = mode === "new" || !model;
  // What the rules give the model. A model that is not saved yet has no such values.
  const inherited = model ? model.inherited : null;
  const [initial] = useState<ModelDraft>(() => (model ? draftOf(model) : emptyDraft()));
  const [draft, setDraft] = useState<ModelDraft>(initial);
  const [advanced, setAdvanced] = useState(false);
  const [error, setError] = useState<DraftError | null>(null);
  const [refused, setRefused] = useState("");
  const [saving, setSaving] = useState(false);
  const form = useRef<HTMLFormElement>(null);
  const formId = useId();
  const advancedId = useId();
  const errId = useId();
  // The roles of a new model follow its id until the owner chooses them.
  const rolesChosen = useRef(false);

  const update = (next: ModelDraft) => {
    setDraft(next);
    setError(null);
    setRefused("");
  };
  // A wrong field gets the focus.
  useEffect(() => {
    if (!error) return;
    form.current?.querySelector<HTMLElement>(`[data-field="${error.field}"]`)?.focus();
  }, [error]);

  const save = async (e?: FormEvent) => {
    e?.preventDefault();
    const plan = planSave(draft, { model, ids: provider.models.map((m) => m.id), inherited });
    if (plan.error) {
      if (ADVANCED.includes(plan.error.field)) setAdvanced(true);
      setError(plan.error);
      return;
    }
    setSaving(true);
    try {
      await api.put(`/api/providers/${provider.id}`, plan.body);
      onSaved();
    } catch (e) {
      setRefused(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  const wrong = (field: Field) => (error?.field === field ? error.message : "");
  const described = (field: Field) => (wrong(field) ? { "aria-invalid": true, "aria-describedby": `${errId}-${field}` } : {});
  const message = (field: Field) => (wrong(field) ? <span className="ui-inline-bad" id={`${errId}-${field}`}>{wrong(field)}</span> : null);
  const override = (on: boolean) => (on && draft.smart ? <span className="md-tag">override</span> : null);
  const idReadOnly = !isNew && !!template && [...template.models, ...template.liveModels].includes(model.id);
  const setLevels = (levels: string[]) => update(levelsOf(draft, levels, inherited));
  /** A number of the basic block: empty means inherit (Smart on). */
  const numberField = (field: "contextWindow" | "maxOutputTokens", label: string, text: string, built: number | undefined) => (
    <div className="md-field-wrap">
      <label className={`md-field${text.trim() && draft.smart ? " md-override" : ""}`}>
        <span className="md-label">{label}</span>
        <span className="md-input">
          <input type="text" inputMode="numeric" data-field={field} placeholder={draft.smart ? (built !== undefined ? String(built) : "inherit") : undefined} value={text} onChange={(e) => update({ ...draft, [field]: e.target.value })} {...described(field)} />
          {override(text.trim() !== "")}
        </span>
      </label>
      {message(field)}
    </div>
  );

  return (
    <Dialog
      title={isNew ? "Add model" : "Edit model"}
      wide
      onClose={onClose}
      footer={
        <>
          <button
            type="button"
            disabled={saving}
            onClick={() => {
              setDraft(initial);
              setError(null);
              setRefused("");
            }}
          >
            Reset
          </button>
          <span className="spacer" />
          {refused && (
            <span className="ui-inline-bad" role="alert">
              Not saved: {refused}
            </span>
          )}
          <button type="button" disabled={saving} onClick={onClose}>
            Cancel
          </button>
          <button type="submit" form={formId} className="primary" disabled={saving}>
            Save
          </button>
        </>
      }
    >
      <form ref={form} id={formId} className="md-form" onSubmit={(e) => void save(e)}>
        <div className="md-smart">
          <Toggle label="Smart configuration" checked={draft.smart} onChange={(v) => update(setSmart(draft, v, inherited))} />
          <span className="hint">Take the built-in values; only what you change here is saved.</span>
        </div>

        <div className="md-field-wrap">
          <label className="md-field">
            <span className="md-label">Model ID</span>
            <span className="md-input">
              <input
                data-field="id"
                autoFocus={isNew}
                readOnly={idReadOnly}
                title={idReadOnly ? "The id the provider's template lists (read only)" : undefined}
                placeholder={isNew ? "The id the provider's API takes" : undefined}
                value={draft.id}
                onChange={(e) => {
                  const id = e.target.value;
                  update(rolesChosen.current || !isNew ? { ...draft, id } : { ...draft, id, roles: rolesFromId(id) });
                }}
                {...described("id")}
              />
            </span>
          </label>
          {message("id")}
        </div>
        {numberField("contextWindow", "Context window", draft.contextWindow, inherited?.contextWindow)}
        {numberField("maxOutputTokens", "Max output tokens", draft.maxOutputTokens, inherited?.maxOutputTokens)}

        <div className="md-advanced">
          <button type="button" className="md-advanced-toggle" aria-expanded={advanced} aria-controls={advancedId} onClick={() => setAdvanced(!advanced)}>
            <svg className={`md-chevron${advanced ? " open" : ""}`} width={14} height={14} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M9 6l6 6-6 6" />
            </svg>
            Advanced settings
          </button>
          {advanced && (
            <div className="md-advanced-body" id={advancedId}>
              <div className="md-toggles">
                <div className="md-toggle">
                  <Toggle label="Accepts images" checked={draft.supportsImage} onChange={() => update(toggleOf(draft, "supportsImage", inherited))} />
                  {override(draft.held.includes("supportsImage"))}
                </div>
                <div className="md-toggle">
                  <Toggle label="Supports tool calls" checked={draft.supportsToolCall} onChange={() => update(toggleOf(draft, "supportsToolCall", inherited))} />
                  {override(draft.held.includes("supportsToolCall"))}
                </div>
              </div>

              <div className="md-block" role="group" aria-label="Roles">
                <span className="md-label">Roles</span>
                <div className="chips">
                  {["chat", "live"].map((role) => (
                    <label key={role} className={`chip${draft.roles.includes(role) ? " on" : ""}`}>
                      <input
                        type="checkbox"
                        checked={draft.roles.includes(role)}
                        onChange={() => {
                          rolesChosen.current = true;
                          update({ ...draft, roles: draft.roles.includes(role) ? draft.roles.filter((r) => r !== role) : [...draft.roles, role] });
                        }}
                      />{" "}
                      {role}
                    </label>
                  ))}
                </div>
                <span className="hint">chat: text turns, and the fast, fallback and vision jobs. live: Gemini Live voice.</span>
              </div>

              <div className="md-block" data-field="reasoningLevels" tabIndex={-1} role="group" aria-label="Reasoning levels" {...described("reasoningLevels")}>
                <span className="md-label">Reasoning levels</span>
                {draft.levels.length === 0 && <span className="hint">{draft.smart ? "The built-in list is empty: add a level to give it one." : "None."}</span>}
                <LevelsEditor levels={draft.levels} onChange={setLevels} />
                {message("reasoningLevels")}
              </div>

              <div className="md-block">
                <label className={`md-field${draft.mapping.trim() && draft.smart ? " md-override" : ""}`}>
                  <span className="md-label">Reasoning parameter mapping</span>
                  <span className="md-input">
                    <textarea
                      className="md-json"
                      data-field="reasoningPatches"
                      spellCheck={false}
                      placeholder={draft.smart ? (inherited && Object.keys(inherited.reasoningPatches).length ? JSON.stringify(inherited.reasoningPatches, null, 2) : "inherit") : undefined}
                      value={draft.mapping}
                      onChange={(e) => update({ ...draft, mapping: e.target.value })}
                      {...described("reasoningPatches")}
                    />
                    {override(draft.mapping.trim() !== "")}
                  </span>
                </label>
                <span className="hint">An object: a level and the patch of the request body sent for it.</span>
                {message("reasoningPatches")}
              </div>
            </div>
          )}
        </div>
      </form>
    </Dialog>
  );
}

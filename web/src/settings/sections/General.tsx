// General: who KAgent is, how it starts, and how it may act as root.
import type { FormEvent } from "react";
import { useStore } from "../../store";
import type { SectionProps } from "../registry";
import { Badge, Field, GroupCard, Row, SectionPage, Toggle } from "../ui";
import { useDraft } from "../useDraft";
import { useSettingPatch } from "../useSettingPatch";
import Select from "../Select";

const PERSONA = { name: "KAgent", address: "", language: "auto" };

export default function General({ title, description }: SectionProps) {
  const s = useStore();
  const settings = s.state?.settings;
  const { save } = useSettingPatch();
  const [persona, setPersona, personaDirty] = useDraft(settings?.persona ?? PERSONA);
  const [text, setText, textDirty] = useDraft(settings?.startup?.greeting?.text ?? "");
  const installed = s.state?.privHelper === "installed";

  const savePersona = (e: FormEvent) => {
    e.preventDefault();
    if (personaDirty) void save("persona", persona);
  };

  return (
    <SectionPage title={title} description={description}>
      <GroupCard title="Persona" description="The name KAgent uses, how it addresses you, and the language it answers in.">
        <form className="ui-form" onSubmit={savePersona}>
          <div className="ui-form-grid">
            <Field label="Name">
              <input value={persona.name} onChange={(e) => setPersona({ ...persona, name: e.target.value })} />
            </Field>
            <Field label="Address me as">
              <input value={persona.address} onChange={(e) => setPersona({ ...persona, address: e.target.value })} />
            </Field>
            <Field label="Language">
              <Select value={persona.language} onChange={(e) => setPersona({ ...persona, language: e.target.value })}>
                <option value="auto">follow me (Slovenian/English)</option>
                <option value="sl">Slovenian</option>
                <option value="en">English</option>
              </Select>
            </Field>
          </div>
          <div className="row">
            <button className="primary" type="submit" disabled={!personaDirty}>
              Save persona
            </button>
          </div>
        </form>
      </GroupCard>

      <GroupCard title="Startup" description="What KAgent does when it starts.">
        <Row label="Start with my session" hint="Keeps the systemd user unit enabled; no root needed.">
          <Toggle checked={settings?.startup?.autostart ?? true} title="Start with my session" onChange={(v) => void save("startup", { autostart: v })} />
        </Row>
        <Row label="Greet me" hint="A desktop notification when the daemon is up.">
          <Toggle checked={settings?.startup?.greeting?.enabled ?? true} title="Greet me" onChange={(v) => void save("startup", { greeting: { enabled: v } })} />
        </Row>
        <Field label="Greeting text" hint="Empty = auto: time of day and your name.">
          <input value={text} placeholder="Good evening. KAgent is up and watching." onChange={(e) => setText(e.target.value)} />
        </Field>
        <div className="row">
          <button className="primary" disabled={!textDirty} onClick={() => void save("startup", { greeting: { text } })}>
            Save greeting
          </button>
        </div>
      </GroupCard>

      <GroupCard title="Window">
        <Toggle
          label="Start as widget"
          checked={settings?.startWidget ?? true}
          onChange={(v) => void save("startWidget", v)}
        />
        <p className="hint">Open the small always-on-top widget when KAgent starts, instead of the full window.</p>
      </GroupCard>

      <GroupCard title="Root helper" description="Root actions need it: sysctl, modules, system services, packages, SMART, eBPF.">
        <Row label="Status">
          <Badge tone={installed ? "ok" : "bad"}>{s.state ? s.state.privHelper : "unknown"}</Badge>
        </Row>
        {!installed && (
          <p className="hint">
            Run <code>kagentd sudoers</code> in a terminal and paste the printed commands. KAgent never runs sudo itself.
          </p>
        )}
        <Row label="Privilege method" hint="How KAgent runs a root action.">
          <Select aria-label="Privilege method" value={settings?.privilege ?? "sudo"} onChange={(e) => void save("privilege", e.target.value)}>
            <option value="sudo">sudo -n (sudoers rule, no prompt)</option>
            <option value="pkexec">pkexec (password prompt each time)</option>
            <option value="none">none (no root actions)</option>
          </Select>
        </Row>
      </GroupCard>
    </SectionPage>
  );
}

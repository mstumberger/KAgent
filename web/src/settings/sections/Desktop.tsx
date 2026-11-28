// Desktop and apps: the editor that opens code, and the folders that hold projects.
import { useStore } from "../../store";
import type { SectionProps } from "../registry";
import { Field, GroupCard, Row, SectionPage } from "../ui";
import { useDraft } from "../useDraft";
import { useSettingPatch } from "../useSettingPatch";
import ScreenWatchCard from "../screen-watch/Card";
import Select from "../Select";

const EDITORS: [string, string][] = [
  ["auto", "Auto (the project's own, then the first installed)"],
  ["vscode", "VS Code"],
  ["kate", "Kate"],
  ["idea", "IntelliJ IDEA"],
  ["pycharm", "PyCharm"],
  ["webstorm", "WebStorm"],
  ["phpstorm", "PhpStorm"],
  ["clion", "CLion"],
  ["zed", "Zed"],
];

export default function Desktop({ title, description }: SectionProps) {
  const settings = useStore().state?.settings;
  const { save } = useSettingPatch();
  const [roots, setRoots, dirty] = useDraft((settings?.desktop?.projectRoots ?? []).join("\n"));
  const lines = roots
    .split("\n")
    .map((x) => x.trim())
    .filter(Boolean);

  return (
    <SectionPage title={title} description={description}>
      <GroupCard title="Editor" description="The editor that opens when you ask to see code.">
        <Row label="Editor">
          <Select
            aria-label="Editor"
            value={settings?.desktop?.editor || "auto"}
            onChange={(e) => void save("desktop", { editor: e.target.value === "auto" ? "" : e.target.value })}
          >
            {EDITORS.map(([id, name]) => (
              <option key={id} value={id}>
                {name}
              </option>
            ))}
          </Select>
        </Row>
      </GroupCard>

      <GroupCard title="Project folders" description="Their subfolders are the projects KAgent may name.">
        <Field label="Project folders" hint="One per line. Empty: ~/SystemInterface, ~/projects, ~/src and the other built-in folders.">
          <textarea rows={5} value={roots} placeholder={"~/SystemInterface\n~/projects"} onChange={(e) => setRoots(e.target.value)} />
        </Field>
        <div className="row">
          <button className="primary" disabled={!dirty} onClick={() => void save("desktop", { projectRoots: lines })}>
            Save folders
          </button>
        </div>
      </GroupCard>

      <ScreenWatchCard />
    </SectionPage>
  );
}

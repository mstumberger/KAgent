// Roles: which model steps in when the chat model cannot (fallback), reads the screen
// (vision) and does the short jobs (fast: session titles, and it summarizes long
// conversations). The chat model itself is set on its row.
import { useStore } from "../../../store";
import { GroupCard, Row } from "../../ui";
import { useSettingPatch } from "../../useSettingPatch";

export default function Roles({ models }: { models: { ref: string }[] }) {
  const settings = useStore().state?.settings;
  const { save } = useSettingPatch();

  /** The options of a role: `placeholder` for none, the models, and a current one that is not among them. */
  const options = (current: string, placeholder: string) => (
    <>
      <option value="">{placeholder}</option>
      {current && !models.some((m) => m.ref === current) && <option value={current}>{current}</option>}
      {models.map((m) => (
        <option key={m.ref} value={m.ref}>
          {m.ref}
        </option>
      ))}
    </>
  );

  const fallback = settings?.fallbackModel ?? "";
  const vision = settings?.visionModel ?? "";
  const fast = settings?.fastModel ?? "";
  return (
    <GroupCard title="Roles" description="The chat model is set with &quot;Use for chat&quot; on its row. These are the other jobs.">
      <Row label="Fallback" hint="Used when the chat model is busy or out of quota.">
        <select aria-label="Fallback chat model" value={fallback} onChange={(e) => void save("fallbackModel", e.target.value)}>
          {options(fallback, "(none)")}
        </select>
      </Row>
      <Row label="Vision" hint='Reads the screen for "what does this say?"'>
        <select aria-label="Vision model" value={vision} onChange={(e) => void save("visionModel", e.target.value)}>
          {options(vision, "(auto: the first Gemini flash model)")}
        </select>
      </Row>
      <Row label="Fast" hint="A smaller model for the short jobs: it names new sessions and summarizes long conversations.">
        <select aria-label="Fast model" value={fast} onChange={(e) => void save("fastModel", e.target.value)}>
          {options(fast, "(none)")}
        </select>
      </Row>
    </GroupCard>
  );
}

// The contracts of every tool, by group, with the per-tool override. Override values
// are `allow`, `ask` and `deny` (settings.toolOverrides); empty follows the mode.
import type { ToolView } from "../../api";
import { Badge, GroupCard, type BadgeTone } from "../ui";

const OVERRIDES: [string, string][] = [
  ["", "follows the mode"],
  ["allow", "allow"],
  ["ask", "ask"],
  ["deny", "deny"],
];

const RISK: Record<string, BadgeTone> = { none: "ok", low: "ok", medium: "warn", high: "bad", critical: "bad" };

export default function ToolContracts({
  tools,
  overrides,
  onOverride,
}: {
  tools: ToolView[];
  overrides: Record<string, string>;
  onOverride: (tool: string, value: string) => void;
}) {
  const groups = [...new Set(tools.map((t) => t.group))];
  return (
    <>
      {groups.map((g) => {
        const list = tools.filter((t) => t.group === g);
        return (
          <GroupCard key={g} title={g} description={`${list.length} tools`}>
            <table className="ui-tools">
              <thead>
                <tr>
                  <th>Tool</th>
                  <th>Override</th>
                  <th>What it does</th>
                </tr>
              </thead>
              <tbody>
                {list.map((t) => {
                  const c = t.contract;
                  return (
                    <tr key={c.name} className={t.enabled ? "" : "off"}>
                      <td>
                        <code>{c.name}</code>
                        <div className="ui-flags">
                          <Badge tone={c.readOnly ? "ok" : "accent"}>{c.readOnly ? "read-only" : "changes"}</Badge>
                          <Badge tone={RISK[c.risk] ?? "neutral"}>{c.risk}</Badge>
                          {c.privileged && <Badge tone="bad">root</Badge>}
                          {c.destructive && <Badge tone="bad">destructive</Badge>}
                          {!t.enabled && <Badge tone="neutral">off</Badge>}
                        </div>
                      </td>
                      <td>
                        <select aria-label={`Override for ${c.name}`} value={overrides[c.name] ?? ""} onChange={(e) => onOverride(c.name, e.target.value)}>
                          {OVERRIDES.map(([value, label]) => (
                            <option key={value} value={value}>
                              {label}
                            </option>
                          ))}
                        </select>
                      </td>
                      <td>{c.description.length > 120 ? `${c.description.slice(0, 120)}...` : c.description}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </GroupCard>
        );
      })}
      {tools.length === 0 && <p className="hint">Loading the tool contracts...</p>}
    </>
  );
}

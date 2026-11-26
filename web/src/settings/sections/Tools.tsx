// Tools and permissions: the permission mode, the always-allow rules and the tool
// groups of a chat and of the MCP server. The per-tool overrides sit in the contracts;
// the limits of a turn (S40) are the last card.
import { useEffect, useState, type FormEvent } from "react";
import { api, type ToolView } from "../../api";
import { useStore } from "../../store";
import type { SectionProps } from "../registry";
import { Field, GroupCard, Row, SectionPage } from "../ui";
import { useDraft } from "../useDraft";
import { useSettingPatch } from "../useSettingPatch";
import ToolContracts from "./ToolContracts";

const MODES = ["read-only", "ask", "trusted", "full"];
const LIMITS = { maxIterations: 40, approvalTimeoutSec: 600 };
type Numberish = number | null;
const positive = (n: Numberish) => n === null || (Number.isInteger(n) && n > 0);
const number = (v: string): Numberish => (v.trim() === "" ? null : Number(v));

export default function Tools({ title, description }: SectionProps) {
  const settings = useStore().state?.settings;
  const { save } = useSettingPatch();
  const [tools, setTools] = useState<ToolView[]>([]);
  const [round, setRound] = useState(0);
  const [limits, setLimits, limitsDirty] = useDraft<{ maxIterations: Numberish; approvalTimeoutSec: Numberish }>({
    maxIterations: settings?.maxIterations ?? LIMITS.maxIterations,
    approvalTimeoutSec: settings?.approvalTimeoutSec ?? LIMITS.approvalTimeoutSec,
  });
  const limitsOk = positive(limits.maxIterations) && positive(limits.approvalTimeoutSec);
  // One PUT per changed key.
  const saveLimits = (e: FormEvent) => {
    e.preventDefault();
    const { maxIterations, approvalTimeoutSec } = limits;
    if (maxIterations === null || approvalTimeoutSec === null) return;
    if (maxIterations !== (settings?.maxIterations ?? LIMITS.maxIterations)) void save("maxIterations", maxIterations);
    if (approvalTimeoutSec !== (settings?.approvalTimeoutSec ?? LIMITS.approvalTimeoutSec)) void save("approvalTimeoutSec", approvalTimeoutSec);
  };

  // `enabled` and `mcp` of a tool follow the groups: read the list again when they move.
  useEffect(() => {
    void api.get<{ tools: ToolView[] }>("/api/tools").then((r) => setTools(r.tools));
  }, [round]);

  const mode = settings?.permissionMode ?? "ask";
  const rules = settings?.rules ?? [];
  const overrides = settings?.toolOverrides ?? {};
  const chat = settings?.toolGroups ?? [];
  const mcp = settings?.mcpToolGroups ?? [];
  // Every group a tool is in, and the groups the settings name that no tool has now.
  const groups = [...new Set([...tools.map((t) => t.group), ...chat, ...mcp])].sort();
  // An empty chat list means every group and an unset MCP list the daemon's default:
  // `/api/tools` says which groups are really on.
  const groupsOn = (list: string[], flag: (t: ToolView) => boolean | undefined) =>
    list.length ? list : groups.filter((g) => tools.some((t) => t.group === g && flag(t) === true));
  const chatOn = groupsOn(chat, (t) => t.enabled);
  const mcpOn = groupsOn(mcp, (t) => t.mcp);
  const toggle = (list: string[], g: string) => (list.includes(g) ? list.filter((x) => x !== g) : [...list, g]);
  // Load the list again after one of these saves.
  const reloadAfter = (done: ReturnType<typeof save>) =>
    void done.then((r) => {
      if (r) setRound((n) => n + 1);
    });
  const saveGroups = (key: string, next: string[]) => reloadAfter(save(key, next));
  // One key per change; the daemon merges objects. Clearing sends `null`.
  const setOverride = (tool: string, value: string) => reloadAfter(save("toolOverrides", { [tool]: value || null }));

  const chips = (list: string[], key: string) => (
    <div className="chips">
      {groups.map((g) => (
        <label key={g} className={`chip${list.includes(g) ? " on" : ""}`}>
          <input type="checkbox" checked={list.includes(g)} onChange={() => saveGroups(key, toggle(list, g))} /> {g}
        </label>
      ))}
    </div>
  );

  return (
    <SectionPage title={title} description={description}>
      <GroupCard title="Permission mode" description="How KAgent decides whether a tool call runs. The header and the tray menu have the same choice.">
        <Row label="Mode" stacked>
          <div className="row" role="group" aria-label="Permission mode">
            {MODES.map((m) => (
              <button key={m} className={m === mode ? "primary" : ""} aria-pressed={m === mode} disabled={m === mode} onClick={() => void save("permissionMode", m)}>
                {m}
              </button>
            ))}
          </div>
        </Row>
        <p className="hint ui-note">
          read-only denies every mutating call; ask shows approval cards (answer them by click or, in a voice session, by saying "yes" / "no"; root actions need
          "approve"); trusted auto-allows unprivileged changes but still asks for typing into terminals, messages and reboots; full control runs everything without
          asking except root (sudo) actions.
        </p>
      </GroupCard>

      <GroupCard title="Always-allow rules" description="Made when you answer a card with &quot;always allow&quot;. Each lets one tool run one kind of call without a card.">
        {rules.length === 0 && <div className="hint">none yet</div>}
        <div className="ui-rules">
          {rules.map((r, i) => (
            <div key={`${r.tool}:${r.prefix}`} className="ui-rule">
              <span>
                {r.tool}: <code>{r.prefix}...</code>
              </span>
              <button className="danger" onClick={() => void save("rules", rules.filter((_, j) => j !== i))}>
                remove
              </button>
            </div>
          ))}
        </div>
      </GroupCard>

      <ToolContracts tools={tools} overrides={overrides} onOverride={setOverride} />

      <GroupCard title="Tool groups for chat" description="The groups the agent may call in a conversation. A tool of a group that is off is not offered to the model.">
        {chips(chatOn, "toolGroups")}
      </GroupCard>

      <GroupCard title="Tool groups for MCP" description="The groups other agents can use over MCP, with the approvals made here.">
        <p className="hint">
          <code>claude mcp add kagent -- kagentd mcp</code> lets Claude Code use them.
        </p>
        {chips(mcpOn, "mcpToolGroups")}
      </GroupCard>

      <GroupCard title="Limits of a turn" description="How far one message may go.">
        <form className="ui-form" onSubmit={saveLimits}>
          <div className="ui-form-grid">
            <Field label="Max iterations" hint="Tool calls and replies one turn may take before it stops.">
              <input type="number" min={1} value={limits.maxIterations ?? ""} onChange={(e) => setLimits({ ...limits, maxIterations: number(e.target.value) })} />
            </Field>
            <Field label="Approval timeout (seconds)" hint="How long an approval card waits before the call counts as refused.">
              <input type="number" min={1} value={limits.approvalTimeoutSec ?? ""} onChange={(e) => setLimits({ ...limits, approvalTimeoutSec: number(e.target.value) })} />
            </Field>
          </div>
          <div className="row">
            <button className="primary" type="submit" disabled={!limitsDirty || !limitsOk}>
              Save limits
            </button>
            {!limitsOk && limitsDirty && <span className="ui-inline-bad">Both must be whole numbers above 0.</span>}
          </div>
        </form>
      </GroupCard>
    </SectionPage>
  );
}

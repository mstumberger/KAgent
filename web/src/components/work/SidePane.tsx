// The side pane (work-windows.md section 5.4): 380 px at the right of the chat, in the full window
// only. Its tabs belong to the open session; with one tab a header, with several a tab bar.
// Escape with the focus inside closes the active tab.
import Icon from "../Icon";
import { useStore } from "../../store";
import { useNarrow } from "../sidebar/useNarrow";
import OutputPane from "./OutputPane";
import PlanPane from "./PlanPane";
import SubagentPane from "./SubagentPane";
import { activate, closeTab, tabsOf, useWork, type SideKind } from "./state";

const ICON: Record<SideKind, string> = { subagent: "bot", plan: "list", output: "terminal" };

export default function SidePane() {
  const s = useStore();
  const w = useWork();
  const overlay = useNarrow();
  const { tabs, active } = tabsOf(w, s.activeSession);
  if (s.view !== "chat" || !active) return null;
  const body = active.kind === "subagent" ? <SubagentPane key={active.key} tab={active} /> : active.kind === "plan" ? <PlanPane key={active.key} tab={active} /> : <OutputPane key={active.key} tab={active} />;
  const owner = active.owner;
  return (
    <>
      {overlay && <div className="side-scrim" onClick={() => closeTab(active.key)} />}
      <aside
        className={`side-pane${overlay ? " overlay" : ""}`}
        aria-label="Side pane"
        onKeyDown={(e) => {
          if (e.key !== "Escape") return;
          e.stopPropagation();
          closeTab(active.key);
        }}
      >
        {tabs.length === 1 ? (
          <div className="side-head">
            <span className="side-icon" aria-hidden="true">
              <Icon name={ICON[active.kind]} size={14} />
            </span>
            <span className="side-title" title={active.title}>
              {active.title}
            </span>
            <button type="button" className="side-close" aria-label="Close" onClick={() => closeTab(active.key)}>
              <Icon name="close" size={13} />
            </button>
          </div>
        ) : (
          <div className="side-head side-tabs" role="tablist">
            {tabs.map((t) => (
              <div key={t.key} className={`side-tab${t.key === active.key ? " active" : ""}`} data-key={t.key}>
                <button type="button" role="tab" aria-selected={t.key === active.key} className="side-tab-main" title={t.title} onClick={() => activate(owner, t.key)}>
                  <Icon name={ICON[t.kind]} size={13} />
                  <span className="side-title">{t.title}</span>
                </button>
                <button type="button" className="side-tab-x" aria-label={`Close ${t.title}`} onClick={() => closeTab(t.key)}>
                  <Icon name="close" size={11} />
                </button>
              </div>
            ))}
          </div>
        )}
        <div className="side-body">{body}</div>
      </aside>
    </>
  );
}

// The owner's providers (models.md section 7.1): in their order, a drag reorders them.
// Under 900 px of viewport width the list is a dropdown, as the settings menu is.
import type { ProviderView } from "../../../api";
import Icon from "../../../components/Icon";
import { UsageBar } from "../../../components/UsageCard";
import { usageOf, useUsage } from "../../../usage";
import Dropdown from "../../Dropdown";
import { useDragList } from "../../dragList";
import { Logo } from "./Catalog";
import { STATUS_LABEL, statusOf } from "./shared";

export default function ProviderList({
  providers,
  selectedId,
  busy,
  adding,
  onSelect,
  onAdd,
  onReorder,
}: {
  providers: ProviderView[];
  selectedId: string | null;
  busy: boolean;
  /** The form of a new provider is open: nothing is selected, and no second one is added. */
  adding: boolean;
  onSelect: (id: string) => void;
  onAdd: () => void;
  onReorder: (order: string[]) => void;
}) {
  const usage = useUsage();
  const drag = useDragList(
    () => providers.map((p) => p.id),
    onReorder,
  );
  return (
    <div className="pv-list">
      <div className="pv-list-narrow">
        <Dropdown
          ariaLabel="Provider"
          value={selectedId ?? ""}
          placeholder={providers.length ? "Choose a provider" : "No providers yet"}
          options={providers.map((p) => ({ value: p.id, label: p.name }))}
          onChange={onSelect}
        />
      </div>
      <nav className="pv-nav" aria-label="Providers">
        {providers.map((p) => {
          const active = p.id === selectedId;
          return (
            <div key={p.id} {...drag.row(p.id, `pv-nav-item${active ? " active" : ""}`)}>
              {providers.length > 1 && (
                <button {...drag.handle(p.id, p.name)}>
                  <Icon name="handle" size={14} />
                </button>
              )}
              <button className="pv-nav-main" aria-current={active ? "page" : undefined} onClick={() => onSelect(p.id)}>
                <Logo name={p.name} />
                <span className="pv-nav-name">{p.name}</span>
                <UsageBar limit={usageOf(usage, p.id)?.tightest ?? p.tightestLimit} />
                <span className={`pv-dot ${statusOf(p)}`} title={STATUS_LABEL[statusOf(p)]} />
              </button>
            </div>
          );
        })}
        {providers.length === 0 && <div className="hint pv-nav-empty">No providers yet</div>}
        {!adding && (
          <div className="ld-actions">
            <button className="ld-add" disabled={busy} onClick={onAdd}>
              <Icon name="plus" size={14} />
              Add provider
            </button>
          </div>
        )}
      </nav>
    </div>
  );
}

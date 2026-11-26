// The catalog of provider templates (searchable), and the logo every provider list uses.
import { useState } from "react";
import type { ProviderView, Template } from "../../../api";
import { API_FORMATS } from "./shared";

export function Logo({ name }: { name: string }) {
  const letter = name.trim().charAt(0).toUpperCase() || "?";
  return (
    <span className="pv-logo" aria-hidden="true">
      {letter}
    </span>
  );
}

export default function Catalog({
  templates,
  existing,
  onPick,
}: {
  templates: Template[];
  existing: ProviderView[];
  onPick: (template: Template) => void;
}) {
  const [q, setQ] = useState("");
  const list = templates.filter((t) => `${t.name} ${t.id} ${t.api.baseUrl}`.toLowerCase().includes(q.toLowerCase()));
  const added = new Set(existing.map((p) => p.templateId));
  return (
    <>
      <input className="pv-search" placeholder="Search providers" value={q} onChange={(e) => setQ(e.target.value)} autoFocus />
      <div className="pv-catalog">
        {list.map((t) => (
          <button key={t.id} className="pv-cat-item" onClick={() => onPick(t)}>
            <Logo name={t.name} />
            <span>
              <span className="pv-cat-name">
                {t.name}
                {added.has(t.id) && <span className="pv-tag">added</span>}
              </span>
              <span className="hint">{API_FORMATS[t.api.type] ?? t.api.type}</span>
            </span>
          </button>
        ))}
        {list.length === 0 && <p className="hint">No providers found</p>}
      </div>
    </>
  );
}

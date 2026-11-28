// A section of the work panel (work-windows.md section 5.1): a header of 28 px with a chevron, the
// title, a count and the actions, and a body. A click on the header folds it; what the owner
// folded is remembered (state.ts).
import type { ReactNode } from "react";
import Icon from "../Icon";
import { sectionOpen, toggleSection, useWork } from "./state";

interface Props {
  id: string;
  title: string;
  /** In the header, in `--dim`. */
  count?: string;
  /** The section has something to show: it opens by itself, and an empty one stays closed. */
  hasSomething: boolean;
  actions?: ReactNode;
  children: ReactNode;
}

export default function WorkSection({ id, title, count, hasSomething, actions, children }: Props) {
  const open = sectionOpen(useWork(), id, hasSomething);
  return (
    <section className="work-section" data-section={id} aria-label={title}>
      <div className="work-section-head">
        <button type="button" className="work-section-toggle" aria-expanded={open} onClick={() => toggleSection(id, open, hasSomething)}>
          <span className={`work-chevron${open ? " open" : ""}`} aria-hidden="true">
            <Icon name="chevron" size={12} />
          </span>
          <span className="work-section-title">{title}</span>
          {count && <span className="work-section-count">{count}</span>}
        </button>
        {actions && <span className="work-section-actions">{actions}</span>}
      </div>
      {open && <div className="work-section-body">{children}</div>}
    </section>
  );
}

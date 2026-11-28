// The frame and the header of a page of the window (shell.md section 4): one frame that scrolls on its own and keeps its
// content in a column (960 px, or 1600 px for a page with a real second pane), and one header with the title, a line under it
// and the page's actions. The settings pages use the same header through `SectionPage` (settings/ui.tsx).
import type { ReactNode } from "react";

/** The one scroller of a page. `wide`: the 1600 px column. `className` adds the page's own class. */
export function Page({ wide = false, className, children }: { wide?: boolean; className?: string; children: ReactNode }) {
  return (
    <div className={`page${wide ? " wide" : ""}${className ? ` ${className}` : ""}`} data-scroll-root="page">
      {children}
    </div>
  );
}

/** The title (h2), an optional line under it, and the actions at the right (under the title when the page is narrow). */
export function PageHeader({ title, description, actions }: { title: string; description?: ReactNode; actions?: ReactNode }) {
  return (
    <div className="page-head">
      <div className="page-head-text">
        <h2>{title}</h2>
        {description && <p className="hint">{description}</p>}
      </div>
      {actions && <div className="page-head-actions">{actions}</div>}
    </div>
  );
}

export { GroupCard as Card, Row as FormRow } from "../../settings/ui";

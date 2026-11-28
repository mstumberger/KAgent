// Back and Forward in the title bar (shell.md section 2.1): after the sidebar toggle, disabled when there is nowhere to go.
import Icon from "../components/Icon";
import { goBack, goForward, useHistoryState } from "./router";

export default function NavButtons() {
  const h = useHistoryState();
  return (
    <div className="nav-buttons" role="group" aria-label="History">
      <button type="button" id="nav-back" className="tab icon-tab" title={h.back} aria-label="Back" disabled={!h.canBack} onClick={goBack}>
        <Icon name="back" size={15} />
      </button>
      <button type="button" id="nav-forward" className="tab icon-tab" title={h.forward} aria-label="Forward" disabled={!h.canForward} onClick={goForward}>
        <Icon name="forward" size={15} />
      </button>
    </div>
  );
}

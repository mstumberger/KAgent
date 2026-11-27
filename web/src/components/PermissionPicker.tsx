// Permission level, switchable from every KAgent window (the foot of the sidebar and the
// widget). The same setting as Tools -> Permission mode and the tray's "Full control".
import { useRef, useState } from "react";
import { api } from "../api";
import { refreshState, useStore } from "../store";
import Icon from "./Icon";
import Menu from "./Menu";

const LEVELS: { id: string; label: string; hint: string }[] = [
  { id: "read-only", label: "Read-only", hint: "Looks only: every change is refused" },
  { id: "ask", label: "Ask", hint: "Every change shows an approval card" },
  { id: "trusted", label: "Trusted", hint: "Changes run; typing, messages, reboot and root still ask" },
  { id: "full", label: "Full control", hint: "Everything runs without asking, except root (sudo) actions" },
];

/** The mode in use and the way to change it: one `PUT /api/settings`, then the state is read again. */
function useMode() {
  const s = useStore();
  const mode = (s.state?.settings.permissionMode as string) ?? "ask";
  const current = LEVELS.find((l) => l.id === mode) ?? LEVELS[1];
  const set = async (id: string) => {
    await api.put("/api/settings", { permissionMode: id });
    await refreshState();
  };
  return { mode, current, set };
}

/**
 * The permission mode at the foot of the sidebar: a full-width row (its dot, the name of the
 * mode, the chevron) that opens the menu above it, or in the rail the dot alone, with the
 * menu on its right.
 */
export function PermissionRow({ rail = false }: { rail?: boolean }) {
  const { mode, current, set } = useMode();
  const button = useRef<HTMLButtonElement>(null);
  const [anchor, setAnchor] = useState<{ left: number; top: number; right: number; bottom: number } | null>(null);
  const toggle = () => {
    const r = button.current?.getBoundingClientRect();
    setAnchor(anchor || !r ? null : { left: r.left, top: r.top, right: r.right, bottom: r.bottom });
  };
  return (
    <>
      <button
        ref={button}
        type="button"
        className={`sb-perm perm-${mode}${rail ? " rail" : ""}`}
        title={rail ? current.label : `Permission: ${current.hint}`}
        aria-label={`Permission level: ${current.label}`}
        aria-haspopup="menu"
        aria-expanded={anchor ? true : undefined}
        onClick={toggle}
      >
        <span className="perm-dot" aria-hidden="true" />
        {!rail && (
          <>
            <span className="sb-perm-name">{current.label}</span>
            <Icon name="chevron" size={12} />
          </>
        )}
      </button>
      {anchor && (
        <Menu
          label="Permission level"
          anchor={anchor}
          placement={rail ? "right" : "below"}
          items={LEVELS.map((l) => ({ id: l.id, label: l.label, checked: l.id === mode }))}
          onPick={(id) => void set(id)}
          onClose={(restore) => {
            setAnchor(null);
            if (restore) button.current?.focus();
          }}
        />
      )}
    </>
  );
}

export default function PermissionPicker({ compact = false }: { compact?: boolean }) {
  const { mode, current, set } = useMode();
  return (
    <label className={`perm perm-${mode}${compact ? " compact" : ""}`} title={`Permission: ${current.hint}`}>
      <span className="perm-dot" aria-hidden="true" />
      <select aria-label="Permission level" value={mode} onChange={(e) => void set(e.target.value)}>
        {LEVELS.map((l) => (
          <option key={l.id} value={l.id}>
            {l.label}
          </option>
        ))}
      </select>
    </label>
  );
}

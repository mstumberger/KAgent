// Permission level, switchable from every KAgent window (header and widget). The same
// setting as Tools -> Permission mode and the tray's "Full control".
import { api } from "../api";
import { refreshState, useStore } from "../store";

const LEVELS: { id: string; label: string; hint: string }[] = [
  { id: "read-only", label: "Read-only", hint: "Looks only: every change is refused" },
  { id: "ask", label: "Ask", hint: "Every change shows an approval card" },
  { id: "trusted", label: "Trusted", hint: "Changes run; typing, messages, reboot and root still ask" },
  { id: "full", label: "Full control", hint: "Everything runs without asking, except root (sudo) actions" },
];

export default function PermissionPicker({ compact = false }: { compact?: boolean }) {
  const s = useStore();
  const mode = (s.state?.settings.permissionMode as string) ?? "ask";
  const current = LEVELS.find((l) => l.id === mode) ?? LEVELS[1];
  const set = async (id: string) => {
    await api.put("/api/settings", { permissionMode: id });
    await refreshState();
  };
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

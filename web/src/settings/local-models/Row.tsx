// One model of the list inside a job group (local-models.md section 4, item 2): name, id, size,
// the fit chip, the note, and what can be done with it right now.
import { Badge, type BadgeTone } from "../ui";
import { size, type Fit, type Job, type LocalModelsView, type Pull, type Recommended } from "./api";

const FIT: Record<Fit, { label: string; tone: BadgeTone; title: string }> = {
  fits: { label: "Fits", tone: "ok", title: "Fits in this card's memory with room to spare" },
  tight: { label: "Tight", tone: "warn", title: "Fits when nothing else uses the card" },
  tooBig: { label: "Too big", tone: "neutral", title: "More than this machine has" },
  slow: { label: "Slow", tone: "warn", title: "No graphics card: it runs on the processor" },
};

export interface Actions {
  busy: boolean;
  download: (id: string) => void;
  cancel: (id: string) => void;
  remove: (m: Recommended) => void;
  use: (setting: "visionModel" | "fallbackModel" | "chatModel", ref: string) => void;
}

const USED: Record<string, string> = { chatModel: "Brain", fastModel: "fast model", fallbackModel: "fallback", visionModel: "vision", liveModel: "voice" };

/** Why Download is not offered, or null. */
function cannotDownload(m: Recommended, view: LocalModelsView): string | null {
  if (!view.ollama.reachable) return "Start Ollama first";
  if (m.fit === "tooBig") return view.gpu ? `Needs about ${size(m.vramMb)} of graphics memory; this card has ${size(view.gpu.vramTotalMb)}` : `Needs about ${size(m.vramMb)}; this machine has ${size(view.ramTotalMb)} of RAM`;
  return null;
}

function Progress({ pull, onCancel }: { pull: Pull; onCancel: () => void }) {
  if (pull.state === "failed") return null;
  const pct = pull.totalMb > 0 ? Math.min(100, Math.round((pull.doneMb / pull.totalMb) * 100)) : 0;
  return (
    <div className="lm-pull">
      {pull.state === "queued" ? (
        <span className="hint">Waiting for the download before it</span>
      ) : (
        <>
          <div className="lm-progress" role="progressbar" aria-label="Download" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct}>
            <span style={{ width: `${pct}%` }} />
          </div>
          <span className="hint">
            {size(pull.doneMb)} of {size(pull.totalMb)}
          </span>
        </>
      )}
      <button type="button" onClick={onCancel}>
        Cancel
      </button>
    </div>
  );
}

export default function Row({ m, job, view, actions }: { m: Recommended; job: Job; view: LocalModelsView; actions: Actions }) {
  const pull = view.pulls.find((p) => p.id === m.id);
  const fit = FIT[m.fit];
  const recommended = m.recommendedFor.includes(job);
  const blocked = cannotDownload(m, view);
  const inUse = m.usedFor.length > 0;
  const use = (setting: "visionModel" | "fallbackModel" | "chatModel", label: string, on: boolean) =>
    m.ref ? (
      <button type="button" key={setting} disabled={actions.busy || on} onClick={() => actions.use(setting, m.ref as string)}>
        {on ? `${label} (in use)` : label}
      </button>
    ) : null;
  return (
    <div className="lm-row" data-id={m.id} data-fit={m.fit} data-recommended={recommended ? "1" : "0"}>
      <div className="lm-main">
        <div className="lm-title">
          <span className="lm-name">{m.name}</span>
          {recommended && <Badge tone="accent">Recommended</Badge>}
          {m.installed && <Badge tone="ok">Installed</Badge>}
          {m.usedFor.map((u) => (
            <Badge key={u} tone="neutral">
              {USED[u] ?? u}
            </Badge>
          ))}
        </div>
        <div className="lm-meta">
          <code>{m.id}</code>
          <span>{size(m.sizeMb)}</span>
          {m.engine === "ollama" && <span>needs about {size(m.vramMb)}</span>}
        </div>
        <p className="hint lm-note">{m.note}</p>
        {m.fitNote && m.fit !== "fits" && <p className="hint lm-fitnote">{m.fitNote}</p>}
        {pull?.state === "failed" && <p className="lm-error" role="alert">Download failed: {pull.error}</p>}
      </div>
      <div className="lm-side">
        <Badge tone={fit.tone}>
          <span title={fit.title}>{fit.label}</span>
        </Badge>
        <div className="lm-actions">
          {!m.downloadable && <span className="hint">Not usable yet</span>}
          {m.downloadable && pull && pull.state !== "failed" && <Progress pull={pull} onCancel={() => actions.cancel(m.id)} />}
          {m.downloadable && pull?.state === "failed" && (
            <>
              <button type="button" disabled={actions.busy} onClick={() => actions.download(m.id)}>
                Retry
              </button>
              <button type="button" disabled={actions.busy} onClick={() => actions.cancel(m.id)}>
                Dismiss
              </button>
            </>
          )}
          {m.downloadable && !pull && !m.installed && (
            <button type="button" className="primary" disabled={actions.busy || blocked !== null} title={blocked ?? `Download ${m.name} through Ollama`} onClick={() => actions.download(m.id)}>
              Download
            </button>
          )}
          {m.downloadable && !pull && m.installed && (
            <>
              {!m.ref && (
                <button type="button" disabled={actions.busy} title="Ollama has it, but Settings -> Models does not list it yet" onClick={() => actions.download(m.id)}>
                  Add to Models
                </button>
              )}
              {job === "vision" && use("visionModel", "Use for vision", m.usedFor.includes("visionModel"))}
              {job === "chat" && use("fallbackModel", "Use as fallback", m.usedFor.includes("fallbackModel"))}
              {job === "chat" && m.tools && use("chatModel", "Use as Brain", m.usedFor.includes("chatModel"))}
              <button type="button" className="danger" disabled={actions.busy || inUse} title={inUse ? "Set as something in Settings: choose another model for that first" : `Remove ${m.name} from this machine`} onClick={() => actions.remove(m)}>
                Remove
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

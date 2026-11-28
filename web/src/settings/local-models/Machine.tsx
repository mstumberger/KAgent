// Card 1 of the Local models page (local-models.md section 4): the graphics card and its memory,
// RAM, and Ollama's state.
import { Badge, GroupCard, Row } from "../ui";
import { size, type LocalModelsView } from "./api";

export default function Machine({ view }: { view: LocalModelsView }) {
  const { gpu, ollama } = view;
  const used = gpu ? Math.max(0, gpu.vramTotalMb - gpu.vramFreeMb) : 0;
  const pct = (mb: number) => (gpu && gpu.vramTotalMb > 0 ? Math.min(100, (mb / gpu.vramTotalMb) * 100) : 0);
  return (
    <GroupCard title="This machine" description="What the models below are judged against.">
      <Row label="Graphics card" hint={gpu ? `Read with ${gpu.source}.` : undefined}>
        {gpu ? (
          <div className="lm-gpu">
            <span className="lm-gpu-name">{gpu.name}</span>
            <div className="lm-bar" role="img" aria-label={`${size(gpu.vramFreeMb)} free of ${size(gpu.vramTotalMb)}, ${size(used)} used by other programs`}>
              <span className="lm-bar-used" style={{ width: `${pct(used)}%` }} />
              <span className="lm-bar-free" style={{ width: `${pct(gpu.vramFreeMb)}%` }} />
            </div>
            <span className="hint">
              {size(gpu.vramFreeMb)} free of {size(gpu.vramTotalMb)}
              {used > 0 && ` (${size(used)} used by other programs)`}
            </span>
          </div>
        ) : (
          <span className="lm-nogpu">No graphics card found: models run on the processor and are slow.</span>
        )}
      </Row>
      <Row label="Memory (RAM)">
        <span>{size(view.ramTotalMb)}</span>
      </Row>
      <Row label="Ollama" hint={ollama.baseUrl}>
        <Badge tone="ok">Running{ollama.version ? ` ${ollama.version}` : ""}</Badge>
      </Row>
    </GroupCard>
  );
}

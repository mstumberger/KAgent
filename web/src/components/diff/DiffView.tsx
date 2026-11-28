// The diff view (diff.md section 5): a tree of the changed files on the left, and the diff of each file on the right,
// side by side or unified, with the unchanged lines collapsed, and next and previous change on F7 and Shift+F7.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import FileView from "./FileView";
import Tree from "./Tree";
import { countChanges, readLayout, runsIn, storeLayout, type Diff, type Layout, type Texts } from "./model";
import "./diff.css";

interface Props {
  diff: Diff;
  /** The two texts of a file by path, so the unchanged lines can be shown. */
  texts?: Record<string, Texts>;
  onOpenEditor?: (path: string, line: number) => void;
  onRevert?: (path: string) => void;
  label?: string;
}

const LAYOUTS: { id: Layout; label: string }[] = [
  { id: "side", label: "Side by side" },
  { id: "unified", label: "Unified" },
];

export default function DiffView({ diff, texts, onOpenEditor, onRevert, label = "Changes" }: Props) {
  const [layout, setLayout] = useState<Layout>(readLayout);
  const [current, setCurrent] = useState(-1);
  const [file, setFile] = useState<string | null>(diff.files[0]?.path ?? null);
  const root = useRef<HTMLDivElement>(null);
  const hover = useRef(false);
  const total = useMemo(() => countChanges(diff.files), [diff.files]);
  // The first change block of each file: the blocks count across files.
  const starts = useMemo(() => {
    const out: number[] = [];
    for (let i = 0; i < diff.files.length; i++) out.push(i === 0 ? 0 : out[i - 1] + diff.files[i - 1].hunks.reduce((m, h) => m + runsIn(h.lines), 0));
    return out;
  }, [diff.files]);

  const go = useCallback(
    (step: 1 | -1) => {
      if (total === 0) return;
      const next = current < 0 ? (step === 1 ? 0 : total - 1) : (current + step + total) % total;
      setCurrent(next);
      const row = root.current?.querySelector<HTMLElement>(`[data-change="${next}"]`);
      row?.scrollIntoView({ block: "center" });
      const owner = row?.closest<HTMLElement>("[data-path]");
      if (owner?.dataset.path) setFile(owner.dataset.path);
    },
    [current, total],
  );
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.key !== "F7" || e.ctrlKey || e.metaKey || e.altKey) return;
      const inside = !!root.current && (root.current.contains(document.activeElement) || hover.current);
      if (!inside) return;
      e.preventDefault();
      go(e.shiftKey ? -1 : 1);
    };
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("keydown", key);
    };
  }, [go]);

  const pick = (path: string) => {
    setFile(path);
    document.getElementById(`dv-file-${path}`)?.scrollIntoView({ block: "start" });
  };
  const choose = (l: Layout) => {
    setLayout(l);
    storeLayout(l);
  };

  return (
    <div className="diffview" ref={root} role="region" aria-label={label} onMouseEnter={() => (hover.current = true)} onMouseLeave={() => (hover.current = false)}>
      <div className="dv-bar">
        <span className="dv-total">
          {diff.files.length} {diff.files.length === 1 ? "file" : "files"}, <span className="dv-plus">+{diff.added}</span> <span className="dv-minus">-{diff.removed}</span>
          {diff.truncated && " (cut: more files than are shown)"}
        </span>
        <span className="spacer" />
        <span className="dv-pos" role="status" aria-live="polite">{total === 0 ? "No changes" : current < 0 ? `${total} ${total === 1 ? "change" : "changes"}` : `Change ${current + 1} of ${total}`}</span>
        <button type="button" aria-label="Previous change (Shift+F7)" title="Previous change (Shift+F7)" disabled={total === 0} onClick={() => go(-1)}>Previous</button>
        <button type="button" aria-label="Next change (F7)" title="Next change (F7)" disabled={total === 0} onClick={() => go(1)}>Next</button>
        <div className="dv-switch" role="group" aria-label="Layout">
          {LAYOUTS.map((l) => (
            <button key={l.id} type="button" aria-pressed={layout === l.id} onClick={() => choose(l.id)}>{l.label}</button>
          ))}
        </div>
      </div>
      {diff.files.length === 0 ? (
        <div className="dv-empty">No differences.</div>
      ) : (
        <div className="dv-body">
          <Tree files={diff.files} current={file} onPick={pick} />
          <div className="dv-files">
            {diff.files.map((f, i) => (
              <FileView key={f.path} file={f} layout={layout} texts={texts?.[f.path]} firstBlock={starts[i]} current={current} onOpenEditor={onOpenEditor} onRevert={onRevert} />
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

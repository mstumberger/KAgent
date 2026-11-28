// One file of a diff: its header, and its hunks and the unchanged lines between them, side by side or unified.
import { useState } from "react";
import {
  MARK,
  gapLines,
  parts,
  runsIn,
  sideRows,
  splitLines,
  unifiedRows,
  type DiffLine,
  type FileDiff,
  type Layout,
  type Texts,
} from "./model";

interface Props {
  file: FileDiff;
  layout: Layout;
  texts?: Texts;
  /** The number of the first change block of this file (the blocks count across files). */
  firstBlock: number;
  current: number;
  onOpenEditor?: (path: string, line: number) => void;
  onRevert?: (path: string) => void;
}

/** The text of a line with the changed characters marked. */
function Text({ line }: { line: DiffLine }) {
  if (!line.ranges || line.ranges.length === 0) return <>{line.text}</>;
  const chars = Array.from(line.text);
  const out: React.ReactNode[] = [];
  let at = 0;
  line.ranges.forEach(([s, e], i) => {
    if (s > at) out.push(chars.slice(at, s).join(""));
    out.push(<mark key={i} className="dv-intra">{chars.slice(s, e).join("")}</mark>);
    at = e;
  });
  if (at < chars.length) out.push(chars.slice(at).join(""));
  return <>{out}</>;
}

function Num({ n, path, onOpen }: { n: number | null; path: string; onOpen?: (path: string, line: number) => void }) {
  if (n === null) return <span className="dv-num" />;
  if (!onOpen) return <span className="dv-num">{n}</span>;
  return (
    <button type="button" className="dv-num dv-open" tabIndex={-1} title="Open in editor" aria-label={`Open ${path} at line ${n} in the editor`} onClick={() => onOpen(path, n)}>
      {n}
    </button>
  );
}

const SIGN = { ctx: " ", add: "+", del: "-" } as const;

function Cell({ line, side }: { line: DiffLine | null; side: "left" | "right" }) {
  if (!line) return <span className="dv-text empty" aria-hidden="true" />;
  // A context line is the same on both sides; it is drawn once per side.
  const kind = line.kind;
  return (
    <span className={`dv-text k-${kind}${side === "left" ? " l" : " r"}`}>
      <span className="dv-sign" aria-hidden="true">{SIGN[kind]}</span>
      <span className="dv-code"><Text line={line} /></span>
    </span>
  );
}

export default function FileView({ file, layout, texts, firstBlock, current, onOpenEditor, onRevert }: Props) {
  const [opened, setOpened] = useState<Record<number, boolean>>({});
  const after = texts?.after != null ? splitLines(texts.after) : null;
  const items = parts(file, after ? after.length : null);
  const blockOf: Record<number, number> = {};
  let next = firstBlock;
  for (const p of items) {
    if (p.type === "hunk") {
      blockOf[p.index] = next;
      next += runsIn(p.hunk.lines);
    }
  }
  const firstLine = file.hunks[0] ? (file.hunks[0].newLines === 0 ? file.hunks[0].newStart + 1 : file.hunks[0].newStart) : 1;
  const summary = file.binary ? "binary file, not shown" : file.tooLarge ? "over 1 MB, not shown" : file.hunks.length === 0 ? (file.status === "modified" ? "no line changes (mode only)" : "empty file") : "";

  const body = items.map((p) => {
    if (p.type === "gap") {
      const shown = opened[p.index] && after;
      if (shown) {
        const rows = gapLines(after, p.oldStart, p.newStart, p.count);
        return (
          <div key={`g${p.index}`} className={`dv-block ${layout}`} data-gap="open">
            {layout === "side"
              ? sideRows(rows, 0).map((r, i) => (
                  <div key={i} className="dv-row">
                    <Num n={r.left?.old ?? null} path={file.path} />
                    <Cell line={r.left} side="left" />
                    <Num n={r.right?.new ?? null} path={file.path} onOpen={onOpenEditor} />
                    <Cell line={r.right} side="right" />
                  </div>
                ))
              : rows.map((l, i) => (
                  <div key={i} className="dv-row">
                    <Num n={l.old} path={file.path} />
                    <Num n={l.new} path={file.path} onOpen={onOpenEditor} />
                    <Cell line={l} side="left" />
                  </div>
                ))}
          </div>
        );
      }
      return (
        <div key={`g${p.index}`} className="dv-gap">
          <span>{p.count} unchanged {p.count === 1 ? "line" : "lines"}</span>
          {after ? (
            <button type="button" aria-label={`Show ${p.count} unchanged lines of ${file.path}`} onClick={() => setOpened({ ...opened, [p.index]: true })}>Show</button>
          ) : (
            <span className="dv-hint">(no copy of the file to show them from)</span>
          )}
        </div>
      );
    }
    const h = p.hunk;
    const side = layout === "side" ? sideRows(h.lines, blockOf[p.index]) : null;
    const uni = layout === "unified" ? unifiedRows(h.lines, blockOf[p.index]) : null;
    return (
      <div key={`h${p.index}`} className={`dv-block ${layout}`} data-hunk={p.index}>
        <div className="dv-hunkhead">@@ -{h.oldStart},{h.oldLines} +{h.newStart},{h.newLines} @@ {h.header}</div>
        {side?.map((r, i) => (
          <div key={i} className={`dv-row${r.block !== null ? " start" : ""}${r.block !== null && r.block === current ? " current" : ""}`} data-change={r.block ?? undefined}>
            <Num n={r.left?.old ?? null} path={file.path} />
            <Cell line={r.left} side="left" />
            <Num n={r.right?.new ?? null} path={file.path} onOpen={onOpenEditor} />
            <Cell line={r.right} side="right" />
          </div>
        ))}
        {uni?.map((r, i) => (
          <div key={i} className={`dv-row${r.block !== null ? " start" : ""}${r.block !== null && r.block === current ? " current" : ""}`} data-change={r.block ?? undefined}>
            <Num n={r.line.old} path={file.path} />
            <Num n={r.line.new} path={file.path} onOpen={onOpenEditor} />
            <Cell line={r.line} side="left" />
          </div>
        ))}
      </div>
    );
  });

  return (
    <section className={`dv-filebox st-${file.status}`} id={`dv-file-${file.path}`} data-path={file.path} aria-label={file.path}>
      <header className="dv-filehead">
        <span className={`dv-mark st-${file.status}`}>{MARK[file.status]}</span>
        <span className="dv-path">{file.oldPath ? `${file.oldPath} -> ${file.path}` : file.path}</span>
        {file.added !== null && (
          <span className="dv-counts">
            <span className="dv-plus">+{file.added}</span> <span className="dv-minus">-{file.removed}</span>
          </span>
        )}
        <span className="spacer" />
        {onOpenEditor && file.status !== "deleted" && (
          <button type="button" onClick={() => onOpenEditor(file.path, firstLine)} aria-label={`Open ${file.path} in the editor`}>Open in editor</button>
        )}
        {onRevert && (
          <button type="button" className="danger" onClick={() => onRevert(file.path)} aria-label={`Revert ${file.path}`}>Revert file</button>
        )}
      </header>
      {summary && <div className="dv-summary">{summary}</div>}
      {body}
    </section>
  );
}

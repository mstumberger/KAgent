// What the diff view knows about the daemon's diff (diff.md section 1): the types, the tree of files, and the rows of a
// file in both layouts. Nothing here reads the DOM.

export type Status = "added" | "modified" | "deleted" | "renamed";
export type Kind = "ctx" | "add" | "del";

export interface DiffLine {
  kind: Kind;
  old: number | null;
  new: number | null;
  text: string;
  /** `[start, end)` in characters of `text` that differ from the paired line. */
  ranges?: [number, number][];
}

export interface Hunk {
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  header: string;
  lines: DiffLine[];
}

export interface FileDiff {
  path: string;
  oldPath: string | null;
  status: Status;
  binary: boolean;
  tooLarge: boolean;
  added: number | null;
  removed: number | null;
  hunks: Hunk[];
}

export interface Diff {
  files: FileDiff[];
  added: number;
  removed: number;
  truncated: boolean;
}

/** The two texts of a file, so the unchanged lines between hunks can be shown. */
export interface Texts {
  before: string | null;
  after: string | null;
}

export type Layout = "side" | "unified";

const KEY = "kagent.diff.layout";

export function readLayout(): Layout {
  try {
    const v = localStorage.getItem(KEY);
    if (v === "side" || v === "unified") return v;
  } catch {
    // no storage: side by side
  }
  return "side";
}

export function storeLayout(layout: Layout) {
  try {
    localStorage.setItem(KEY, layout);
  } catch {
    // no storage: not remembered
  }
}

/** The lines of a text as the daemon splits them. */
export function splitLines(text: string): string[] {
  if (!text) return [];
  const body = text.endsWith("\n") ? text.slice(0, -1) : text;
  return body.split("\n").map((l) => (l.endsWith("\r") ? l.slice(0, -1) : l));
}

export const MARK: Record<Status, string> = { added: "A", modified: "M", deleted: "D", renamed: "R" };

// ---------------------------------------------------------------- the tree

export interface TreeNode {
  name: string;
  /** Set on a file. */
  file?: FileDiff;
  children: TreeNode[];
}

/** The files as folders and files; a folder that holds only one folder is joined to it (`src/a`). */
export function buildTree(files: FileDiff[]): TreeNode[] {
  const root: TreeNode = { name: "", children: [] };
  for (const f of files) {
    const parts = f.path.split("/");
    let at = root;
    parts.forEach((part, i) => {
      const last = i === parts.length - 1;
      let next = at.children.find((c) => c.name === part && !!c.file === last);
      if (!next) {
        next = { name: part, children: [], ...(last ? { file: f } : {}) };
        at.children.push(next);
      }
      at = next;
    });
  }
  const join = (node: TreeNode): TreeNode => {
    node.children = node.children.map(join);
    while (!node.file && node.name && node.children.length === 1 && !node.children[0].file) {
      node.name = `${node.name}/${node.children[0].name}`;
      node.children = node.children[0].children;
    }
    return node;
  };
  join(root);
  const order = (nodes: TreeNode[]) => {
    nodes.sort((a, b) => Number(!!a.file) - Number(!!b.file) || a.name.localeCompare(b.name));
    nodes.forEach((n) => order(n.children));
  };
  order(root.children);
  return root.children;
}

// ---------------------------------------------------------------- rows

/** A row of the side-by-side layout. `block` is the number of the change block that starts on this row. */
export interface SideRow {
  left: DiffLine | null;
  right: DiffLine | null;
  block: number | null;
}

export interface UnifiedRow {
  line: DiffLine;
  block: number | null;
}

/** One part of a file: a hunk, or the unchanged lines between hunks (`gap`). */
export type Part = { type: "hunk"; hunk: Hunk; index: number } | { type: "gap"; index: number; oldStart: number; newStart: number; count: number; known: boolean };

const endNew = (h: Hunk) => (h.newLines === 0 ? h.newStart : h.newStart + h.newLines - 1);
const endOld = (h: Hunk) => (h.oldLines === 0 ? h.oldStart : h.oldStart + h.oldLines - 1);
const firstNew = (h: Hunk) => (h.newLines === 0 ? h.newStart + 1 : h.newStart);

/** The hunks and the gaps around them. The last gap runs to the end of the new text, when it is known (`total`). */
export function parts(file: FileDiff, total: number | null): Part[] {
  const out: Part[] = [];
  let newAt = 1;
  let oldAt = 1;
  file.hunks.forEach((h, i) => {
    const count = firstNew(h) - newAt;
    if (count > 0) out.push({ type: "gap", index: i, oldStart: oldAt, newStart: newAt, count, known: true });
    out.push({ type: "hunk", hunk: h, index: i });
    newAt = endNew(h) + 1;
    oldAt = endOld(h) + 1;
  });
  if (file.hunks.length > 0 && total !== null && total >= newAt) out.push({ type: "gap", index: file.hunks.length, oldStart: oldAt, newStart: newAt, count: total - newAt + 1, known: true });
  return out;
}

/** The unchanged lines of a gap as context lines, from the new text. */
export function gapLines(after: string[], oldStart: number, newStart: number, count: number): DiffLine[] {
  return Array.from({ length: count }, (_, i) => ({ kind: "ctx" as const, old: oldStart + i, new: newStart + i, text: after[newStart - 1 + i] ?? "" }));
}

/** How many runs of changed lines a hunk has. */
export function runsIn(lines: DiffLine[]): number {
  let n = 0;
  let inRun = false;
  for (const l of lines) {
    const changed = l.kind !== "ctx";
    if (changed && !inRun) n++;
    inRun = changed;
  }
  return n;
}

export function countChanges(files: FileDiff[]): number {
  return files.reduce((n, f) => n + f.hunks.reduce((m, h) => m + runsIn(h.lines), 0), 0);
}

/** Side-by-side rows: a removed line and an added line of one run share a row, in order; the rest face an empty cell. */
export function sideRows(lines: DiffLine[], firstBlock: number): SideRow[] {
  const rows: SideRow[] = [];
  let block = firstBlock;
  let i = 0;
  while (i < lines.length) {
    if (lines[i].kind === "ctx") {
      rows.push({ left: lines[i], right: lines[i], block: null });
      i++;
      continue;
    }
    const dels: DiffLine[] = [];
    const adds: DiffLine[] = [];
    while (i < lines.length && lines[i].kind !== "ctx") {
      (lines[i].kind === "del" ? dels : adds).push(lines[i]);
      i++;
    }
    const n = Math.max(dels.length, adds.length);
    for (let k = 0; k < n; k++) rows.push({ left: dels[k] ?? null, right: adds[k] ?? null, block: k === 0 ? block : null });
    block++;
  }
  return rows;
}

export function unifiedRows(lines: DiffLine[], firstBlock: number): UnifiedRow[] {
  let block = firstBlock;
  let inRun = false;
  return lines.map((line) => {
    const changed = line.kind !== "ctx";
    const start = changed && !inRun;
    inRun = changed;
    return { line, block: start ? block++ : null };
  });
}

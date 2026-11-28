// The files of a diff as a tree with a mark and the counts on each (diff.md section 5).
import { MARK, buildTree, type FileDiff, type TreeNode } from "./model";

interface Props {
  files: FileDiff[];
  current: string | null;
  onPick: (path: string) => void;
}

function Counts({ f }: { f: FileDiff }) {
  if (f.binary) return <span className="dv-counts">binary</span>;
  if (f.added === null) return <span className="dv-counts">large</span>;
  return (
    <span className="dv-counts">
      <span className="dv-plus">+{f.added}</span> <span className="dv-minus">-{f.removed}</span>
    </span>
  );
}

function Node({ node, current, onPick }: { node: TreeNode; current: string | null; onPick: (path: string) => void }) {
  if (node.file) {
    const f = node.file;
    return (
      <li>
        <button type="button" className={`dv-file st-${f.status}`} aria-current={current === f.path ? "true" : undefined} title={f.oldPath ? `${f.oldPath} -> ${f.path}` : f.path} onClick={() => onPick(f.path)}>
          <span className={`dv-mark st-${f.status}`} aria-label={f.status}>{MARK[f.status]}</span>
          <span className="dv-name">{node.name}</span>
          <Counts f={f} />
        </button>
      </li>
    );
  }
  return (
    <li>
      <div className="dv-dir">{node.name}</div>
      <ul>
        {node.children.map((c) => (
          <Node key={c.name + (c.file ? "f" : "d")} node={c} current={current} onPick={onPick} />
        ))}
      </ul>
    </li>
  );
}

export default function Tree({ files, current, onPick }: Props) {
  const tree = buildTree(files);
  return (
    <div className="dv-tree" role="navigation" aria-label="Changed files">
      <ul>
        {tree.map((n) => (
          <Node key={n.name + (n.file ? "f" : "d")} node={n} current={current} onPick={onPick} />
        ))}
      </ul>
    </div>
  );
}

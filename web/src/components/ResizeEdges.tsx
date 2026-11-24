// Invisible resize handles on every edge and corner of KAgent's frameless window
// (the daemon resizes the X11 window while the pointer is held).
import { isAppWindow, startResize } from "../frame";

const EDGES = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];

export default function ResizeEdges() {
  if (!isAppWindow()) return null;
  return (
    <>
      {EDGES.map((e) => (
        <div key={e} className={`rz rz-${e}`} onPointerDown={(ev) => startResize(ev, e)} aria-hidden="true" />
      ))}
    </>
  );
}

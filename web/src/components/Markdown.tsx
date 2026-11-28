// Minimal, safe markdown: builds React elements (never innerHTML), so model output
// cannot inject markup. Supports fenced code, headings, lists, quotes, tables, inline
// code, bold, italic and http(s) links, and one directive,
// `::widget{...}` (widgets.md section 2): it becomes a panel in a sandboxed frame.
import type { ReactNode } from "react";
import Widget from "./Widget";

/** A directive line (widgets.md section 2): `::name{...}` alone on the line. */
const DIRECTIVE = /^::([a-z]+)\{(.*)\}$/;
/** A widget id as the tool writes it: a plain session id, a slash, a number. */
const WIDGET_ID = /^[A-Za-z0-9_-]{1,64}\/[1-9][0-9]{0,2}$/;
/** The marker's own form: `name="value"` pairs, separated by spaces or commas. */
const PAIRS = /^([a-z][a-z0-9_]*\s*=\s*"[^"]*"\s*,?\s*)+$/;

/** What the braces of a directive hold: a JSON object, or the marker's own pairs; `null` for anything else. */
function directiveAttrs(body: string): Record<string, string> | null {
  try {
    const parsed: unknown = body.trim().startsWith("{") ? JSON.parse(body) : JSON.parse(`{${body}}`);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      const attrs: Record<string, string> = {};
      for (const [name, value] of Object.entries(parsed)) {
        if (typeof value !== "string") return null;
        attrs[name] = value;
      }
      return attrs;
    }
  } catch {
    // Not JSON: the marker's own form below.
  }
  if (!PAIRS.test(body)) return null;
  return Object.fromEntries([...body.matchAll(/([a-z][a-z0-9_]*)\s*=\s*"([^"]*)"/g)].map((m) => [m[1], m[2]]));
}

/** `widget` is the only directive. An unknown name, or a body that is not understood, is text. */
function Directive({ name, body, line }: { name: string; body: string; line: string }) {
  const id = name === "widget" ? directiveAttrs(body)?.id : undefined;
  if (id && WIDGET_ID.test(id) && Number(id.slice(id.indexOf("/") + 1)) <= 200) return <Widget id={id} />;
  return <p>{inline(line, "directive")}</p>;
}

function inline(text: string, key: string): ReactNode[] {
  const out: ReactNode[] = [];
  const re = /(`[^`]+`)|(\*\*[^*]+\*\*)|(\*[^*\s][^*]*\*)|(\[[^\]]+\]\(https?:\/\/[^)\s]+\))|(https?:\/\/[^\s)]+)/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let i = 0;
  while ((m = re.exec(text))) {
    if (m.index > last) out.push(text.slice(last, m.index));
    const t = m[0];
    const k = `${key}-${i++}`;
    if (m[1]) out.push(<code key={k}>{t.slice(1, -1)}</code>);
    else if (m[2]) out.push(<strong key={k}>{t.slice(2, -2)}</strong>);
    else if (m[3]) out.push(<em key={k}>{t.slice(1, -1)}</em>);
    else if (m[4]) {
      const [, label, href] = /\[([^\]]+)\]\((.+)\)/.exec(t)!;
      out.push(<a key={k} href={href} target="_blank" rel="noreferrer noopener">{label}</a>);
    } else out.push(<a key={k} href={t} target="_blank" rel="noreferrer noopener">{t}</a>);
    last = m.index + t.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

const SEPARATOR = /^\s*\|?[\s:|-]+\|?\s*$/;

function cells(row: string): string[] {
  return row.trim().replace(/^\|/, "").replace(/\|$/, "").split("|").map((c) => c.trim());
}

/** A pipe table. With a separator as its second row the first is the header; without one, every row is a body row. */
function Table({ rows, id }: { rows: string[]; id: string }) {
  const headed = rows.length > 1 && SEPARATOR.test(rows[1]) && rows[1].includes("-");
  const head = headed ? cells(rows[0]) : null;
  const body = (headed ? rows.slice(2) : rows).filter((r) => !(SEPARATOR.test(r) && r.includes("-"))).map(cells);
  return (
    <div className="md-table">
      <table>
        {head && (
          <thead>
            <tr>{head.map((c, j) => <th key={j}>{inline(c, `${id}-h${j}`)}</th>)}</tr>
          </thead>
        )}
        <tbody>
          {body.map((r, k) => (
            <tr key={k}>{r.map((c, j) => <td key={j}>{inline(c, `${id}-${k}-${j}`)}</td>)}</tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export default function Markdown({ text }: { text: string }) {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const blocks: ReactNode[] = [];
  let i = 0;
  let n = 0;
  const key = () => `b${n++}`;
  while (i < lines.length) {
    const line = lines[i];
    const fence = /^```(\S*)/.exec(line);
    if (fence) {
      const body: string[] = [];
      i++;
      while (i < lines.length && !lines[i].startsWith("```")) body.push(lines[i++]);
      i++;
      blocks.push(
        <pre key={key()} className="md-code" data-lang={fence[1] || undefined}>
          <code>{body.join("\n")}</code>
        </pre>,
      );
      continue;
    }
    const directive = DIRECTIVE.exec(line);
    if (directive) {
      blocks.push(<Directive key={key()} name={directive[1]} body={directive[2]} line={line} />);
      i++;
      continue;
    }
    const h = /^(#{1,4})\s+(.*)$/.exec(line);
    if (h) {
      const level = Math.min(h[1].length + 2, 6);
      const Tag = `h${level}` as "h3";
      blocks.push(<Tag key={key()}>{inline(h[2], `h${n}`)}</Tag>);
      i++;
      continue;
    }
    if (/^\s*([-*+]|\d+[.)])\s+/.test(line)) {
      const ordered = /^\s*\d+[.)]/.test(line);
      // A model often numbers items 1., 2., 3. with blank lines between them: each is its own list, so keep its number.
      const start = ordered ? Number(/^\s*(\d+)/.exec(line)![1]) : undefined;
      const items: string[] = [];
      while (i < lines.length && /^\s*([-*+]|\d+[.)])\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*([-*+]|\d+[.)])\s+/, ""));
        i++;
      }
      const ListTag = ordered ? "ol" : "ul";
      blocks.push(
        <ListTag key={key()} start={start}>
          {items.map((it, j) => (
            <li key={j}>{inline(it, `l${n}-${j}`)}</li>
          ))}
        </ListTag>,
      );
      continue;
    }
    if (line.startsWith(">")) {
      const q: string[] = [];
      while (i < lines.length && lines[i].startsWith(">")) q.push(lines[i++].replace(/^>\s?/, ""));
      blocks.push(<blockquote key={key()}>{inline(q.join(" "), `q${n}`)}</blockquote>);
      continue;
    }
    // A table row starts with a pipe. The closing pipe is not required: while an answer streams, the last row is
    // cut anywhere ("| Budget | GP"), and it must still be consumed here.
    if (/^\s*\|/.test(line)) {
      const t: string[] = [];
      while (i < lines.length && /^\s*\|/.test(lines[i])) t.push(lines[i++]);
      blocks.push(<Table key={key()} rows={t} id={`t${n}`} />);
      continue;
    }
    if (!line.trim()) {
      i++;
      continue;
    }
    const para: string[] = [];
    while (i < lines.length && lines[i].trim() && !/^(```|#{1,4}\s|>|\s*([-*+]|\d+[.)])\s+|\s*\||::[a-z]+\{)/.test(lines[i])) para.push(lines[i++]);
    // Every pass must consume a line. A line that looks like a block start but matched no block above (a directive
    // cut mid-stream, "::widget{") would otherwise be looked at forever: this loop once hung the page that way.
    if (!para.length) para.push(lines[i++]);
    blocks.push(<p key={key()}>{inline(para.join(" "), `p${n}`)}</p>);
  }
  return <div className="md">{blocks}</div>;
}

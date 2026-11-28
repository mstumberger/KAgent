#!/usr/bin/env bash
# The markdown renderer must finish on ANY text, cut anywhere: an answer is rendered again on every streamed piece, so
# every prefix of it is an input. A row of a table cut before its closing pipe once made the renderer loop forever
# (the page hung and ran out of memory). Renders every prefix of sample answers; fails on a hang or a wrong shape.
set -euo pipefail
cd "$(dirname "$0")/../web"
out=$(mktemp -d "$(pwd)/.md-test.XXXXXX")
trap 'rm -rf "$out"' EXIT
cat > "$out/entry.tsx" <<'TSX'
import { renderToStaticMarkup } from "react-dom/server";
import Markdown from "../src/components/Markdown";

const answer = [
  "## Recommended", "", "| Budget | GPU | VRAM |", "|--------|-----|------|", "| **Entry** | RTX 3060 | 12 GB |", "| Mid | `4070` | 16 GB |", "",
  "1. **One** - first", "", "2. **Two** - second", "", "- a", "- b", "", "> quote", "", "```rust", "let x = 1; | not a table", "```", "",
  "::widget{id=\"not an id\"}", "text | with a pipe", "| no closing pipe", "|", "||", "| a | b", "::widget{", "::x{", "#", "#####", ">", "-", "1.", "```",
].join("\n");
const hostile = ["|", "| a", "::widget{", "::a{b", "\n|\n|", "x\n| a | b\n|--", "| a |\n| - |", "|||||", "::", "::{", " | x", "\t| x"];

let failed = 0;
const check = (name: string, ok: boolean) => { if (!ok) { failed++; console.log("FAIL " + name); } };
const render = (text: string) => renderToStaticMarkup(<Markdown text={text} />);

const started = Date.now();
for (let n = 0; n <= answer.length; n++) render(answer.slice(0, n));
for (const h of hostile) render(h);
check("every prefix renders in time", Date.now() - started < 5000);

const full = render(answer);
check("a table with a header", /<table><thead><tr><th>Budget<\/th><th>GPU<\/th><th>VRAM<\/th><\/tr><\/thead>/.test(full));
check("cells take inline marks", full.includes("<td><strong>Entry</strong></td>") && full.includes("<td><code>4070</code></td>"));
check("the separator row is not shown", !full.includes("-----"));
check("numbered items keep their number across blank lines", full.includes('<ol start="2">'));
check("a pipe inside code is not a table", full.includes("let x = 1; | not a table"));
const cut = render("| Budget | GP");
check("a row cut mid-stream is a table row", cut.includes("<td>Budget</td><td>GP</td>"));
check("a directive cut mid-stream is text", render("::widget{").includes("::widget{"));
check("three dashes are a rule, not text", render("a\n\n---\n\nb") === '<div class="md"><p>a</p><hr/><p>b</p></div>');
check("a table without a separator has no header", !render("| a | b |\n| c | d |").includes("<th>"));
console.log(failed ? `MARKDOWN FAILED: ${failed}` : "MARKDOWN OK");
process.exit(failed ? 1 : 0);
TSX
./node_modules/.bin/vite build --ssr "$out/entry.tsx" --outDir "$out/dist" --logLevel error
timeout 30 node "$out"/dist/entry.*js

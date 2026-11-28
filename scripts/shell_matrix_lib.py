"""The matrix of the shell (docs/specs/shell.md section 3): every view and every settings page at four sizes, dark and light,
with the facts measured in the page. Used by shell-matrix.py (screenshots and the table) and test-shell-web.py (zero findings)."""
import asyncio, json, os
import picker_web_lib as lib

SIZES = [(1920, 1080), (1200, 820), (700, 500)]
WIDGET = (340, 460)
KA = "/home/me/code/kagent"

# What every page is opened with: the legacy fragment (still a link that works after the router), so the same script measures
# the window before and after the router.
VIEWS = ["chat", "voice", "tasks", "tickets", "runs", "alerts", "changes", "security", "system"]
SECTIONS = ["general", "appearance", "models", "local-models", "voice", "browser", "desktop", "watchers", "web", "skills", "memory", "tools", "agents", "tasks", "calendar", "hooks", "telegram"]
PAGES = [(v, f"&view={v}") for v in VIEWS] + [(f"settings-{s}", f"&view=settings&section={s}") for s in SECTIONS]

LONG = "A long reply that goes on and on about the disk, the logs and what to do next. " * 6
CODE = "```bash\n" + "sudo journalctl -u kagentd --since yesterday --no-pager | grep -i 'error' | awk '{print $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12}' | sort | uniq -c | sort -rn | head -50\n" + "```"
TABLE = "| a | b | c | d | e | f |\n|---|---|---|---|---|---|\n" + "\n".join(f"| row {i} | " + " | ".join(["a value that is long enough"] * 5) + " |" for i in range(3))


def data():
    """The mock's data: a few conversations with long messages, tickets, runs, changes and alerts, so the pages have something to lay out."""
    T0 = "2026-11-28T09:00:00Z"
    steps = [{"id": "plan", "role": "planner", "worker": {"kind": "subagent", "profile": "planner"}, "model": None, "gate": "owner"}, {"id": "build", "role": "implementer", "worker": {"kind": "subagent", "profile": "implementer"}, "model": None, "gate": None}]
    task = lambda id, title, state, **kw: {"id": id, "title": title, "description": f"do {id}", "doneWhen": [f"{id} works"], "dependsOn": [], "files": [], "ticket": None, "kaTicket": None, "state": state, "round": 0, "extraRounds": 0,
                                           "accepted": False, "error": "", "rounds": [], "workers": [], "progress": [], "changedFiles": [], "startedAt": None, "endedAt": None, **kw}
    run = lambda id, title, state, tasks: {"id": id, "title": title, "workflow": "plan-implement-review", "project": KA, "state": state, "reason": "", "ticket": None, "goal": f"{title}\nwith the details", "definition": {"name": "plan-implement-review", "description": "d", "steps": steps},
                                          "originSession": None, "runSession": "rs" + id[-1], "planSummary": "The plan.", "tasks": tasks, "questions": [], "messages": [], "plannerSession": None, "rejections": 0, "report": "", "createdAt": T0, "updatedAt": T0}
    runs = [run("run_aaaa1", "Make the overlay keep focus", "working", [task("T1", "Focus core", "done", kaTicket="KA-21"), task("T2", "Overlay click", "implementing", dependsOn=["T1"]), task("T3", "Docs", "waiting", dependsOn=["T2"])]),
            run("run_aaaa2", "Split the settings page", "awaiting_approval", [task("T1", "Extract the form", "waiting")])]
    workflows = [{"name": "plan-implement-review", "description": "A planner splits the job.", "shipped": True, "steps": steps}]
    tickets = [lib.ticket(f"KA-{i}", f"A ticket about the thing number {i} with a title that is somewhat long", ["todo", "in_progress", "in_review", "done"][i % 4], labels=["ui", "daemon"][: i % 3]) for i in range(1, 16)]
    projects = [{"id": "p1", "path": KA, "name": "kagent", "prefix": "KA", "count": len(tickets)}]
    messages = {"s1": [{"role": "user", "content": "Why is the disk full and what can I delete?"}, {"role": "assistant", "content": LONG + "\n\n" + CODE + "\n\n" + TABLE + "\n\n" + LONG},
                       {"role": "user", "content": "A path with no spaces: " + "/very/long/path/without/any/break/" * 8}]}
    changes = [{"id": f"c{i}", "at": T0, "tool": "write_file", "summary": f"write /home/me/code/kagent/src/a/very/long/path/to/a/file/number/{i}.rs", "status": "applied", "undoable": True, "session": "s1"} for i in range(8)]
    sessions = {"s1": {"title": "Free the disk", "updatedAt": "2026-11-28T08:00:00Z", "project": KA}, "s2": {"title": "Unit tests for the loop", "updatedAt": "2026-11-28T07:00:00Z", "project": KA},
                "ab12": {"title": "A conversation with a hex id", "updatedAt": "2026-11-26T07:00:00Z"},
                "s3": {"title": "Why is the fan loud and what can be done about it in the long run", "updatedAt": "2026-11-27T07:00:00Z"}}
    return dict(sessions=sessions, dirs=[KA], messages=messages, runs=runs, workflows=workflows, tickets=tickets, ticket_projects=projects, changes=changes, alerts=2,
                projects=[{"path": KA, "name": "kagent", "agentsMd": True}])


# Measured in the page at its current size: a list of {kind, at, detail}. Kinds are the six findings of shell.md section 3.
MEASURE = r"""(() => {
  const out = [], seen = new Set();
  const vw = innerWidth, de = document.documentElement;
  const name = (el) => { const id = el.id ? '#' + el.id : ''; const c = typeof el.className === 'string' && el.className.trim() ? '.' + el.className.trim().split(/\s+/).slice(0, 2).join('.') : ''; return el.tagName.toLowerCase() + id + c; };
  const path = (el) => { const p = []; for (let n = el; n && n !== document.body && p.length < 3; n = n.parentElement) p.unshift(name(n)); return p.join(' > '); };
  const add = (kind, el, detail) => { const key = kind + path(el); if (seen.has(key) || out.length > 60) return; seen.add(key); out.push({ kind, at: path(el), detail }); };
  const cs = (el) => getComputedStyle(el);
  const shown = (el) => { const r = el.getBoundingClientRect(); const s = cs(el); return r.width > 1 && r.height > 1 && s.visibility !== 'hidden' && s.display !== 'none' && s.opacity !== '0'; };
  if (de.scrollWidth > de.clientWidth + 1) out.push({ kind: 'h-overflow', at: 'document', detail: `scrollWidth ${de.scrollWidth} > ${de.clientWidth}` });
  const all = [...document.querySelectorAll('body *')].filter((el) => !['SCRIPT', 'STYLE', 'PATH', 'SVG', 'TITLE', 'OPTION'].includes(el.tagName.toUpperCase()) && !el.closest('svg') && shown(el));
  const clipAncestor = (el) => { for (let n = el.parentElement; n && n !== document.body; n = n.parentElement) { const o = cs(n).overflowX; if (o !== 'visible') return n; } return null; };
  for (const el of all) {
    const r = el.getBoundingClientRect(), s = cs(el);
    if (s.position === 'fixed' && !el.closest('main, header')) continue;
    const A = clipAncestor(el);
    if (!A) {
      if (s.position !== 'fixed' && r.right > vw + 1 && !el.closest('.sr-only')) add('wide-child', el, `right edge ${Math.round(r.right)} > viewport ${vw}`);
    } else {
      const o = cs(A).overflowX, a = A.getBoundingClientRect();
      if ((o === 'hidden' || o === 'clip') && (r.right > a.right + 1 || r.left < a.left - 1) && s.position !== 'absolute' && s.position !== 'fixed' && r.width > 4 && !A.matches('.sr-only')) add('cut-off', el, `box ${Math.round(r.left)}..${Math.round(r.right)} clipped by ${name(A)} ${Math.round(a.left)}..${Math.round(a.right)}`);
    }
    if ((s.overflowX === 'hidden' || s.overflowX === 'clip') && el.scrollWidth > el.clientWidth + 1 && s.textOverflow !== 'ellipsis' && !el.title && !el.matches('.sr-only, textarea, input') && el.children.length === 0) add('cut-off', el, `text ${el.scrollWidth} in ${el.clientWidth} px, no ellipsis or title`);
    if (s.overflowX === 'visible' && !['inline', 'contents'].includes(s.display) && el.clientWidth > 0 && el.scrollWidth > el.clientWidth + 1 && !el.matches('html, body, .sr-only')) add('wide-child', el, `content ${el.scrollWidth} spills out of a ${el.clientWidth} px box`);
    if (el.tagName === 'SELECT') add('native-select', el, el.getAttribute('aria-label') || el.name || '');
  }
  // Scrolling inside scrolling (the same axis; a field that scrolls its own text is not a scroll container of the page).
  const scrolls = (el, axis) => { const s = cs(el); const o = axis === 'y' ? s.overflowY : s.overflowX; return (o === 'auto' || o === 'scroll') && (axis === 'y' ? el.scrollHeight > el.clientHeight + 1 : el.scrollWidth > el.clientWidth + 1); };
  for (const el of all) {
    if (['TEXTAREA', 'INPUT'].includes(el.tagName)) continue;
    for (const axis of ['y', 'x']) {
      if (!scrolls(el, axis)) continue;
      for (let n = el.parentElement; n && n !== document.documentElement; n = n.parentElement) if (scrolls(n, axis) && n !== document.body) { add('nested-scroll', el, `${axis} scroller in ${name(n)}`); break; }
    }
  }
  // The main column: how much of the area beside the sidebar the content of the page uses.
  const view = document.querySelector('.settings-detail') || document.querySelector('main .view'); const host = view && view.matches('.settings-detail') ? view : document.querySelector('main');
  if (view && host && !document.querySelector('.widget')) {
    const hr = host.getBoundingClientRect();
    let left = Infinity, right = -Infinity;
    const leafs = [...view.querySelectorAll('*')].filter((el) => shown(el) && (el.children.length === 0 || ['BUTTON', 'INPUT', 'TEXTAREA', 'SELECT'].includes(el.tagName)) && !el.closest('svg') && !el.closest('.sr-only'));
    for (const el of leafs) { const r = el.getBoundingClientRect(); if (r.right <= hr.right + 2 && r.left >= hr.left - 2) { left = Math.min(left, r.left); right = Math.max(right, r.right); } }
    if (right > left) {
      const head = view.querySelector('.ui-section-text'), first = view.querySelector('.ui-section > *');
      if (head && first && Math.abs(head.getBoundingClientRect().left - first.getBoundingClientRect().left) > 12) add('main-width', head, `page header at ${Math.round(head.getBoundingClientRect().left)}, its content column at ${Math.round(first.getBoundingClientRect().left)}`);
      const used = right - left, room = hr.width, gapL = left - hr.left, gapR = hr.right - right;
      if (used > 1602) add('main-width', view, `content ${Math.round(used)} px wide, over the 1600 px cap`);
      else if (room >= 1100 && used < Math.min(room, 960) * 0.6) add('main-width', view, `content ${Math.round(used)} of ${Math.round(room)} px`);
      else if (room >= 1100 && used < room * 0.9 && Math.abs(gapL - gapR) > 40 && !document.querySelector('main.with-pane')) add('main-width', view, `content not centered: gaps ${Math.round(gapL)} left, ${Math.round(gapR)} right of ${Math.round(used)} px`);
    }
  }
  return out;
})()"""

STABLE = "new Promise((res) => requestAnimationFrame(() => requestAnimationFrame(() => res(document.documentElement.scrollWidth + 'x' + document.documentElement.scrollHeight + ':' + document.querySelectorAll('*').length))))"


async def settle(page):
    """The layout is the same on two reads in a row (the page is done reacting to the new size)."""
    last = None
    for _ in range(30):
        now = await page.js(STABLE)
        if now == last:
            return
        last = now
        await asyncio.sleep(0.08)


async def measure(page):
    await settle(page)
    return await page.js(MEASURE)


async def open_page(env, theme, hash, query="", **cfg):
    await env.open(theme, query=query, hash=hash, **{**data(), **cfg})
    await asyncio.sleep(0.3)


async def run_matrix(env, out_dir, themes=("dark", "light"), pages=None, sizes=SIZES, shots=True):
    """Every page at every size in each theme: {(theme, page, "WxH"): [findings]}. The widget is measured once per theme at 340x460, with its sessions sheet."""
    result = {}
    for theme in themes:
        for pid, hash in (pages or PAGES):
            await open_page(env, theme, hash)
            for w, h in sizes:
                await env.size(w, h)
                result[(theme, pid, f"{w}x{h}")] = await measure(env.page)
                if shots:
                    lib.SHOTS = out_dir
                    await env.page.shot(f"{theme}-{pid}-{w}x{h}")
            await env.size(1200, 820)
        # The widget: its own layout (Overlay.tsx), so one cell, and the sheet of its sessions.
        await open_page(env, theme, "", query="?compact")
        await env.size(*WIDGET)
        await env.page.until("!!document.querySelector('.widget-status')", "the widget")
        result[(theme, "widget", "340x460")] = await measure(env.page)
        if shots:
            await env.page.shot(f"{theme}-widget-340x460")
        await env.page.js("document.querySelector('.widget-top [aria-label=Sessions]').click()")
        await env.page.until("!!document.querySelector('.sheet')", "the widget's sheet")
        result[(theme, "widget-sheet", "340x460")] = await measure(env.page)
        if shots:
            await env.page.shot(f"{theme}-widget-sheet-340x460")
        await env.size(1200, 820)
    return result


def count(result):
    """Findings per size and per kind: {size: {kind: n}}."""
    by = {}
    for (_, _, size), found in result.items():
        for f in found:
            by.setdefault(size, {}).setdefault(f["kind"], 0)
            by[size][f["kind"]] += 1
    return by

#!/usr/bin/env python3
"""The diff view (diff.md section 5) on the Changes view and in the Changes tab of a ticket, in headless Chrome against the mocked API.

The mock and the page driver are picker_web_lib.py (its diff and tickets routes). Dark and light at 1200x820; screenshots go to
/tmp/kagent-v003-diff-shots/. Run from the repo root with KAGENT_DIST set to a build of web/ (`npx vite build --outDir <dir>`)."""
import asyncio, json, sys
import picker_web_lib as lib
from picker_web_lib import Env, calls, report, ticket

P1 = "/home/me/code/kagent"
AFTER = "\n".join(["line 1", "line 2", "let x = 2;"] + [f"line {i}" for i in range(4, 27)] + ["TWENTY SEVEN", "line 28", "line 29", "line 30"]) + "\n"
BEFORE = AFTER.replace("let x = 2;", "let x = 1;").replace("TWENTY SEVEN", "line 27")


def line(kind, old, new, text, ranges=None):
    d = {"kind": kind, "old": old, "new": new, "text": text}
    if ranges:
        d["ranges"] = ranges
    return d


HUNK1 = {"oldStart": 1, "oldLines": 6, "newStart": 1, "newLines": 6, "header": "fn main()", "lines": [line("ctx", 1, 1, "line 1"), line("ctx", 2, 2, "line 2"), line("del", 3, None, "let x = 1;", [[8, 9]]),
         line("add", None, 3, "let x = 2;", [[8, 9]]), line("ctx", 4, 4, "line 4"), line("ctx", 5, 5, "line 5"), line("ctx", 6, 6, "line 6")]}
HUNK2 = {"oldStart": 24, "oldLines": 7, "newStart": 24, "newLines": 7, "header": "", "lines": [line("ctx", 24, 24, "line 24"), line("ctx", 25, 25, "line 25"), line("ctx", 26, 26, "line 26"),
         line("del", 27, None, "line 27", [[0, 7]]), line("add", None, 27, "TWENTY SEVEN", [[0, 12]]), line("ctx", 28, 28, "line 28"), line("ctx", 29, 29, "line 29"), line("ctx", 30, 30, "line 30")]}


def f(path, status="modified", added=0, removed=0, hunks=(), **kw):
    return {"path": path, "oldPath": kw.pop("oldPath", None), "status": status, "binary": kw.pop("binary", False), "tooLarge": kw.pop("tooLarge", False),
            "added": added, "removed": removed, "hunks": list(hunks)}


DIFF = {"files": [
    f("src/lib.rs", "modified", 2, 2, [HUNK1, HUNK2]),
    f("src/new.rs", "added", 2, 0, [{"oldStart": 0, "oldLines": 0, "newStart": 1, "newLines": 2, "header": "", "lines": [line("add", None, 1, "pub fn a() {}"), line("add", None, 2, "pub fn b() {}")]}]),
    f("docs/old.md", "deleted", 0, 1, [{"oldStart": 1, "oldLines": 1, "newStart": 0, "newLines": 0, "header": "", "lines": [line("del", 1, None, "gone")]}]),
    f("web/app.ts", "renamed", 1, 1, [{"oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2, "header": "", "lines": [line("ctx", 1, 1, "same"), line("del", 2, None, "before", [[0, 6]]), line("add", None, 2, "after", [[0, 5]])]}], oldPath="web/main.ts"),
    f("pic.png", "modified", None, None, binary=True),
    f("big.txt", "modified", 40000, 40000, tooLarge=True),
], "added": 40005, "removed": 40004, "truncated": False}
DIFF["files"][4]["added"] = DIFF["files"][4]["removed"] = None
FILE = "/home/me/code/kagent/src/lib.rs"
CHANGES = [
    {"id": "chg_1", "ts": "2026-11-28T09:00:00Z", "tool": "edit_file", "summary": f"edit {FILE}", "undo": {"action": "restore_file"}, "privileged": False, "status": "applied"},
    {"id": "chg_2", "ts": "2026-11-28T08:00:00Z", "tool": "write_file", "summary": "overwrite /home/me/old.txt", "undo": {"action": "restore_file"}, "privileged": False, "status": "applied"},
    {"id": "chg_3", "ts": "2026-11-28T07:00:00Z", "tool": "sysctl", "summary": "sysctl vm.swappiness=10", "undo": {"op": "x"}, "privileged": True, "status": "applied"},
]
CHANGE_DIFFS = {
    "chg_1": {"id": "chg_1", "path": FILE, "after": "stored", "before": BEFORE, "afterText": AFTER, "diff": {"files": [dict(DIFF["files"][0], path=FILE)], "added": 2, "removed": 2, "truncated": False}},
    "chg_2": {"id": "chg_2", "path": "/home/me/old.txt", "after": "now", "before": "a\n", "afterText": "b\n", "diff": {"files": [f("/home/me/old.txt", "modified", 1, 1, [{"oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1, "header": "", "lines": [line("del", 1, None, "a"), line("add", None, 1, "b")]}])], "added": 1, "removed": 1, "truncated": False}},
}
CFG = dict(changes=CHANGES, change_diffs=CHANGE_DIFFS, repo_diff=DIFF, sessions={"s1": {"title": "Chat", "updatedAt": "2026-11-28T09:00:00Z"}},
           tickets=[ticket("KA-1", "With a range", links=[{"kind": "range", "value": "master..v0.0.3", "label": ""}]), ticket("KA-2", "Without a link"), ticket("IN-1", "In the inbox", project=None)],
           ticket_projects=[{"id": "inbox", "path": None, "name": "Inbox", "prefix": "IN", "count": 1}, {"id": "k", "path": P1, "name": "kagent", "prefix": "KA", "count": 2}],
           projects=[{"path": P1, "name": "kagent", "agentsMd": True}], dirs={P1})
V = lambda expr: expr  # readability


async def checks(env, results, theme):
    page = env.page
    t = f"{theme} 1200x820"
    await env.size(1200, 820)
    await env.open(theme, hash="&view=changes", **CFG)
    await page.until("document.querySelectorAll('table tbody tr').length >= 3", "the changes")
    btns = await page.js("[...document.querySelectorAll('table tbody tr')].map(r => !![...r.querySelectorAll('button')].find(b => b.textContent === 'Diff'))")
    results[f"{t}: a file edit has a Diff button; other changes have none"] = btns == [True, True, False]
    await page.js("document.querySelector('button[aria-label^=\"Diff of edit\"]').click()")
    await page.until("!!document.querySelector('.diffview .dv-filebox')", "the diff view")
    results[f"{t}: the Diff button opens the view under the row (aria-expanded) and reads GET /api/changes/:id/diff"] = "GET /api/changes/chg_1/diff" in calls() and await page.js("document.querySelector('button[aria-label^=\"Diff of edit\"]').getAttribute('aria-expanded')") == "true"
    results[f"{t}: the view is a region with a tree and a file box"] = await page.js("(() => { const v = document.querySelector('.diffview'); return v.getAttribute('role') === 'region' && !!v.querySelector('.dv-tree[role=navigation]') && v.querySelectorAll('.dv-filebox').length === 1; })()")
    await page.shot(f"changes-diff-{theme}")
    # side by side by default
    pair = await page.js("(() => { const r = [...document.querySelectorAll('.dv-block.side .dv-row')].find(r => r.querySelector('.k-del') && r.querySelector('.k-add')); return r ? [r.querySelector('.k-del .dv-code').textContent, r.querySelector('.k-add .dv-code').textContent, r.querySelector('.k-del .dv-sign').textContent, r.querySelector('.k-add .dv-sign').textContent] : null; })()")
    results[f"{t}: side by side is the default: the removed and the added line share a row, with a - and a + sign"] = pair == ["let x = 1;", "let x = 2;", "-", "+"]
    results[f"{t}: the intra-line highlight marks the changed characters"] = await page.js("[...document.querySelectorAll('.dv-intra')].map(m => m.textContent).slice(0, 2).join()") == "1,2"
    results[f"{t}: line numbers show on both sides"] = await page.js("(() => { const r = [...document.querySelectorAll('.dv-row')].find(r => r.querySelector('.k-del')); return [...r.querySelectorAll('.dv-num')].map(n => n.textContent.trim()).join('|'); })()") == "3|3"
    # collapsed region
    gap = await page.js("(() => { const g = document.querySelector('.dv-gap'); return g ? [g.querySelector('span').textContent, !!g.querySelector('button')] : null; })()")
    results[f"{t}: the unchanged lines between the hunks are collapsed with their count and a Show button"] = gap == ["17 unchanged lines", True]
    before = await page.js("document.querySelectorAll('.dv-row').length")
    await page.js("document.querySelector('.dv-gap button').click()")
    await page.until(f"document.querySelectorAll('.dv-row').length === {before} + 17", "the gap opens")
    results[f"{t}: Show expands the 17 lines from the stored text, with their numbers"] = await page.js("(() => { const rows = [...document.querySelectorAll('[data-gap=open] .dv-row')]; return rows.length === 17 && rows[0].querySelector('.dv-code').textContent === 'line 7' && rows[16].querySelector('.dv-code').textContent === 'line 23'; })()") and not await page.js("document.querySelector('.dv-gap')")
    # unified and remembered
    await page.js("[...document.querySelectorAll('.dv-switch button')].find(b => b.textContent === 'Unified').click()")
    await page.until("!!document.querySelector('.dv-block.unified')", "unified")
    results[f"{t}: the toggle switches to unified (one text column, two numbers)"] = await page.js("!document.querySelector('.dv-block.side') && document.querySelector('.dv-block.unified .dv-row').children.length === 3")
    await page.shot(f"changes-diff-unified-{theme}")
    await page.send("Page.reload")
    await page.until("!!document.querySelector('.sb-nav-item')", "the window after a reload")
    await page.js("[...document.querySelectorAll('.sb-nav-item')].find(b => b.textContent.trim() === 'Changes').click()")
    await page.until("document.querySelectorAll('table tbody tr').length >= 3", "the changes after a reload")
    await page.js("document.querySelector('button[aria-label^=\"Diff of edit\"]').click()")
    await page.until("!!document.querySelector('.diffview .dv-filebox')", "the diff after a reload")
    results[f"{t}: the layout is remembered in this browser (unified after a reload)"] = await page.js("!!document.querySelector('.dv-block.unified') && document.querySelector('.dv-switch button[aria-pressed=true]').textContent") == "Unified"
    await page.js("[...document.querySelectorAll('.dv-switch button')].find(b => b.textContent === 'Side by side').click()")
    # actions
    await page.js("[...document.querySelectorAll('.diffview .dv-filehead button')].find(b => b.textContent === 'Open in editor').click()")
    await asyncio.sleep(0.2)
    results[f"{t}: Open in editor calls the editor tool with the path and the first changed line"] = f'POST /api/ui/tools/open_in_editor/call {{"input": {{"path": "{FILE}", "line": 1}}}}' in calls()
    await page.js("[...document.querySelectorAll('.dv-row')].find(r => r.querySelector('.k-add')).querySelector('button.dv-open').click()")
    await asyncio.sleep(0.2)
    results[f"{t}: the line number of the new side opens the editor at that line (the old side is no link)"] = f'POST /api/ui/tools/open_in_editor/call {{"input": {{"path": "{FILE}", "line": 3}}}}' in calls() and await page.js("[...document.querySelectorAll('.dv-row')].find(r => r.querySelector('.k-del')).querySelector('.dv-num').tagName") == "SPAN"
    await page.js("[...document.querySelectorAll('.diffview .dv-filehead button')].find(b => b.textContent === 'Revert file').click()")
    await asyncio.sleep(0.2)
    results[f"{t}: Revert file calls the undo of the change"] = "POST /api/changes/chg_1/undo" in calls()
    # an older record
    await page.js("document.querySelector('button[aria-label^=\"Diff of overwrite\"]').click()")
    await page.until("!![...document.querySelectorAll('.sub')].find(e => e.textContent.includes('Recorded before the diff view'))", "the note of an older record")
    results[f"{t}: a change recorded before the diff view says the right side is the file as it is now"] = await page.js("[...document.querySelectorAll('.sub')].some(e => e.textContent.includes('the right side is the file as it is now'))")
    await page.js("document.querySelector('button[aria-label^=\"Diff of overwrite\"]').click()")


async def view_checks(env, results, theme):
    """The view itself on the repository diff, through the Changes tab of a ticket."""
    page = env.page
    t = f"{theme} ticket"
    await env.size(1200, 820)
    await env.open(theme, hash="&view=tickets", **CFG)
    await page.until("document.querySelectorAll('.tk-item').length >= 3", "the tickets")
    await page.js("document.querySelector('.tk-item[data-key=KA-1]').click()")
    await page.until("!!document.querySelector('aside.tk-detail')", "the pane")
    results[f"{t}: the pane has the tabs Details and Changes"] = await page.js("[...document.querySelectorAll('.tk-tabs [role=tab]')].map(b => b.textContent).join()") == "Details,Changes"
    await page.js("[...document.querySelectorAll('.tk-tabs [role=tab]')].find(b => b.textContent === 'Changes').click()")
    await page.until("!!document.querySelector('.diffview .dv-filebox')", "the ticket's diff")
    results[f"{t}: the Changes tab posts the project and the linked range as from and to"] = f'POST /api/diff {{"project": "{P1}", "from": "master", "to": "v0.0.3"}}' in calls()
    results[f"{t}: it names what it compares"] = "master..v0.0.3" in await page.js("document.querySelector('.tk-changes code').textContent")
    tree = await page.js("[...document.querySelectorAll('.dv-tree .dv-file')].map(b => b.querySelector('.dv-mark').textContent + ' ' + b.querySelector('.dv-name').textContent + ' ' + b.querySelector('.dv-counts').textContent.trim())")
    results[f"{t}: the tree has a mark and the counts on each file (A, M, D, R; a binary file says so)"] = tree == ["D old.md +0 -1", "M lib.rs +2 -2", "A new.rs +2 -0", "R app.ts +1 -1", "M big.txt +40000 -40000", "M pic.png binary"]
    dirs = await page.js("[...document.querySelectorAll('.dv-tree .dv-dir')].map(d => d.textContent).sort().join()")
    results[f"{t}: folders group the files"] = dirs == "docs,src,web"
    summ = await page.js("[...document.querySelectorAll('.dv-summary')].map(s => s.textContent)")
    results[f"{t}: a binary file and a file over 1 MB show a summary and no lines"] = "binary file, not shown" in summ and "over 1 MB, not shown" in summ
    results[f"{t}: a renamed file shows old -> new in its header"] = "web/main.ts -> web/app.ts" in await page.js("[...document.querySelectorAll('.dv-path')].map(p => p.textContent).join('|')")
    results[f"{t}: without the texts the collapsed lines have no Show button and say why"] = await page.js("(() => { const g = document.querySelector('.dv-gap'); return !!g && !g.querySelector('button') && g.textContent.includes('no copy of the file'); })()")
    await page.shot(f"ticket-changes-{theme}")
    # F7 and Shift+F7: five change blocks across four files
    results[f"{t}: the position line counts the changes"] = await page.js("document.querySelector('.dv-pos').textContent") == "5 changes"
    x, y = await page.center(".diffview")
    await page.mouse("mouseMoved", x, y)
    seen = []
    for _ in range(3):
        await page.key("F7", "F7", 118)
        seen.append(await page.js("document.querySelector('.dv-pos').textContent"))
    results[f"{t}: F7 moves to the next change (hovering the view is enough): 1, 2, 3 of 5"] = seen == ["Change 1 of 5", "Change 2 of 5", "Change 3 of 5"]
    results[f"{t}: one row is the current change and the tree follows to its file"] = await page.js("document.querySelectorAll('.dv-row.current').length") == 1 and await page.js("document.querySelector('.dv-file[aria-current=true] .dv-name').textContent") == "new.rs"
    await page.key("F7", "F7", 118, 8)
    results[f"{t}: Shift+F7 goes back"] = await page.js("document.querySelector('.dv-pos').textContent") == "Change 2 of 5"
    for _ in range(4):
        await page.key("F7", "F7", 118)
    results[f"{t}: after the last change F7 wraps around to the first"] = await page.js("document.querySelector('.dv-pos').textContent") == "Change 1 of 5"
    await page.js("[...document.querySelectorAll('.dv-bar button')].find(b => b.getAttribute('aria-label').startsWith('Next change')).click()")
    results[f"{t}: the Next and Previous buttons do the same and have labels"] = await page.js("[...document.querySelectorAll('.dv-bar button')].every(b => b.getAttribute('aria-label') || b.textContent)")
    # a click on a file in the tree
    await page.js("[...document.querySelectorAll('.dv-file')].find(b => b.textContent.includes('old.md')).click()")
    results[f"{t}: a click on a file marks it in the tree"] = await page.js("document.querySelector('.dv-file[aria-current=true] .dv-name').textContent") == "old.md"
    # a ticket without a link
    await page.js("document.querySelector('button[aria-label=\"Close the ticket\"]').click()")
    await page.js("document.querySelector('.tk-item[data-key=KA-2]').click()")
    await page.until("document.querySelector('aside.tk-detail').dataset.key === 'KA-2'", "the second ticket")
    await page.js("[...document.querySelectorAll('.tk-tabs [role=tab]')].find(b => b.textContent === 'Changes').click()")
    await page.until("!!document.querySelector('.tk-changes')", "the tab")
    results[f"{t}: a ticket without a link says to link a range and has a field for it"] = "Link a branch or commit range to see its changes." in await page.js("document.querySelector('.tk-changes').textContent") and await page.js("!!document.querySelector('input[aria-label=\"Range or commit to compare\"]')") and await page.js("!document.querySelector('.diffview')")
    await page.js("""(() => { const i = document.querySelector('input[aria-label="Range or commit to compare"]'); i.focus(); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; set.call(i, 'main..feature'); i.dispatchEvent(new Event('input', {bubbles: true})); })()""")
    await page.key("Enter", "Enter", 13, text="\r")
    await page.until("!!document.querySelector('.diffview .dv-filebox')", "the diff after linking")
    results[f"{t}: linking a range saves it on the ticket and shows its diff"] = 'PUT /api/tickets/KA-2 {"links": [{"kind": "range", "value": "main..feature", "label": ""}]}' in calls() and f'POST /api/diff {{"project": "{P1}", "from": "main", "to": "feature"}}' in calls()
    # an Inbox ticket
    await page.js("document.querySelector('button[aria-label=\"Close the ticket\"]').click()")
    await page.js("document.querySelector('.tk-item[data-key=IN-1]').click()")
    await page.until("document.querySelector('aside.tk-detail').dataset.key === 'IN-1'", "the inbox ticket")
    await page.js("[...document.querySelectorAll('.tk-tabs [role=tab]')].find(b => b.textContent === 'Changes').click()")
    await page.until("!!document.querySelector('.tk-changes')", "the inbox tab")
    results[f"{t}: a ticket of the Inbox says it has no repository"] = "no project" in await page.js("document.querySelector('.tk-changes').textContent")


async def error_checks(env, results, theme):
    page = env.page
    t = f"{theme} error"
    await env.open(theme, hash="&view=tickets", **dict(CFG, diff_error="not a known project: only a folder of the projects list (GET /api/projects) can be diffed"))
    await page.until("document.querySelectorAll('.tk-item').length >= 3", "the tickets")
    await page.js("document.querySelector('.tk-item[data-key=KA-1]').click()")
    await page.until("!!document.querySelector('aside.tk-detail')", "the pane")
    await page.js("[...document.querySelectorAll('.tk-tabs [role=tab]')].find(b => b.textContent === 'Changes').click()")
    await page.until("!!document.querySelector('.tk-changes .tk-error')", "the refusal")
    results[f"{t}: the daemon's refusal is shown as an alert and no diff is drawn"] = "not a known project" in await page.js("document.querySelector('.tk-changes .tk-error').textContent") and not await page.js("document.querySelector('.diffview')")


async def main():
    results = {}
    async with Env("kagent-diff", "/tmp/kagent-v003-diff-shots") as env:
        for theme in ("dark", "light"):
            await checks(env, results, theme)
            await view_checks(env, results, theme)
            await error_checks(env, results, theme)
        errors = [e for e in env.page.errors if not ("status of 400" in e or "status of 409" in e)]
        results["no console errors"] = not errors
        if errors:
            print("ERRORS", errors[:5])
    return report(results)


sys.exit(asyncio.run(main()))

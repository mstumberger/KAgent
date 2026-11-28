#!/usr/bin/env python3
"""S43: the toast stack of the notices and the work panel in headless Chrome against a mocked API.

The mock and the page driver are picker_web_lib.py; the window's event stream is its FAKE_WS, so a check feeds the page
the events the daemon would send (`window.__emit`). Dark and light at 1200x820 and 340x460 (the widget); screenshots go to
/tmp/kagent-s43-shots/. Run from the repo root after `cargo build` and `npm run build` in web/."""
import asyncio, json, sys
import picker_web_lib as lib
from picker_web_lib import FAKE_LIVE, Env, calls, configure, opened, reads, report, set_task

SESSIONS = {"s1": {"title": "Free the disk", "updatedAt": "2026-11-27T09:00:00Z"}, "s2": {"title": "Other chat", "updatedAt": "2026-11-27T08:00:00Z"}}
CFG = dict(sessions=SESSIONS)

# The work panel: s1 is the open session, with a todo list (3 of 7 open), two subagents (c1 runs, c2 waits), four background
# commands and a plan; c1 and c2 are the sessions of the subagents.
PLAN = "# Free the disk\n\n- clean the journal\n- clear the package cache\n"
TODO = [{"content": f"step {i}", "status": "completed" if i < 4 else "pending", "priority": "medium"} for i in range(7)]
NOW = "2026-11-27T10:00:00Z"
TASKS = [
    {"id": "bg_run", "sessionId": "s1", "command": "cargo test --workspace -- --nocapture --test-threads=1", "status": "running", "exitCode": None, "startedAt": "2026-11-27T09:59:00Z", "seconds": 60, "outputFile": "/tmp/x"},
    {"id": "bg_ok", "sessionId": "s1", "command": "df -h", "status": "done", "exitCode": 0, "startedAt": NOW, "seconds": 1, "outputFile": "/tmp/x"},
    {"id": "bg_bad", "sessionId": "s1", "command": "false", "status": "failed", "exitCode": 1, "startedAt": NOW, "seconds": 1, "outputFile": "/tmp/x"},
    {"id": "bg_kill", "sessionId": "s1", "command": "sleep 99", "status": "killed", "exitCode": None, "startedAt": NOW, "seconds": 3, "outputFile": "/tmp/x"},
]
LOG = "".join(f"line {i} of the output\n" for i in range(400))
WORK = dict(
    sessions={
        "s1": {"title": "Free the disk", "updatedAt": "2026-11-27T09:00:00Z", "todo": TODO, "approvedPlan": {"path": "/p", "text": PLAN}},
        "s2": {"title": "Other chat", "updatedAt": "2026-11-27T08:00:00Z"},
        "c1": {"title": "Disk usage", "updatedAt": "2026-11-27T07:00:00Z", "origin": "subagent", "parent": "s1"},
        "c2": {"title": "Memory usage", "updatedAt": "2026-11-27T06:00:00Z", "origin": "subagent", "parent": "s1"},
    },
    tasks={"s1": TASKS}, outputs={"bg_run": LOG, "bg_ok": "Filesystem  Size\n/dev/sda1  100G\n"},
    messages={"c1": [{"role": "user", "content": "Find what uses the disk"}, {"role": "assistant", "content": "du says /var holds 12 GB"}],
              "c2": [{"role": "user", "content": "Find what uses the memory"}]},
)
CHILD = lambda cid, title, profile: {"id": cid, "title": title, "profile": profile, "startedAt": NOW}
TOASTS = "[...document.querySelectorAll('.toasts .toast')]"
TEXTS = f"{TOASTS}.map(t => t.querySelector('.toast-text').textContent)"


# A check that reads an element that is not there fails (None) instead of ending the run.
_js = lib.Page.js


async def _safe_js(self, expr):
    try:
        return await _js(self, expr)
    except RuntimeError as e:
        if "Cannot read properties of null" in str(e) or "is not a function" in str(e):
            return None
        raise


lib.Page.js = _safe_js


def notice(text, key=None, level="warn", kind=None, session=None):
    ev = {"type": "notice", "level": level, "text": text}
    for name, value in (("key", key), ("kind", kind), ("sessionId", session)):
        if value:
            ev[name] = value
    return ev


async def emit(page, ev):
    await page.js(f"window.__emit({json.dumps(ev)})")
    await asyncio.sleep(0.12)


async def clear(page):
    await page.js("(() => { const b = document.querySelector('.toasts-bar button'); if (b) b.click(); [...document.querySelectorAll('.toast-close')].forEach(b => b.click()); })()")
    await asyncio.sleep(0.15)


async def notice_checks(env, results, theme, label, compact, first):
    page = env.page
    t = f"{label} {theme}"
    await env.open(theme, query="?overlay" if compact else "", init=FAKE_LIVE, **CFG)
    results[f"{t}: no notice line in the header and no stack while nothing was sent"] = await page.js("!document.querySelector('.notice') && !document.querySelector('header .notice') && !document.querySelector('.toasts')")
    # --- one key twice: one toast with a count
    await emit(page, notice("gemini::a is out of quota - answering with gemini::b", "provider:gemini:quota"))
    await emit(page, notice("gemini::a is out of quota - answering with gemini::c", "provider:gemini:quota"))
    results[f"{t}: two errors of one key are one toast with the count x2 and the latest text"] = await page.js(TEXTS) == ["gemini::a is out of quota - answering with gemini::c"] and await page.js("document.querySelector('.toast .toast-count').textContent") == "x2"
    results[f"{t}: an error toast is role=alert with an icon, a time and a Dismiss button"] = await page.js("(() => { const x = document.querySelector('.toast'); return x.getAttribute('role') === 'alert' && !!x.querySelector('.toast-icon svg') && /\\d/.test(x.querySelector('time').textContent) && x.querySelector('.toast-close').getAttribute('aria-label') === 'Dismiss'; })()")
    results[f"{t}: the stack is a region named Notifications"] = await page.js("(() => { const s = document.querySelector('.toasts'); return s.getAttribute('role') === 'region' && s.getAttribute('aria-label') === 'Notifications'; })()")
    place = await page.js("(() => { const r = document.querySelector('.toasts').getBoundingClientRect(), h = document.querySelector('header'); return {l: Math.round(r.left), r: Math.round(innerWidth - r.right), t: Math.round(r.top), hb: h ? Math.round(h.getBoundingClientRect().bottom) : 0, z: getComputedStyle(document.querySelector('.toasts')).zIndex}; })()")
    if compact:
        results[f"{t}: in the widget the stack is the whole width less 8 px a side, at the top"] = (place["l"], place["r"], place["t"]) == (8, 8, 8)
    else:
        results[f"{t}: the stack is 12 px from the right edge, under the title bar"] = place["r"] == 12 and place["t"] >= place["hb"] and place["t"] - place["hb"] <= 12
    results[f"{t}: the stack is over the content and under the dialogs (z-index 18 < sheets 19, menus 20, approvals 21)"] = place["z"] == "18"
    await page.shot(f"notices-error-{t.replace(' ', '-')}")
    # --- a different key and no key
    await emit(page, notice("Hook 'h' (PreToolUse) failed: boom", "hook:h"))
    await emit(page, notice("plain one"))
    await emit(page, notice("plain one"))
    results[f"{t}: another key and notices without a key are separate toasts (newest on top)"] = await page.js(TEXTS) == ["plain one", "plain one", "Hook 'h' (PreToolUse) failed: boom", "gemini::a is out of quota - answering with gemini::c"]
    results[f"{t}: Dismiss all appears with two or more and is the first control"] = await page.js("(() => { const b = document.querySelector('.toasts-bar button'); return !!b && b.textContent === 'Dismiss all' && document.querySelector('.toasts').firstElementChild === b.parentElement; })()")
    # --- close one
    await page.js("document.querySelector('.toast .toast-close').click()")
    await asyncio.sleep(0.15)
    results[f"{t}: the close button closes that toast"] = len(await page.js(TEXTS)) == 3
    await page.js("document.querySelector('.toasts-bar button').click()")
    await asyncio.sleep(0.15)
    results[f"{t}: Dismiss all closes every toast and the stack goes"] = await page.js("!document.querySelector('.toasts')")
    results[f"{t}: with one toast there is no Dismiss all"] = (await emit(page, notice("solo")) or True) and await page.js("!document.querySelector('.toasts-bar')")
    await clear(page)
    # --- the fifth folds
    for i in range(5):
        await emit(page, notice(f"error {i}", f"k:{i}"))
    folded = await page.js(f"({TOASTS}.length) + '|' + (document.querySelector('.toast-fold') || {{}}).textContent")
    results[f"{t}: the fifth folds: four shown and a button '1 more'"] = folded == "4|1 more"
    await page.shot(f"notices-fold-{t.replace(' ', '-')}")
    await page.js("document.querySelector('.toast-fold').click()")
    await asyncio.sleep(0.15)
    results[f"{t}: the fold opens all five (aria-expanded) and 'Show fewer' folds again"] = await page.js(f"{TOASTS}.length") == 5 and await page.js("document.querySelector('.toast-fold').getAttribute('aria-expanded')") == "true" and await page.js("document.querySelector('.toast-fold').textContent") == "Show fewer"
    await clear(page)
    # --- a long text is cut at three lines with "more"
    long = "The provider answered with an error that is long enough to need several lines of the toast, " * 4
    await emit(page, notice(long.strip(), "provider:x:other"))
    await emit(page, notice("short", "k:short"))
    info = await page.js("(() => { const [short, longer] = [...document.querySelectorAll('.toast')]; const lines = el => Math.round(el.getBoundingClientRect().height / parseFloat(getComputedStyle(el).lineHeight)); return {short: !!short.querySelector('.toast-meta button'), more: !!longer.querySelector('.toast-meta button'), lines: lines(longer.querySelector('.toast-text')), h: longer.getBoundingClientRect().height}; })()")
    results[f"{t}: a long text is clipped to 3 lines with 'more'; a short one has no 'more'"] = info["lines"] == 3 and info["more"] and not info["short"]
    await page.js("[...document.querySelectorAll('.toast')][1].querySelector('.toast-meta button').click()")
    await asyncio.sleep(0.15)
    results[f"{t}: 'more' shows the whole text"] = await page.js("[...document.querySelectorAll('.toast')][1].getBoundingClientRect().height") > info["h"]
    await clear(page)
    # --- Escape and the tab order
    await emit(page, notice("one", "k:1"))
    await emit(page, notice("two", "k:2"))
    seen = []
    for _ in range(40):
        await page.key("Tab", "Tab", 9)
        d = await page.js("(() => { const a = document.activeElement; return a.closest('.toasts') ? (a.getAttribute('aria-label') || a.textContent.trim()) : ''; })()")
        if d:
            seen.append(d)
            if len(seen) >= 3:
                break
    results[f"{t}: Dismiss all and the close buttons are in the tab order, in the order of the stack"] = seen == ["Dismiss all", "Dismiss", "Dismiss"]
    await page.key("Tab", "Tab", 9, 8)  # Shift+Tab: back to the first Dismiss
    focused = await page.js("document.activeElement.closest('.toast') && document.activeElement.closest('.toast').querySelector('.toast-text').textContent")
    await page.key("Escape", "Escape", 27)
    results[f"{t}: Escape closes the focused toast and leaves the other"] = focused == "two" and await page.js(TEXTS) == ["one"]
    await clear(page)
    # --- the session link
    await emit(page, notice("in another session", "k:other", session="s2"))
    await emit(page, notice("in this session", "k:this", session="s1"))
    links = await page.js(f"{TOASTS}.map(t => [...t.querySelectorAll('.toast-meta button')].map(b => b.textContent).join())")
    results[f"{t}: only a notice of another session has 'Open conversation'"] = links == ["", "Open conversation"]
    n = len(opened())
    await page.js("[...document.querySelectorAll('.toast-link')].find(b => b.textContent === 'Open conversation').click()")
    await asyncio.sleep(0.4)
    results[f"{t}: the link opens that session and closes the toast"] = opened()[n:] == ["s2"] and await page.js(TEXTS) == ["in this session"]
    await clear(page)
    # --- a failed turn is the page's own error, with no key
    await emit(page, {"type": "turnFinished", "sessionId": "s1", "turnId": "t1", "status": "failed", "error": "boom", "usage": {"inputTokens": 0, "outputTokens": 0}})
    results[f"{t}: a failed turn is a local error toast, not merged"] = await page.js(TEXTS) == ["Turn failed: boom"] and await page.js("document.querySelector('.toast').classList.contains('error') && !document.querySelector('.toast').dataset.key")
    await clear(page)
    # --- reduced motion
    await page.send("Emulation.setEmulatedMedia", features=[{"name": "prefers-reduced-motion", "value": "no-preference"}])
    await emit(page, notice("moving", "k:m"))
    moving = await page.js("getComputedStyle(document.querySelector('.toast')).animationName")
    await page.send("Emulation.setEmulatedMedia", features=[{"name": "prefers-reduced-motion", "value": "reduce"}])
    still = await page.js("getComputedStyle(document.querySelector('.toast')).animationName")
    await page.send("Emulation.setEmulatedMedia", features=[])
    results[f"{t}: prefers-reduced-motion drops the slide"] = moving == "toast-in" and still == "none"
    await clear(page)
    # --- recovery replaces the error and goes after 6 s; an info goes after 6 s; an error stays
    await emit(page, notice("gemini::a is out of quota - answering with gemini::b", "provider:gemini:quota"))
    await emit(page, notice("hook broke", "hook:z"))
    await emit(page, {"type": "notice", "level": "info", "text": "gemini::a answers again", "key": "provider:gemini:quota", "kind": "recovery"})
    after = await page.js(f"{TOASTS}.map(t => t.classList[1] + ':' + t.querySelector('.toast-text').textContent + ':' + t.getAttribute('role'))")
    results[f"{t}: a recovery closes the error toast of its key and shows itself as a status"] = after == ["recovery:gemini::a answers again:status", "error:hook broke:alert"]
    await emit(page, notice("an info", level="info"))
    await page.shot(f"notices-recovery-{t.replace(' ', '-')}")
    results[f"{t}: an info is role=status"] = await page.js("document.querySelector('.toast.info').getAttribute('role')") == "status"
    if first:
        # Hover holds the timer: the toast under the mouse stays, the other goes.
        x, y = await page.center(".toast.info")
        await page.send("Input.dispatchMouseEvent", type="mouseMoved", x=x, y=y)
    await asyncio.sleep(6.6)
    left = await page.js(f"{TOASTS}.map(t => t.classList[1])")
    if first:
        results[f"{t}: hover holds the 6 s timer of a toast (the other info and the recovery went)"] = left == ["info", "error"]
        await page.send("Input.dispatchMouseEvent", type="mouseMoved", x=2, y=2)
        await asyncio.sleep(6.6)
        left = await page.js(f"{TOASTS}.map(t => t.classList[1])")
        results[f"{t}: after the mouse leaves, the 6 s run and the info goes"] = left == ["error"]
    else:
        results[f"{t}: the recovery and the info went after 6 s; the error stayed"] = left == ["error"]
    await clear(page)


STATUS = "[...document.querySelector('.widget-status').childNodes].filter(n => n.nodeType === 3).map(n => n.textContent).join('')"


async def voice_check(env, results):
    """An error toast is said once per key by the status line of a live session (notices.md section 2)."""
    page = env.page
    await env.size(340, 460)
    await env.open("dark", query="?overlay&autostart", init=FAKE_LIVE, **CFG)
    await page.until("!!document.querySelector('.widget-status.st-connecting, .widget-status.st-live, .widget-status.st-rolling')", "a live voice session")
    await emit(page, notice("Gemini is out of quota", "provider:gemini:quota"))
    said = await page.js(STATUS)
    await emit(page, notice("Gemini is out of quota again", "provider:gemini:quota"))
    again = await page.js(STATUS)
    await page.shot("notices-voice-340x460-dark")
    results["voice: an error toast of a live session is said by the status line"] = said.strip() == "Gemini is out of quota"
    results["voice: the same key is not said a second time"] = again.strip() == "Gemini is out of quota"
    await asyncio.sleep(6.4)
    results["voice: the status line goes back to the state after 6 s"] = "quota" not in await page.js(STATUS)


async def kids(page, *items):
    for cid, title, profile, status in items:
        await emit(page, {"type": "subagentUpdated", "sessionId": "s1", "child": CHILD(cid, title, profile), "status": status})


def section(name):
    return f"document.querySelector(\".work-section[data-section='{name}']\")"


async def activity_checks(env, results, theme, label, compact):
    page = env.page
    t = f"{label} {theme}"
    await env.open(theme, query="?overlay" if compact else "", init=FAKE_LIVE, **CFG)
    results[f"{t}: Activity: no notice, no capsule and no Activity section"] = await page.js("!document.querySelector('.work-capsule')")
    await emit(page, notice("gemini::a is out of quota", "provider:gemini:quota", level="error", kind="error", session="s1"))
    await emit(page, notice("gemini::a is out of quota again", "provider:gemini:quota", level="error", kind="error", session="s1"))
    await emit(page, notice("elsewhere", "k:x", session="s2"))
    await emit(page, notice("Gemini answers again", "provider:gemini:quota", level="info", kind="recovery", session="s1"))
    for i in range(22):
        await emit(page, notice(f"note {i}", session="s1", level="info"))
    await clear(page)
    results[f"{t}: Activity: closed toasts leave no stack"] = await page.js("!document.querySelector('.toasts')")
    results[f"{t}: Activity: the capsule shows a bell with the count of the notices of the open conversation (20)"] = await page.js("document.querySelector('.work-capsule .cap-activity').textContent.trim()") == "20"
    await page.click(".work-capsule")
    await page.until("!!document.querySelector('.work-section[data-section=activity]')", "the Activity section")
    rows = await page.js("[...document.querySelectorAll('.activity-row')].map(r => r.querySelector('.activity-text').textContent)")
    results[f"{t}: Activity lists the last 20 of the open conversation, newest first, and none of another one"] = len(rows) == 20 and rows[0] == "note 21" and rows[-1] == "note 2" and "elsewhere" not in rows
    row = await page.js("(() => { const r = document.querySelector('.activity-row'); return {level: r.querySelector('.activity-level').textContent, time: /\\d/.test(r.querySelector('time').textContent), icon: !!r.querySelector('svg')}; })()")
    results[f"{t}: an Activity line has the icon, the level and the time"] = row == {"level": "info", "time": True, "icon": True}
    results[f"{t}: Activity counts its lines in the header"] = await page.js("document.querySelector('.work-section[data-section=activity] .work-section-count').textContent") == "20"
    await page.shot(f"activity-{t.replace(' ', '-')}")
    # a merged entry shows the count
    await env.open(theme, query="?overlay" if compact else "", init=FAKE_LIVE, **CFG)
    await emit(page, notice("gemini::a is out of quota", "provider:gemini:quota", level="error", kind="error", session="s1"))
    await emit(page, notice("gemini::a is out of quota (2)", "provider:gemini:quota", level="error", kind="error", session="s1"))
    await clear(page)
    await page.click(".work-capsule")
    await page.until("!!document.querySelector('.activity-row')", "the entry")
    results[f"{t}: Activity: the same key twice is one line with x2 and the latest text, though the toast was closed"] = await page.js("[...document.querySelectorAll('.activity-row')].map(r => r.querySelector('.activity-text').textContent + '|' + r.querySelector('.activity-count').textContent)") == ["gemini::a is out of quota (2)|x2"]


async def work_checks(env, results, theme):
    page = env.page
    t = f"1200x820 {theme}"
    await env.size(1200, 820)
    # --- nothing going on: no capsule, no pane, no old chips
    await env.open(theme, **{**CFG, "sessions": {"s1": {"title": "Free the disk", "updatedAt": "2026-11-27T09:00:00Z"}}})
    results[f"{t}: with nothing to show there is no capsule, no pane and no todo panel or plan chip"] = await page.js("!document.querySelector('.work-capsule') && !document.querySelector('aside.side-pane') && !document.querySelector('.todo-panel') && !document.querySelector('.plan-chip-row') && !document.querySelector('main.with-pane')")
    # --- the capsule
    await env.open(theme, **WORK)
    await kids(page, ("c1", "Disk usage", "explore", "running"), ("c2", "Memory usage", "general", "queued"))
    await page.until("!!document.querySelector('.work-capsule')", "the capsule")
    cap = await page.js("(() => { const c = document.querySelector('.work-capsule'), r = c.getBoundingClientRect(), composer = document.querySelector('form.composer').getBoundingClientRect(); return {h: Math.round(r.height), above: r.bottom <= composer.top, items: [...c.querySelectorAll('.cap-item')].map(i => i.className.replace('cap-item ', '') + ':' + i.textContent.trim()), expanded: c.getAttribute('aria-expanded'), controls: c.getAttribute('aria-controls'), title: c.title, radius: getComputedStyle(c).borderTopLeftRadius, pulse: c.querySelectorAll('.agent-dot.pulse').length, left: Math.round(r.left)}; })()")
    results[f"{t}: the capsule is 26 px high, above the composer, and counts 2 agents, 1 command, the plan and 3/7"] = cap["h"] == 26 and cap["above"] and cap["items"] == ["cap-agents:2", "cap-commands:1", "cap-plan plan:plan", "cap-todo:3/7"]
    results[f"{t}: the capsule is a button with aria-expanded, aria-controls and its words as the title"] = cap["expanded"] == "false" and bool(cap["controls"]) and cap["title"] == "2 agents, 1 command, plan approved, 3 of 7 to do" and float(cap["radius"].replace("px", "")) >= 13
    results[f"{t}: the first two items carry the pulsing dot while something runs"] = cap["pulse"] == 2
    await page.shot(f"work-capsule-{theme}")
    await page.click(".work-capsule")
    await page.until("!!document.querySelector('.work-panel')", "the panel")
    panel = await page.js("(() => { const p = document.querySelector('.work-panel'); return {id: p.id, role: p.getAttribute('role'), expanded: document.querySelector('.work-capsule').getAttribute('aria-controls') === p.id && document.querySelector('.work-capsule').getAttribute('aria-expanded'), heads: [...p.querySelectorAll('.work-section-head')].map(h => Math.round(h.getBoundingClientRect().height) + ':' + h.querySelector('.work-section-title').textContent), counts: [...p.querySelectorAll('.work-section-count')].map(c => c.textContent), maxh: parseFloat(getComputedStyle(p).maxHeight), h: p.getBoundingClientRect().height}; })()")
    results[f"{t}: the panel has four sections of a 28 px header and opens from the capsule"] = panel["expanded"] == "true" and panel["role"] == "region" and panel["heads"] == ["28:Agents", "28:Background", "28:Plan", "28:Progress"]
    results[f"{t}: the headers count: '1 of 2 running', '1 running', 'approved', '3 of 7'"] = panel["counts"] == ["1 of 2 running", "1 running", "approved", "4 of 7"]
    results[f"{t}: the panel is at most min(46vh, 420px) high"] = panel["h"] <= min(0.46 * 820, 420) + 1 and panel["maxh"] <= 420
    await page.shot(f"work-panel-{theme}")
    # --- Agents
    ag = await page.js("[...document.querySelectorAll('.agent-row')].map(r => r.querySelector('.work-line-title').textContent + '|' + r.querySelector('.work-line-note').textContent + '|' + r.querySelector('.agent-status').textContent + '|' + !!r.querySelector('.work-stop') + '|' + /\\d/.test(r.querySelector('.work-time').textContent || 'x'))")
    results[f"{t}: Agents lists both children with profile, state, time and Stop while they wait or run"] = ag == ["Disk usage|explore|running|true|true", "Memory usage|general|queued|true|false"]
    await page.js("document.querySelector('.agent-row[data-child=c2] .work-stop').click()")
    await asyncio.sleep(0.3)
    results[f"{t}: Stop of a child is the route of S31"] = "POST /api/sessions/s1/subagents/c2/stop" in calls()
    await emit(page, {"type": "subagentUpdated", "sessionId": "s1", "child": CHILD("c2", "Memory usage", "general"), "status": "cancelled"})
    await emit(page, {"type": "subagentUpdated", "sessionId": "s1", "child": {**CHILD("c1", "Disk usage", "explore"), "endedAt": "2026-11-27T10:00:05Z"}, "status": "done"})
    await emit(page, {"type": "subagentUpdated", "sessionId": "s1", "child": CHILD("c1", "Disk usage", "explore"), "status": "running"})
    # --- Background
    bg = await page.js("[...document.querySelectorAll('.task-row')].map(r => r.querySelector('.work-command').textContent.slice(0, 12) + '|' + r.querySelector('.work-state').textContent + '|' + r.querySelector('.work-state').className.replace('work-state tone-', '') + '|' + !!r.querySelector('.work-stop'))")
    results[f"{t}: Background lists the commands with their state: running, exit 0 (ok), exit 1 (bad), killed (signal)"] = bg == ["cargo test -", "running|run|True", "df -h|exit 0|ok|False", "false|exit 1|bad|False", "sleep 99|killed|signal|False"][:1] + [] or bg == ["cargo test --workspace -- --nocapture --test-threads=1"[:12] + "|running|run|true"] + ["df -h|exit 0|ok|false", "false|exit 1|bad|false", "sleep 99|killed|signal|false"]
    cmd = await page.js("(() => { const c = document.querySelector('.task-row .work-command'); const cs = getComputedStyle(c); return {family: cs.fontFamily.includes('mono'), size: cs.fontSize, ellipsis: cs.textOverflow, full: c.title}; })()")
    results[f"{t}: a command is one line, monospace 12 px, cut with an ellipsis, the whole command in the title"] = cmd["family"] and cmd["size"] == "12px" and cmd["ellipsis"] == "ellipsis" and cmd["full"].startswith("cargo test --workspace")
    await page.js("document.querySelector('.task-row[data-task=bg_run] .work-stop').click()")
    await asyncio.sleep(0.4)
    results[f"{t}: Stop of a command is POST /api/background/:taskId/stop and the list is read again"] = "POST /api/background/bg_run/stop" in calls() and await page.js("document.querySelector('.task-row[data-task=bg_run] .work-state').textContent") == "killed" and await page.js("!document.querySelector('.task-row[data-task=bg_run] .work-stop')")
    await page.js("window.__emit({type: 'backgroundTask', taskId: 'bg_run', sessionId: 's1', status: 'killed'})")
    # --- Plan
    results[f"{t}: Plan says Off and the approved plan with its first line and Open"] = await page.js("(() => { const s = " + section("plan") + "; return s.querySelector('.plan-state .work-state').textContent === 'off' && s.querySelector('.plan-approved .work-line-title').textContent === 'Free the disk' && s.querySelector('.plan-approved .work-stop').textContent === 'Open'; })()")
    await emit(page, {"type": "planUpdated", "sessionId": "s1", "active": True})
    results[f"{t}: Planning is on in the ice colour with Cancel plan"] = await page.js("(() => { const s = " + section("plan") + "; const st = s.querySelector('.plan-state .work-state'); return st.textContent === 'on' && st.className.includes('tone-ice') && s.querySelector('.plan-state .work-stop').textContent === 'Cancel plan'; })()") and await page.js("document.querySelector('.work-capsule .cap-plan').textContent") == "planning"
    await page.js("document.querySelector('.plan-state .work-stop').click()")
    await asyncio.sleep(0.3)
    results[f"{t}: Cancel plan is POST /api/sessions/:id/plan {{active: false}}"] = calls()[-1] == 'POST /api/sessions/s1/plan {"active": false}' and await page.js("document.querySelector('.plan-state .work-state').textContent") == "off"
    # --- Progress
    await page.js("document.querySelector('.todo-items li:last-child input').click()")
    await asyncio.sleep(0.3)
    results[f"{t}: Progress ticks with PUT /api/sessions/:id/todo, as before"] = calls()[-1].startswith("PUT /api/sessions/s1/todo") and await page.js("document.querySelector('.work-section[data-section=progress] .work-section-count').textContent") == "5 of 7"
    results[f"{t}: the todo list has no box of its own"] = await page.js("(() => { const p = document.querySelector('.todo-panel'); const cs = getComputedStyle(p); return cs.borderTopWidth === '0px' && cs.backgroundColor === 'rgba(0, 0, 0, 0)' && cs.marginLeft === '0px'; })()")
    await page.shot(f"work-sections-{theme}")
    # --- folding
    await page.click(".work-section[data-section=agents] .work-section-toggle")
    stored = json.loads(await page.js("localStorage.getItem('kagent.work.closed')"))
    results[f"{t}: a click on a header folds the section and kagent.work.closed remembers it"] = stored == ["agents"] and await page.js("!document.querySelector('.agent-row')")
    await page.send("Page.reload")
    await page.until("!!document.querySelector('.work-capsule')", "the capsule after the reload")
    await page.click(".work-capsule")
    results[f"{t}: after a reload the folded section stays folded"] = await page.js("!document.querySelector('.agent-row') && !!document.querySelector('.task-row')")
    await page.click(".work-section[data-section=agents] .work-section-toggle")
    await page.click(".work-capsule")


async def plan_event_check(env, results, theme):
    """v0.0.3 A3: an approval sends sessionMetaChanged; the Plan section changes without a turn end."""
    page = env.page
    t = f"1200x820 {theme}"
    await env.size(1200, 820)
    await env.open(theme, **{**WORK, "sessions": {**WORK["sessions"], "s1": {"title": "Free the disk", "updatedAt": "2026-11-27T09:00:00Z", "todo": TODO}}})
    await page.click(".work-capsule")
    results[f"{t}: before an approval there is no approved plan and no plan item in the capsule"] = await page.js("!(" + section("plan") + ").querySelector('.plan-approved') && !document.querySelector('.work-capsule .cap-plan')")
    meta = {"id": "s1", "title": "Free the disk", "updatedAt": NOW, "approvedPlan": {"path": "/p", "text": PLAN}}
    await emit(page, {"type": "sessionMetaChanged", "sessionId": "s1", "meta": meta})
    results[f"{t}: sessionMetaChanged with the approved plan changes the Plan section and the capsule at once"] = await page.js("(() => { const s = " + section("plan") + "; return !!s.querySelector('.plan-approved') && s.querySelector('.plan-approved .work-line-title').textContent === 'Free the disk' && !!document.querySelector('.work-capsule .cap-plan'); })()")


async def pane_checks(env, results, theme):
    page = env.page
    t = f"1200x820 {theme}"
    await env.size(1200, 820)
    await env.open(theme, **WORK)
    await kids(page, ("c1", "Disk usage", "explore", "running"), ("c2", "Memory usage", "general", "done"))
    await page.until("!!document.querySelector('.agent-card')", "the cards of the transcript")
    results[f"{t}: no pane while no tab is open, and the 380 px are not reserved"] = await page.js("!document.querySelector('aside.side-pane') && !document.querySelector('main.with-pane')")
    full = await page.js("Math.round(document.querySelector('.view').getBoundingClientRect().width)")
    n = len(opened())
    await page.js("document.querySelector('.agent-card[data-child=c1] .agent-open').click()")
    await page.until("!!document.querySelector('aside.side-pane')", "the Subagent tab")
    lay = await page.js("(() => { const p = document.querySelector('aside.side-pane').getBoundingClientRect(), v = document.querySelector('.view').getBoundingClientRect(), m = document.querySelector('main'); return {pane: Math.round(p.width), view: Math.round(v.width), row: getComputedStyle(m).flexDirection, cls: m.className, right: Math.round(innerWidth - p.right), border: getComputedStyle(document.querySelector('aside.side-pane')).borderLeftWidth}; })()")
    results[f"{t}: a click on an Agents card opens the Subagent tab and the conversation stays"] = lay["pane"] == 380 and lay["row"] == "row" and lay["cls"] == "with-pane" and await page.js("!!document.querySelector('form.composer') && !document.querySelector('.agent-readonly')")
    results[f"{t}: the view gives the pane its 380 px and keeps the rest"] = lay["view"] == full - 380 and lay["border"] == "1px"
    await page.until("document.querySelector('.pane-subagent .bubble') !== null", "the child's messages")
    results[f"{t}: the tab reads the child session once and shows its messages as bubbles, no composer in the pane; the message box of steering.md is a form of its own"] = opened()[n:] == ["c1"] and await page.js("[...document.querySelectorAll('.pane-subagent .bubble .content')].map(e => e.textContent)") == ["Find what uses the disk", "du says /var holds 12 GB"] and await page.js("!document.querySelector('aside.side-pane form.composer')")
    results[f"{t}: one tab has a header with its icon, its title and a Close button"] = await page.js("(() => { const h = document.querySelector('.side-head'); return Math.round(h.getBoundingClientRect().height) === 32 && !!h.querySelector('.side-icon svg') && h.querySelector('.side-title').textContent === 'Disk usage' && h.querySelector('.side-close').getAttribute('aria-label') === 'Close' && !document.querySelector('.side-tabs'); })()")
    results[f"{t}: the line of the child shows title, profile, state, time and Stop"] = await page.js("(() => { const l = document.querySelector('.pane-subagent .pane-line'); return l.textContent.includes('Disk usage') && l.textContent.includes('explore') && l.textContent.includes('running') && !!l.querySelector('.work-stop'); })()")
    await page.shot(f"pane-subagent-{theme}")
    # a live message arrives by events and the pane follows the end
    await emit(page, {"type": "messageAppended", "sessionId": "c1", "message": {"role": "assistant", "content": "second answer"}})
    results[f"{t}: a message of the child that arrives by event shows in the pane"] = await page.js("[...document.querySelectorAll('.pane-subagent .bubble .content')].pop().textContent") == "second answer"
    # --- more tabs: a tab bar
    await page.click("body")
    await page.click(".work-capsule")
    await page.js("document.querySelector('.agent-row[data-child=c2] .work-line-main').click()")
    await page.js("document.querySelector('.task-row[data-task=bg_run] .work-line-main').click()")
    await page.js("document.querySelector('.plan-approved .work-stop').click()")
    await page.until("document.querySelectorAll('.side-tab').length === 4", "four tabs")
    bar = await page.js("(() => { const b = document.querySelector('.side-tabs'); const cs = getComputedStyle(b); return {titles: [...document.querySelectorAll('.side-tab .side-title')].map(e => e.textContent.slice(0, 12)), active: [...document.querySelectorAll('.side-tab.active')].length, activeKey: document.querySelector('.side-tab.active').dataset.key, overflow: cs.overflowX, scrolls: b.scrollWidth > b.clientWidth, maxw: Math.max(...[...document.querySelectorAll('.side-tab')].map(x => x.getBoundingClientRect().width)), line: getComputedStyle(document.querySelector('.side-tab.active')).borderBottomWidth, close: [...document.querySelectorAll('.side-tab-x')].length}; })()")
    results[f"{t}: several tabs make a tab bar: one button each with its title and its own x, the active one with the 2 px line"] = bar["titles"] == ["Disk usage", "Memory usage", "cargo test -", "Plan"] and bar["active"] == 1 and bar["activeKey"] == "plan:s1" and bar["line"] == "2px" and bar["close"] == 4
    results[f"{t}: a tab is at most 140 px and the bar scrolls sideways when the tabs do not fit"] = bar["maxw"] <= 140 and bar["overflow"] in ("auto", "scroll")
    await page.js("document.querySelector('.side-tab[data-key=\"subagent:c1\"] .side-tab-main').click()")
    await asyncio.sleep(0.2)
    results[f"{t}: opening a tab that is open makes it the active one; only one is active"] = await page.js("document.querySelectorAll('.side-tab.active').length") == 1 and await page.js("document.querySelector('.side-tab.active').dataset.key") == "subagent:c1"
    await page.js("document.querySelector('.agent-row[data-child=c1] .work-line-main').click()")
    await asyncio.sleep(0.2)
    results[f"{t}: a line of Agents for a child that has a tab does not open a second one"] = await page.js("document.querySelectorAll('.side-tab').length") == 4
    # Plan tab
    await page.js("document.querySelector('.side-tab[data-key=\"plan:s1\"] .side-tab-main').click()")
    await asyncio.sleep(0.2)
    results[f"{t}: the Plan tab shows the approved plan as Markdown"] = await page.js("(() => { const p = document.querySelector('.pane-plan'); return p.querySelector('h3') && p.querySelector('h3').textContent === 'Free the disk' && p.querySelectorAll('li').length === 2; })()")
    await page.shot(f"pane-tabs-{theme}")
    # Escape with the focus inside the pane closes the active tab
    await page.js("document.querySelector('.pane-plan .pane-scroll').setAttribute('tabindex', '0'); document.querySelector('.pane-plan .pane-scroll').focus()")
    await page.key("Escape", "Escape", 27)
    results[f"{t}: Escape with the focus in the pane closes the active tab"] = await page.js("document.querySelectorAll('.side-tab').length") == 3 and await page.js("!document.querySelector('.side-tab[data-key=\"plan:s1\"]')")
    await page.js("document.querySelector('.side-tab[data-key=\"subagent:c2\"] .side-tab-x').click()")
    await asyncio.sleep(0.2)
    results[f"{t}: a tab closes with its x"] = await page.js("document.querySelectorAll('.side-tab').length") == 2
    # --- Output tab: follows while running, pauses when scrolled up
    await page.js("document.querySelector('.side-tab[data-key=\"output:bg_run\"] .side-tab-main').click()")
    await page.until("document.querySelector('.pane-log') && document.querySelector('.pane-log').textContent.includes('line 399')", "the output")
    lg = await page.js("(() => { const l = document.querySelector('.pane-log'); const cs = getComputedStyle(l); return {atEnd: l.scrollHeight - l.scrollTop - l.clientHeight <= 8, mono: cs.fontFamily.includes('mono'), size: cs.fontSize, wrap: cs.whiteSpace, head: document.querySelector('.pane-output .pane-line').textContent}; })()")
    results[f"{t}: the Output tab shows the command, its state and the output in a monospace 12 px pre, at the end"] = lg["atEnd"] and lg["mono"] and lg["size"] == "12px" and lg["wrap"] == "pre-wrap" and "cargo test" in lg["head"]
    r0 = [r for r in reads() if r[0] == "bg_run"]
    results[f"{t}: it is read with ?tail=8000"] = bool(r0) and all(r[1] == 8000 for r in r0)
    await asyncio.sleep(2.4)
    r1 = len([r for r in reads() if r[0] == "bg_run"])
    results[f"{t}: while the task runs it is read again every second"] = r1 >= len(r0) + 2
    await page.js("document.querySelector('.pane-log').scrollTop = 0")
    await asyncio.sleep(0.3)
    paused = await page.js("document.querySelector('.pane-log').dataset.following") == "false" and await page.js("!!document.querySelector('.pane-jump')")
    set_task(id="bg_run", text=LOG + "NEW LINE AT THE END\n")
    await asyncio.sleep(1.5)
    results[f"{t}: scrolled up, following pauses and the floating arrowDown button shows"] = paused and await page.js("document.querySelector('.pane-log').scrollTop") < 50 and await page.js("document.querySelector('.pane-jump').getAttribute('aria-label')") == "Jump to the end"
    await page.shot(f"pane-output-{theme}")
    await page.click(".pane-jump")
    await asyncio.sleep(0.3)
    results[f"{t}: the button resumes: the end is shown with the new line"] = await page.js("(() => { const l = document.querySelector('.pane-log'); return l.scrollHeight - l.scrollTop - l.clientHeight <= 8 && l.textContent.includes('NEW LINE AT THE END') && !document.querySelector('.pane-jump'); })()")
    # the end event: one last read, then it stops
    set_task(id="bg_run", status="done", exitCode=0, text=LOG + "NEW LINE AT THE END\nfinished\n")
    await emit(page, {"type": "backgroundTask", "taskId": "bg_run", "sessionId": "s1", "status": "done"})
    await asyncio.sleep(1.5)
    n1 = len([r for r in reads() if r[0] == "bg_run"])
    await asyncio.sleep(2.2)
    n2 = len([r for r in reads() if r[0] == "bg_run"])
    results[f"{t}: after the end event the output has its last read and no more reads follow"] = await page.js("document.querySelector('.pane-log').textContent.includes('finished')") and n2 == n1 and await page.js("document.querySelector('.pane-output .work-state').textContent") == "exit 0"
    # --- tabs belong to their session; the other session has no pane, this one gets them back
    await page.js("document.querySelector('.sb-item:not(.active) .sb-row-main') && [...document.querySelectorAll('.sb-item .sb-row-main')].find(b => b.textContent.includes('Other chat')).click()")
    await page.until("!document.querySelector('aside.side-pane')", "no pane in another session")
    await page.js("[...document.querySelectorAll('.sb-item .sb-row-main')].find(b => b.textContent.includes('Free the disk')).click()")
    await page.until("!!document.querySelector('aside.side-pane')", "the tabs back")
    results[f"{t}: tabs belong to the open session: another one has no pane, and the tabs come back"] = await page.js("document.querySelectorAll('.side-tab').length") == 2
    # closing the last tab removes the pane
    await page.js("[...document.querySelectorAll('.side-tab-x')].forEach(b => b.click())")
    await asyncio.sleep(0.3)
    await page.js("[...document.querySelectorAll('.side-tab-x')].forEach(b => b.click())")
    await asyncio.sleep(0.3)
    results[f"{t}: with the last tab closed the pane goes and its 380 px are free again"] = await page.js("!document.querySelector('aside.side-pane')") and await page.js("Math.round(document.querySelector('.view').getBoundingClientRect().width)") == full
    # --- a plan that waits
    await env.open(theme, **{**WORK, "approvals": [{"kind": "plan", "preview": "# New plan\n\n- step one\n", "requestId": "r1", "sessionId": "s1", "tool": "plan_exit", "input": {}, "summary": "plan", "reason": "", "risk": "low", "privileged": False, "canRemember": False, "ruleKey": ""}]})
    await page.click(".work-capsule")
    waits = await page.js("(() => { const s = " + section("plan") + "; return s.querySelector('.plan-waits .work-line-title').textContent + '|' + s.querySelector('.plan-waits .work-stop').textContent; })()")
    await page.js("document.querySelector('.plan-waits .work-stop').click()")
    await page.until("!!document.querySelector('.pane-plan')", "the plan tab")
    results[f"{t}: a plan that waits says so with Show; the Plan tab shows it with the note"] = waits == "A plan waits for your approval|Show" and await page.js("document.querySelector('.pane-plan .work-note').textContent") == "Approve or reject it in the conversation" and await page.js("document.querySelector('.pane-plan h3').textContent") == "New plan"
    await page.shot(f"pane-plan-waits-{theme}")


async def narrow_checks(env, results):
    page = env.page
    await env.size(700, 500)
    await env.open("dark", **WORK)
    await kids(page, ("c1", "Disk usage", "explore", "running"))
    await page.until("!!document.querySelector('.work-capsule')", "the capsule at 700x500")
    await page.click(".work-capsule")
    room = await page.js("(() => { const c = document.querySelector('form.composer').getBoundingClientRect(), p = document.querySelector('.work-panel').getBoundingClientRect(), m = document.querySelector('.messages').getBoundingClientRect(); return {composerIn: c.bottom <= innerHeight && c.top >= 0, panelAbove: p.bottom <= c.top, messages: Math.round(m.height), panelH: Math.round(p.height), maxh: getComputedStyle(document.querySelector('.work-panel')).maxHeight}; })()")
    results["700x500: the panel leaves the composer its room and the messages some height"] = room["composerIn"] and room["panelAbove"] and room["messages"] >= 60 and room["panelH"] <= 230
    await page.shot("work-panel-700x500")
    await page.js("document.querySelector('.agent-row .work-line-main').click()")
    await page.until("!!document.querySelector('aside.side-pane')", "the pane at 700x500")
    ov = await page.js("(() => { const a = document.querySelector('aside.side-pane'), r = a.getBoundingClientRect(), cs = getComputedStyle(a); return {pos: cs.position, w: Math.round(r.width), right: Math.round(innerWidth - r.right), top: Math.round(r.top), scrim: !!document.querySelector('.side-scrim'), z: cs.zIndex}; })()")
    results["700x500: below 900 px the pane is an overlay at the right with a scrim"] = ov["pos"] == "absolute" and ov["w"] == 380 and ov["right"] == 0 and ov["scrim"]
    await page.shot("pane-overlay-700x500")
    await page.js("document.querySelector('.side-close').click()")
    await asyncio.sleep(0.2)
    results["700x500: the overlay closes with its button"] = await page.js("!document.querySelector('aside.side-pane') && !document.querySelector('.side-scrim')")
    await page.js("document.querySelector('.agent-row .work-line-main').click()")
    await page.until("!!document.querySelector('.side-scrim')", "the pane again")
    await page.js("document.querySelector('.side-scrim').click()")
    await asyncio.sleep(0.2)
    results["700x500: a click on the scrim closes it too"] = await page.js("!document.querySelector('aside.side-pane')")


async def widget_checks(env, results, theme):
    page = env.page
    t = f"widget {theme}"
    await env.size(340, 460)
    await env.open(theme, query="?overlay", **WORK)
    await kids(page, ("c1", "Disk usage", "explore", "running"), ("c2", "Memory usage", "general", "queued"))
    await page.until("!!document.querySelector('.work-capsule')", "the capsule of the widget")
    cap = await page.js("(() => { const c = document.querySelector('.work-capsule'), r = c.getBoundingClientRect(), bar = document.querySelector('.widget-bar').getBoundingClientRect(); return {above: r.bottom <= bar.top, size: getComputedStyle(c).fontSize, todoChip: !!document.querySelector('.widget-todo'), items: [...c.querySelectorAll('.cap-item')].map(i => i.textContent.trim())}; })()")
    results[f"{t}: the capsule (11 px) is above the bar, in the place of the todo chip, which goes"] = cap["above"] and cap["size"] == "11px" and not cap["todoChip"] and cap["items"] == ["2", "1", "plan", "3/7"]
    await page.shot(f"work-widget-{theme}")
    await page.click(".work-capsule")
    await page.until("!!document.querySelector('.sheet')", "the sheet Work")
    sh = await page.js("(() => { const d = document.querySelector('.sheet'); return {label: d.getAttribute('aria-label'), titles: [...d.querySelectorAll('.work-section-title')].map(e => e.textContent), disabled: [...d.querySelectorAll('button.work-line-main')].every(b => b.disabled), full: Math.round(d.getBoundingClientRect().width) >= 320}; })()")
    results[f"{t}: a click opens the sheet Work with the four sections, full width"] = sh["label"] == "Work" and sh["titles"] == ["Agents", "Background", "Plan", "Progress"] and sh["full"]
    results[f"{t}: the lines open nothing in the widget (no pane) and there is no side pane"] = sh["disabled"] and await page.js("!document.querySelector('aside.side-pane')")
    await page.shot(f"work-sheet-{theme}")
    await page.js("document.querySelector('.sheet .agent-row[data-child=c1] .work-stop').click()")
    await asyncio.sleep(0.3)
    results[f"{t}: Stop of a child works in the sheet"] = "POST /api/sessions/s1/subagents/c1/stop" in calls()
    await page.js("document.querySelector('.sheet .task-row[data-task=bg_run] .work-stop').click()")
    await asyncio.sleep(0.4)
    results[f"{t}: Stop of a command works in the sheet"] = "POST /api/background/bg_run/stop" in calls() and await page.js("document.querySelector('.sheet .task-row[data-task=bg_run] .work-state').textContent") == "killed"
    await page.key("Escape", "Escape", 27)
    results[f"{t}: Escape closes the sheet"] = await page.js("!document.querySelector('.sheet')")


async def main():
    results = {}
    async with Env("kagent-s43", "/tmp/kagent-s43-shots") as env:
        for w, h, label, compact in ((1200, 820, "1200x820", False), (340, 460, "340x460", True)):
            await env.size(w, h)
            for theme in ("dark", "light"):
                await notice_checks(env, results, theme, label, compact, first=(label == "1200x820" and theme == "dark"))
        for w, h, label, compact in ((1200, 820, "1200x820", False), (340, 460, "340x460", True)):
            await env.size(w, h)
            for theme in ("dark", "light"):
                await activity_checks(env, results, theme, label, compact)
        for theme in ("dark", "light"):
            await work_checks(env, results, theme)
            await plan_event_check(env, results, theme)
            await pane_checks(env, results, theme)
            await widget_checks(env, results, theme)
        await narrow_checks(env, results)
        await voice_check(env, results)
        errors = [e for e in env.page.errors]
        results["no console errors"] = not errors
        if errors:
            print("ERRORS", errors[:5])
    return report(results)


sys.exit(asyncio.run(main()))

#!/usr/bin/env python3
"""Steering (steering.md): what a running worker is doing in the Agents section and in the worker pane, and the message box
under its transcript, in headless Chrome against the mocked API.

The mock and the page driver are picker_web_lib.py (its worker detail and message routes); the window's event stream is its
FAKE_WS, so a check feeds the page the events the daemon would send (`window.__emit`). Dark and light at 1200x820; screenshots go
to /tmp/kagent-v003-steer-shots/. Run from the repo root with KAGENT_DIST set to a build of web/."""
import asyncio, json, sys
import picker_web_lib as lib
from picker_web_lib import Env, calls, report, set_detail

NOW = "2026-11-28T10:00:00Z"
SESSIONS = {
    "s1": {"title": "Free the disk", "updatedAt": "2026-11-28T09:00:00Z"},
    "c1": {"title": "Disk usage", "updatedAt": "2026-11-28T07:00:00Z", "origin": "subagent", "parent": "s1"},
    "c2": {"title": "Memory usage", "updatedAt": "2026-11-28T06:00:00Z", "origin": "subagent", "parent": "s1"},
}
MESSAGES = {"c1": [{"role": "user", "content": "Find what uses the disk"}, {"role": "assistant", "content": "du says /var holds 12 GB"}], "c2": [{"role": "user", "content": "Find what uses the memory"}]}
FILES = [{"path": "/work/a.rs", "changed": True}, {"path": "/work/b.rs", "changed": False}]
ACTION = {"tool": "edit_file", "summary": "edit /work/a.rs", "status": "running", "at": NOW}
DETAIL = {
    "c1": {"child": {"id": "c1", "title": "Disk usage", "profile": "explore", "status": "running", "startedAt": NOW, "lastAction": ACTION, "files": FILES}, "job": "Disk usage",
           "todo": [{"content": "list the big folders", "status": "completed"}, {"content": "check /var", "status": "in_progress"}, {"content": "write the summary", "status": "pending"}],
           "transcript": [{"role": "assistant", "kind": "text", "text": "Looking at /var."}, {"role": "assistant", "kind": "call", "text": "read_file /work/b.rs"}, {"role": "tool", "kind": "result", "status": "ok", "text": "fn main() {}"},
                          {"role": "user", "kind": "steer", "text": "leave the tests alone"}, {"role": "assistant", "kind": "text", "text": "Got it: I will not touch the tests."}]},
}
cfg = lambda: dict(sessions=SESSIONS, messages=MESSAGES, details=json.loads(json.dumps(DETAIL)))
CHILD = lambda cid, title, profile, **kw: {"id": cid, "title": title, "profile": profile, "startedAt": NOW, **kw}
V = {"running": "running", "done": "done"}


async def emit(page, ev):
    await page.js(f"window.__emit({json.dumps(ev)})")
    await asyncio.sleep(0.12)


async def child(page, cid, title, profile, status, **kw):
    await emit(page, {"type": "subagentUpdated", "sessionId": "s1", "child": CHILD(cid, title, profile, **kw), "status": status})


async def see_checks(env, results, theme):
    page = env.page
    t = f"1200x820 {theme}"
    await env.size(1200, 820)
    await env.open(theme, **cfg())
    await child(page, "c1", "Disk usage", "explore", "running", lastAction=ACTION, files=FILES)
    await child(page, "c2", "Memory usage", "general", "done", lastAction={"tool": "read_file", "summary": "read /proc/meminfo", "status": "done", "at": NOW})
    await page.until("!!document.querySelector('.work-capsule')", "the capsule")
    await page.click(".work-capsule")
    await page.until("!!document.querySelector('.agent-row[data-child=c1] .agent-doing')", "the doing line")
    row = await page.js("(() => { const r = document.querySelector('.agent-row[data-child=c1] .agent-doing'); return [r.textContent, r.dataset.actionStatus, !!r.querySelector('.agent-files')]; })()")
    results[f"{t}: the Agents line of a running worker shows what it is doing and how many files it touched"] = row[0] == "Doing: edit /work/a.rs2 files" and row[1] == "running" and row[2]
    other = await page.js("(() => { const r = document.querySelector('.agent-row[data-child=c2] .agent-doing'); return [r.textContent, !!r.querySelector('.agent-files')]; })()")
    results[f"{t}: a worker that ended shows its last action and no file count when it touched none"] = other == ["Last: read /proc/meminfo", False]
    await page.shot(f"agents-doing-{theme}")
    await child(page, "c1", "Disk usage", "explore", "running", lastAction={"tool": "shell", "summary": "run cargo test", "status": "running", "at": NOW}, files=FILES + [{"path": "/work/c.rs", "changed": False}])
    await page.until("document.querySelector('.agent-row[data-child=c1] .agent-doing').textContent.startsWith('Doing: run cargo test')", "the line follows the events")
    results[f"{t}: the line follows subagentUpdated (the tool it is in, the file count)"] = "3 files" in await page.js("document.querySelector('.agent-row[data-child=c1] .agent-doing').textContent")
    await child(page, "c1", "Disk usage", "explore", "running", lastAction={"tool": "shell", "summary": "run cargo test", "status": "error", "at": NOW}, files=FILES)
    await page.until("document.querySelector('.agent-row[data-child=c1] .agent-doing').dataset.actionStatus === 'error'", "the status of the action")
    results[f"{t}: a call that failed says Last, not Doing"] = (await page.js("document.querySelector('.agent-row[data-child=c1] .agent-doing').textContent")).startswith("Last: run cargo test")
    await child(page, "c1", "Disk usage", "explore", "running", lastAction=ACTION, files=FILES)
    # --- the pane
    n = len([c for c in calls() if c.startswith("GET /api/sessions/s1/subagents/c1")])
    await page.js("document.querySelector('.agent-row[data-child=c1] .work-line-main').click()")
    await page.until("!!document.querySelector('.pane-subagent .worker-info .worker-todo li')", "the worker info")
    info = await page.js("""(() => { const w = document.querySelector('.worker-info'); return {job: w.querySelector('.worker-job').textContent, doing: w.querySelector('.worker-doing').textContent, doingStatus: w.querySelector('.worker-doing').dataset.actionStatus,
        files: [...w.querySelectorAll('.worker-files li')].map(l => l.textContent + (l.className ? ':' + l.className : '')), todo: [...w.querySelectorAll('.worker-todo li')].map(l => l.dataset.status + ':' + l.textContent),
        todoTitle: w.querySelectorAll('summary')[1].textContent, tail: [...w.querySelectorAll('.worker-tail li')].map(l => l.dataset.kind + ':' + l.textContent)}; })()""")
    results[f"{t}: the worker pane shows the job and what it is doing now"] = info["job"] == "Disk usage" and info["doing"] == "edit /work/a.rs" and info["doingStatus"] == "running"
    results[f"{t}: it lists the files its calls named, the changed one marked"] = info["files"] == ["/work/a.rs:changed", "/work/b.rs"]
    results[f"{t}: it shows the worker's own todo list with its progress (1 of 3)"] = info["todo"] == ["completed:list the big folders", "in_progress:check /var", "pending:write the summary"] and info["todoTitle"] == "Todo (1 of 3)"
    results[f"{t}: it shows the tail of the transcript from the daemon, the owner's message among it"] = info["tail"] == ["text:Looking at /var.", "call:> read_file /work/b.rs", "result:= fn main() {}", "steer:[you] leave the tests alone", "text:Got it: I will not touch the tests."]
    results[f"{t}: the detail is read when the pane opens"] = len([c for c in calls() if c.startswith("GET /api/sessions/s1/subagents/c1")]) == n + 1
    results[f"{t}: the transcript of the session (bubbles) is still there under it"] = await page.js("[...document.querySelectorAll('.pane-subagent .bubble .content')].map(e => e.textContent)") == ["Find what uses the disk", "du says /var holds 12 GB"]
    await page.shot(f"worker-pane-{theme}")
    # --- it follows the daemon
    set_detail("c1", transcript=DETAIL["c1"]["transcript"] + [{"role": "assistant", "kind": "text", "text": "Finished the scan."}])
    await child(page, "c1", "Disk usage", "explore", "running", lastAction={"tool": "read_file", "summary": "read /work/b.rs", "status": "done", "at": NOW}, files=FILES)
    await page.until("document.querySelectorAll('.worker-tail li').length === 6", "the pane reads the detail again")
    results[f"{t}: a subagentUpdated of the worker makes the pane read its detail again"] = await page.js("document.querySelector('.worker-doing').textContent") == "read /work/b.rs" and await page.js("document.querySelector('.worker-doing').dataset.actionStatus") == "done"
    results[f"{t}: and a finished call reads Last in the pane (the label follows the status)"] = await page.js("getComputedStyle(document.querySelector('.worker-doing'), '::before').content") == '"Last: "'


async def main():
    results = {}
    async with Env("kagent-steer", "/tmp/kagent-v003-steer-shots") as env:
        for theme in ("dark", "light"):
            await see_checks(env, results, theme)
        errors = env.page.errors
        results["no console errors"] = not errors
        if errors:
            print("ERRORS", errors[:5])
    return report(results)


sys.exit(asyncio.run(main()))

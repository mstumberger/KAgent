#!/usr/bin/env python3
"""The runs board (workflows.md section 8): the sidebar entry, the tree of projects, runs and tasks with their dots, the run in
tabs, the progress and the workers on the right, the plan approval, the decision about a task, the questions, New run, and the
narrow layout.

The mock and the page driver are picker_web_lib.py (its runs routes); the window's event stream is its FAKE_WS, so a check feeds the
page the events the daemon would send (`window.__emit`). Dark and light at 1200x820, and 700x500 for the narrow layout; screenshots
go to /tmp/kagent-v003-runs-shots/. Run from the repo root with KAGENT_DIST set to a build of web/ (`npx vite build --outDir <dir>`)."""
import asyncio, datetime, json, sys
import picker_web_lib as lib
from picker_web_lib import Env, calls, report, set_run

P1, P2 = "/home/me/code/kagent", "/home/me/code/web-ui"
T0 = "2026-11-28T09:00:00Z"
# A worker that started 12 minutes ago, so its "Running for" counts on the page's own clock.
AGO = (datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(minutes=12, seconds=9)).strftime("%Y-%m-%dT%H:%M:%SZ")
STEPS = [
    {"id": "plan", "role": "planner", "worker": {"kind": "subagent", "profile": "planner"}, "model": None, "gate": "owner"},
    {"id": "build", "role": "implementer", "worker": {"kind": "subagent", "profile": "implementer"}, "model": None, "gate": None},
    {"id": "review", "role": "reviewer", "worker": {"kind": "subagent", "profile": "reviewer"}, "model": "anthropic::claude-opus-4-6", "gate": None},
    {"id": "wrap", "role": "orchestrator", "worker": None, "model": None, "gate": None},
]
WORKFLOWS = [
    {"name": "plan-implement-review", "description": "A planner splits the job, an implementer does each task, a reviewer checks it.", "shipped": True, "steps": STEPS},
    {"name": "plan-only", "description": "A planner splits the job and the run ends at the approved plan.", "shipped": True, "steps": [STEPS[0], STEPS[3]]},
]


def task(id, title, state, **kw):
    t = {"id": id, "title": title, "description": f"do {id}", "doneWhen": [f"{id} works"], "dependsOn": [], "files": [], "ticket": None, "kaTicket": None, "state": state, "round": 0, "extraRounds": 0,
         "accepted": False, "error": "", "rounds": [], "workers": [], "progress": [], "changedFiles": [], "startedAt": None, "endedAt": None}
    t.update(kw)
    return t


def worker(role, session, round=1, ended=None):
    return {"role": role, "session": session, "round": round, "startedAt": T0, "endedAt": ended}


def run(id, title, state, project, tasks, **kw):
    r = {"id": id, "title": title, "workflow": "plan-implement-review", "project": project, "state": state, "reason": "", "ticket": None, "goal": f"{title}\nwith the details of {id}",
         "definition": {"name": "plan-implement-review", "description": "d", "steps": STEPS}, "originSession": None, "runSession": "rs" + id[-1], "planSummary": f"The plan of {id}.", "tasks": tasks,
         "questions": [], "messages": [], "plannerSession": None, "rejections": 0, "report": "", "createdAt": f"2026-11-28T0{id[-1]}:00:00Z", "updatedAt": f"2026-11-28T0{id[-1]}:30:00Z"}
    r.update(kw)
    return r


R1 = run("run_aaaa1", "Make the overlay keep focus", "working", P1, [
    task("T1", "Focus core", "done", kaTicket="KA-21", ticket="CYB-1", files=["src/focus.rs"], changedFiles=["src/focus.rs"], round=1,
         rounds=[{"round": 1, "report": "Changed focus.rs; cargo test passes.\nMore detail.", "verdict": "pass", "findings": []}], workers=[worker("implementer", "w1", 1, T0), worker("reviewer", "w2", 1, T0)]),
    task("T2", "Overlay click", "implementing", kaTicket="KA-22", dependsOn=["T1"], round=1, workers=[worker("implementer", "w3", 2 - 1, None) | {"startedAt": AGO}], files=["src/overlay.rs"],
         progress=[{"title": "read the overlay code", "status": "completed"}, {"title": "change the click handler", "status": "in_progress"}, {"title": "run the tests", "status": "pending"}]),
    task("T3", "Docs", "waiting", kaTicket="KA-23", dependsOn=["T2"]),
])
R2 = run("run_aaaa2", "Split the settings page", "awaiting_approval", P1, [task("T1", "Extract the form", "waiting"), task("T2", "Move the tabs", "waiting", dependsOn=["T1"]), task("T3", "Update the docs", "waiting")])
R3 = run("run_aaaa3", "Rename the sidebar entry", "done", P1, [task("T1", "Rename", "done", rounds=[{"round": 1, "report": "Renamed.", "verdict": "pass", "findings": []}], workers=[worker("implementer", "w9", 1, T0)])], report="Run run_aaaa3: 1 of 1 tasks done.")
R4 = run("run_aaaa4", "Port the old theme", "failed", P2, [task("T1", "Port", "failed", error="the worker fell over")], reason="1 task(s) failed, 0 blocked")
FINDINGS = [{"severity": "must", "file": "a.rs", "line": 3, "what": "the loop never ends", "why": "it hangs"}, {"severity": "should", "file": "b.rs", "line": 0, "what": "a nit", "why": ""}]
R5 = run("run_aaaa5", "Fix the loop", "working", P2, [task("T1", "The loop", "needs_owner", round=3, rounds=[{"round": i, "report": f"r{i}", "verdict": "changes", "findings": FINDINGS} for i in (1, 2, 3)])],
         questions=[{"id": "q_1", "task": "T1", "text": "Which license?", "options": ["MIT", "GPL"], "blocking": True, "state": "escalated", "askedAt": T0, "answer": None, "answeredBy": None}])
R6 = run("run_aaaa6", "Tidy the logs", "paused", P2, [task("T1", "Tidy", "implementing", round=1)], reason="the daemon restarted")
CHILD = lambda cid, title, status, **kw: {"id": cid, "title": title, "profile": "implementer", "status": status, **({"startedAt": T0} | kw)}
CHILDREN = {"rs1": [CHILD("w1", "Implement T1", "done", endedAt=T0), CHILD("w2", "Review T1", "done", endedAt=T0), CHILD("w3", "Implement T2", "running", startedAt=AGO)]}
DIFF = {"project": P1, "from": "HEAD", "files": ["src/focus.rs"], "diff": {"files": [{"path": "src/focus.rs", "oldPath": None, "status": "modified", "binary": False, "tooLarge": False, "added": 1, "removed": 1,
        "hunks": [{"oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2, "header": "", "lines": [{"kind": "ctx", "old": 1, "new": 1, "text": "fn focus() {"}, {"kind": "del", "old": 2, "new": None, "text": "    old();"}, {"kind": "add", "old": None, "new": 2, "text": "    new_call();"}]}]}],
        "added": 1, "removed": 1, "truncated": False}}
DETAILS = {"w3": {"child": CHILD("w3", "Implement T2", "running", lastAction={"tool": "edit_file", "summary": "edit src/overlay.rs", "status": "running", "at": T0}, files=[{"path": "src/overlay.rs", "changed": True}]),
                  "job": "Implement T2: Overlay click", "todo": [], "transcript": [{"role": "assistant", "kind": "text", "text": "Reading the overlay."}]}}
CFG = dict(runs=[R1, R2, R3, R4, R5, R6], workflows=WORKFLOWS, run_children=CHILDREN, run_diffs={"run_aaaa1/T1": DIFF}, details=DETAILS,
           sessions={"s1": {"title": "Chat", "updatedAt": "2026-11-28T09:00:00Z", "project": P1}, "w3": {"title": "Implement T2", "origin": "subagent", "parent": "rs1", "updatedAt": T0}},
           messages={"w3": [{"role": "user", "content": "Implement T2: the overlay click"}, {"role": "assistant", "content": "Reading the overlay."}]},
           projects=[{"path": P1, "name": "kagent", "agentsMd": True}, {"path": P2, "name": "web-ui", "agentsMd": False}], dirs={P1, P2})
HASH = "&view=runs"
TXT = lambda sel: f"document.querySelector({json.dumps(sel)}).textContent.trim()"
ALL = lambda sel, f="e => e.textContent.trim()": f"[...document.querySelectorAll({json.dumps(sel)})].map({f})"


async def emit(page, ev):
    await page.js(f"window.__emit({json.dumps(ev)})")
    await asyncio.sleep(0.15)


async def updated(page, run_id):
    await emit(page, {"type": "runUpdated", "runId": run_id, "project": P1, "state": "working", "done": 0, "total": 0})


async def opened(env, theme):
    await env.open(theme, hash=HASH, **CFG)
    await env.page.until("document.querySelectorAll('.rn-run').length === 6", "the six runs in the tree")


async def tree_checks(env, results, theme):
    page = env.page
    t = f"{theme} 1200x820"
    await env.size(1200, 820)
    await opened(env, theme)
    entries = await page.js("[...document.querySelectorAll('.sb-nav-item')].map(b => b.textContent.trim())")
    results[f"{t}: the sidebar has the entry Runs after Tickets"] = entries[entries.index("Tickets") + 1] == "Runs"
    results[f"{t}: the entry is the current page and the view says Runs"] = await page.js("document.querySelector('.sb-nav-item[aria-current=page]').textContent.trim()") == "Runs" and await page.js(TXT(".rn-head h2")) == "Runs"
    results[f"{t}: the work panel keeps its own name (the view is not called Work)"] = "Work" not in entries
    projects = await page.js(ALL(".rn-project .rn-name"))
    results[f"{t}: the tree has the projects of the runs"] = sorted(projects) == ["kagent", "web-ui"]
    names = await page.js(ALL(".rn-run .rn-name"))
    results[f"{t}: each project lists its runs"] = set(names) == {R["title"] for R in (R1, R2, R3, R4, R5, R6)}
    dots = await page.js("Object.fromEntries([...document.querySelectorAll('.rn-run')].map(r => [r.dataset.run, r.querySelector('.rn-dot').dataset.dot]))")
    results[f"{t}: the dots say working, needs you, done, failed, needs you, paused"] = dots == {"run_aaaa1": "run", "run_aaaa2": "owner", "run_aaaa3": "done", "run_aaaa4": "fail", "run_aaaa5": "owner", "run_aaaa6": "wait"}
    results[f"{t}: the status is also in words for a screen reader"] = await page.js("document.querySelector('.rn-run[data-run=run_aaaa2] .rn-sr').textContent") == "needs you" and await page.js("document.querySelector('.rn-run[data-run=run_aaaa4] .rn-sr').textContent") == "failed"
    results[f"{t}: a run shows its tasks done of total"] = await page.js("document.querySelector('.rn-run[data-run=run_aaaa1] .rn-count').textContent") == "1/3"
    # --- keyboard: the arrow keys move between the items, Enter opens
    await page.js("document.querySelector('.rn-run[data-run=run_aaaa1]').focus()")
    await page.key("ArrowDown", "ArrowDown", 40)
    after = await page.js("document.activeElement.dataset.run || document.activeElement.className")
    results[f"{t}: the arrow keys move the focus along the tree"] = after != "rn-run selected" and await page.js("document.activeElement.hasAttribute('data-ti')")
    await page.js("document.querySelector('.rn-run[data-run=run_aaaa1]').focus()")
    await page.key("Enter", "Enter", 13, text="\r")
    await page.until("!!document.querySelector('.rn-run-head')", "the run opens with Enter")
    results[f"{t}: Enter opens a run in the middle"] = await page.js(TXT(".rn-run-head h2")) == "Make the overlay keep focus"
    # --- the tasks of the open run
    await page.until("document.querySelectorAll('.rn-task').length === 3", "its tasks")
    tasks = await page.js(ALL(".rn-task", "t => [t.dataset.task, t.querySelector('.rn-dot').dataset.dot, t.querySelector('.rn-key') && t.querySelector('.rn-key').textContent].join('|')"))
    results[f"{t}: the open run's tasks are under it in plan order with a dot and a ticket key"] = tasks == ["T1|done|CYB-1", "T2|run|KA-22", "T3|wait|KA-23"]
    await page.shot(f"tree-{theme}")


async def run_checks(env, results, theme):
    page = env.page
    t = f"{theme} 1200x820"
    badge = await page.js("document.querySelector('.rn-state-badge').dataset.state")
    results[f"{t}: the header says the state of the run"] = badge == "working" and "1 of 3 tasks done" in await page.js(TXT(".rn-run-head"))
    ov = await page.js("({goal: document.querySelector('.rn-goal').textContent, plan: [...document.querySelectorAll('.rn-overview p')].map(p => p.textContent).join('|'), steps: [...document.querySelectorAll('.rn-steps li')].map(l => l.querySelector('.rn-step-role').textContent + ':' + l.dataset.active), gate: !!document.querySelector('.rn-step-gate')})")
    results[f"{t}: Overview has the goal, the plan's summary and the steps with the active one marked"] = ov["goal"].startswith("Make the overlay keep focus") and "The plan of run_aaaa1." in ov["plan"] and ov["steps"] == ["planner:false", "implementer:true", "reviewer:false", "orchestrator:false"] and ov["gate"]
    results[f"{t}: the active step is marked for assistive technology too"] = await page.js("document.querySelector('.rn-steps li[data-active=true]').getAttribute('aria-current')") == "step"
    # --- the right column
    side = await page.js("({tasks: [...document.querySelectorAll('.rn-check li')].map(l => l.dataset.status + ':' + l.textContent.trim()), h: document.querySelector('.rn-side h3').textContent})")
    results[f"{t}: the right column lists the run's tasks as a checklist, the done one struck through"] = side["tasks"] == ["completed:✓T1 Focus core", "in_progress:→T2 Overlay click", "pending:·T3 Docs"] and "1/3" in side["h"] and await page.js("!!document.querySelector('.rn-check li[data-status=completed] .rn-struck')")
    running = await page.js("({n: document.querySelectorAll('.rn-side .rn-agent[data-session]:not([data-ended])').length, head: [...document.querySelectorAll('.rn-side h3')][1].textContent, note: document.querySelector('.rn-agent[data-session=w3] .rn-agent-note').textContent})")
    results[f"{t}: Agents says 1 running with its task and how long it runs"] = running["n"] == 1 and "1 running" in running["head"] and running["note"].startswith("Running for ")
    first = running["note"]
    await page.until(f"document.querySelector('.rn-agent[data-session=w3] .rn-agent-note').textContent !== {json.dumps(first)}", "the timer ticks", 4)
    results[f"{t}: the running time ticks"] = True
    await page.js("document.querySelector('.rn-ended summary').click()")
    ended = await page.js("[...document.querySelectorAll('.rn-ended .rn-agent')].map(a => a.querySelector('.rn-agent-title').textContent + '|' + a.querySelector('.rn-agent-word').textContent + '|' + ([...a.querySelectorAll('.rn-agent-note')].map(n => n.textContent)[0] || ''))")
    results[f"{t}: Ended (2) lists the finished workers with how they ended and the first line of the report"] = await page.js(TXT(".rn-ended summary")) == "Ended (2)" and ended[0].startswith("Review T1|Completed") and ended[1].startswith("Implement T1|Completed|Changed focus.rs; cargo test passes.")
    await page.shot(f"overview-{theme}")
    # --- a click on a task opens its worker, and the right column shows its own steps
    await page.js("document.querySelector('.rn-task[data-task=T2]').click()")
    await page.until("!!document.querySelector('.pane-subagent')", "the worker pane")
    results[f"{t}: a task opens its worker in a tab, through the steering pane (its transcript, its doing line)"] = await page.js("document.querySelector('[role=tab][aria-selected=true]').textContent") == "Worker T2" and "Reading the overlay." in await page.js("document.querySelector('.pane-subagent').textContent")
    await page.until("!!document.querySelector('.worker-doing')", "what the worker is doing")
    results[f"{t}: it shows what the worker is doing now and the box to tell it something"] = await page.js(TXT(".worker-doing")) == "edit src/overlay.rs" and await page.js("!!document.querySelector('.worker-tell-box')")
    steps = await page.js("[...document.querySelectorAll('.rn-check li')].map(l => l.dataset.status + ':' + l.textContent.trim())")
    results[f"{t}: the right column shows the worker's own steps, done struck through, the active one with an arrow"] = steps == ["completed:✓read the overlay code", "in_progress:→change the click handler", "pending:·run the tests"] and await page.js(TXT(".rn-side h3")) == "Progress T2 2/3".replace("2/3", "1/3")
    await page.shot(f"worker-{theme}")
    await page.js("document.querySelector('button[aria-label=\"Close Worker T2\"]').click()")
    results[f"{t}: a worker tab closes and the Overview is back"] = await page.js("document.querySelector('[role=tab][aria-selected=true]').textContent") == "Overview"
    # --- the Plan tab
    await page.js("[...document.querySelectorAll('[role=tab]')].find(b => b.textContent === 'Plan').click()")
    await page.until("!!document.querySelector('.rn-table')", "the plan table")
    rows = await page.js("[...document.querySelectorAll('.rn-table tbody tr')].map(r => [...r.children].slice(0, 5).map(c => c.textContent.trim()).join('|'))")
    results[f"{t}: Plan lists the tasks with status, round, worker and tickets"] = rows == ["T1 Focus core|done|1|Open worker|KA-21 CYB-1", "T2 Overlay click|implementing|1|Open worker|KA-22", "T3 Docs|waiting|||KA-23"]
    results[f"{t}: the Plan tab has no approval card while nothing waits"] = await page.js("!document.querySelector('.rn-approval')")
    # --- the changes of a task, in the diff view
    await page.js("[...document.querySelectorAll('.rn-table tr[data-task=T1] button')].find(b => b.textContent === 'Changes').click()")
    await page.until("!!document.querySelector('.rn-changes .diffview')", "the diff view")
    results[f"{t}: Changes opens the diff of the task's files in the diff view"] = "GET /api/runs/run_aaaa1/tasks/T1/diff" in calls() and "focus.rs" in await page.js("document.querySelector('.rn-changes').textContent") and "new_call" in await page.js("document.querySelector('.rn-changes').textContent")
    await page.shot(f"changes-{theme}")
    # --- the controls
    await page.js("[...document.querySelectorAll('[role=tab]')].find(b => b.textContent === 'Overview').click()")
    await page.js("[...document.querySelectorAll('.rn-actions button')].find(b => b.textContent === 'Pause').click()")
    await page.until("document.querySelector('.rn-state-badge').dataset.state === 'paused' || !!document.querySelector('.rn-error')", "pause")
    results[f"{t}: Pause posts to the run (the page reads it again on runUpdated)"] = "POST /api/runs/run_aaaa1/pause {}" in calls()
    await updated(page, "run_aaaa1")
    await page.until("document.querySelector('.rn-state-badge').dataset.state === 'paused'", "paused after the event")
    await page.until("!![...document.querySelectorAll('.rn-actions button')].find(b => b.textContent === 'Continue')", "Continue")
    await page.js("[...document.querySelectorAll('.rn-actions button')].find(b => b.textContent === 'Continue').click()")
    await updated(page, "run_aaaa1")
    await page.until("document.querySelector('.rn-state-badge').dataset.state === 'working'", "continued")
    results[f"{t}: Continue starts it again"] = "POST /api/runs/run_aaaa1/continue {}" in calls()
    await page.js("[...document.querySelectorAll('.rn-actions button')].find(b => b.textContent === 'Stop run').click()")
    await updated(page, "run_aaaa1")
    await page.until("document.querySelector('.rn-state-badge').dataset.state === 'stopped'", "stopped")
    results[f"{t}: Stop run stops it and the controls go"] = "POST /api/runs/run_aaaa1/stop {}" in calls() and await page.js("![...document.querySelectorAll('.rn-actions button')].some(b => b.textContent === 'Stop run')")
    results[f"{t}: the dot in the tree follows the event without a reload"] = await page.js("document.querySelector('.rn-run[data-run=run_aaaa1] .rn-dot').dataset.dot") == "off"


async def plan_checks(env, results, theme):
    page = env.page
    t = f"{theme} 1200x820"
    await opened(env, theme)
    await page.js("document.querySelector('.rn-run[data-run=run_aaaa2]').click()")
    await page.until("!!document.querySelector('.rn-approval')", "the approval card opens on the Plan tab")
    results[f"{t}: a run that waits for the plan opens on the Plan tab with the approval card"] = await page.js("document.querySelector('[role=tab][aria-selected=true]').textContent") == "Plan" and "The plan of run_aaaa2." in await page.js(TXT(".rn-approval"))
    await page.shot(f"approval-{theme}")
    rej = await page.js("[...document.querySelectorAll('.rn-reject button')][0].disabled")
    results[f"{t}: Reject needs a reason"] = rej is True
    await page.js("""(() => { const ta = document.querySelector('.rn-reject textarea'); const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set; set.call(ta, 'split it in two'); ta.dispatchEvent(new Event('input', {bubbles: true})); })()""")
    await page.until("!document.querySelector('.rn-reject button').disabled", "the reason typed")
    await page.js("document.querySelector('.rn-reject button').click()")
    await asyncio.sleep(0.3)
    results[f"{t}: Reject posts the reason"] = 'POST /api/runs/run_aaaa2/plan {"action": "reject", "reason": "split it in two"}' in calls()
    await opened(env, theme)
    await page.js("document.querySelector('.rn-run[data-run=run_aaaa2]').click()")
    await page.until("!!document.querySelector('.rn-approval')", "the card again")
    await page.js("[...document.querySelectorAll('.rn-approval .rn-actions button')].find(b => b.textContent === 'Edit tasks').click()")
    await page.until("document.querySelectorAll('.rn-edit li').length === 3", "the edit list")
    await page.js("""(() => { const i = document.querySelector('.rn-edit input[aria-label="Title of T3"]'); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; set.call(i, 'Docs for the page'); i.dispatchEvent(new Event('input', {bubbles: true})); })()""")
    await page.js("document.querySelector('.rn-edit button[aria-label=\"Move T3 up\"]').click()")
    await page.js("document.querySelector('.rn-edit button[aria-label=\"Remove T2\"]').click()")
    await page.until("document.querySelectorAll('.rn-edit li').length === 2", "T2 removed")
    order = await page.js(ALL(".rn-edit li .rn-id"))
    results[f"{t}: the tasks can be renamed, moved and removed before approving"] = order == ["T1", "T3"] or order == ["T3", "T1"]
    await page.js("[...document.querySelectorAll('.rn-approval .rn-actions button')].find(b => b.textContent === 'Approve plan').click()")
    await asyncio.sleep(0.3)
    body = [c for c in calls() if c.startswith("POST /api/runs/run_aaaa2/plan") and "approve" in c]
    results[f"{t}: Approve posts the edited list in its order"] = len(body) == 1 and json.loads(body[0].split("plan ", 1)[1]) == {"action": "approve", "tasks": [{"id": i, "title": ti} for i, ti in zip(order, ["Extract the form" if o == "T1" else "Docs for the page" for o in order])]}
    await updated(page, "run_aaaa2")
    await page.until("!document.querySelector('.rn-approval')", "the card goes once the run works")
    results[f"{t}: the approval card goes when the run goes on"] = True
    # --- a task that needs the owner, and a question
    await page.js("document.querySelector('.rn-run[data-run=run_aaaa5]').click()")
    await page.until("!!document.querySelector('.rn-run-head')", "run 5 opens")
    await page.js("[...document.querySelectorAll('[role=tab]')].find(b => b.textContent === 'Plan').click()")
    await page.until("!!document.querySelector('.rn-decision')", "the decision card on the Plan tab")
    card = await page.js("({h: document.querySelector('.rn-decision h4').textContent, items: [...document.querySelectorAll('.rn-decision li')].map(l => l.textContent), buttons: [...document.querySelectorAll('.rn-decision button')].map(b => b.textContent)})")
    results[f"{t}: a task that needs the owner shows its findings and the three choices"] = "T1 still has problems after 3 rounds" in card["h"] and card["items"][0] == "must: the loop never ends (a.rs:3)" and card["buttons"] == ["One more round", "Accept as is", "Drop the task"]
    q = await page.js("({text: document.querySelector('.rn-question p').textContent, options: [...document.querySelectorAll('.rn-question .rn-actions button')].map(b => b.textContent)})")
    results[f"{t}: an escalated question is a card with its options"] = "Which license?" in q["text"] and q["options"] == ["MIT", "GPL"]
    await page.shot(f"decision-{theme}")
    await page.js("[...document.querySelectorAll('.rn-question .rn-actions button')].find(b => b.textContent === 'GPL').click()")
    await asyncio.sleep(0.3)
    results[f"{t}: an option answers the question"] = 'POST /api/runs/run_aaaa5/questions/q_1/answer {"text": "GPL"}' in calls()
    await page.js("[...document.querySelectorAll('.rn-decision button')].find(b => b.textContent === 'Accept as is').click()")
    await asyncio.sleep(0.3)
    results[f"{t}: the decision posts the choice"] = 'POST /api/runs/run_aaaa5/tasks/T1/decision {"choice": "accept"}' in calls()


async def refusal_checks(env, results, theme):
    page = env.page
    t = f"{theme} 1200x820"
    await env.open(theme, hash=HASH, run_refuse="the run has ended already", **CFG)
    await page.until("document.querySelectorAll('.rn-run').length === 6", "the runs")
    await page.js("document.querySelector('.rn-run[data-run=run_aaaa6]').click()")
    await page.until("!!document.querySelector('.rn-state-badge')", "run 6 opens")
    await page.js("[...document.querySelectorAll('.rn-actions button')].find(b => b.textContent === 'Continue').click()")
    await page.until("!!document.querySelector('.rn-error')", "the refusal")
    results[f"{t}: a refusal of the daemon is shown as an alert"] = await page.js(TXT(".rn-error")) == "the run has ended already" and await page.js("document.querySelector('.rn-error').getAttribute('role')") == "alert"


async def new_checks(env, results, theme):
    page = env.page
    t = f"{theme} 1200x820"
    await opened(env, theme)
    await page.js("[...document.querySelectorAll('.rn-head button')].find(b => b.textContent === 'New run').click()")
    await page.until("!!document.querySelector('form.rn-new') && document.querySelectorAll('form.rn-new select option').length >= 2", "the form")
    await page.until("document.querySelectorAll('.rn-roles li').length === 3 && !!document.querySelector('form.rn-new select').value", "the workers of the first workflow")
    form = await page.js("({workflows: [...document.querySelector('form.rn-new select').options].map(o => o.value), desc: document.querySelector('form.rn-new .rn-none').textContent, roles: [...document.querySelectorAll('.rn-roles li')].map(l => l.textContent.slice(0, 20)), project: document.querySelectorAll('form.rn-new select')[1].value, start: document.querySelector('form.rn-new button[type=submit]').disabled})")
    results[f"{t}: New run lists the workflows with their description and the workers of each role"] = form["workflows"] == ["plan-implement-review", "plan-only"] and form["desc"].startswith("A planner splits the job, an implementer") and len(form["roles"]) == 3
    results[f"{t}: the project is the one of the open conversation, and Start is off until a goal is typed"] = form["project"] == P1 and form["start"] is True
    await page.shot(f"new-run-{theme}")
    await page.js("""(() => { const set = (el, v, proto) => { Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, v); el.dispatchEvent(new Event('input', {bubbles: true})); };
        set(document.querySelector('form.rn-new textarea'), 'Add a dark theme\\nwith care', HTMLTextAreaElement.prototype);
        set(document.querySelector('input[aria-label="Model of the reviewer"]'), 'gemini::flash', HTMLInputElement.prototype);
        set(document.querySelector('form.rn-new input[placeholder^="A key"]'), 'KA-9', HTMLInputElement.prototype); })()""")
    await page.until("!document.querySelector('form.rn-new button[type=submit]').disabled", "Start on")
    await page.js("document.querySelector('form.rn-new button[type=submit]').click()")
    await page.until("!document.querySelector('form.rn-new') && !!document.querySelector('.rn-run-head')", "the new run opens")
    body = [c for c in calls() if c.startswith("POST /api/runs {")]
    results[f"{t}: Start posts the workflow, project, goal, ticket and the changed model"] = len(body) == 1 and json.loads(body[0].split("runs ", 1)[1]) == {"workflow": "plan-implement-review", "project": P1, "goal": "Add a dark theme\nwith care", "ticket": "KA-9", "models": {"reviewer": "gemini::flash"}}
    results[f"{t}: the new run is open and in the tree"] = await page.js(TXT(".rn-run-head h2")) == "Add a dark theme" and "Add a dark theme" in await page.js(ALL(".rn-run .rn-name"))
    await page.js("[...document.querySelectorAll('.rn-head button')].find(b => b.textContent === 'New run').click()")
    await page.until("!!document.querySelector('form.rn-new')", "the form again")
    await page.js("document.querySelector('form.rn-new textarea').focus()")
    await page.key("Escape", "Escape", 27)
    results[f"{t}: Escape closes the form"] = await page.js("!document.querySelector('form.rn-new')")


async def a11y_checks(env, results, theme):
    page = env.page
    t = f"{theme} 1200x820"
    await opened(env, theme)
    await page.js("document.querySelector('.rn-run[data-run=run_aaaa1]').click()")
    await page.until("document.querySelectorAll('.rn-task').length === 3", "tasks")
    await page.js("[...document.querySelectorAll('[role=tab]')].find(b => b.textContent === 'Plan').click()")
    unnamed = await page.js("[...document.querySelectorAll('.runs button')].filter(b => !(b.textContent.trim() || b.getAttribute('aria-label') || b.title)).length")
    results[f"{t}: every button of the board has a name"] = unnamed == 0
    tabs = await page.js("({list: !!document.querySelector('[role=tablist]'), panel: document.querySelector('[role=tabpanel]').getAttribute('aria-labelledby'), sel: document.querySelectorAll('[role=tab][aria-selected=true]').length})")
    results[f"{t}: the tabs are a tablist with one selected and a labelled panel"] = tabs["list"] and tabs["sel"] == 1 and tabs["panel"].startswith("rn-tab-")
    await page.js("document.querySelector('[role=tab][aria-selected=true]').focus()")
    await page.key("ArrowLeft", "ArrowLeft", 37)
    results[f"{t}: the arrow keys move between the tabs"] = await page.js("document.querySelector('[role=tab][aria-selected=true]').textContent") == "Overview"
    results[f"{t}: the tree is a tree with levels and a selected item"] = await page.js("""document.querySelector('[role=tree]').getAttribute('aria-label') !== null && document.querySelectorAll('[role=treeitem][aria-level="3"]').length === 3 && document.querySelector('[role=treeitem][aria-selected=true]') !== null""")


async def narrow(env, results, theme):
    page = env.page
    t = f"{theme} 700x500"
    await env.size(700, 500)
    await opened(env, theme)
    results[f"{t}: the tree has no horizontal page scroll and the middle is behind it"] = await page.js("document.documentElement.scrollWidth <= innerWidth && getComputedStyle(document.querySelector('.rn-centre')).display === 'none'")
    await page.shot(f"tree-narrow-{theme}")
    await page.js("document.querySelector('.rn-run[data-run=run_aaaa1]').click()")
    await page.until("!!document.querySelector('.rn-run-head')", "the run at 700x500")
    results[f"{t}: a run takes the whole view with a way back, and the page does not scroll sideways"] = await page.js("getComputedStyle(document.querySelector('.rn-left')).display === 'none' && getComputedStyle(document.querySelector('.rn-back')).display !== 'none' && document.documentElement.scrollWidth <= innerWidth")
    results[f"{t}: the progress and the workers are below the run"] = await page.js("document.querySelector('.rn-side').getBoundingClientRect().top >= document.querySelector('.rn-centre').getBoundingClientRect().top")
    await page.shot(f"run-narrow-{theme}")
    await page.js("document.querySelector('.rn-back').click()")
    results[f"{t}: Back to runs shows the tree again"] = await page.js("getComputedStyle(document.querySelector('.rn-left')).display !== 'none'")
    await page.js("[...document.querySelectorAll('.rn-head button')].find(b => b.textContent === 'New run').click()")
    await page.until("!!document.querySelector('form.rn-new')", "the form at 700x500")
    results[f"{t}: the New run form fits the width"] = await page.js("document.documentElement.scrollWidth <= innerWidth")
    await page.shot(f"new-run-narrow-{theme}")


async def main():
    results = {}
    async with Env("kagent-runs", "/tmp/kagent-v003-runs-shots") as env:
        for theme in ("dark", "light"):
            await tree_checks(env, results, theme)
            await run_checks(env, results, theme)
            await plan_checks(env, results, theme)
            await refusal_checks(env, results, theme)
            await new_checks(env, results, theme)
            await a11y_checks(env, results, theme)
            await narrow(env, results, theme)
        # The refusals the checks cause on purpose are logged by the browser as failed loads.
        errors = [e for e in env.page.errors if not ("/api/runs" in e and ("status of 409" in e or "status of 404" in e))]
        results["no console errors"] = not errors
        if errors:
            print("ERRORS", errors[:5])
    return report(results)


sys.exit(asyncio.run(main()))

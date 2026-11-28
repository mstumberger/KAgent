"""The mock API, the CDP page driver and the helpers of test-runs-web.py (workflow runs), test-picker-web.py (S41), test-sidebar-web.py (S42), test-work-web.py (S43), test-vision-web.py (S46), test-local-models-web.py (S47) and test-screen-watch-web.py (S48)."""
import asyncio, base64, json, os, re, urllib.parse, shutil, subprocess, sys, tempfile, threading, time, urllib.request, urllib.error
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import websockets

ROOT = os.getcwd()
# KAGENT_DIST: a build of the window somewhere else (the owner's daemon serves web/dist, which a test must not replace).
DIST = os.environ.get("KAGENT_DIST") or f"{ROOT}/web/dist"
SHOTS = "/tmp/kagent-s41-shots"
PORT, CDP = 18802, 9334
DAEMON = "http://127.0.0.1:7878"
TOKEN = {"v": ""}


def model(pid, pname, mid, levels=(), image=False, local=None, free=None, ctx=200000, tools=True):
    """A model of GET /api/models; `local` and `free` are the flags of the chooser (chooser.md section 2), left out when None."""
    flags = {k: v for k, v in (("local", local), ("free", free)) if v is not None}
    return {"id": mid, "name": mid, "ref": f"{pid}::{mid}", "providerId": pid, "providerName": pname, **flags,
            "config": {"enabled": True, "contextWindow": ctx, "maxOutputTokens": 8000, "supportsToolCall": tools, "supportsImage": image,
                       "reasoningLevels": list(levels), "reasoningPatches": {}, "supportsLive": False, "liveProtocol": None, "requestPatch": None, "roles": ["chat"]}}


MODELS = [model("anthropic", "Anthropic", "claude-opus-4-6", ["low", "medium", "high"], True), model("anthropic", "Anthropic", "claude-haiku-4-5"),
          model("gemini", "Google Gemini", "models/gemini-3.8-flash", [], True)]
CFG = {}
CALLS = []
SESSIONS = {}
# The folders of the mock file system and the projects `GET /api/projects` lists (S42; set with `reset`).
DIRS = set()
PROJECTS = []
# The tickets of the mock (tickets.md): {"projects": [...], "tickets": [...]}; `reset(tickets=..., ticket_projects=...)` fills them.
TICKETS = {"projects": [], "tickets": []}
# The workflow runs of the mock (workflows.md): {id: run}, the workflows GET /api/workflows lists, and the children of a session
# (`GET /api/sessions/:id/subagents`); `reset(runs=[...], workflows=[...], run_children={...}, run_diffs={"<run>/<task>": diff})` fill them.
RUNS = {}
WORKFLOWS = []
OPENED = []  # the sessions the window read (`GET /api/sessions/:id`), in order
# The work panel (S43; set with `reset`): the background tasks per session, the output of a task, the messages per session,
# the pending approvals, and the reads of an output (`GET /api/background/:id/output`) as (task, tail).
TASKS = {}
OUTPUT = {}
MESSAGES = {}
APPROVALS = []
READS = []
# The images of the mock (S46): the bytes of each upload by session and id, and the uploads in order as (session, size).
IMAGES = {}
UPLOADS = []
# The local models page (S47; set with `reset(local=<the view of GET /api/local-models>)`): the view the mock answers and changes
# as the page downloads, cancels, removes and sets a model. Empty: the routes fall through to the daemon.
LOCAL = {}
# Routes a suite adds to the mock: callables (method, path, body) -> (status, body) or None; checked before the daemon routes.
ROUTES = []
# The model keys the page saved with PUT /api/settings (chooser.md section 1), in order, as JSON text.
SETTING_PUTS = []
# The screen watch (S48; set with `reset(watch=<the watch of GET /api/screen-watch>, watch_model=<GET /api/screen-watch/model>)`):
# the watch the daemon would report (None: none runs), and the model line of the settings card.
WATCH = {}


# The voice session of the window opens a WebSocket on /live; a stand-in that stays open keeps the voice chip in the title bar.
FAKE_LIVE = """(() => { const Real = window.WebSocket; window.WebSocket = function (url, ...rest) { if (String(url).includes('/live?')) { const fake = { readyState: 1, send() {}, close() {}, onopen: null }; setTimeout(() => { fake.onopen && fake.onopen(); fake.onmessage && fake.onmessage({ data: JSON.stringify({ type: 'status', status: 'connecting' }) }); }, 20); return fake; } return new Real(url, ...rest); }; window.WebSocket.OPEN = 1; window.WebSocket.CONNECTING = 0; window.WebSocket.CLOSED = 3; window.WebSocket.CLOSING = 2; })();"""

# The window's own event stream (`/ws`) stands in for a socket the page can be fed through: `window.__emit(event)` is what
# the daemon would send (S43). The voice socket (`/live`) is left to the test that needs it.
FAKE_WS = """(() => { const Real = window.WebSocket; window.__socks = []; window.WebSocket = function (url, ...rest) { if (String(url).includes('/ws?')) { const fake = { readyState: 1, send() {}, close() {}, onopen: null, onmessage: null, onclose: null }; window.__socks.push(fake); setTimeout(() => fake.onopen && fake.onopen(), 10); return fake; } return new Real(url, ...rest); }; window.WebSocket.OPEN = 1; window.WebSocket.CONNECTING = 0; window.WebSocket.CLOSED = 3; window.WebSocket.CLOSING = 2; window.__emit = (ev) => window.__socks.forEach((s) => s.onmessage && s.onmessage({ data: JSON.stringify(ev) })); })();"""


def reset(**kw):
    CFG.clear()
    CFG.update(valid=True, default="gemini::models/gemini-3.8-flash", models=MODELS)
    CFG.update(kw)
    CALLS.clear()
    OPENED.clear()
    READS.clear()
    IMAGES.clear()
    UPLOADS.clear()
    # Stored images the session already has: {session: {id: base64 of a PNG}}.
    for sid, stored in (kw.get("images") or {}).items():
        for iid, b64 in stored.items():
            data = base64.b64decode(b64)
            IMAGES.setdefault(sid, {})[iid] = (data, *png_size(data))
    SETTING_PUTS.clear()
    LOCAL.clear()
    LOCAL.update(json.loads(json.dumps(kw["local"])) if kw.get("local") else {})
    WATCH.clear()
    WATCH.update(watch=json.loads(json.dumps(kw.get("watch"))), model=kw.get("watch_model") or {"model": "gemma3:4b", "server": "http://127.0.0.1:11434", "refusal": None})
    STUB["settings"].pop("screenWatch", None)
    TASKS.clear()
    TASKS.update({k: [dict(t) for t in v] for k, v in kw.get("tasks", {}).items()})
    OUTPUT.clear()
    OUTPUT.update(kw.get("outputs", {}))
    MESSAGES.clear()
    MESSAGES.update(kw.get("messages", {}))
    APPROVALS[:] = kw.get("approvals", [])
    SESSIONS.clear()
    DIRS.clear()
    DIRS.update(kw.get("dirs", ()))
    PROJECTS[:] = kw.get("projects", [])
    RUNS.clear()
    RUNS.update({r["id"]: json.loads(json.dumps(r)) for r in kw.get("runs", [])})
    WORKFLOWS[:] = json.loads(json.dumps(kw.get("workflows", [])))
    TICKETS["projects"] = json.loads(json.dumps(kw.get("ticket_projects", [])))
    TICKETS["tickets"] = json.loads(json.dumps(kw.get("tickets", [])))
    SESSIONS["s1"] = meta("s1", kw.get("session", {}))
    for sid, extra in (kw.get("sessions") or {}).items():
        SESSIONS[sid] = meta(sid, extra)


def meta(sid, extra=None):
    m = {"id": sid, "title": "Chat " + sid, "createdAt": "2026-11-27T08:00:00Z", "updatedAt": "2026-11-27T08:00:00Z", "messageCount": 0, "origin": "chat",
         "todo": [], "planning": False, "approvedPlan": None, "model": None, "reasoningLevel": None, "project": None, "pinned": False, "imagesNoticeShown": False, "archived": False, "renamed": False}
    m.update(extra or {})
    return m


def local_models(method, path, body):
    """The local models routes of the mock (S47): None when the path is not one, else (status, body). Every change is in CALLS."""
    ref_of = lambda m: m.get("ref")
    if path == "/api/local-models" and method == "GET":
        return 200, LOCAL
    if path == "/api/local-models/pull" and method == "POST":
        CALLS.append(f"POST {path} {json.dumps(body)}")
        m = next((x for x in LOCAL["recommended"] if x["id"] == body.get("id")), None)
        if not m:
            return 400, {"error": {"code": "invalid_input", "message": "not on the list"}}
        LOCAL["pulls"] = [p for p in LOCAL["pulls"] if p["id"] != m["id"]] + [{"id": m["id"], "state": "pulling", "doneMb": 0, "totalMb": m["sizeMb"]}]
        return 202, {"id": m["id"], "pulls": LOCAL["pulls"]}
    mc = re.fullmatch(r"/api/local-models/pull/(.+)", path)
    if mc and method == "DELETE":
        CALLS.append(f"DELETE {path}")
        LOCAL["pulls"] = [p for p in LOCAL["pulls"] if p["id"] != urllib.parse.unquote(mc.group(1))]
        return 204, b""
    mr = re.fullmatch(r"/api/local-models/(.+)", path)
    if mr and method == "DELETE":
        CALLS.append(f"DELETE {path}")
        m = next((x for x in LOCAL["recommended"] if x["id"] == urllib.parse.unquote(mr.group(1))), None)
        if m and m["usedFor"]:
            return 409, {"error": {"code": "conflict", "message": f"{m['id']} is still set as {', '.join(m['usedFor'])} in Settings"}}
        if m:
            m.update(installed=False, installedId=None, ref=None)
        return 204, b""
    return None


def local_settings(body):
    """A `PUT /api/settings` that names a model (the "Use for ..." buttons) changes `usedFor` of the mock's view."""
    for key in ("chatModel", "fastModel", "fallbackModel", "visionModel"):
        if key in (body or {}):
            CALLS.append(f"PUT /api/settings {json.dumps(body)}")
            for m in LOCAL["recommended"]:
                m["usedFor"] = [u for u in m["usedFor"] if u != key] + ([key] if m.get("ref") and m["ref"] == body[key] else [])


def run_row(r):
    """The row of GET /api/runs for a run."""
    needs = r["state"] == "awaiting_approval" or any(t["state"] == "needs_owner" for t in r["tasks"]) or any(q["state"] == "escalated" for q in r.get("questions", []))
    return {"id": r["id"], "title": r["title"], "workflow": r["workflow"], "project": r["project"], "state": r["state"], "reason": r.get("reason", ""),
            "done": sum(t["state"] == "done" for t in r["tasks"]), "total": len(r["tasks"]), "needsOwner": needs, "ticket": r.get("ticket"), "createdAt": r["createdAt"], "updatedAt": r["updatedAt"]}


def runs_route(method, path, body):
    """The runs routes of the mock (workflows.md section 6): None when the path is not one, else (status, body). Calls are in CALLS."""
    err = lambda status, code, message: (status, {"error": {"code": code, "message": message}})
    if path == "/api/workflows" and method == "GET":
        return 200, {"workflows": WORKFLOWS, "invalid": []}
    if path == "/api/runs" and method == "GET":
        return 200, {"runs": [run_row(r) for r in RUNS.values()]}
    if path == "/api/runs" and method == "POST":
        CALLS.append(f"POST {path} {json.dumps(body)}")
        if CFG.get("run_error"):
            return err(409, "conflict", CFG["run_error"])
        n = len(RUNS) + 1
        goal = body["goal"]
        r = {"id": f"run_new{n}", "title": goal.splitlines()[0][:80], "workflow": body["workflow"], "project": body["project"], "state": "planning", "reason": "", "ticket": body.get("ticket"), "goal": goal,
             "definition": next(({"name": w["name"], "description": w["description"], "steps": w["steps"]} for w in WORKFLOWS if w["name"] == body["workflow"]), {"name": body["workflow"], "description": "", "steps": []}),
             "originSession": None, "runSession": f"rs_new{n}", "planSummary": "", "tasks": [], "questions": [], "messages": [], "plannerSession": None, "rejections": 0, "report": "",
             "createdAt": "2026-11-28T11:00:00Z", "updatedAt": "2026-11-28T11:00:00Z"}
        RUNS[r["id"]] = r
        return 201, r
    m = re.fullmatch(r"/api/runs/([^/]+)(?:/(stop|pause|continue|plan)|/tasks/([^/]+)/(decision|diff)|/questions/([^/]+)/answer)?", path)
    if m:
        r = RUNS.get(m.group(1))
        if not r:
            return err(404, "not_found", f"no run '{m.group(1)}'")
        if not m.group(2) and not m.group(3) and not m.group(5):
            return 200, r
        if m.group(3) and m.group(4) == "diff":
            CALLS.append(f"GET {path}")
            d = CFG.get("run_diffs", {}).get(f"{m.group(1)}/{m.group(3)}")
            return (200, d) if d else err(409, "conflict", "not a git repository")
        CALLS.append(f"POST {path} {json.dumps(body)}")
        if CFG.get("run_refuse"):
            return err(409, "conflict", CFG["run_refuse"])
        what = m.group(2)
        if what == "stop":
            r["state"] = "stopped"
        elif what == "pause":
            r["state"] = "paused"
        elif what == "continue":
            r["state"] = "working"
        elif what == "plan":
            if body.get("action") == "approve":
                if body.get("tasks"):
                    keep = {t["id"]: t for t in r["tasks"]}
                    r["tasks"] = [dict(keep[t["id"]], title=t.get("title") or keep[t["id"]]["title"]) for t in body["tasks"]]
                r["state"] = "working"
            else:
                r["state"] = "planning"
        elif m.group(4) == "decision":
            t = next(x for x in r["tasks"] if x["id"] == m.group(3))
            t["state"] = {"more_round": "changes", "accept": "done", "drop": "dropped"}[body["choice"]]
        elif m.group(5):
            q = next(x for x in r["questions"] if x["id"] == m.group(5))
            q.update(state="answered", answer=body["text"], answeredBy="owner")
        return 200, r
    m = re.fullmatch(r"/api/sessions/([^/]+)/subagents", path)
    if m and method == "GET" and (m.group(1) in CFG.get("run_children", {}) or any(r["runSession"] == m.group(1) for r in RUNS.values())):
        return 200, {"children": CFG.get("run_children", {}).get(m.group(1), [])}
    return None


def ticket(key, title, state="todo", project="/home/me/code/kagent", **extra):
    """A ticket as GET /api/tickets lists it (tickets.md section 2)."""
    t = {"key": key, "project": project, "title": title, "description": "", "state": state, "priority": "none", "labels": [], "parent": None, "links": [], "assignee": None,
         "doneWhen": [], "notes": [], "history": [{"at": "2026-11-28T09:00:00Z", "by": "owner", "what": "created"}], "createdAt": "2026-11-28T09:00:00Z", "updatedAt": "2026-11-28T09:00:00Z"}
    t.update(extra)
    return t


def tickets_route(method, path, body):
    """The tickets routes of the mock: None when the path is not one, else (status, body). Every change is in CALLS."""
    err = lambda status, message: (status, {"error": {"code": "invalid_input" if status == 400 else "not_found", "message": message}})
    if path == "/api/tickets" and method == "GET":
        return 200, TICKETS
    if path == "/api/tickets" and method == "POST":
        CALLS.append(f"POST {path} {json.dumps(body)}")
        title = (body.get("title") or "").strip()
        if not title or len(title) > 200:
            return err(400, f"title: 1 to 200 characters (it has {len(title)})")
        project = body.get("project")
        info = next((p for p in TICKETS["projects"] if p["path"] == project), None)
        if project and not info and project not in DIRS and not any(x["path"] == project for x in PROJECTS):
            return err(400, f"not a directory: {project}")
        if not info:
            info = {"id": "p%d" % len(TICKETS["projects"]), "path": project, "name": project.rsplit("/", 1)[-1] if project else "Inbox", "prefix": "IN" if not project else "NW", "count": 0}
            TICKETS["projects"].append(info)
        n = max([int(t["key"].rsplit("-", 1)[1]) for t in TICKETS["tickets"] if t["key"].startswith(info["prefix"] + "-")] + [0]) + 1
        info["count"] += 1
        t = ticket(f"{info['prefix']}-{n}", title, body.get("state", "todo"), project, **{k: v for k, v in body.items() if k in ("description", "priority", "labels", "doneWhen")})
        TICKETS["tickets"].append(t)
        return 201, t
    m = re.fullmatch(r"/api/tickets/([A-Za-z0-9]+-\d+)", path)
    if m:
        t = next((x for x in TICKETS["tickets"] if x["key"] == m.group(1)), None)
        if not t:
            return err(404, f"no ticket {m.group(1)}")
        if method == "DELETE":
            CALLS.append(f"DELETE {path}")
            TICKETS["tickets"].remove(t)
            return 204, b""
        if method == "PUT":
            CALLS.append(f"PUT {path} {json.dumps(body)}")
            if "title" in body and not (1 <= len(body["title"].strip()) <= 200):
                return err(400, f"title: 1 to 200 characters (it has {len(body['title'].strip())})")
            said = []
            for field in ("title", "description", "state", "priority", "labels", "parent", "links", "assignee", "doneWhen"):
                if field in body and body[field] != t[field]:
                    t[field] = body[field]
                    said.append(f"{field}: {body[field]}" if field in ("state", "priority") else field)
            if body.get("note"):
                t["notes"].append({"at": "2026-11-28T10:00:00Z", "by": "owner", "text": body["note"]})
                said.append("note added")
            t["history"] += [{"at": "2026-11-28T10:00:00Z", "by": "owner", "what": w} for w in said]
            return 200, t
    return None


def diff_route(method, path, body):
    """The diff and change routes of the mock (diff.md): None when the path is not one, else (status, body). Calls are in CALLS.
    `reset(changes=[...], change_diffs={id: {...}}, repo_diff={...}, diff_error="message")` fills them."""
    err = lambda status, code, message: (status, {"error": {"code": code, "message": message}})
    if path == "/api/changes" and method == "GET":
        return 200, CFG.get("changes", [])
    m = re.fullmatch(r"/api/changes/([^/]+)/diff", path)
    if m and method == "GET":
        CALLS.append(f"GET {path}")
        d = CFG.get("change_diffs", {}).get(m.group(1))
        return (200, d) if d else err(409, "conflict", "this change has no stored copy of the file: only an edit_file or write_file change keeps one")
    m = re.fullmatch(r"/api/changes/([^/]+)/undo", path)
    if m and method == "POST":
        CALLS.append(f"POST {path}")
        return 200, {"status": "ok", "output": "restored", "change": {}}
    if path == "/api/diff" and method == "POST":
        CALLS.append(f"POST {path} {json.dumps(body)}")
        if CFG.get("diff_error"):
            return err(400, "invalid_input", CFG["diff_error"])
        return 200, {"project": body["project"], "from": body["from"], "to": body.get("to"), "diff": CFG["repo_diff"]}
    if path == "/api/ui/tools/open_in_editor/call" and method == "POST":
        CALLS.append(f"POST {path} {json.dumps(body)}")
        return 200, {"status": "ok", "output": "opened", "durationMs": 1}
    return None


def set_run(id, fields=None, tasks=None):
    """Change a run of the mock while the page runs: its own fields and fields of some tasks ({task id: {...}})."""
    urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{PORT}/__run", data=json.dumps({"id": id, "fields": fields or {}, "tasks": tasks or {}}).encode(), method="POST", headers={"Content-Type": "application/json"})).read()


def set_detail(child, **fields):
    """Change the detail the mock answers for a worker while the page runs (steering.md)."""
    urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{PORT}/__detail", data=json.dumps({"child": child, "fields": fields}).encode(), method="POST", headers={"Content-Type": "application/json"})).read()


def folder_refusal(path):
    """The 400 message of the daemon for a folder it would refuse (sidebar.md section 3)."""
    if not (path.startswith("/") or path.startswith("~/")):
        return "path must be absolute, or start with ~/"
    if path not in DIRS:
        return f"not a directory: {path}"
    return None


# When the owner's own daemon holds port 7878 the tests must not start (or stop) one: the routes that are not mocked
# are answered by a small in-memory stand-in instead (state and settings only; everything else is 404).
STUB = {"on": False, "settings": json.loads('{"schemaVersion":1,"chatModel":"","fastModel":"","knownToolGroups":[],"fallbackModel":"","liveModel":"","liveVoice":"Kore","permissionMode":"ask","rules":[],"toolOverrides":{},"toolGroups":["system","kernel","services","files","shell","hardware","skills","change","desktop","security","trace","tasks","agent","memory","web"],"mcpToolGroups":[],"privilege":"sudo","maxIterations":40,"approvalTimeoutSec":600,"options":{"reasoningLevel":null,"maxOutputTokens":null,"temperature":null},"persona":{"name":"KAgent","address":"","language":"auto"},"startup":{"autostart":true,"greeting":{"enabled":true,"text":""}}}')}


def merge(into, patch):
    for k, v in patch.items():
        if isinstance(v, dict) and isinstance(into.get(k), dict):
            merge(into[k], v)
        else:
            into[k] = v


def stub(method, path, body):
    j = lambda status, v: (status, json.dumps(v).encode(), "application/json")
    if path == "/api/state" and method == "GET":
        return j(200, {"version": "stub", "settings": STUB["settings"], "pendingApprovals": [], "running": [], "chatModelValid": True, "privHelper": "absent", "alertsOpen": 0, "voice": {"thinkMedianMs": None}})
    if path == "/api/settings" and method == "PUT":
        merge(STUB["settings"], body or {})
        return j(200, STUB["settings"])
    if path == "/api/settings" and method == "GET":
        return j(200, STUB["settings"])
    if path == "/api/skills" and method == "GET":
        return j(200, {"skills": []})
    if path == "/api/sessions" and method == "GET":
        return j(200, {"sessions": []})
    if path == "/api/models" and method == "GET":
        return j(200, {"chat": [], "fast": [], "vision": [], "live": []})
    if path == "/api/providers/templates":
        return j(200, {"templates": []})
    if path.startswith("/api/alerts") and method == "GET":
        return j(200, [])
    if path == "/api/tasks" and method == "GET":
        return j(200, {"tasks": []})
    if path == "/api/tasks/templates" and method == "GET":
        return j(200, {"templates": []})
    if path == "/api/ui/report":
        return j(200, {"ok": True})
    # What views and settings pages read when they open: the stand-in has nothing, which is an empty answer, not a 404 (the browser logs a 404 as a console error).
    if method == "GET" and path == "/api/agents":
        return j(200, {"agents": [], "invalid": []})
    if method == "GET" and path == "/api/tools":
        return j(200, {"tools": []})
    if method == "GET" and path == "/api/usage":
        return j(200, {"providers": []})
    if method == "GET" and path == "/api/screen-watch":
        return j(200, None)
    if method == "GET" and path == "/api/sentinel":
        return j(200, {"honeytokens": [], "baseline": False})
    if method == "GET" and path == "/api/doctor":
        return j(200, {"score": 100, "checks": []})
    if method == "GET" and path == "/api/net":
        return j(200, {"configured": False, "config": {"host": "", "user": "kagent-ro", "fingerprint": "", "flowPort": 2055, "honeypotIp": "", "trapUser": "", "https": True}, "router": {"ok": False}})
    if method == "GET" and path == "/api/system":
        return j(200, {"hostname": "stub", "distro": "Linux", "kernel": "0", "arch": "x86_64", "user": "me", "uptimeHours": 1, "loadAvg": ["0", "0", "0"], "cpu": {"model": "stub", "logicalCores": 1},
                       "memoryMiB": {"total": 1, "available": 1, "swapTotal": 0, "swapFree": 0}, "temperatures": [], "battery": None})
    if method == "GET" and path == "/api/local-models":
        return j(200, {"ollama": {"reachable": False, "version": None, "baseUrl": "http://127.0.0.1:11434"}, "gpu": None, "ramTotalMb": 0, "installed": [], "recommended": [], "pulls": []})
    return j(404, {"error": {"code": "not_found", "message": f"{path} (stub)"}})


def port_busy(port=7878):
    import socket
    with socket.socket() as c:
        c.settimeout(0.5)
        return c.connect_ex(("127.0.0.1", port)) == 0


def daemon(method, path, body=None):
    if STUB["on"]:
        return stub(method, path, body)
    req = urllib.request.Request(DAEMON + path, method=method, data=json.dumps(body).encode() if body is not None else None,
                                 headers={"Authorization": f"Bearer {TOKEN['v']}", "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=10) as r:
            return r.status, r.read(), r.headers.get("Content-Type", "application/json")
    except urllib.error.HTTPError as e:
        return e.code, e.read(), e.headers.get("Content-Type", "application/json")


def png_size(data):
    """(width, height) of a PNG, from its IHDR."""
    import struct
    return struct.unpack(">II", data[16:24]) if data[:8] == b"\x89PNG\r\n\x1a\n" else (1, 1)


def image_upload(sid, ctype, raw):
    """The answer of `POST /api/sessions/:id/images` for a multipart body with the field `file`: the ImageRef, as the daemon makes it."""
    boundary = re.search(r"boundary=(.+)", ctype).group(1).strip('"').encode()
    for part in raw.split(b"--" + boundary):
        head, _, data = part.partition(b"\r\n\r\n")
        if b'name="file"' in head:
            data = data.rsplit(b"\r\n", 1)[0]
            w, h = png_size(data)
            iid = f"im{len(UPLOADS)}"
            UPLOADS.append([sid, len(data)])
            IMAGES.setdefault(sid, {})[iid] = (data, w, h)
            return 200, json.dumps({"id": iid, "mime": "image/png", "width": w, "height": h, "bytes": len(data)}).encode(), "application/json"
    return 400, json.dumps({"error": {"code": "invalid_input", "message": "no file"}}).encode(), "application/json"


def uploads():
    return json.loads(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/__uploads").read())["uploads"]


class Quiet(ThreadingHTTPServer):
    def handle_error(self, request, client_address):
        pass  # a reset connection of a page that went away


class Mock(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def send(self, status, body, ctype="application/json"):
        body = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass  # the page navigated away

    def handle_any(self, method):
        path = self.path.split("?")[0]
        n = int(self.headers.get("Content-Length", 0) or 0)
        raw = self.rfile.read(n) if n else b""
        ctype = self.headers.get("Content-Type", "")
        body = json.loads(raw) if raw and not ctype.startswith("multipart/") else None
        if path == "/__cfg":
            reset(**(body or {}))
            return self.send(200, {"ok": True})
        if path == "/__calls":
            return self.send(200, {"calls": CALLS})
        if path == "/__output":
            # A task changes while the page runs: its output and/or its status (S43).
            for ts in TASKS.values():
                for t in ts:
                    if t["id"] == body["id"]:
                        t.update({k: v for k, v in body.items() if k in ("status", "exitCode", "seconds")})
            if "text" in body:
                OUTPUT[body["id"]] = body["text"]
            return self.send(200, {"ok": True})
        if path == "/__detail":
            CFG.setdefault("details", {}).setdefault(body["child"], {}).update(body["fields"])
            return self.send(200, {"ok": True})
        if path == "/__usage":
            # The limits change while the page runs (usage.md): the view of GET /api/usage the mock answers.
            CFG["usage"] = body
            return self.send(200, {"ok": True})
        if path == "/api/usage" and method == "GET":
            return self.send(200, CFG.get("usage") or {"providers": []})
        for extra in ROUTES:
            out = extra(method, path, body)
            if out:
                return self.send(out[0], out[1], "application/json")
        if path == "/__settingputs":
            return self.send(200, {"puts": SETTING_PUTS})
        if path == "/api/settings" and method == "PUT" and any(k in (body or {}) for k in ("chatModel", "fallbackModel", "fastModel", "visionModel", "usage")):
            SETTING_PUTS.append(json.dumps(body))
        if path == "/__uploads":
            return self.send(200, {"uploads": UPLOADS})
        if path == "/__reads":
            return self.send(200, {"reads": READS})
        if path == "/__opened":
            return self.send(200, {"opened": OPENED})
        if path == "/__watch":
            # The watch changes while the page runs (a look was made, the screen got locked): merged into the one the mock reports.
            WATCH["watch"] = dict(WATCH["watch"] or {}, **body) if body else None
            return self.send(200, {"ok": True})
        if path == "/api/screen-watch" and method == "GET":
            return self.send(200, WATCH.get("watch"))
        if path == "/api/screen-watch" and method == "DELETE":
            CALLS.append("DELETE /api/screen-watch")
            ended, WATCH["watch"] = WATCH.get("watch"), None
            return self.send(200, dict(ended, state="stopped")) if ended else self.send(404, {"error": {"code": "not_found", "message": "no screen watch is running"}})
        if path == "/api/screen-watch/model" and method == "GET":
            return self.send(200, WATCH["model"])
        if path == "/api/settings" and method == "PUT" and "screenWatch" in (body or {}):
            CALLS.append(f"PUT /api/settings {json.dumps(body)}")
        if path == "/__local":
            # The mock's view changes while the page runs (a download makes progress): merged into the view.
            LOCAL.update(body)
            return self.send(200, {"ok": True})
        if LOCAL and path.startswith("/api/local-models"):
            out = local_models(method, path, body)
            if out:
                return self.send(out[0], out[1], "application/json")
        if LOCAL and path == "/api/settings" and method == "PUT":
            local_settings(body)
        if path.startswith(("/api/changes", "/api/diff", "/api/ui/tools/open_in_editor")):
            out = diff_route(method, path, body)
            if out:
                return self.send(out[0], out[1], "application/json")
        if path.startswith(("/api/workflows", "/api/runs")) or (RUNS and path.endswith("/subagents")):
            out = runs_route(method, path, body)
            if out:
                return self.send(out[0], out[1], "application/json")
        if path == "/__run":
            # A run changes while the page runs (a task moved on): merged into it, then the page is told `runUpdated`.
            r = RUNS[body["id"]]
            for k, v in body.get("fields", {}).items():
                r[k] = v
            for tid, fields in body.get("tasks", {}).items():
                next(t for t in r["tasks"] if t["id"] == tid).update(fields)
            return self.send(200, {"ok": True})
        if path.startswith("/api/tickets"):
            out = tickets_route(method, path, body)
            if out:
                return self.send(out[0], out[1], "application/json")
        if path == "/api/state":
            status, data, _ = daemon("GET", "/api/state")
            state = json.loads(data)
            state["chatModelValid"] = CFG["valid"]
            state["settings"]["chatModel"] = CFG["default"]
            state["settings"]["liveModel"] = "gemini::live"
            state["settings"]["visionModel"] = CFG.get("vision", "")
            state["running"] = CFG.get("running", [])
            state["pendingApprovals"] = APPROVALS
            state["alertsOpen"] = CFG.get("alerts", 0)
            return self.send(200, state)
        mt = re.fullmatch(r"/api/sessions/([^/]+)/(background|todo|plan)", path)
        if mt and mt.group(1) in SESSIONS:
            m = SESSIONS[mt.group(1)]
            if mt.group(2) == "background" and method == "GET":
                return self.send(200, {"tasks": TASKS.get(m["id"], [])})
            if mt.group(2) == "todo" and method == "PUT":
                CALLS.append(f"PUT {path} {json.dumps(body)}")
                m["todo"] = body["items"]
                return self.send(200, {"items": m["todo"]})
            if mt.group(2) == "plan" and method == "POST":
                CALLS.append(f"POST {path} {json.dumps(body)}")
                m["planning"] = bool(body["active"])
                return self.send(200, {"planning": m["planning"]})
        mb = re.fullmatch(r"/api/background/([^/]+)/(output|stop)", path)
        if mb:
            found = next((t for ts in TASKS.values() for t in ts if t["id"] == mb.group(1)), None)
            if not found:
                return self.send(404, {"error": {"code": "not_found", "message": f"no background task '{mb.group(1)}'"}})
            if mb.group(2) == "stop" and method == "POST":
                CALLS.append(f"POST {path}")
                if found["status"] != "running":
                    return self.send(409, {"error": {"code": "conflict", "message": f"the task already ended ({found['status']})"}})
                found["status"] = "killed"
                return self.send(200, found)
            if mb.group(2) == "output" and method == "GET":
                tail = int(re.search(r"tail=(\d+)", self.path).group(1)) if "tail=" in self.path else 4000
                READS.append([found["id"], tail])
                return self.send(200, {"task": found, "output": OUTPUT.get(found["id"], "")[-tail:]})
        mi = re.fullmatch(r"/api/sessions/([^/]+)/images(?:/([^/]+))?", path)
        if mi and mi.group(1) in SESSIONS:
            if method == "POST" and not mi.group(2):
                return self.send(*image_upload(mi.group(1), ctype, raw))
            if method == "GET" and mi.group(2) in IMAGES.get(mi.group(1), {}):
                return self.send(200, IMAGES[mi.group(1)][mi.group(2)][0], "image/png")
        mp = re.fullmatch(r"/api/sessions/([^/]+)/messages", path)
        if mp and method == "POST" and mp.group(1) in SESSIONS:
            CALLS.append(f"POST {path} {json.dumps(body)}")
            if CFG.get("refuse"):
                return self.send(409, {"error": {"code": "refused", "message": CFG["refuse"]}})
            return self.send(200, {"status": "started", "turnId": "t1"})
        mt = re.fullmatch(r"/api/agents/([^/]+)/message", path)
        if mt and method == "POST":
            # The owner's message to a worker (steering.md section 3); `reset(tell_error=(status, message))` makes it refuse.
            CALLS.append(f"POST {path} {json.dumps(body)}")
            if CFG.get("tell_error"):
                status, message = CFG["tell_error"]
                return self.send(status, {"error": {"code": "conflict", "message": message}})
            return self.send(202, {"id": f"st_{len([c for c in CALLS if '/message' in c])}", "state": "queued"})
        md = re.fullmatch(r"/api/sessions/([^/]+)/subagents/([^/]+)", path)
        if md and method == "GET":
            # What the worker pane draws (steering.md section 2): `reset(details={child id: {...}})`; the calls are in CALLS.
            CALLS.append(f"GET {path}")
            d = (CFG.get("details") or {}).get(md.group(2))
            m = SESSIONS.get(md.group(2))
            if not d and m and m.get("origin") == "subagent" and m.get("parent") == md.group(1):
                # A worker the test did not describe: its job and nothing else.
                d = {"child": {"id": m["id"], "title": m["title"], "profile": "general", "status": "running"}, "job": m["title"], "todo": [], "transcript": []}
            return self.send(200, d) if d else self.send(404, {"error": {"code": "not_found", "message": f"no subagent '{md.group(2)}' belongs to session {md.group(1)}"}})
        ms = re.fullmatch(r"/api/sessions/([^/]+)/subagents/([^/]+)/stop", path)
        if ms and method == "POST":
            CALLS.append(f"POST {path}")
            return self.send(200, {"status": "cancelled"})
        if path == "/api/models":
            return self.send(200, {"chat": CFG["models"], "fast": CFG["models"], "vision": [], "live": []})
        if path == "/api/providers":
            def row(m, enabled=True):
                return {"id": m["id"], "name": m["id"], "enabled": enabled, "roles": ["chat"], "smart": True, "config": {}, "inherited": m["config"], "effective": m["config"]}
            gone = dict(model("anthropic", "Anthropic", "gone-model"))
            providers = []
            for pid, pname in (("anthropic", "Anthropic"), ("gemini", "Google Gemini")):
                base = "http://127.0.0.1:11434/v1" if pid in CFG.get("localProviders", ()) else CFG.get("baseUrls", {}).get(pid, "https://x.example/v1")
                rows = [row(m) for m in MODELS if m["providerId"] == pid] + ([row(gone, False)] if pid == "anthropic" else [])
                providers.append({"id": pid, "name": pname, "templateId": pid, "enabled": True, "api": {"type": "openai-chat-completions", "baseUrl": base, "headers": {}},
                                  "apiKeyMasked": "sk-...abcd", "hasKey": True, "models": rows, "modelsFetchedAt": None, "lastError": "",
                                  "local": pid in CFG.get("localProviders", ()),
                                  "tightestLimit": next((u.get("tightest") for u in (CFG.get("usage") or {}).get("providers", []) if u["id"] == pid), None)})
            return self.send(200, {"providers": providers})
        if path == "/api/projects" and method == "GET":
            return self.send(200, {"roots": ["/home/me/code"], "projects": PROJECTS})
        if path == "/api/sessions" and method == "GET":
            return self.send(200, {"sessions": list(SESSIONS.values())})
        if path == "/api/sessions" and method == "POST":
            CALLS.append(f"POST {path} {json.dumps(body)}")
            extra = {k: v for k, v in (body or {}).items() if k in ("model", "reasoningLevel")}
            if (body or {}).get("project"):
                refusal = folder_refusal(body["project"])
                if refusal:
                    return self.send(400, {"error": {"code": "invalid_input", "message": refusal}})
                extra["project"] = body["project"]
            nxt = max(int(k[1:]) for k in SESSIONS if k[1:].isdigit()) + 1 if SESSIONS else 1
            m = meta(f"s{nxt}", {**extra, "title": (body or {}).get("title", "")})
            m["updatedAt"] = m["createdAt"] = CFG.get("now", m["createdAt"])
            SESSIONS[m["id"]] = m
            return self.send(201, m)
        mm = re.fullmatch(r"/api/sessions/([^/]+)(/model|/project|/pin|/images-notice|/title|/archive)?", path)
        if mm and mm.group(1) in SESSIONS:
            m = SESSIONS[mm.group(1)]
            if method == "DELETE" and not mm.group(2):
                CALLS.append(f"DELETE {path}")
                # The daemon refuses a running conversation (shell.md section 5); `reset(deny_delete=[ids])` makes the mock refuse the others too.
                if m["id"] in CFG.get("running", []) or m["id"] in CFG.get("deny_delete", []):
                    return self.send(409, {"error": {"code": "conflict", "message": "the conversation is running: stop it first"}})
                del SESSIONS[m["id"]]
                return self.send(204, b"", "application/json")
            if mm.group(2) == "/title" and method == "PUT":
                CALLS.append(f"PUT {path} {json.dumps(body)}")
                title = " ".join(str((body or {}).get("title", "")).split())
                if not 1 <= len(title) <= 120:
                    return self.send(400, {"error": {"code": "invalid_input", "message": "the title is empty" if not title else "the title is too long"}})
                m["title"], m["renamed"] = title, True
                return self.send(200, m)
            if mm.group(2) == "/archive" and method == "PUT":
                CALLS.append(f"PUT {path} {json.dumps(body)}")
                if body.get("archived") and m["id"] in CFG.get("running", []):
                    return self.send(409, {"error": {"code": "conflict", "message": "the conversation is running: stop it first"}})
                m["archived"] = bool(body["archived"])
                return self.send(200, m)
            if mm.group(2) == "/project" and method == "PUT":
                CALLS.append(f"PUT {path} {json.dumps(body)}")
                value = (body or {}).get("path")
                if value:
                    refusal = folder_refusal(value)
                    if refusal:
                        return self.send(400, {"error": {"code": "invalid_input", "message": refusal}})
                m["project"] = value or None
                return self.send(200, m)
            if mm.group(2) == "/images-notice" and method == "PUT":
                CALLS.append(f"PUT {path}")
                m["imagesNoticeShown"] = True
                return self.send(200, m)
            if mm.group(2) == "/pin" and method == "PUT":
                CALLS.append(f"PUT {path} {json.dumps(body)}")
                m["pinned"] = bool(body["pinned"])
                return self.send(200, m)
            if mm.group(2) == "/model" and method == "PUT":
                CALLS.append(f"PUT {path} {json.dumps(body)}")
                for key in ("model", "reasoningLevel"):
                    if key in body:
                        m[key] = body[key] or None
                if "model" in body and "reasoningLevel" not in body:
                    levels = next((x["config"]["reasoningLevels"] for x in MODELS if x["ref"] == m["model"]), [])
                    if m["reasoningLevel"] not in levels:
                        m["reasoningLevel"] = None
                return self.send(200, m)
            if method == "GET":
                OPENED.append(m["id"])
                return self.send(200, {"meta": m, "messages": MESSAGES.get(m["id"], [])})
        if path.startswith("/api/"):
            status, data, ctype = daemon(method, path, body)
            return self.send(status, data, ctype)
        if path in ("/ws", "/live"):
            return self.send(404, b"no")
        file = DIST + path
        if not os.path.isfile(file):
            file = DIST + "/index.html"
        types = {".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".woff2": "font/woff2", ".html": "text/html"}
        return self.send(200, open(file, "rb").read(), types.get(os.path.splitext(file)[1], "application/octet-stream"))

    do_GET = lambda self: self.handle_any("GET")
    do_POST = lambda self: self.handle_any("POST")
    do_PUT = lambda self: self.handle_any("PUT")
    do_DELETE = lambda self: self.handle_any("DELETE")


class Page:
    def __init__(self, ws):
        self.ws, self.n, self.waiting, self.errors = ws, 0, {}, []

    async def reader(self):
        async for raw in self.ws:
            m = json.loads(raw)
            if "id" in m and m["id"] in self.waiting:
                self.waiting.pop(m["id"]).set_result(m)
            elif m.get("method") == "Runtime.exceptionThrown":
                self.errors.append(m["params"]["exceptionDetails"].get("text", "") + str(m["params"]["exceptionDetails"].get("exception", {}).get("description", "")))
            elif m.get("method") == "Log.entryAdded" and m["params"]["entry"]["level"] == "error":
                e = m["params"]["entry"]
                if "WebSocket" not in e["text"] and "/ws" not in e.get("url", "") and "/live" not in e.get("url", ""):
                    self.errors.append(e["text"] + " " + e.get("url", ""))

    async def send(self, method, **params):
        self.n += 1
        fut = asyncio.get_event_loop().create_future()
        self.waiting[self.n] = fut
        await self.ws.send(json.dumps({"id": self.n, "method": method, "params": params}))
        m = await asyncio.wait_for(fut, 20)
        if "error" in m:
            raise RuntimeError(f"{method}: {m['error']}")
        return m.get("result", {})

    async def js(self, expr):
        r = await self.send("Runtime.evaluate", expression=expr, returnByValue=True, awaitPromise=True)
        if "exceptionDetails" in r:
            raise RuntimeError(str(r["exceptionDetails"]))
        return r["result"].get("value")

    async def until(self, expr, what, seconds=10):
        end = time.time() + seconds
        while time.time() < end:
            if await self.js(expr):
                return
            await asyncio.sleep(0.1)
        body = await self.js("document.body.innerText.slice(0, 500) + ' | ' + location.href")
        await self.shot("timeout")
        raise RuntimeError(f"timeout: {what}: {body!r}; errors: {self.errors[:4]}")

    async def shot(self, name):
        r = await self.send("Page.captureScreenshot", format="png")
        open(f"{SHOTS}/{name}.png", "wb").write(base64.b64decode(r["data"]))

    async def key(self, key, code, vk, mods=0, text=None):
        base = dict(key=key, code=code, windowsVirtualKeyCode=vk, nativeVirtualKeyCode=vk, modifiers=mods)
        await self.send("Input.dispatchKeyEvent", type="keyDown", **base, **({"text": text} if text else {}))
        await self.send("Input.dispatchKeyEvent", type="keyUp", **base)
        await asyncio.sleep(0.12)

    async def click(self, sel):
        await self.js(f"document.querySelector({json.dumps(sel)}).click()")
        await asyncio.sleep(0.15)

    async def ctrl_m(self):
        await self.key("m", "KeyM", 77, 2)

    async def mouse(self, kind, x, y):
        await self.send("Input.dispatchMouseEvent", type=kind, x=x, y=y, button="left", clickCount=1)

    async def center(self, sel):
        r = await self.js(f"(() => {{ const r = document.querySelector({json.dumps(sel)}).getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; }})()")
        return r


async def dd_open(page, trigger_sel):
    """Open the list of a Dropdown (the themed select of the settings screen) with a real click on its trigger."""
    # The page may still be moving when this is called (a save just re-rendered the card, a toast came in): a click at
    # coordinates read a moment ago then misses the trigger. So: wait until the trigger stands still, click, and when
    # the list did not open, read the place again and click again (the screen-watch suite failed every other run).
    where = f"(() => {{ const r = document.querySelector({json.dumps(trigger_sel)}).getBoundingClientRect(); return Math.round(r.x) + ',' + Math.round(r.y); }})()"
    for attempt in range(4):
        await page.js(f"document.querySelector({json.dumps(trigger_sel)}).scrollIntoView({{block: 'center'}})")
        before = await page.js(where)
        await asyncio.sleep(0.12)
        if await page.js(where) != before:
            continue
        x, y = await page.center(trigger_sel)
        await page.send("Input.dispatchMouseEvent", type="mousePressed", x=x, y=y, button="left", clickCount=1)
        await page.send("Input.dispatchMouseEvent", type="mouseReleased", x=x, y=y, button="left", clickCount=1)
        try:
            await page.until("!!document.querySelector('.dd-list')", f"the list of {trigger_sel}", seconds=2)
            return
        except RuntimeError:
            if attempt == 3:
                raise
    await page.until("!!document.querySelector('.dd-list')", f"the list of {trigger_sel}")


async def dd_set(page, trigger_sel, value):
    """Choose the option whose value is `value` in the Dropdown of `trigger_sel`."""
    await dd_open(page, trigger_sel)
    await page.js(f"[...document.querySelectorAll('.dd-list [role=option]')].find(o => o.dataset.value === {json.dumps(value)}).click()")
    await page.until("!document.querySelector('.dd-list')", "the list closed by the pick")


async def dd_values(page, trigger_sel):
    """The values of the options of a Dropdown (its list is opened and closed)."""
    await dd_open(page, trigger_sel)
    values = await page.js("[...document.querySelectorAll('.dd-list [role=option]')].map(o => o.dataset.value)")
    await page.key("Escape", "Escape", 27)
    await page.until("!document.querySelector('.dd-list')", "the list closed")
    return values


def set_task(**body):
    """Change a task of the mock while the page runs: `id`, and `text` (its output), `status`, `exitCode`."""
    urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{PORT}/__output", data=json.dumps(body).encode(), method="POST", headers={"Content-Type": "application/json"})).read()


def reads():
    return json.loads(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/__reads").read())["reads"]


def local(**body):
    """Change the local models view of the mock while the page runs (S47)."""
    urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{PORT}/__local", data=json.dumps(body).encode(), method="POST", headers={"Content-Type": "application/json"})).read()


def watch(**body):
    """Change the screen watch of the mock while the page runs (S48); no arguments: it ended."""
    urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{PORT}/__watch", data=json.dumps(body).encode(), method="POST", headers={"Content-Type": "application/json"})).read()


def usage_set(view):
    """Change the view of GET /api/usage the mock answers, while the page runs."""
    urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{PORT}/__usage", data=json.dumps(view).encode(), method="POST", headers={"Content-Type": "application/json"})).read()


def setting_puts():
    return json.loads(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/__settingputs").read())["puts"]


def calls():
    return json.loads(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/__calls").read())["calls"]


def opened():
    return json.loads(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/__opened").read())["opened"]


def configure(**kw):
    urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{PORT}/__cfg", data=json.dumps(kw).encode(), method="POST", headers={"Content-Type": "application/json"})).read()




class Env:
    """A real kagentd on a temporary KAGENT_HOME behind the mock, and a headless Chrome page (S42: the start of test-picker-web.py as one helper)."""

    def __init__(self, prefix, shots):
        global SHOTS
        SHOTS = shots
        os.makedirs(SHOTS, exist_ok=True)
        for f in os.listdir(SHOTS):
            os.remove(f"{SHOTS}/{f}")
        self.prefix, self.home, self.proc, self.chrome, self.page, self.reader = prefix, None, None, None, None, None

    async def __aenter__(self):
        reset()
        self.home = tempfile.mkdtemp(prefix=f"{self.prefix}-web-")
        env = dict(os.environ, KAGENT_HOME=self.home, KAGENT_NO_TRAY="1", KAGENT_NO_NOTIFY="1")
        STUB["on"] = port_busy()
        if STUB["on"]:
            print("NOTE: port 7878 is in use (the owner's daemon): the daemon routes the mock does not answer come from a stand-in", file=sys.stderr)
        else:
            self.proc = subprocess.Popen([f"{ROOT}/target/debug/kagentd", "serve"], env=env, stdout=subprocess.DEVNULL, stderr=open(f"{self.home}/daemon.log", "wb"))
        for _ in range(0 if STUB["on"] else 100):
            if os.path.exists(f"{self.home}/config/token"):
                TOKEN["v"] = open(f"{self.home}/config/token").read().strip()
                try:
                    if daemon("GET", "/api/state")[0] == 200:
                        break
                except Exception:
                    pass
            time.sleep(0.2)
        server = Quiet(("127.0.0.1", PORT), Mock)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        profile = self.profile = tempfile.mkdtemp(prefix=f"{self.prefix}-chrome-")
        self.chrome = subprocess.Popen(["google-chrome", "--headless=new", f"--remote-debugging-port={CDP}", f"--user-data-dir={profile}", "--use-fake-device-for-media-stream",
                                        "--use-fake-ui-for-media-stream", "--no-first-run", "--disable-gpu", "--hide-scrollbars", "--window-size=1200,820", "about:blank"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        tabs = []
        for _ in range(100):
            try:
                tabs = json.loads(urllib.request.urlopen(f"http://127.0.0.1:{CDP}/json").read())
                if any(t["type"] == "page" for t in tabs):
                    break
            except Exception:
                time.sleep(0.2)
        tab = next(t for t in tabs if t["type"] == "page")
        self.ws = await websockets.connect(tab["webSocketDebuggerUrl"], max_size=2**26)
        self.page = Page(self.ws)
        self.reader = asyncio.create_task(self.page.reader())
        for domain in ("Page", "Runtime", "Log"):
            await self.page.send(f"{domain}.enable")
        return self

    async def open(self, theme, query="", ready=None, init="", hash="", path="", **cfg):
        """Load the window in `theme` (kagent.theme) with the mock reset to `cfg`; `init` runs before the page's own scripts.
        The stored state of the window (localStorage) is cleared on each call except the theme; `/ws` is the fake of FAKE_WS."""
        reset(**cfg)
        page = self.page
        await page.send("Page.navigate", url="about:blank")
        if getattr(self, "_init", None):
            await page.send("Page.removeScriptToEvaluateOnNewDocument", identifier=self._init)
        self._n = getattr(self, "_n", 0) + 1
        source = (f"try {{ if (sessionStorage.getItem('open') !== '{self._n}') {{ sessionStorage.setItem('open', '{self._n}'); localStorage.clear(); localStorage.setItem('kagent.theme','{theme}'); }} }} catch (e) {{}} "
                  f"{FAKE_WS} {init}")
        self._init = (await page.send("Page.addScriptToEvaluateOnNewDocument", source=source))["identifier"]
        await page.send("Page.navigate", url=f"http://127.0.0.1:{PORT}/{path}{query}#token=abc123{hash}")
        ready = ready or (".widget-status" if "overlay" in query or "compact" in query else "#sidebar-toggle")
        await page.until(f"!!document.querySelector('{ready}') && !document.body.innerText.includes('No token')", f"the page ({theme}, {query})")
        await asyncio.sleep(0.5)

    async def size(self, w, h):
        await self.page.send("Emulation.setDeviceMetricsOverride", width=w, height=h, deviceScaleFactor=1, mobile=False)

    async def __aexit__(self, *exc):
        if self.reader:
            self.reader.cancel()
        await self.ws.close()
        for p in (self.chrome, self.proc):
            if p:
                p.terminate()
        # The browser's profile is about 170 MB, and /tmp is memory on many machines: every run of every suite left
        # one behind until /tmp was full (561 of them, 29 GB). Wait for the browser to go, then remove it.
        if self.chrome:
            try:
                self.chrome.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.chrome.kill()
                self.chrome.wait(timeout=5)
        remove_profile(getattr(self, "profile", ""))
        shutil.rmtree(self.home, ignore_errors=True)


def remove_profile(path):
    """Remove a test browser's profile. Its helper processes may still write for a moment after the browser ended and
    bring the folder back, so: remove, look again, a few times."""
    if not path:
        return
    for _ in range(10):
        shutil.rmtree(path, ignore_errors=True)
        time.sleep(0.3)
        if not os.path.exists(path):
            return


def report(results):
    """Print every check and the summary line; the exit code of the script."""
    bad = [k for k, v in results.items() if not v]
    for k, v in results.items():
        print("PASS" if v else "FAIL", k)
    print(f"WEB {'OK' if not bad else 'FAILED'}: {len(results) - len(bad)}/{len(results)} checks")
    return 0 if not bad else 1

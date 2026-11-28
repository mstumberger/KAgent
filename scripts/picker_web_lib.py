"""The mock API, the CDP page driver and the helpers of test-picker-web.py (S41), test-sidebar-web.py (S42) and test-work-web.py (S43)."""
import asyncio, base64, json, os, re, shutil, subprocess, sys, tempfile, threading, time, urllib.request, urllib.error
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import websockets

ROOT = os.getcwd()
DIST = f"{ROOT}/web/dist"
SHOTS = "/tmp/kagent-s41-shots"
PORT, CDP = 18802, 9334
DAEMON = "http://127.0.0.1:7878"
TOKEN = {"v": ""}


def model(pid, pname, mid, levels=(), image=False):
    return {"id": mid, "name": mid, "ref": f"{pid}::{mid}", "providerId": pid, "providerName": pname,
            "config": {"enabled": True, "contextWindow": 200000, "maxOutputTokens": 8000, "supportsToolCall": True, "supportsImage": image,
                       "reasoningLevels": list(levels), "reasoningPatches": {}, "supportsLive": False, "liveProtocol": None, "requestPatch": None, "roles": ["chat"]}}


MODELS = [model("anthropic", "Anthropic", "claude-opus-4-6", ["low", "medium", "high"], True), model("anthropic", "Anthropic", "claude-haiku-4-5"),
          model("gemini", "Google Gemini", "models/gemini-3.8-flash", [], True)]
CFG = {}
CALLS = []
SESSIONS = {}
# The folders of the mock file system and the projects `GET /api/projects` lists (S42; set with `reset`).
DIRS = set()
PROJECTS = []
OPENED = []  # the sessions the window read (`GET /api/sessions/:id`), in order
# The work panel (S43; set with `reset`): the background tasks per session, the output of a task, the messages per session,
# the pending approvals, and the reads of an output (`GET /api/background/:id/output`) as (task, tail).
TASKS = {}
OUTPUT = {}
MESSAGES = {}
APPROVALS = []
READS = []


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
    SESSIONS["s1"] = meta("s1", kw.get("session", {}))
    for sid, extra in (kw.get("sessions") or {}).items():
        SESSIONS[sid] = meta(sid, extra)


def meta(sid, extra=None):
    m = {"id": sid, "title": "Chat " + sid, "createdAt": "2026-11-27T08:00:00Z", "updatedAt": "2026-11-27T08:00:00Z", "messageCount": 0, "origin": "chat",
         "todo": [], "planning": False, "approvedPlan": None, "model": None, "reasoningLevel": None, "project": None, "pinned": False}
    m.update(extra or {})
    return m


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
        body = json.loads(raw) if raw else None
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
        if path == "/__reads":
            return self.send(200, {"reads": READS})
        if path == "/__opened":
            return self.send(200, {"opened": OPENED})
        if path == "/api/state":
            status, data, _ = daemon("GET", "/api/state")
            state = json.loads(data)
            state["chatModelValid"] = CFG["valid"]
            state["settings"]["chatModel"] = CFG["default"]
            state["settings"]["liveModel"] = "gemini::live"
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
                rows = [row(m) for m in MODELS if m["providerId"] == pid] + ([row(gone, False)] if pid == "anthropic" else [])
                providers.append({"id": pid, "name": pname, "templateId": pid, "enabled": True, "api": {"type": "openai-chat-completions", "baseUrl": "https://x.example/v1", "headers": {}},
                                  "apiKeyMasked": "sk-...abcd", "hasKey": True, "models": rows, "modelsFetchedAt": None, "lastError": ""})
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
        mm = re.fullmatch(r"/api/sessions/([^/]+)(/model|/project|/pin)?", path)
        if mm and mm.group(1) in SESSIONS:
            m = SESSIONS[mm.group(1)]
            if method == "DELETE" and not mm.group(2):
                CALLS.append(f"DELETE {path}")
                del SESSIONS[m["id"]]
                return self.send(204, b"", "application/json")
            if mm.group(2) == "/project" and method == "PUT":
                CALLS.append(f"PUT {path} {json.dumps(body)}")
                value = (body or {}).get("path")
                if value:
                    refusal = folder_refusal(value)
                    if refusal:
                        return self.send(400, {"error": {"code": "invalid_input", "message": refusal}})
                m["project"] = value or None
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


def set_task(**body):
    """Change a task of the mock while the page runs: `id`, and `text` (its output), `status`, `exitCode`."""
    urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{PORT}/__output", data=json.dumps(body).encode(), method="POST", headers={"Content-Type": "application/json"})).read()


def reads():
    return json.loads(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/__reads").read())["reads"]


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
        profile = tempfile.mkdtemp(prefix=f"{self.prefix}-chrome-")
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

    async def open(self, theme, query="", ready=None, init="", **cfg):
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
        await page.send("Page.navigate", url=f"http://127.0.0.1:{PORT}/{query}#token=abc123")
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
        shutil.rmtree(self.home, ignore_errors=True)


def report(results):
    """Print every check and the summary line; the exit code of the script."""
    bad = [k for k, v in results.items() if not v]
    for k, v in results.items():
        print("PASS" if v else "FAIL", k)
    print(f"WEB {'OK' if not bad else 'FAILED'}: {len(results) - len(bad)}/{len(results)} checks")
    return 0 if not bad else 1

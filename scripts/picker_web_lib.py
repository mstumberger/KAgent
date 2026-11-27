"""The mock API, the CDP page driver and the helpers of test-picker-web.py (S41)."""
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


def reset(**kw):
    CFG.clear()
    CFG.update(valid=True, default="gemini::models/gemini-3.8-flash", models=MODELS)
    CFG.update(kw)
    CALLS.clear()
    SESSIONS.clear()
    SESSIONS["s1"] = meta("s1", kw.get("session", {}))


def meta(sid, extra=None):
    m = {"id": sid, "title": "Chat " + sid, "createdAt": "2026-11-27T08:00:00Z", "updatedAt": "2026-11-27T08:00:00Z", "messageCount": 0, "origin": "chat",
         "todo": [], "planning": False, "approvedPlan": None, "model": None, "reasoningLevel": None}
    m.update(extra or {})
    return m


def daemon(method, path, body=None):
    req = urllib.request.Request(DAEMON + path, method=method, data=json.dumps(body).encode() if body is not None else None,
                                 headers={"Authorization": f"Bearer {TOKEN['v']}", "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=10) as r:
            return r.status, r.read(), r.headers.get("Content-Type", "application/json")
    except urllib.error.HTTPError as e:
        return e.code, e.read(), e.headers.get("Content-Type", "application/json")


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
        self.wfile.write(body)

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
        if path == "/api/state":
            status, data, _ = daemon("GET", "/api/state")
            state = json.loads(data)
            state["chatModelValid"] = CFG["valid"]
            state["settings"]["chatModel"] = CFG["default"]
            state["settings"]["liveModel"] = "gemini::live"
            return self.send(200, state)
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
        if path == "/api/sessions" and method == "GET":
            return self.send(200, {"sessions": list(SESSIONS.values())})
        if path == "/api/sessions" and method == "POST":
            CALLS.append(f"POST {path} {json.dumps(body)}")
            m = meta(f"s{len(SESSIONS) + 1}", {k: v for k, v in (body or {}).items() if k in ("model", "reasoningLevel")})
            SESSIONS[m["id"]] = m
            return self.send(201, m)
        mm = re.fullmatch(r"/api/sessions/([^/]+)(/model)?", path)
        if mm and mm.group(1) in SESSIONS:
            m = SESSIONS[mm.group(1)]
            if mm.group(2) and method == "PUT":
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
                return self.send(200, {"meta": m, "messages": []})
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

    async def ctrl_m(self):
        await self.key("m", "KeyM", 77, 2)

    async def mouse(self, kind, x, y):
        await self.send("Input.dispatchMouseEvent", type=kind, x=x, y=y, button="left", clickCount=1)

    async def center(self, sel):
        r = await self.js(f"(() => {{ const r = document.querySelector({json.dumps(sel)}).getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; }})()")
        return r


def calls():
    return json.loads(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/__calls").read())["calls"]


def configure(**kw):
    urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{PORT}/__cfg", data=json.dumps(kw).encode(), method="POST", headers={"Content-Type": "application/json"})).read()



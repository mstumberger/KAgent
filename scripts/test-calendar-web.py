#!/usr/bin/env python3
"""S45: the Calendar page in headless Chrome against a mocked API, in every connection state.

A real kagentd (temporary KAGENT_HOME, port 7878) answers everything except the calendar and
telegram routes, which a small proxy mocks: no Google host is ever reached. Screenshots go to
/tmp/kagent-s45-shots/. Run from the repo root."""
import asyncio, base64, json, os, shutil, subprocess, sys, tempfile, threading, time, urllib.request, urllib.error
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import websockets

ROOT = os.getcwd()
DIST = os.environ.get("KAGENT_DIST") or f"{ROOT}/web/dist"
SHOTS = "/tmp/kagent-s45-shots"
PORT, CDP = 18801, 9333
from picker_web_lib import STUB, port_busy, stub  # noqa: E402
DAEMON = "http://127.0.0.1:7878"
OWNER = "owner@example.com"
MODE = {"name": "off", "paired": True, "hasKey": True, "opened": [], "calls": []}
TOKEN = {"v": ""}

CALS = [
    {"id": OWNER, "name": "Personal", "color": "#4fb6c6", "writable": True},
    {"id": "family@group.calendar.google.com", "name": "Family", "color": "#f6bf26", "writable": True},
    {"id": "sl.slovenian#holiday@group.v.calendar.google.com", "name": "Holidays in Slovenia", "color": "#0b8043", "writable": False},
    {"id": "work@example.com", "name": "Work", "color": "#d50000", "writable": True},
]
LAST = {"at": "2026-11-27T07:45:00+01:00", "sent": ["telegram", "spoken"],
        "text": "Good morning, Marko. Today, Friday, 27 November:\n- 10:00 Dentist, at Kardeljeva ploscad 1, Ljubljana\n- 10:30 School play\nDentist and School play overlap from 10:30 to 11:00.\nLeave by 09:20 for Dentist: 30 minutes by car with traffic, plus 10."}


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


def settings():
    return json.loads(daemon("GET", "/api/state")[1])["settings"]


def view():
    name = MODE["name"]
    cfg = settings().get("calendar", {})
    on_ids = cfg.get("calendars", ["primary"])
    connected = name in ("connected", "reconnect")
    state = {"off": "off", "client": "off", "connecting": "connecting", "connected": "connected", "reconnect": "reconnect", "error": "error"}[name]
    cals = [dict(c, on=(c["id"] in on_ids or (c["id"] == OWNER and "primary" in on_ids))) for c in CALS] if connected else []
    return {"state": state, "account": OWNER if connected else None, "calendars": cals,
            "hasClientSecret": name != "off", "hasMapsKey": MODE["hasKey"] and connected,
            "lastReport": LAST if connected else None,
            "error": "Google answered 500: try again in a moment" if name == "error" else None}


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
        path = self.path
        n = int(self.headers.get("Content-Length", 0) or 0)
        raw = self.rfile.read(n) if n else b""
        body = json.loads(raw) if raw else None
        if path.startswith("/__mode"):
            q = dict(p.split("=") for p in path.split("?", 1)[1].split("&"))
            MODE.update(name=q["name"], paired=q.get("paired", "1") == "1", hasKey=q.get("key", "1") == "1", opened=[], calls=[])
            return self.send(200, {"ok": True})
        if path.startswith("/__calls"):
            return self.send(200, {"calls": MODE["calls"]})
        if path.startswith("/api/calendar"):
            MODE["calls"].append(f"{method} {path} {json.dumps(body)}")
            if path == "/api/calendar" and method == "GET":
                return self.send(200, view())
            if path == "/api/calendar/connect":
                if MODE["name"] == "off":
                    return self.send(400, {"error": {"code": "invalid_input", "message": "paste the client id and the client secret first"}})
                return self.send(200, {"url": "https://accounts.google.com/o/oauth2/v2/auth?client_id=mock"})
            if path == "/api/calendar/client":
                daemon("PUT", "/api/settings", {"calendar": {"clientId": body["clientId"]}})
                if MODE["name"] == "off":
                    MODE["name"] = "client"
                return self.send(204, b"")
            if path == "/api/calendar/disconnect":
                MODE["name"] = "client"
                return self.send(204, b"")
            if path == "/api/calendar/maps-key":
                MODE["hasKey"] = body.get("key") is not None
                return self.send(204, b"")
            if path == "/api/calendar/maps-test":
                return self.send(200, {"ok": True, "place": {"name": "Ljubljana", "address": "Ljubljana, Slovenia"}})
            if path == "/api/calendar/report":
                cfg = settings().get("calendar", {}).get("report", {})
                sent = (["telegram"] if cfg.get("telegram", True) and MODE["paired"] else []) + (["spoken"] if cfg.get("spoken", True) else [])
                return self.send(200, {"text": LAST["text"], "sent": sent})
            return self.send(404, {"error": {"code": "not_found", "message": path}})
        if path == "/api/telegram":
            return self.send(200, {"enabled": True, "configured": True, "status": "polling" if MODE["paired"] else "unpaired", "botUsername": "kagent_bot",
                                   "paired": MODE["paired"], "chatId": 42 if MODE["paired"] else None, "pairedAt": "2026-11-26T10:00:00+00:00", "lastError": None, "pendingCode": None})
        if path.startswith("/api/"):
            status, data, ctype = daemon(method, path, body)
            return self.send(status, data, ctype)
        if path in ("/ws", "/live"):
            return self.send(404, b"no")
        file = DIST + path.split("?")[0]
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
        body = await self.js("document.body.innerText.slice(0, 600) + ' | ' + location.href")
        await self.shot("timeout")
        raise RuntimeError(f"timeout: {what}: {body!r}; errors: {self.errors[:4]}")

    async def shot(self, name):
        r = await self.send("Page.captureScreenshot", format="png")
        open(f"{SHOTS}/{name}.png", "wb").write(base64.b64decode(r["data"]))


def set_mode(name, paired=True, key=True):
    urllib.request.urlopen(f"http://127.0.0.1:{PORT}/__mode?name={name}&paired={int(paired)}&key={int(key)}").read()


async def main():
    os.makedirs(SHOTS, exist_ok=True)
    for f in os.listdir(SHOTS):
        os.remove(f"{SHOTS}/{f}")
    home = tempfile.mkdtemp(prefix="kagent-s45-web-")
    env = dict(os.environ, KAGENT_HOME=home, KAGENT_NO_TRAY="1", KAGENT_NO_NOTIFY="1")
    # The owner's daemon on port 7878 is never started or stopped by a test: a stand-in answers instead (picker_web_lib.py).
    STUB["on"] = port_busy()
    proc = None if STUB["on"] else subprocess.Popen([f"{ROOT}/target/debug/kagentd", "serve"], env=env, stdout=subprocess.DEVNULL, stderr=open(f"{home}/daemon.log", "wb"))
    chrome = None
    results = {}
    try:
        for _ in range(0 if STUB["on"] else 100):
            if os.path.exists(f"{home}/config/token"):
                TOKEN["v"] = open(f"{home}/config/token").read().strip()
                try:
                    if daemon("GET", "/api/state")[0] == 200:
                        break
                except Exception:
                    pass
            time.sleep(0.2)
        server = ThreadingHTTPServer(("127.0.0.1", PORT), Mock)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        profile = tempfile.mkdtemp(prefix="kagent-s45-chrome-")
        chrome = subprocess.Popen(["google-chrome", "--headless=new", f"--remote-debugging-port={CDP}", f"--user-data-dir={profile}", "--use-fake-device-for-media-stream",
                                   "--use-fake-ui-for-media-stream", "--no-first-run", "--disable-gpu", "--hide-scrollbars", "--window-size=1200,820", "about:blank"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(100):
            try:
                tabs = json.loads(urllib.request.urlopen(f"http://127.0.0.1:{CDP}/json").read())
                if any(t["type"] == "page" for t in tabs):
                    break
            except Exception:
                time.sleep(0.2)
        tab = next(t for t in tabs if t["type"] == "page")
        async with websockets.connect(tab["webSocketDebuggerUrl"], max_size=2**26) as ws:
            page = Page(ws)
            reader = asyncio.create_task(page.reader())
            for domain in ("Page", "Runtime", "Log"):
                await page.send(f"{domain}.enable")
            await page.send("Emulation.setDeviceMetricsOverride", width=1200, height=820, deviceScaleFactor=1, mobile=False)

            async def open_page(theme, mode, calendars=None, **kw):
                # The real daemon's calendar settings for this state.
                patch = {"calendar": {"enabled": mode in ("connected", "reconnect"), "calendars": calendars or ["primary", "family@group.calendar.google.com"], "defaultCalendar": "primary",
                                      "clientId": "" if mode == "off" else "123-mock.apps.googleusercontent.com", "home": "Glavni trg 5, 2000 Maribor"}}
                daemon("PUT", "/api/settings", patch)
                set_mode(mode, **kw)
                await page.send("Page.navigate", url="about:blank")
                await page.send("Page.addScriptToEvaluateOnNewDocument", source=f"try {{ localStorage.setItem('kagent.theme','{theme}'); }} catch (e) {{}} window.__opened=[]; window.open=(u)=>{{window.__opened.push(u);return null}};")
                await page.send("Page.navigate", url=f"http://127.0.0.1:{PORT}/#token=abc123&view=settings&section=calendar")
                await page.until("!!document.querySelector('.cal-status')", f"the connection card ({mode}, {theme})")
                await asyncio.sleep(0.4)

            async def shots(name):
                await page.shot(f"{name}-top")
                await page.js("document.querySelector('.ui-section-scroll').scrollTop = 99999")
                await asyncio.sleep(0.2)
                await page.shot(f"{name}-bottom")

            # ---- every state, both themes
            for theme in ("dark", "light"):
                for mode, kw in (("off", {}), ("client", {}), ("connecting", {}), ("connected", {}), ("connected-unpaired", {"paired": False, "key": False}), ("reconnect", {}), ("error", {})):
                    real = "connected" if mode.startswith("connected") else mode
                    await open_page(theme, real, **kw)
                    await shots(f"{mode}-{theme}")
                    results[f"{mode}/{theme}: page opens"] = True
                    badge = await page.js("document.querySelector('.cal-status .ui-badge').textContent")
                    results[f"{mode}/{theme}: state badge says {badge!r}"] = badge == {"off": "Not connected", "client": "Not connected", "connecting": "Waiting for Google", "connected": "Connected", "reconnect": "Reconnect needed", "error": "Error"}[real]
                    theme_attr = await page.js("document.documentElement.dataset.theme")
                    results[f"{mode}/{theme}: theme in use is {theme}"] = theme_attr == theme

            # ---- behaviour (dark)
            await open_page("dark", "client")
            results["client: Connect is enabled with a saved client"] = await page.js("[...document.querySelectorAll('button')].find(b => b.textContent === 'Connect')?.disabled === false")
            await page.js("[...document.querySelectorAll('button')].find(b => b.textContent === 'Connect').click()")
            await page.until("window.__opened.length === 1", "window.open of Connect")
            results["client: Connect opens Google's address in the browser"] = (await page.js("window.__opened[0]")).startswith("https://accounts.google.com/o/oauth2/v2/auth")
            await open_page("dark", "off")
            results["off: Connect is disabled until a client is saved"] = await page.js("[...document.querySelectorAll('button')].find(b => b.textContent === 'Connect')?.disabled === true")
            results["off: calendars card asks to connect first"] = await page.js("document.body.textContent.includes('Connect an account first')")

            await open_page("dark", "connected")
            await page.until("document.querySelectorAll('.cal-item').length === 4", "four calendars")
            results["connected: the account's address is shown"] = await page.js(f"document.querySelector('.cal-account').textContent === '{OWNER}'")
            results["connected: the read-only calendar cannot be the default"] = await page.js("(() => { const row = [...document.querySelectorAll('.cal-item')].find(r => r.textContent.includes('Holidays')); return row.querySelector('input[type=radio]').disabled && row.textContent.includes('read-only'); })()")
            results["connected: Personal is the default, Family is on"] = await page.js("(() => { const radio = n => [...document.querySelectorAll('.cal-item')].find(r => r.textContent.includes(n)).querySelector('input[type=radio]'); const tog = n => [...document.querySelectorAll('.cal-item')].find(r => r.textContent.includes(n)).querySelector('input[type=checkbox]'); return radio('Personal').checked && !radio('Family').checked && tog('Family').checked && !tog('Work').checked; })()")
            # Switch Work on, make it the default, and read the daemon's settings back.
            await page.js("[...document.querySelectorAll('.cal-item')].find(r => r.textContent.includes('Work')).querySelector('input[type=checkbox]').click()")
            await page.until("[...document.querySelectorAll('.cal-item')].find(r => r.textContent.includes('Work')).querySelector('input[type=checkbox]').checked", "Work on")
            await page.js("[...document.querySelectorAll('.cal-item')].find(r => r.textContent.includes('Work')).querySelector('input[type=radio]').click()")
            await asyncio.sleep(0.6)
            cfg = settings()["calendar"]
            results["connected: switching Work on and making it the default is saved by the daemon"] = "work@example.com" in cfg["calendars"] and cfg["defaultCalendar"] == "work@example.com"
            # Switching the default off moves the default to a writable one that stays on.
            await page.js("[...document.querySelectorAll('.cal-item')].find(r => r.textContent.includes('Work')).querySelector('input[type=checkbox]').click()")
            await asyncio.sleep(0.6)
            cfg = settings()["calendar"]
            results["connected: switching the default off gives the default to another writable calendar"] = "work@example.com" not in cfg["calendars"] and cfg["defaultCalendar"] in cfg["calendars"]
            # The last calendar cannot be switched off.
            await open_page("dark", "connected", calendars=["primary"])
            results["connected: the last calendar that is on cannot be switched off"] = await page.js("[...document.querySelectorAll('.cal-item')].find(r => r.textContent.includes('Personal')).querySelector('input[type=checkbox]').disabled")
            # Report card.
            await open_page("dark", "connected", paired=False)
            results["report: Telegram is disabled with 'pair the bot first' when no chat is paired"] = await page.js("(() => { const row = [...document.querySelectorAll('.ui-row')].find(r => r.textContent.startsWith('Telegram')); return row.querySelector('input').disabled && row.textContent.includes('Pair the bot first'); })()")
            await open_page("dark", "connected")
            results["report: Telegram is enabled when a chat is paired"] = await page.js("[...document.querySelectorAll('.ui-row')].find(r => r.textContent.startsWith('Telegram')).querySelector('input').disabled === false")
            await page.js("[...document.querySelectorAll('button')].find(b => b.textContent === 'Send now').click()")
            await page.until("document.querySelector('.cal-last pre')?.textContent.includes('Good morning')", "the text of Send now")
            results["report: Send now shows the text and where it went"] = await page.js("document.querySelector('.cal-last .hint').textContent.includes('Telegram and the next voice session')")
            await page.js("[...document.querySelectorAll('.ui-row')].find(r => r.textContent.startsWith('Weekends')).querySelector('input[type=checkbox]').click()")
            await asyncio.sleep(0.6)
            results["report: switching the weekend time off saves an empty time"] = settings()["calendar"]["report"]["weekends"] == ""
            # Places.
            await page.js("[...document.querySelectorAll('button')].find(b => b.textContent === 'Test').click()")
            await page.until("document.body.textContent.includes('Found Ljubljana: Ljubljana, Slovenia')", "the maps test result")
            results["places: Test shows what the key found"] = True
            await page.js("[...document.querySelectorAll('button')].find(b => b.textContent === 'Disconnect').click()")
            await asyncio.sleep(0.6)
            results["connected: Disconnect asked the daemon"] = any("POST /api/calendar/disconnect" in c for c in json.loads(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/__calls").read())["calls"])

            # ---- narrow
            await page.send("Emulation.setDeviceMetricsOverride", width=700, height=500, deviceScaleFactor=1, mobile=False)
            await open_page("light", "connected")
            await page.shot("connected-light-700x500")
            results["700x500: no horizontal scroll"] = await page.js("document.documentElement.scrollWidth <= window.innerWidth + 1 && document.querySelector('.ui-section-scroll').scrollWidth <= document.querySelector('.ui-section-scroll').clientWidth + 1")
            results["no console errors"] = not page.errors
            if page.errors:
                print("ERRORS", page.errors[:5])
            reader.cancel()
    finally:
        for p in (chrome, proc):
            if p:
                p.terminate()
        # The browser's profile (about 170 MB, in /tmp) goes with the run: left behind, they filled /tmp.
        if chrome:
            try:
                chrome.wait(timeout=10)
            except subprocess.TimeoutExpired:
                chrome.kill()
        for _ in range(10):
            shutil.rmtree(locals().get("profile", "") or "/nonexistent", ignore_errors=True)
            time.sleep(0.3)
            if not os.path.exists(locals().get("profile", "") or "/nonexistent"):
                break
        shutil.rmtree(home, ignore_errors=True)
    bad = [k for k, v in results.items() if not v]
    for k, v in results.items():
        print("PASS" if v else "FAIL", k)
    print(f"WEB {'OK' if not bad else 'FAILED'}: {len(results) - len(bad)}/{len(results)} checks")
    return 0 if not bad else 1


sys.exit(asyncio.run(main()))

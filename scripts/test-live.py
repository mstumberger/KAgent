#!/usr/bin/env python3
"""End-to-end test of the live voice bridge, without a Gemini key or a chat model.

Starts a fake Gemini BidiGenerateContent server (a websocket) that also serves an
OpenAI-compatible /v1/chat/completions with a fixed answer, runs kagentd against both
(KAGENT_GEMINI_WS; the chat provider's base URL is the fake), and drives /live like the
browser does. Each scenario is its own session with its own script in the fake.

Checks (voice.md section 10):
  live mode    setup shape, audio relay both ways, transcripts, a tool call executed by
               the daemon (system_info), resumption across goAway, the history and the
               context frame after setupComplete, levels hub fed.
  routed mode  a `think` call gets a toolResponse (status ok, the fixed answer) and the
               session holds the turn; a stop phrase gives cancelled; a pending card
               answered by voice gives handled; the transcript is not stored twice; a
               call the server cancelled gets its reply as a [SAY] text turn.
  confirmation a live call that needs approval is asked out loud first (no card) and runs
               after a spoken yes, its card answered by voice.
  look         (vision.md section 3) `screen` is declared to the live model whatever voice.toolGroups
               holds; the first call of the session is asked out loud and sends no frame; after a
               spoken yes the call sends one JPEG video frame and answers ok, a later call (another
               target) is not asked; a locked screen is refused with no frame. The screenshot is a
               generated picture (KAGENT_FAKE_SCREENSHOT), the lock state is faked (KAGENT_FAKE_LOCKED).
  auto mode    the tools declared to the live model.

Usage: python3 scripts/test-live.py   (builds kagentd first)

The daemon needs port 7878: the script refuses to start when a kagentd process or a
listener on 7878 exists, runs its own under a temporary KAGENT_HOME and stops it by
its pid at the end.
"""
import asyncio, base64, datetime, json, os, shutil, struct, subprocess, sys, tempfile, threading, urllib.error, urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import websockets

FAKE_PORT = 18765
CHAT_PORT = 18766
DAEMON_PORT = 7878
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIXED = "**Your CPU** is at `42` degrees."  # what the chat model always answers
SPOKEN = "Your CPU is at 42 degrees."  # the same, as speakable makes it
REPORT = "Good morning, Ana. Today, a test of the morning report."  # waits in <data>/calendar-state.json

state = {
    "connections": 0, "setups": [], "recv": [], "tool_responses": {}, "audio_in": 0,
    "script": "legacy", "chat_requests": [], "gate": threading.Event(), "tool_response": None, "response_ids": [],
    "base": "", "token": "", "fake_error": None, "script_done": False, "active": 0,
}


def make_png(path, w=1600, h=900):
    """A picture the size of a monitor: the test never takes a screenshot of the owner's screen."""
    import zlib
    rows = b"".join(b"\x00" + bytes(v for x in range(w) for v in ((x * 7) % 256, (y * 5) % 256, (x + y) % 256)) for y in range(h))
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def pcm_b64(n=1600, amp=8000):
    return base64.b64encode(b"".join(struct.pack("<h", amp if i % 20 < 10 else -amp) for i in range(n))).decode()


def chunk(delta, finish=None):
    return "data: " + json.dumps({"id": "chatcmpl-fake", "object": "chat.completion.chunk", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}) + "\n\n"


def sse(answer):
    """The OpenAI-compatible stream of one answer: `("text", text)` or `("tool", name)`."""
    kind, value = answer
    if kind == "tool":
        call = {"index": 0, "id": "chatcall1", "type": "function", "function": {"name": value, "arguments": "{}"}}
        body = chunk({"role": "assistant", "content": None, "tool_calls": [call]}) + chunk({}, "tool_calls")
    else:
        body = chunk({"role": "assistant", "content": value}) + chunk({}, "stop")
    return body + 'data: {"choices": [], "usage": {"prompt_tokens": 120, "completion_tokens": 9}}\n\ndata: [DONE]\n\n'


class Chat(BaseHTTPRequestHandler):
    """POST /v1/chat/completions: a fixed answer, a tool call, or an answer that waits."""

    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        last = (body.get("messages") or [{}])[-1]
        text = last.get("content") if isinstance(last.get("content"), str) else ""
        state["chat_requests"].append({"role": last.get("role"), "text": text})
        if last.get("role") == "user" and "slow" in text:
            state["gate"].wait(20)  # released when "stop" was said; a cancelled turn drops the answer
            answer = ("text", "Slow answer.")
        elif last.get("role") == "user" and "card" in text:
            answer = ("tool", "system_info")  # asks for a card (toolOverrides in the test settings)
        else:
            answer = ("text", FIXED)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        try:
            self.wfile.write(sse(answer).encode())
            self.wfile.flush()
        except OSError:
            pass  # the turn was cancelled and the daemon hung up
        self.close_connection = True


def api(method, path, body=None):
    req = urllib.request.Request(state["base"] + path, method=method, data=json.dumps(body).encode() if body is not None else None,
                                 headers={"Authorization": f"Bearer {state['token']}", "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=10) as r:
        raw = r.read()
        return json.loads(raw) if raw else None


async def until(pred, what, seconds=10):
    """Wait for `pred` in the page's events; a wait that ends without it fails, never a silent timeout."""
    try:
        async with asyncio.timeout(seconds):
            while not pred():
                await asyncio.sleep(0.05)
    except TimeoutError:
        raise TimeoutError(f"not ready: {what}") from None


async def wait_api(pred, what, seconds=10):
    """Wait until `GET /api/state` satisfies `pred`."""
    state["step"] = what
    try:
        async with asyncio.timeout(seconds):
            while not pred(await asyncio.to_thread(api, "GET", "/api/state")):
                await asyncio.sleep(0.05)
    except TimeoutError:
        raise TimeoutError(f"not ready: {what}") from None


async def wait_state(pred, what, seconds=10):
    """Wait until the fake's own records satisfy `pred`."""
    state["step"] = what
    try:
        async with asyncio.timeout(seconds):
            while not pred():
                await asyncio.sleep(0.05)
    except TimeoutError:
        raise TimeoutError(f"not ready: {what}") from None


# ---- the fake Gemini: what the server says and receives

async def send(ws, obj):
    await ws.send(json.dumps(obj).encode())


def received(m):
    """Keep what the fake was sent: every tool response by id."""
    for r in m.get("toolResponse", {}).get("functionResponses", []):
        state["tool_responses"][r["id"]] = r["response"]["result"]
        state["response_ids"].append(r["id"])
    return m


async def next_message(ws, got):
    m = received(json.loads(await ws.recv()))
    got.append(m)
    return m


async def until_message(ws, got, pred, what, seconds=10):
    """Receive until a message satisfies `pred`."""
    state["step"] = what
    try:
        async with asyncio.timeout(seconds):
            while not pred(await next_message(ws, got)):
                pass
    except TimeoutError:
        raise TimeoutError(f"not ready: {what}") from None


async def audio_then(ws, got):
    """Wait for two frames of mic audio."""
    while state["audio_in"] < 2:
        if "realtimeInput" in await next_message(ws, got):
            state["audio_in"] += 1


async def hear(ws, text):
    await send(ws, {"serverContent": {"inputTranscription": {"text": text}}})


async def call(ws, call_id, name, args):
    await send(ws, {"toolCall": {"functionCalls": [{"id": call_id, "name": name, "args": args}]}})


async def speak(ws, text):
    await send(ws, {"serverContent": {"modelTurn": {"parts": [{"inlineData": {"mimeType": "audio/pcm;rate=24000", "data": pcm_b64()}}]},
                                      "outputTranscription": {"text": text}, "turnComplete": True}})


async def done(ws):
    await send(ws, {"serverContent": {"turnComplete": True}})


def response(call_id):
    return lambda m: any(r["id"] == call_id for r in m.get("toolResponse", {}).get("functionResponses", []))


def is_say(m):
    turns = m.get("clientContent", {}).get("turns", [])
    return any(p.get("text", "").startswith("[SAY] ") for t in turns for p in t.get("parts", []))


async def legacy(ws, conn, got):
    """live mode: a tool call by the live model, a roll, typed input."""
    if conn == 1:
        await audio_then(ws, got)
        await hear(ws, "what system am I running")
        await call(ws, "t1", "system_info", {})  # group system: one of the default voice groups
        state["tool_response"] = await until_message(ws, got, lambda m: "toolResponse" in m, "the tool response of system_info")
        await speak(ws, "You are running Linux.")
        await send(ws, {"sessionResumptionUpdate": {"newHandle": "H1", "resumable": True}})
        await asyncio.sleep(0.2)
        await send(ws, {"goAway": {"timeLeft": "1s"}})
        await asyncio.sleep(0.5)
        await ws.close()
    else:
        async for raw in ws:
            m = received(json.loads(raw))
            got.append(m)
            if m.get("clientContent", {}).get("turnComplete"):  # the context frame does not complete a turn
                await send(ws, {"serverContent": {"outputTranscription": {"text": "Typed answer."}, "turnComplete": True}})


async def routed(ws, conn, got):
    """routed mode: think, stop, a card answered by voice, barge-in."""
    await audio_then(ws, got)
    # 1. A question: the brain answers and the live model reads it.
    await hear(ws, "what is my CPU temperature")
    await call(ws, "k1", "think", {"text": "what is my cpu temperature"})
    await until_message(ws, got, response("k1"), "the answer of think k1")
    await speak(ws, SPOKEN)
    # 2. A question that takes a while, then "stop" while the brain thinks.
    state["gate"].clear()
    await hear(ws, "tell me a slow story")
    await call(ws, "k2", "think", {"text": "tell me a slow story"})
    await wait_state(lambda: any("slow" in r["text"] for r in state["chat_requests"]), "the slow chat request")
    await call(ws, "k3", "think", {"text": "stop"})
    await until_message(ws, got, response("k3"), "the answer of the stop call")
    state["gate"].set()
    await until_message(ws, got, response("k2"), "the answer of the cancelled call")
    await done(ws)
    # 3. A pending card, answered through the think call.
    await call(ws, "k4", "think", {"text": "show me a card"})
    await wait_state(lambda: any("card" in r["text"] for r in state["chat_requests"]), "the chat request that calls the tool")
    await wait_api(lambda s: s["pendingApprovals"], "the approval card of system_info")
    await call(ws, "k5", "think", {"text": "no"})
    await until_message(ws, got, response("k5"), "the answer of think k5")
    await until_message(ws, got, response("k4"), "the answer of think k4")
    await done(ws)
    # 3b. A pending card, answered from the transcript: the think call for the same words is not a request.
    await call(ws, "k7", "think", {"text": "show me a card again"})
    await wait_state(lambda: sum("card" in r["text"] for r in state["chat_requests"]) >= 2, "the second chat request that calls the tool")
    await wait_api(lambda s: s["pendingApprovals"], "the second approval card")
    await hear(ws, "no")
    await asyncio.sleep(0.2)
    await call(ws, "k8", "think", {"text": "no"})
    # Either answer may come first.
    await until_message(ws, got, lambda m: "k7" in state["tool_responses"] and "k8" in state["tool_responses"], "the answers of think k7 and k8")
    await done(ws)
    # 4. Barge-in: the server cancels the call; the brain turn goes on and its reply comes as text.
    await call(ws, "k6", "think", {"text": "what is my GPU temperature"})
    await send(ws, {"toolCallCancellation": {"ids": ["k6"]}})
    await until_message(ws, got, is_say, "the [SAY] text turn")


async def confirm_by_voice(ws, conn, got):
    """live mode (voice.md section 12): a call that needs approval is asked out loud first,
    raises no card, and runs after the owner's spoken yes, its card answered by voice."""
    await call(ws, "c4", "system_info", {})
    await until_message(ws, got, response("c4"), "the needs_confirmation answer of c4")
    state["cards_after_c4"] = len(api("GET", "/api/state")["pendingApprovals"])
    await hear(ws, "yes")
    await asyncio.sleep(0.3)
    await call(ws, "c5", "system_info", {})
    await until_message(ws, got, response("c5"), "the result of c5")
    await asyncio.sleep(0.3)


async def look(ws, conn, got):
    """live mode (vision.md section 3): the first look is asked out loud, the yes covers the later ones."""
    # 1. The model calls `screen`: nothing is captured, the owner is asked.
    await call(ws, "l1", "screen", {})
    await until_message(ws, got, response("l1"), "the question of l1")
    state["frames_after_l1"] = sum(1 for m in got if "video" in m.get("realtimeInput", {}))
    # 2. The owner says yes; the model calls again with the same arguments: one frame, then the answer.
    await hear(ws, "yes")
    await asyncio.sleep(0.3)
    await call(ws, "l2", "screen", {})
    await until_message(ws, got, response("l2"), "the answer of l2")
    # 3. Another look of the session (the monitor): no question.
    await call(ws, "l3", "screen", {"target": "monitor"})
    await until_message(ws, got, response("l3"), "the answer of l3")
    # 4. A locked screen: refused, no frame.
    state["lock"]()
    await call(ws, "l4", "screen", {})
    await until_message(ws, got, response("l4"), "the answer of l4")
    state["frames_total"] = sum(1 for m in got if "video" in m.get("realtimeInput", {}))


async def quiet(ws, conn, got):
    """Nothing is said: the test reads the setup and stops."""


async def voiceroll(ws, conn, got):
    """voice-control.md section 2: the voice is changed in settings; at the next turnComplete the connection rolls."""
    if conn == 1:
        await audio_then(ws, got)
        await send(ws, {"sessionResumptionUpdate": {"newHandle": "H2", "resumable": True}})
        await until(lambda: state.get("voice_put"), "the test to change the voice")
        await speak(ws, "The voice changes after this sentence.")
        await asyncio.sleep(2)  # the daemon closes this connection to roll it
    else:
        async for raw in ws:
            got.append(received(json.loads(raw)))


SCRIPTS = {"look": look, "voiceroll": voiceroll, "legacy": legacy, "routed": routed, "confirm": confirm_by_voice, "auto": quiet, "nochat": quiet}


async def fake_gemini(ws):
    state["connections"] += 1
    conn = state["connections"]
    state["active"] += 1
    state["setups"].append(json.loads(await ws.recv()))  # Gemini sends binary frames
    await send(ws, {"setupComplete": {}})
    try:
        await SCRIPTS[state["script"]](ws, conn, state["recv"])
        state["script_done"] = True
        async for _ in ws:  # the script is over: the connection stays until the daemon closes it (the browser stopped)
            pass
    except websockets.ConnectionClosed:
        pass  # the test closed the browser socket: the daemon closed this one
    except Exception as e:  # reported by the scenario: a script that stops early must not pass silently
        state["fake_error"] = f"{state['script']}: {type(e).__name__}: {e}"
    finally:
        state["active"] -= 1


# ---- what the browser does

async def wait_script(ws, got):
    """The browser says nothing: the fake's script runs to its end."""
    await until(lambda: state["script_done"], "the end of the fake's script", 30)


async def live_session(session, script, drive):
    # The previous session's upstream socket closes a moment after the browser's: its
    # script must be over before this one's flags are reset.
    await until(lambda: state["active"] == 0, "the fake's earlier connections to close")
    state["script"] = script
    state["fake_error"] = None
    state["script_done"] = False
    url = f"ws://127.0.0.1:{DAEMON_PORT}/live?token={state['token']}&session={session}"
    async with websockets.connect(url) as ws:
        got = []

        async def reader():
            async for raw in ws:
                got.append(json.loads(raw))

        rt = asyncio.create_task(reader())
        # The session is live before the mic starts, so what follows `setupComplete` comes first.
        await until(lambda: any(g.get("status") == "live" for g in got), "the live status")
        state["audio_in"] = 0
        for _ in range(3):
            await ws.send(json.dumps({"type": "audio", "data": pcm_b64(320)}))
            await asyncio.sleep(0.05)
        await drive(ws, got)
        await ws.send(json.dumps({"type": "stop"}))
        await until(lambda: any(g.get("status") == "idle" for g in got), "the idle status", 5)
        rt.cancel()
        if state["fake_error"]:
            raise RuntimeError(f"the fake Gemini did not get through its script: {state['fake_error']}")
        return got


def messages(session):
    return [(m["role"], m["content"], m.get("via"), m.get("toolName")) for m in api("GET", f"/api/sessions/{session}")["messages"]]


def declared(setup):
    return [f["name"] for f in setup.get("tools", [{}])[0].get("functionDeclarations", [])]


def client_content(frames):
    return [t["parts"][0]["text"] for m in frames if "clientContent" in m for t in m["clientContent"]["turns"]]


async def main():
    subprocess.run(["cargo", "build", "-q", "-p", "kagentd"], cwd=ROOT, check=True)
    busy = subprocess.run(["pgrep", "-x", "kagentd"], capture_output=True, text=True).stdout.strip()
    listening = subprocess.run(["ss", "-ltn"], capture_output=True, text=True).stdout
    taken = [p for p in (DAEMON_PORT, FAKE_PORT, CHAT_PORT) if f":{p} " in listening]
    if busy or taken:
        print(f"NOT STARTED: kagentd running (pid {busy})" if busy else "", f"ports in use: {taken}" if taken else "")
        return 3
    home = tempfile.mkdtemp(prefix="kagent-live-")
    log = open(f"{home}/kagentd.log", "wb")
    fake_shot = f"{home}/fake-screen.png"
    make_png(fake_shot)
    lock_file = f"{home}/locked"
    env = dict(os.environ, KAGENT_HOME=home, KAGENT_NO_TRAY="1", KAGENT_NO_NOTIFY="1", KAGENT_GEMINI_WS=f"ws://127.0.0.1:{FAKE_PORT}/ws", KAGENT_FAKE_SCREENSHOT=fake_shot, KAGENT_FAKE_LOCKED=lock_file)
    daemon = None
    # A morning report that waits for a voice session (calendar.md section 6), made "today" before the daemon starts.
    os.makedirs(f"{home}/data", exist_ok=True)
    with open(f"{home}/data/calendar-state.json", "w") as f:
        json.dump({"spoken": {"day": datetime.date.today().isoformat(), "text": REPORT}}, f)
    state["lock"] = lambda: open(lock_file, "w").close()  # the screen is locked from here on
    server = await websockets.serve(fake_gemini, "127.0.0.1", FAKE_PORT)
    chat = ThreadingHTTPServer(("127.0.0.1", CHAT_PORT), Chat)
    threading.Thread(target=chat.serve_forever, daemon=True).start()
    checks = {}
    try:
        daemon = subprocess.Popen([os.path.join(ROOT, "target/debug/kagentd"), "serve"], env=env, stdout=subprocess.DEVNULL, stderr=log)
        state["base"] = f"http://127.0.0.1:{DAEMON_PORT}"
        await until(lambda: os.path.exists(f"{home}/config/token"), "the token of the test daemon", 20)
        await until(_up, "the HTTP listener of the test daemon", 20)
        state["token"] = open(f"{home}/config/token").read().strip()
        # One provider: its models answer text turns on the fake's /v1, and its live models are the fake's websocket.
        api("POST", "/api/providers", {"templateId": "gemini", "apiKey": "fake-gemini-key-123456", "baseUrl": f"http://127.0.0.1:{CHAT_PORT}/v1"})
        # The owner's name is set here: a new install has none, and it must reach the vocabulary.
        api("PUT", "/api/settings", {"liveModel": "gemini::gemini-3.8-live", "liveVoice": "Puck", "chatModel": "gemini::gemini-3.8-flash",
                                     "persona": {"name": "KAgent", "address": "Ana", "language": "auto"}})

        # ---------------------------------------------------------------- live mode
        api("PUT", "/api/settings", {"voice": {"brain": "live"}})
        session = api("POST", "/api/sessions", {"title": "voice test"})["id"]
        for text in ("what is one plus one", "and two plus two", "is it warm today"):
            api("POST", f"/api/sessions/{session}/messages", {"text": text})
            await wait_state(lambda n=len(state["chat_requests"]) + 1: len(state["chat_requests"]) >= n, f"the answer to {text!r}")
            await wait_api(lambda s: session not in s["running"], "the turn to end")
        state.update(connections=0, audio_in=0, script="legacy")
        state["setups"].clear(); state["recv"].clear()

        async def drive_legacy(ws, got):
            await until(lambda: sum(1 for g in got if g.get("status") == "live") >= 2, "the roll and the reconnection")
            await ws.send(json.dumps({"type": "text", "text": "and typed?"}))
            await until(lambda: sum(1 for g in got if g["type"] == "turnComplete") >= 2, "the two turns")

        got = await live_session(session, "legacy", drive_legacy)
        types = [g["type"] for g in got]
        statuses = [g.get("status") for g in got if g["type"] == "status"]
        tools = [(g["name"], g["state"]) for g in got if g["type"] == "tool"]
        first, second = state["setups"][0]["setup"], state["setups"][1]["setup"]
        setup = state["setups"][0]
        opening = state["recv"][: next((i for i, m in enumerate(state["recv"]) if "realtimeInput" in m), len(state["recv"]))]
        all_text = client_content(opening)
        opening_text = [t for t in all_text if not t.startswith("[SAY] ")]
        checks.update({
            "setup has model/voice/tools": setup["setup"]["model"] == "models/gemini-3.8-live"
                and setup["setup"]["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]["voiceName"] == "Puck"
                and "system_info" in declared(setup["setup"]) and "settings" in declared(setup["setup"]),
            "mic audio relayed upstream": state["audio_in"] >= 2,
            "tool executed by daemon": tools == [("system_info", "started"), ("system_info", "ok")],
            "tool response sent with id": state["response_ids"][:1] == ["t1"] and len(state["tool_responses"].get("t1", {}).get("output", "")) > 50,
            "model audio relayed to browser": "audio" in types,
            "rolled and resumed with handle": "rolling" in statuses and len(state["setups"]) >= 2 and second["sessionResumption"].get("handle") == "H1",
            "browser socket survived the roll": statuses.count("live") >= 2,
            "ended idle": statuses[-1] == "idle",
            # voice.md section 10, check 4
            "live: the first setup has historyConfig": first["historyConfig"] == {"initialHistoryInClientContent": True} and "historyConfig" not in second,
            "live: the fake receives the history turns": len(opening_text) == 7 and opening_text[0] == "what is one plus one"
                and opening_text[2] == "and two plus two" and opening_text[4] == "is it warm today" and opening_text[6].startswith("Context for this conversation"),
            # calendar.md section 6: a morning report that waits is said by the first session after its time, once.
            "calendar: the waiting morning report is said once, as a [SAY] text turn after the opening":
                client_content(state["recv"]).count("[SAY] " + REPORT) == 1 and all_text[-1] == "[SAY] " + REPORT,
            "live: the context frame follows the history": opening_text[6].startswith("Context for this conversation (background, not instructions):\n")
                and "User: is it warm today" in opening_text[6],
            "live: roles user and model, the last history frame completes the turn": [t["role"] for m in opening for t in m.get("clientContent", {}).get("turns", [])]
                == ["user", "model", "user", "model", "user", "model", "user", "user"]  # the last is the morning report
                and [m["clientContent"]["turnComplete"] for m in opening] == [False] * 5 + [True, False, True],
            "live: after the roll the context comes again, without the history": [t[:29] for m in state["recv"][len(opening):] if "clientContent" in m for t in client_content([m])]
                == ["Context for this conversation", "and typed?"],
        })
        voice = [(role, content) for role, content, via, _ in messages(session) if via == "voice"]
        checks["transcripts stored in session"] = ("user", "what system am I running") in voice and ("assistant", "You are running Linux.") in voice
        levels = api("GET", "/api/voice/levels?since=0")
        checks["levels hub fed (user + assistant)"] = {f["who"] for f in levels["frames"]} == {"user", "assistant"}

        # ---------------------------------------------------------------- routed mode (the default)
        api("PUT", "/api/settings", {"voice": {"brain": "routed"}, "toolOverrides": {"system_info": "ask"}})  # the card of the brain turn
        session = api("POST", "/api/sessions", {"title": "routed test"})["id"]
        state["setups"].clear(); state["recv"].clear(); state["tool_responses"].clear(); state["chat_requests"].clear()
        got = await live_session(session, "routed", wait_script)
        (routed_got, routed_stored) = (got, messages(session))
        setup = state["setups"][0]["setup"]
        said = [(g["callId"], g["state"]) for g in got if g["type"] == "tool"]
        answers = state["tool_responses"]
        stored = messages(session)
        checks.update({
            # voice.md section 10, check 1
            "routed: only think is declared, no history, no context": declared(setup) == ["think"] and "historyConfig" not in setup
                and all("clientContent" not in m or is_say(m) for m in state["recv"]),
            "routed: the vocabulary is in the setup": "Ana" in setup["inputAudioTranscription"]["customVocabulary"] and len(setup["inputAudioTranscription"]["customVocabulary"]) <= 100,
            "routed: a think call gets status ok and the fixed answer": answers.get("k1") == {"reply": SPOKEN, "status": "ok"},
            "routed: the session holds the turn (via voice)": ("user", "what is my cpu temperature", "voice", None) in stored and ("assistant", FIXED, "voice", None) in stored,
            "routed: the think events say thinking": said[:2] == [("k1", "started"), ("k1", "ok")],
            # check 2
            "routed: a stop phrase gives cancelled": answers.get("k3") == {"reply": "", "status": "cancelled"},
            "routed: the cancelled brain turn stored no answer": answers.get("k2") == {"reply": "", "status": "cancelled"} and not any("Slow answer" in c for _, c, _, _ in stored),
            # check 3
            "routed: a pending card answered by voice gives handled": answers.get("k5") == {"reply": "", "status": "handled"}
                and [g["state"] for g in routed_got if g["type"] == "tool" and g["callId"] == "k5"] == ["started", "ok"],
            "routed: the card is resolved and the brain turn goes on": answers.get("k4") == {"reply": SPOKEN, "status": "ok"}
                and any(role == "tool" and "denied by voice" in c for role, c, _, _ in stored) and not api("GET", "/api/state")["pendingApprovals"],
            "routed: the same words from the transcript are that answer once": answers.get("k8") == {"reply": "", "status": "handled"} and answers.get("k7")["status"] == "ok"
                and not any(role == "user" and c == "no" for role, c, _, _ in stored),
            # interruption (voice.md section 2)
            "routed: a call the server cancelled gets its reply as [SAY]": [t for m in state["recv"] if is_say(m) for t in client_content([m])] == [f"[SAY] {SPOKEN}"]
                and "k6" not in answers and state["recv"][-1]["clientContent"]["turnComplete"] is True,
            # check 6: the brain turn wrote every turn: the words once, the answer once, nothing of what was read aloud.
            "routed: the transcript is not stored twice": len(stored) == 13
                and sum(1 for role, c, _, _ in stored if role == "user" and c == "what is my cpu temperature") == 1
                and sum(1 for role, c, _, _ in stored if role == "assistant" and c == FIXED) == 4  # k1, k4, k7, k6
                and not any(c == SPOKEN for _, c, _, _ in stored),
        })

        median = api("GET", "/api/state")["voice"]["thinkMedianMs"]
        checks["routed: GET /api/state tells the median of the brain turns"] = isinstance(median, int) and median >= 0

        # ---------------------------------------------------------------- spoken confirmation of a live tool call
        api("PUT", "/api/settings", {"voice": {"brain": "live"}})
        session = api("POST", "/api/sessions", {"title": "confirm test"})["id"]
        state["setups"].clear(); state["recv"].clear(); state["tool_responses"].clear()
        await live_session(session, "confirm", wait_script)
        stored = messages(session)
        asked = state["tool_responses"].get("c4", {})
        # voice.md section 12
        checks["live: a call that needs approval is asked out loud first, with no card"] = (
            asked.get("status") == "needs_confirmation" and "Say yes to go ahead, or no." in asked.get("say", "")
            and state.get("cards_after_c4") == 0)
        checks["live: after a spoken yes the same call runs, its card answered by voice"] = (
            state["tool_responses"].get("c5", {}).get("status") == "ok"
            and any(role == "tool" and "denied" not in c for role, c, _, _ in stored)
            and not api("GET", "/api/state")["pendingApprovals"])

        # ---------------------------------------------------------------- look at my screen (vision.md section 3)
        api("PUT", "/api/settings", {"voice": {"brain": "live", "toolGroups": ["tasks"]}})  # no desktop group: screen is declared all the same
        session = api("POST", "/api/sessions", {"title": "look test"})["id"]
        state["setups"].clear(); state["recv"].clear(); state["tool_responses"].clear()
        await live_session(session, "look", wait_script)
        setup = state["setups"][0]["setup"]
        screen_decl = next((f for f in setup["tools"][0]["functionDeclarations"] if f["name"] == "screen"), {})
        video = [(i, m["realtimeInput"]["video"]) for i, m in enumerate(state["recv"]) if "video" in m.get("realtimeInput", {})]
        reply_at = lambda call_id: next((i for i, m in enumerate(state["recv"]) if any(r["id"] == call_id for r in m.get("toolResponse", {}).get("functionResponses", []))), -1)
        asked = state["tool_responses"].get("l1", {})
        checks["look: screen is declared whatever voice.toolGroups holds, without a question parameter"] = (
            bool(screen_decl) and "terminal" not in declared(setup) and "question" not in json.dumps(screen_decl))
        checks["look: the first look is asked out loud and sends no frame"] = (
            asked.get("status") == "needs_confirmation" and asked.get("say") == "I want to look at your active window. Say yes to go ahead, or no."
            and state.get("frames_after_l1") == 0)
        checks["look: after a spoken yes the call sends one JPEG video frame, then answers ok"] = (
            state["tool_responses"].get("l2") == {"status": "ok", "output": "the screenshot is in your view now"}
            and len(video) >= 1 and video[0][1]["mimeType"] == "image/jpeg" and base64.b64decode(video[0][1]["data"])[:2] == b"\xff\xd8"
            and video[0][0] < reply_at("l2"))
        checks["look: a later look of the same session is not asked, and sends its own frame"] = (
            state["tool_responses"].get("l3", {}).get("status") == "ok" and len(video) == 2 and video[1][0] < reply_at("l3"))
        checks["look: a locked screen is refused (the screen is locked) and sends no frame"] = (
            state["tool_responses"].get("l4", {}).get("output") == "the screen is locked" and state.get("frames_total") == 2
            and not api("GET", "/api/state")["pendingApprovals"])

        # ---------------------------------------------------------------- the speaking voice changes mid session (voice-control.md section 2)
        api("PUT", "/api/settings", {"voice": {"brain": "live"}, "liveVoice": "Puck"})
        session = api("POST", "/api/sessions", {"title": "voice roll"})["id"]
        state["setups"].clear(); state["recv"].clear(); state["voice_put"] = False; state["connections"] = 0

        async def drive_roll(ws, got):
            await asyncio.sleep(0.3)
            api("PUT", "/api/settings", {"liveVoice": "Charon"})
            state["voice_put"] = True
            await until(lambda: len(state["setups"]) >= 2 and sum(1 for g in got if g.get("status") == "live") >= 2, "the roll with the new voice", 15)

        got = await live_session(session, "voiceroll", drive_roll)
        voice_of = lambda i: state["setups"][i]["setup"]["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]["voiceName"]
        checks["voice: changing liveVoice rolls the connection at turnComplete, with the new voice and the handle"] = (
            len(state["setups"]) == 2 and voice_of(0) == "Puck" and voice_of(1) == "Charon"
            and state["setups"][1]["setup"]["sessionResumption"].get("handle") == "H2")

        # ---------------------------------------------------------------- auto mode: the tool set
        api("PUT", "/api/settings", {"voice": {"brain": "auto"}})
        session = api("POST", "/api/sessions", {"title": "auto test"})["id"]
        state["setups"].clear()
        await live_session(session, "auto", wait_script)
        names = declared(state["setups"][0]["setup"])
        checks["auto: think and the tools of voice.toolGroups"] = ("think" in names and "system_info" in names and "audio_control" in names and "tasks" in names
                                                                 and "kernel_info" not in names and "shell" not in names and names.index("think") == 0
                                                                 and "settings" in names)  # declared though voice.toolGroups has no agent group
        checks["live: the default groups are desktop, tasks and system"] = "kernel_info" not in declared(first) and "system_info" in declared(first)

        # ---------------------------------------------------------------- routed without a chat model
        api("PUT", "/api/settings", {"chatModel": "", "voice": {"brain": "routed"}})
        session = api("POST", "/api/sessions", {"title": "no chat model"})["id"]
        state["setups"].clear(); state["recv"].clear()
        got = await live_session(session, "nochat", wait_script)
        checks["routed without a chat model runs live and says so"] = (
            any(g.get("status") == "live" and g.get("detail") == "no chat model: the live model thinks" for g in got)
            and "think" not in declared(state["setups"][0]["setup"]) and state["setups"][0]["setup"]["historyConfig"] == {"initialHistoryInClientContent": True})

        # ---------------------------------------------------------------- settings: an unknown voice group is refused
        try:
            api("PUT", "/api/settings", {"voice": {"toolGroups": ["desktop", "nope"]}})
            checks["voice.toolGroups with an unknown group is refused (400)"] = False
        except urllib.error.HTTPError as e:
            checks["voice.toolGroups with an unknown group is refused (400)"] = e.code == 400 and b"unknown tool group 'nope'" in e.read()
        api("PUT", "/api/settings", {"voice": {"toolGroups": ["desktop", "router"]}})
        checks["voice.toolGroups takes known groups"] = api("GET", "/api/state")["settings"]["voice"]["toolGroups"] == ["desktop", "router"]

        ok = all(checks.values())
        for k, v in checks.items():
            print(("PASS " if v else "FAIL ") + k)
        if not ok:
            print("routed session:", json.dumps(routed_stored))
        print(f"LIVE {'OK' if ok else 'FAILED'}: {sum(checks.values())}/{len(checks)} checks")
    except BaseException:
        # What the daemon said: the log of the test home is deleted below.
        log.flush()
        with open(log.name) as tail:
            print(f"kagentd exit status {daemon.poll() if daemon else 'not started'}; its log:\n" + "\n".join(tail.read().splitlines()[-30:]))
        print(f"the fake: step {state['step']!r}, error {state['fake_error']!r}, done {state['script_done']}, active {state['active']}, "
              f"responses {json.dumps(state['tool_responses'])[:900]}")
        print("chat requests:", json.dumps([(r["role"], r["text"][:44]) for r in state["chat_requests"]])[:900])
        raise
    finally:
        if daemon is not None:
            daemon.terminate()
            try:
                daemon.wait(timeout=5)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait(timeout=5)
        chat.shutdown()
        chat.server_close()
        server.close()
        log.close()
        shutil.rmtree(home, ignore_errors=True)
    # Nothing of this test is left running.
    left = subprocess.run(["pgrep", "-a", "-x", "kagentd"], capture_output=True, text=True).stdout.strip()
    listening = f":{DAEMON_PORT} " in subprocess.run(["ss", "-ltn"], capture_output=True, text=True).stdout
    print(f"CLEANUP {'OK' if not left and not listening else 'FAILED'}: no kagentd process, nothing listens on {DAEMON_PORT}" if not left and not listening
          else f"CLEANUP FAILED: kagentd pid {left!r}, listening on {DAEMON_PORT}: {listening}")
    return 0 if ok and not left and not listening else 1


def _up():
    """Whether the daemon answers HTTP (an error status counts: it is up and refuses)."""
    try:
        urllib.request.urlopen(state["base"] + "/api/health", timeout=2)
    except urllib.error.HTTPError:
        pass
    except OSError:
        return False
    return True


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))

#!/usr/bin/env python3
"""End-to-end test of the live voice bridge without a Gemini key.

Starts a fake Gemini BidiGenerateContent server, runs kagentd against it
(KAGENT_GEMINI_WS), and drives /live like the browser does. Checks: setup shape,
audio relay both ways, transcripts, a tool call executed by the daemon (kernel_info),
session resumption across goAway, transcripts stored in the session, levels hub fed.

Usage: python3 scripts/test-live.py   (builds kagentd first)
"""
import asyncio, base64, json, os, struct, subprocess, sys, tempfile, time, urllib.request

import websockets

FAKE_PORT = 18765
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
state = {"connections": 0, "setups": [], "tool_response": None, "audio_in": 0}


def pcm_b64(n=1600, amp=8000):
    return base64.b64encode(b"".join(struct.pack("<h", amp if i % 20 < 10 else -amp) for i in range(n))).decode()


async def fake_gemini(ws):
    state["connections"] += 1
    conn = state["connections"]
    setup = json.loads(await ws.recv())
    state["setups"].append(setup)
    await ws.send(json.dumps({"setupComplete": {}}).encode())  # Gemini sends binary frames
    if conn == 1:
        # Wait for some mic audio, then "hear" a question and call a tool.
        while state["audio_in"] < 2:
            m = json.loads(await ws.recv())
            if "realtimeInput" in m:
                state["audio_in"] += 1
        await ws.send(json.dumps({"serverContent": {"inputTranscription": {"text": "what kernel am I running"}}}).encode())
        await ws.send(json.dumps({"toolCall": {"functionCalls": [{"id": "t1", "name": "kernel_info", "args": {"section": "version"}}]}}).encode())
        while True:
            m = json.loads(await ws.recv())
            if "toolResponse" in m:
                state["tool_response"] = m
                break
        await ws.send(json.dumps({"serverContent": {
            "modelTurn": {"parts": [{"inlineData": {"mimeType": "audio/pcm;rate=24000", "data": pcm_b64()}}]},
            "outputTranscription": {"text": "You are running Linux."}, "turnComplete": True}}).encode())
        await ws.send(json.dumps({"sessionResumptionUpdate": {"newHandle": "H1", "resumable": True}}).encode())
        await asyncio.sleep(0.2)
        await ws.send(json.dumps({"goAway": {"timeLeft": "1s"}}).encode())
        await asyncio.sleep(0.5)
        await ws.close()
    else:
        async for raw in ws:
            m = json.loads(raw)
            if "clientContent" in m:
                await ws.send(json.dumps({"serverContent": {"outputTranscription": {"text": "Typed answer."}, "turnComplete": True}}).encode())


def api(base, token, method, path, body=None):
    req = urllib.request.Request(base + path, method=method, data=json.dumps(body).encode() if body is not None else None,
                                 headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=10) as r:
        raw = r.read()
        return json.loads(raw) if raw else None


async def main():
    subprocess.run(["cargo", "build", "-q", "-p", "kagentd"], cwd=ROOT, check=True)
    home = tempfile.mkdtemp(prefix="kagent-live-")
    env = dict(os.environ, KAGENT_HOME=home, KAGENT_NO_TRAY="1", KAGENT_GEMINI_WS=f"ws://127.0.0.1:{FAKE_PORT}/ws")
    server = await websockets.serve(fake_gemini, "127.0.0.1", FAKE_PORT)
    daemon = subprocess.Popen([os.path.join(ROOT, "target/debug/kagentd"), "serve"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        base = "http://127.0.0.1:7878"
        for _ in range(50):
            if os.path.exists(f"{home}/config/token"):
                break
            time.sleep(0.1)
        time.sleep(1)
        token = open(f"{home}/config/token").read().strip()
        api(base, token, "POST", "/api/providers", {"templateId": "gemini", "apiKey": "fake-gemini-key-123456"})
        api(base, token, "PUT", "/api/settings", {"liveModel": "gemini::gemini-3.8-live", "liveVoice": "Puck"})
        session = api(base, token, "POST", "/api/sessions", {"title": "voice test"})["id"]

        got = []
        async with websockets.connect(f"ws://127.0.0.1:7878/live?token={token}&session={session}") as ws:
            async def reader():
                async for raw in ws:
                    got.append(json.loads(raw))
            rt = asyncio.create_task(reader())
            for _ in range(3):
                await ws.send(json.dumps({"type": "audio", "data": pcm_b64(320)}))
                await asyncio.sleep(0.05)
            # Wait for the first turn, the roll, and the reconnection.
            for _ in range(100):
                if sum(1 for g in got if g.get("status") == "live") >= 2:
                    break
                await asyncio.sleep(0.1)
            await ws.send(json.dumps({"type": "text", "text": "and typed?"}))
            for _ in range(50):
                if sum(1 for g in got if g["type"] == "turnComplete") >= 2:
                    break
                await asyncio.sleep(0.1)
            await ws.send(json.dumps({"type": "stop"}))
            await asyncio.sleep(0.5)
            rt.cancel()

        types = [g["type"] for g in got]
        statuses = [g.get("status") for g in got if g["type"] == "status"]
        tools = [(g["name"], g["state"]) for g in got if g["type"] == "tool"]
        checks = {
            "setup has model/voice/tools": state["setups"][0]["setup"]["model"] == "models/gemini-3.8-live"
                and state["setups"][0]["setup"]["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]["voiceName"] == "Puck"
                and any(f["name"] == "kernel_info" for f in state["setups"][0]["setup"]["tools"][0]["functionDeclarations"]),
            "mic audio relayed upstream": state["audio_in"] >= 2,
            "tool executed by daemon": tools == [("kernel_info", "started"), ("kernel_info", "ok")],
            "tool response sent with id": state["tool_response"]["toolResponse"]["functionResponses"][0]["id"] == "t1"
                and "Linux" in state["tool_response"]["toolResponse"]["functionResponses"][0]["response"]["result"]["output"],
            "model audio relayed to browser": "audio" in types,
            "rolled and resumed with handle": "rolling" in statuses and len(state["setups"]) >= 2
                and state["setups"][1]["setup"]["sessionResumption"].get("handle") == "H1",
            "browser socket survived the roll": statuses.count("live") >= 2,
            "ended idle": statuses[-1] == "idle",
        }
        msgs = api(base, token, "GET", f"/api/sessions/{session}")["messages"]
        voice = [(m["role"], m["content"]) for m in msgs if m.get("via") == "voice"]
        checks["transcripts stored in session"] = ("user", "what kernel am I running") in voice and ("assistant", "You are running Linux.") in voice
        levels = api(base, token, "GET", "/api/voice/levels?since=0")
        checks["levels hub fed (user + assistant)"] = {f["who"] for f in levels["frames"]} == {"user", "assistant"}
        ok = all(checks.values())
        for k, v in checks.items():
            print(("PASS " if v else "FAIL ") + k)
        if not ok:
            print("events:", json.dumps(got)[:2000])
        print("LIVE OK" if ok else "LIVE FAILED")
        return 0 if ok else 1
    finally:
        daemon.terminate()
        server.close()


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))

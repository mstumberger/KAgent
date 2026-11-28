#!/usr/bin/env python3
"""The screenshots of the README: docs/images/*.png, taken in headless Chrome from the real window against the mocked API.

The mock and the page driver are picker_web_lib.py (the web test suites use it too). Everything shown is invented: the owner
is Alex, the machine is workstation, the projects are website and api-server. The dark theme, 1200x820 unless noted.
Writes only to docs/images/. Run from the repo root with a build of the window somewhere that is not the owner's web/dist:

    (cd web && npx vite build --outDir /tmp/kagent-dist)
    KAGENT_DIST=/tmp/kagent-dist python3 scripts/take-screenshots.py

When port 7878 is in use (a daemon of your own) it is not touched: a stand-in answers the few routes the mock leaves out."""
import asyncio, base64, importlib.util, json, os, shutil, subprocess, sys
from datetime import datetime, timedelta, timezone
import picker_web_lib as lib
from picker_web_lib import FAKE_LIVE, Env, report

OUT = os.path.join(os.getcwd(), "docs", "images")
TMP = "/tmp/kagent-shots-work"
NOW = datetime.now(timezone.utc)
ago = lambda **kw: (NOW - timedelta(**kw)).strftime("%Y-%m-%dT%H:%M:%SZ")
WEB, API = "/home/alex/projects/website", "/home/alex/projects/api-server"
MODEL = "anthropic::claude-opus-4-6"

# ---- the chat: a disk question with a tool card, a todo list and a background command
CALL = {"id": "c1", "name": "disk_usage", "input": {"path": "/var"}}
HISTORY = [
    {"role": "user", "content": "The disk on workstation is almost full. What is using it?", "via": "text"},
    {"role": "assistant", "content": "I will look at the biggest folders under /var first.", "toolCalls": [CALL]},
    {"role": "tool", "toolCallId": "c1", "toolName": "disk_usage", "isError": False,
     "content": "/var/lib/docker   41.2 GB\n/var/log/journal   9.8 GB\n/var/cache/apt     3.1 GB\n/var/lib/flatpak   2.4 GB"},
    {"role": "assistant", "content": "The disk is 93% full. Docker images take 41 GB, the journal 9.8 GB.\n\n"
     "- `docker system prune` frees the unused images.\n"
     "- The journal can be limited to 2 GB. That is a change, so I will ask first.\n\n"
     "Which one do you want to start with?"},
]
TODO = [{"content": "Find what uses the disk", "status": "completed", "priority": "high"},
        {"content": "Show what docker prune would remove", "status": "in_progress", "priority": "medium"},
        {"content": "Limit the journal to 2 GB", "status": "pending", "priority": "medium"}]
TASKS = [{"id": "bg1", "sessionId": "s1", "command": "du -xh --max-depth=2 /var | sort -h | tail", "status": "done", "exitCode": 0,
          "startedAt": ago(minutes=3), "seconds": 12, "outputFile": "/tmp/x"}]
CHAT = dict(sessions={"s1": {"title": "Free the disk", "updatedAt": ago(minutes=1), "model": MODEL, "todo": TODO},
                      "s2": {"title": "Slow boot", "updatedAt": ago(hours=3)}, "s3": {"title": "Wi-Fi drops at night", "updatedAt": ago(days=1)}},
            messages={"s1": HISTORY}, tasks={"s1": TASKS}, outputs={"bg1": "41.2G /var/lib/docker\n9.8G /var/log/journal\n"})

VOICE = dict(CHAT, messages={"s1": [
    {"role": "user", "content": "How full is the disk?", "via": "voice"},
    {"role": "assistant", "content": "The root disk is 93 percent full. Docker images take most of it, 41 gigabytes."},
    {"role": "user", "content": "Show me what a prune would remove.", "via": "voice"},
    {"role": "assistant", "content": "Eleven unused images, about 18 gigabytes. Nothing is removed until you say yes."}]})

# ---- the sidebar: two projects with their conversations
SIDEBAR = dict(
    sessions={
        "s1": {"title": "Free the disk", "updatedAt": ago(minutes=2), "model": MODEL},
        "s2": {"title": "Why is the fan loud", "updatedAt": ago(hours=4)},
        "s3": {"title": "Add the health route", "updatedAt": ago(minutes=20), "project": API, "pinned": True},
        "s4": {"title": "Fix the contact form", "updatedAt": ago(minutes=9), "project": WEB},
        "s5": {"title": "Run the unit tests", "updatedAt": ago(hours=2), "project": WEB},
        "s7": {"title": "Rate limit the login", "updatedAt": ago(hours=6), "project": API},
    },
    messages={"s1": HISTORY}, dirs=[WEB, API], alerts=0,
    projects=[{"path": WEB, "name": "website", "agentsMd": True}, {"path": API, "name": "api-server", "agentsMd": False}])

BASE_URLS = {"anthropic": "https://api.anthropic.com/v1", "gemini": "https://generativelanguage.googleapis.com/v1beta"}
SETTINGS = dict(sessions={"s1": {"title": "Free the disk", "updatedAt": ago(minutes=1)}}, baseUrls=BASE_URLS)
OWNER = "Alex"
CALS = [{"id": "primary", "name": "Personal", "color": "#4fb6c6", "writable": True},
        {"id": "family", "name": "Family", "color": "#f6bf26", "writable": True},
        {"id": "holidays", "name": "Public holidays", "color": "#0b8043", "writable": False},
        {"id": "work", "name": "Work", "color": "#d50000", "writable": True}]
REPORT = {"at": ago(hours=2), "sent": ["telegram", "spoken"],
          "text": "Good morning, Alex. Today, Friday:\n- 10:00 Dentist, at Main Street 1\n- 10:30 School play\nDentist and School play overlap from 10:30 to 11:00.\nLeave by 09:20 for Dentist."}


def calendar_view():
    return {"state": "connected", "account": OWNER, "calendars": [dict(c, on=c["id"] in ("primary", "family", "work")) for c in CALS],
            "hasClientSecret": True, "hasMapsKey": True, "lastReport": REPORT, "error": None}


_daemon = lib.daemon


def daemon(method, path, body=None):
    """The routes of the calendar and the Telegram card that the mock leaves to the daemon (the stand-in has neither)."""
    ok = lambda v: (200, json.dumps(v).encode(), "application/json")
    if path == "/api/calendar" and method == "GET":
        return ok(calendar_view())
    if path == "/api/telegram" and method == "GET":
        return ok({"enabled": True, "configured": True, "status": "polling", "botUsername": "workstation_bot", "paired": True, "chatId": 1,
                   "pairedAt": ago(days=3), "lastError": None, "pendingCode": None})
    return _daemon(method, path, body)


lib.daemon = daemon


def local_view():
    """The Local models view as GET /api/local-models answers it: a 12 GB card with 9.5 GB free, two models installed."""
    shipped = json.load(open("config/local-models.json"))["models"]
    fits = {"qwen2.5:14b-instruct": "tight", "gemma3:12b": "tight", "llama3.2-vision:11b": "tight"}
    picks = {"llama3.1:8b": ["chat"], "gemma3:4b": ["vision"], "bge-m3": ["embedding"]}
    used = {"qwen2.5:7b-instruct": ["fallbackModel"], "gemma3:4b": ["visionModel"]}
    rec = []
    for e in shipped:
        have = e["id"] in used
        rec.append(dict(e, downloadable=e["engine"] == "ollama", fit=fits.get(e["id"], "fits"),
                        fitNote="2738 MB are in use by other programs at the moment" if e["vramMb"] > 9550 else None,
                        recommendedFor=picks.get(e["id"], []), installed=have, installedId=e["id"] if have else None,
                        ref=f"ollama::{e['id']}" if have else None, usedFor=used.get(e["id"], [])))
    gpu = {"name": "NVIDIA GeForce RTX 3060", "vramTotalMb": 12288, "vramFreeMb": 9550, "source": "nvidia-smi"}
    return {"ollama": {"reachable": True, "version": "0.9.1", "baseUrl": "http://127.0.0.1:11434"}, "gpu": gpu, "ramTotalMb": 64000,
            "installed": [{"id": i, "sizeMb": 3300, "jobs": [], "listed": True, "ref": f"ollama::{i}", "usedFor": u} for i, u in used.items()],
            "recommended": rec, "pulls": []}


def notice(text, key, kind, level="warn"):
    return {"type": "notice", "level": level, "text": text, "key": key, "kind": kind}


async def shot(page, name, clip=None):
    """A PNG of the page (or of `clip`, a box in CSS pixels) into the work folder; optimised into OUT at the end."""
    args = dict(format="png")
    if clip:
        args["clip"] = dict(x=clip[0], y=clip[1], width=clip[2], height=clip[3], scale=1)
    r = await page.send("Page.captureScreenshot", **args)
    open(f"{TMP}/{name}.png", "wb").write(base64.b64decode(r["data"]))


def optimise():
    """Smaller PNGs with what is installed: Pillow's quantizer when a file is over 300 KB."""
    from PIL import Image
    os.makedirs(OUT, exist_ok=True)
    for f in sorted(os.listdir(TMP)):
        src, dst = f"{TMP}/{f}", f"{OUT}/{f}"
        shutil.copy(src, dst)
        if os.path.getsize(dst) > 300 * 1024:
            Image.open(src).convert("RGB").quantize(colors=256, method=Image.Quantize.MEDIANCUT).save(dst, optimize=True)


async def main():
    shutil.rmtree(TMP, ignore_errors=True)
    os.makedirs(TMP)
    results = {}
    async with Env("kagent-shots", "/tmp/kagent-shots-tmp") as env:
        page = env.page
        await env.size(1200, 820)

        async def view(name, expect, hash="", query="", w=1200, h=820, ready=None, init=FAKE_LIVE, after=None, clip=None, **cfg):
            await env.size(w, h)
            await env.open("dark", query=query, hash=hash, ready=ready, init=init, **cfg)
            if after:
                await after()
            await asyncio.sleep(0.6)
            results[f"{name}: {expect}"] = bool(await page.js(f"document.body.innerText.includes({json.dumps(expect)})"))
            await shot(page, name, clip)

        async def open_work():
            await page.until("!!document.querySelector('.work-capsule')", "the work capsule")
            await page.click(".work-capsule")
            await asyncio.sleep(0.4)

        await view("chat", "Which one do you want to start with?", ready=".sb-list", after=open_work, **CHAT)
        await view("sidebar-projects", "api-server", ready=".sb-list", **SIDEBAR)
        async def no_sidebar():
            await page.click("#sidebar-toggle")
            await asyncio.sleep(0.4)

        await view("settings-models", "Anthropic", hash="&view=settings&section=models", ready=".settings", after=no_sidebar, **SETTINGS)
        await view("settings-calendar", "Public holidays", hash="&view=settings&section=calendar", ready=".settings", h=1100, after=no_sidebar, **SETTINGS)
        await view("settings-local-models", "RTX 3060", hash="&view=settings&section=local-models", ready=".lm-row", local=local_view(), after=no_sidebar, **SETTINGS)
        await view("voice", "Nothing is removed", hash="&view=voice", ready=".sb-list", **VOICE)
        await view("widget", "", query="?compact", w=340, h=460, ready=".widget-status", **CHAT)

        async def toasts():
            await page.js(f"window.__emit({json.dumps(notice('gemini::models/gemini-3.8-flash is out of quota - answering with anthropic::claude-haiku-4-5', 'provider:gemini:quota', 'error', 'error'))})")
            await asyncio.sleep(0.2)
            await page.js(f"window.__emit({json.dumps(notice('Ollama is reachable again', 'provider:ollama:down', 'recovery', 'info'))})")
            await asyncio.sleep(0.4)

        await view("toasts", "out of quota", ready=".sb-list", after=toasts, clip=(1200 - 520, 0, 520, 260), sessions=CHAT["sessions"])
        results["no page errors"] = not page.errors
        if page.errors:
            print(page.errors[:5], file=sys.stderr)
    optimise()
    return report(results)


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))

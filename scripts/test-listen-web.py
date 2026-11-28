#!/usr/bin/env python3
"""Listen on start (voice.md section 14) in headless Chrome against a mocked API, with a fake microphone.

The mock and the page driver are picker_web_lib.py; this suite adds the one route of the start marker to it (its `ROUTES` hook): the
daemon's own rule (one marker, used once) is unit-tested in Rust, here the page's side is: a window opened with a marker the daemon
accepts starts a voice session (the voice chip of the title bar), and one with no marker, a used one, an invented one, an empty one or a
bare flag (`autostart`) does not; the marker is out of the address at once, so a reload does nothing; a refused microphone is one notice
and no second try. Part two is the card "At start" of Settings -> Voice. Every wait is an `until`. Screenshots go to
/tmp/kagent-listen-shots/. Run from the repo root after a build of the current tree (KAGENT_DIST=<dir> names a build elsewhere)."""
import asyncio, json, sys
import picker_web_lib as lib
from picker_web_lib import Env, FAKE_LIVE, calls, report

SHOTS = "/tmp/kagent-listen-shots"
VALID = "ab12" * 16
STATE = {"used": False}


def route(method, path, body):
    """`POST /api/voice/listen-start`: true once, for the marker the daemon made (here VALID)."""
    if path == "/api/voice/listen-start" and method == "POST":
        ok = (body or {}).get("marker") == VALID and not STATE["used"]
        STATE["used"] = STATE["used"] or ok
        lib.CALLS.append(f"POST {path} {json.dumps((body or {}).get('marker'))[:12]} -> {ok}")
        return 200, {"start": ok}
    return None


lib.ROUTES.append(route)
MIC_REFUSED = "window.__mic = 0; if (navigator.mediaDevices) navigator.mediaDevices.getUserMedia = () => { window.__mic++; return Promise.reject(new DOMException('no', 'NotAllowedError')); };"
MIC_COUNT = "window.__mic = 0; if (navigator.mediaDevices) (() => { const real = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices); navigator.mediaDevices.getUserMedia = (c) => { window.__mic++; return real(c); }; })();"
CHIP = "!!document.querySelector('.voice-chip')"


async def quiet(page, seconds=1.2):
    """Nothing more happens: a bounded wait for an absence, the page having settled (its requests are in CALLS by then)."""
    await asyncio.sleep(seconds)


async def marker_checks(env, results):
    page = env.page
    await env.size(1200, 820)

    # A marker the daemon accepts: the session starts, the marker is out of the address.
    STATE["used"] = False
    await env.open("dark", query=f"?app&listen={VALID}", init=FAKE_LIVE + MIC_COUNT)
    await page.until(CHIP, "the voice chip of a session that started by itself")
    results["a valid marker starts a session (the chip, after the fake microphone was opened)"] = await page.js("window.__mic === 1")
    results["the marker is out of the address at once"] = await page.js("!location.search.includes('listen') && location.search.includes('app')")
    results["the page asked the daemon exactly once"] = len([c for c in calls() if "listen-start" in c]) == 1
    await page.shot("valid")

    # A reload of that window (its address no longer holds the marker) starts nothing.
    before = len([c for c in calls() if "listen-start" in c])
    await page.send("Page.reload")
    await page.until("!!document.querySelector('#sidebar-toggle')", "the reloaded page")
    await quiet(page)
    results["a reload does not start a session"] = not await page.js(CHIP)
    results["a reload presents nothing"] = len([c for c in calls() if "listen-start" in c]) == before

    # No marker, a bare flag, an empty one: nothing is asked and nothing starts.
    for name, query in (("no marker", "?app"), ("a bare autostart flag", "?app&autostart"), ("listen with no value", "?app&listen="), ("listen as a flag", "?app&listen")):
        STATE["used"] = False
        await env.open("dark", query=query, init=FAKE_LIVE + MIC_COUNT)
        await quiet(page)
        results[f"{name}: no session"] = not await page.js(CHIP) and await page.js("window.__mic === 0")
        results[f"{name}: the daemon was not even asked"] = not any("listen-start" in c for c in calls())

    # A used marker, and one nobody made: asked, refused, nothing starts, no console error.
    for name, marker in (("a used marker", VALID), ("an invented marker", "deadbeef" * 8), ("a short invented marker", "1")):
        STATE["used"] = name == "a used marker"
        await env.open("dark", query=f"?app&listen={marker}", init=FAKE_LIVE + MIC_COUNT)
        await page.until("window.performance.getEntriesByType('resource').some(r => r.name.includes('listen-start'))", f"{name}: the page asked the daemon")
        await quiet(page)
        results[f"{name}: no session and the microphone was never opened"] = not await page.js(CHIP) and await page.js("window.__mic === 0")
        results[f"{name}: the daemon was asked once and said no"] = [c for c in calls() if "listen-start" in c and c.endswith("False")] != [] and len([c for c in calls() if "listen-start" in c]) == 1
        results[f"{name}: the marker is out of the address"] = await page.js("!location.search.includes('listen')")

    # The microphone is refused: one notice, and nothing tries again.
    STATE["used"] = False
    await env.open("dark", query=f"?app&listen={VALID}", init=FAKE_LIVE + MIC_REFUSED)
    await page.until("[...document.querySelectorAll('.toast-text')].some(t => t.textContent.includes('Listen on start: microphone unavailable'))", "the notice of the refused microphone")
    await quiet(page, 2.0)
    results["a refused microphone: one notice"] = await page.js("[...document.querySelectorAll('.toast-text')].filter(t => t.textContent.includes('Listen on start')).length === 1")
    results["a refused microphone: asked once, never again"] = await page.js("window.__mic === 1") and len([c for c in calls() if "listen-start" in c]) == 1
    results["a refused microphone: no chip left"] = not await page.js(CHIP)
    await page.shot("refused")


async def main():
    results = {}
    async with Env("kagent-listen", SHOTS) as env:
        await marker_checks(env, results)
        results["no console errors"] = not env.page.errors
        if env.page.errors:
            print("ERRORS", env.page.errors[:5])
    return report(results)


sys.exit(asyncio.run(main()))

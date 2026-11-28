#!/usr/bin/env python3
"""S48: the screen watch chip in the title bar and the card of the settings page, in headless Chrome against a mocked API.

The mock and the page driver are picker_web_lib.py (its screen watch routes answer the watch, its model line, and Stop).
Checks: the chip appears when the daemon says a watch runs (at load and from the event), counts down, its popover shows the goal,
the last note and the looks and Stop ends it; it cannot be hidden (every view, no close button, the daemon's word only); the
widget shows it too; the card on the Desktop and apps page names the model, saves each setting and says what is missing. Dark and
light at 1200x820 and 340x460; screenshots go to /tmp/kagent-s48-shots/. Run from the repo root after a build of the current tree
(KAGENT_DIST=<dir> names a build elsewhere, so the owner's web/dist is left alone)."""
import asyncio, datetime, json, sys
from picker_web_lib import Env, calls, dd_set, report, watch

SHOTS = "/tmp/kagent-s48-shots"


def until(minutes):
    return (datetime.datetime.now(datetime.timezone.utc).astimezone() + datetime.timedelta(minutes=minutes)).isoformat(timespec="seconds")


def running(**kw):
    w = {"id": "w_1a2b", "sessionId": "s1", "goal": "tell me when the build in the terminal finishes", "target": "window", "windowId": "0x0460000a",
         "windowTitle": "konsole: cargo build", "until": until(19.5), "minutes": 20, "everySec": 10, "mode": "once", "state": "watching", "looks": 42,
         "lastLookAt": until(0), "lastNote": "still compiling (312/480)", "paused": False}
    w.update(kw)
    return w


CHIP = ".watch-chip"
LABEL = "document.querySelector('.watch-min').textContent"
POP = ".watch-pop"


async def main():
    results = {}
    async with Env("kagent-s48", SHOTS) as env:
        page = env.page

        async def show(theme, name, w=1200, h=820, query="", hash="", ready=None, **cfg):
            await env.size(w, h)
            await env.open(theme, query=query, hash=hash, ready=ready, **cfg)
            await page.shot(f"{name}-{theme}-{w}x{h}")

        for theme in ("dark", "light"):
            # ---- the full window: the chip, its label, its title, the popover
            await show(theme, "chip", watch=running())
            t = f"{theme}/1200x820"
            await page.until(f"!!document.querySelector('{CHIP}')", "the chip from GET /api/screen-watch")
            results[f"{t}: the chip is in the title bar next to the voice chip's place"] = await page.js(f"(() => {{ const c = document.querySelector('{CHIP}'); return !!c.closest('header') && c.getBoundingClientRect().top < 60; }})()")
            results[f"{t}: it shows an eye and the minutes left"] = await page.js(f"!!document.querySelector('{CHIP} svg') && /^(19|20) min$/.test({LABEL})")
            results[f"{t}: its title is the goal"] = await page.js(f"document.querySelector('{CHIP} button').title === 'tell me when the build in the terminal finishes'")
            results[f"{t}: it is named for a screen reader"] = await page.js(f"document.querySelector('{CHIP} button').getAttribute('aria-label').startsWith('Screen watch: ')")
            await page.click(f"{CHIP} button")
            results[f"{t}: a click opens the popover"] = await page.js(f"!!document.querySelector('{POP}') && document.querySelector('{CHIP} button').getAttribute('aria-expanded') === 'true'")
            text = await page.js(f"document.querySelector('{POP}').textContent")
            results[f"{t}: the popover shows the goal, the target, the last note and the looks"] = all(x in text for x in ("tell me when the build in the terminal finishes", "konsole: cargo build", "still compiling (312/480)", "42, one every 10 s", "at the first hit"))
            results[f"{t}: the popover stays inside the window"] = await page.js(f"(() => {{ const r = document.querySelector('{POP}').getBoundingClientRect(); return r.left >= 0 && r.right <= window.innerWidth && r.bottom <= window.innerHeight; }})()")
            await page.shot(f"popover-{theme}-1200x820")
            await page.key("Escape", "Escape", 27)
            results[f"{t}: Escape closes the popover and the chip stays"] = await page.js(f"!document.querySelector('{POP}') && !!document.querySelector('{CHIP}')")

            # ---- the widget layout
            await show(theme, "widget", w=340, h=460, query="?compact", watch=running())
            t = f"{theme}/340x460"
            await page.until(f"!!document.querySelector('{CHIP}')", "the chip in the widget")
            results[f"{t}: the widget shows the eye and the minutes"] = await page.js(f"!!document.querySelector('{CHIP} svg') && /min$/.test({LABEL})")
            await page.click(f"{CHIP} button")
            results[f"{t}: the popover fits the widget"] = await page.js(f"(() => {{ const r = document.querySelector('{POP}').getBoundingClientRect(); return r.left >= 0 && r.right <= window.innerWidth && r.bottom <= window.innerHeight && r.width > 200; }})()")
            results[f"{t}: no horizontal scroll"] = await page.js("document.documentElement.scrollWidth <= window.innerWidth + 1")
            await page.shot(f"popover-{theme}-340x460")

        # ---- no watch: no chip
        await show("dark", "none")
        results["no chip when the daemon says no watch runs"] = not await page.js(f"!!document.querySelector('{CHIP}')")

        # ---- the event starts it, counts down, updates it, ends it
        await show("dark", "event")
        await page.js(f"window.__emit({json.dumps({'type': 'screenWatchChanged', 'watch': running(until=until(5), looks=0, lastNote='')})})")
        await page.until(f"!!document.querySelector('{CHIP}')", "the chip from the event")
        label = await page.js(LABEL)
        results["the event shows the chip with the minutes left"] = label in ("4 min", "5 min")
        if label not in ("4 min", "5 min"):
            print("label was", repr(label))
        await page.js("(() => { const real = Date.now; Date.now = () => real() + 3 * 60000; })()")
        await page.until(f"/^[12] min$/.test({LABEL})", "the countdown")
        results["the minutes count down"] = await page.js(f"/^[12] min$/.test({LABEL})")
        await page.click(f"{CHIP} button")
        await page.js(f"window.__emit({json.dumps({'type': 'screenWatchChanged', 'watch': running(until=until(5), looks=7, lastNote='the build is at 90 percent')})})")
        await page.until(f"document.querySelector('{POP}').textContent.includes('the build is at 90 percent')", "the new note in the open popover")
        results["an event updates the open popover (looks and last note)"] = await page.js(f"document.querySelector('{POP}').textContent.includes('7, one every 10 s')")
        await page.js(f"window.__emit({json.dumps({'type': 'screenWatchChanged', 'watch': running(until=until(5), paused=True)})})")
        await page.until(f"{LABEL} === 'paused'", "paused")
        results["a locked screen shows paused"] = await page.js(f"document.querySelector('{CHIP}').dataset.state === 'paused' && document.querySelector('{POP}').textContent.includes('the screen is locked')")
        await page.shot("event-paused")
        for state in ("met", "expired", "failed", "stopped"):
            await page.js(f"window.__emit({json.dumps({'type': 'screenWatchChanged', 'watch': running(state='watching')})})")
            await page.until(f"!!document.querySelector('{CHIP}')", "back")
            await page.js(f"window.__emit({json.dumps({'type': 'screenWatchChanged', 'watch': running(state=state)})})")
            await page.until(f"!document.querySelector('{CHIP}')", f"the chip gone at {state}")
            results[f"the chip goes when the daemon says {state}"] = True

        # ---- Stop
        await show("dark", "stop", watch=running())
        await page.until(f"!!document.querySelector('{CHIP}')", "the chip")
        await page.click(f"{CHIP} button")
        await page.js(f"document.querySelector('{POP} .watch-stop').click()")
        await page.until(f"!document.querySelector('{CHIP}')", "the chip gone after Stop")
        results["Stop sends DELETE /api/screen-watch and the chip goes"] = "DELETE /api/screen-watch" in calls()

        # ---- it cannot be hidden: every view, no close button, the daemon's word only
        await show("dark", "views", watch=running())
        await page.until(f"!!document.querySelector('{CHIP}')", "the chip")
        seen = {}
        for view in ("settings", "tasks", "alerts", "security", "system", "changes", "voice"):
            await page.js(f"location.hash = '#token=abc123&view={view}'; location.reload()")
            await asyncio.sleep(0.8)
            await page.until("!!document.querySelector('#sidebar-toggle')", f"the {view} view")
            seen[view] = await page.js(f"!!document.querySelector('{CHIP}')")
        results["the chip shows in every view"] = all(seen.values())
        if not all(seen.values()):
            print("missing in", [v for v, ok in seen.items() if not ok])
        await show("dark", "views2", watch=running())
        await page.until(f"!!document.querySelector('{CHIP}')", "the chip")
        results["nothing in the chip closes or hides it"] = await page.js(f"[...document.querySelectorAll('{CHIP} button')].every(b => !/close|hide|dismiss/i.test(b.getAttribute('aria-label') + b.textContent))")
        await page.click(f"{CHIP} button")
        await page.js("document.body.click()")
        await page.js("document.body.dispatchEvent(new PointerEvent('pointerdown', {bubbles: true}))")
        results["clicking elsewhere closes the popover, not the chip"] = await page.js(f"!document.querySelector('{POP}') && !!document.querySelector('{CHIP}')")
        # The page reads the daemon again by itself: a watch that ended while it missed the event goes, one it missed appears.
        await show("dark", "reread")
        results["a missed start: no chip yet"] = not await page.js(f"!!document.querySelector('{CHIP}')")
        watch(**running())
        await page.js("(() => { const real = window.setInterval; window.setInterval = (f, ms, ...a) => real(f, ms > 1000 ? 100 : ms, ...a); })()")
        await page.js(f"window.__emit({json.dumps({'type': 'screenWatchChanged', 'watch': running()})})")
        await page.until(f"!!document.querySelector('{CHIP}')", "the chip")
        watch()
        await page.js(f"window.__emit({json.dumps({'type': 'screenWatchChanged', 'watch': running(until=until(5))})})")
        await page.until(f"!document.querySelector('{CHIP}')", "the chip gone after the page read the daemon again", seconds=40)
        results["a missed end: the page reads the daemon again and the chip goes"] = True

        # ---- the card on Desktop and apps
        for theme in ("dark", "light"):
            for (w, h) in ((1200, 820), (340, 460)):
                await show(theme, "card", w=w, h=h, hash="&view=settings&section=desktop", ready=".ui-card")
                t = f"{theme}/{w}x{h}"
                await page.until("[...document.querySelectorAll('.ui-card-head h3')].some(h => h.textContent === 'Screen watch')", "the card")
                await page.until("document.querySelector('.sw-model').dataset.state !== 'loading'", "the model line")
                results[f"{t}: the card Screen watch is on Desktop and apps"] = await page.js("document.querySelector('.settings').dataset.section === 'desktop' && [...document.querySelectorAll('.ui-card-head h3')].some(h => h.textContent === 'Screen watch')")
                results[f"{t}: it names the model it uses"] = await page.js("document.querySelector('.sw-model').textContent.includes('gemma3:4b')")
                results[f"{t}: defaults show: allowed, 20 minutes, 10 seconds, cloud and Telegram off"] = await page.js("""(() => { const q = (l) => document.querySelector(`[aria-label="${l}"]`); return q('Allow screen watch').checked && !q('Confirm a hit with the cloud model').checked && !q('Also tell me on Telegram').checked && q('Default length').value === '20' && q('Look every').value === '10'; })()""")
                results[f"{t}: the card has no horizontal scroll"] = await page.js("document.documentElement.scrollWidth <= window.innerWidth + 1 && document.querySelector('.ui-section-scroll').scrollWidth <= document.querySelector('.ui-section-scroll').clientWidth + 1")
                await page.js("document.querySelector('.sw-model').scrollIntoView({block: 'center'})")
                await asyncio.sleep(0.2)
                await page.shot(f"card-{theme}-{w}x{h}")

        await show("dark", "save", hash="&view=settings&section=desktop", ready=".ui-card")
        await page.until("document.querySelector('.sw-model').dataset.state !== 'loading'", "the model line")
        await page.js("document.querySelector('[aria-label=\"Confirm a hit with the cloud model\"]').click()")
        await page.until("document.querySelector('[aria-label=\"Confirm a hit with the cloud model\"]').checked", "cloud confirm on")
        results["the cloud switch saves screenWatch.cloudConfirm"] = 'PUT /api/settings {"screenWatch": {"cloudConfirm": true}}' in calls()
        await page.js("document.querySelector('[aria-label=\"Allow screen watch\"]').click()")
        await page.until("!document.querySelector('[aria-label=\"Allow screen watch\"]').checked", "allow off")
        results["the allow switch saves screenWatch.enabled"] = 'PUT /api/settings {"screenWatch": {"enabled": false}}' in calls()
        await page.js("document.querySelector('[aria-label=\"Also tell me on Telegram\"]').click()")
        await page.until("document.querySelector('[aria-label=\"Also tell me on Telegram\"]').checked", "telegram on")
        results["the Telegram switch saves screenWatch.telegram"] = 'PUT /api/settings {"screenWatch": {"telegram": true}}' in calls()
        await dd_set(page, ".dd-trigger[aria-label=\"Default length\"]", "60")
        await page.until("document.querySelector('[aria-label=\"Default length\"]').value === '60'", "length saved")
        results["the length saves screenWatch.defaultMinutes"] = 'PUT /api/settings {"screenWatch": {"defaultMinutes": 60}}' in calls()
        await dd_set(page, ".dd-trigger[aria-label=\"Look every\"]", "30")
        await page.until("document.querySelector('[aria-label=\"Look every\"]').value === '30'", "interval saved")
        results["the interval saves screenWatch.everySec"] = 'PUT /api/settings {"screenWatch": {"everySec": 30}}' in calls()

        # ---- no local model: the refusal, and the way to the page
        for theme in ("dark", "light"):
            await show(theme, "nomodel", hash="&view=settings&section=desktop", ready=".ui-card",
                       watch_model={"model": None, "server": None, "refusal": "Screen watch needs a local vision model: Settings -> Local models recommends gemma3:4b for this machine."})
            await page.until("document.querySelector('.sw-model').dataset.state === 'none'", "the refusal")
            results[f"{theme}: without a local model the card says what is missing and where to get it"] = await page.js("document.querySelector('.sw-model').textContent.includes('recommends gemma3:4b') && [...document.querySelectorAll('.sw-model button')].some(b => b.textContent === 'Open Local models')")
            await page.js("document.querySelector('.sw-model').scrollIntoView({block: 'center'})")
            await page.shot(f"nomodel-{theme}")
        await page.js("[...document.querySelectorAll('.sw-model button')].find(b => b.textContent === 'Open Local models').click()")
        await page.until("document.querySelector('.settings') && document.querySelector('.settings').dataset.section === 'local-models'", "the Local models page")
        results["Open Local models goes to Settings -> Local models"] = await page.js("document.querySelector('.settings').dataset.section === 'local-models'")

        results["no console errors"] = not page.errors
        if page.errors:
            print("ERRORS", page.errors[:5])
    return report(results)


sys.exit(asyncio.run(main()))

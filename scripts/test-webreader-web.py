#!/usr/bin/env python3
"""R1-R3: Settings -> Web (the chain of engines, the reader, the warmed sites) and the notice of a human check, in headless Chrome
against a mocked API.

The mock and the page driver are picker_web_lib.py; this suite adds the routes of the reader to it (its `ROUTES` hook). Checks: the
default chain, a move, the switches and the cards they bring (the Brave key, the SearXNG address), the reader's switch and sites,
the warmed list with Forget, "Clear the reader's profile" behind a confirmation, the state without a browser, and a notice with an
"Open it" button that posts to the daemon (and shows a refusal). Dark and light at 1200x820, dark at 700x500. Every wait is an
`until`. Screenshots go to /tmp/kagent-webreader-shots/. Run from the repo root after a build of the current tree (KAGENT_DIST=<dir>
names a build elsewhere, so the owner's web/dist is left alone)."""
import asyncio, json, re, sys
import picker_web_lib as lib
from picker_web_lib import Env, calls, report

SHOTS = "/tmp/kagent-webreader-shots"
HASH = "&view=settings&section=web"
READER = {}
NOTICE = {"type": "notice", "level": "warn", "text": "news.example.com asks for a human check. Open it, pass the check once and close the window.", "key": "reader:check:news.example.com",
          "kind": "error", "action": {"label": "Open it", "path": "/api/web/reader/open", "body": {"url": "https://news.example.com/a"}}}


def view(browser="/usr/bin/brave-browser", warmed=()):
    return {"enabled": True, "browser": browser, "running": False, "windowOpen": False, "profile": "/home/me/.local/share/kagent/reader-profile", "warmed": [dict(host=h, at=1795000000000 + i) for i, h in enumerate(warmed)]}


def route(method, path, body):
    """The reader's routes of the daemon, as far as the page uses them. Every change is in CALLS."""
    if path == "/api/settings" and method == "PUT" and ("search" in (body or {}) or "web" in (body or {})):
        lib.CALLS.append(f"PUT /api/settings {json.dumps(body)}")
        return None
    if path == "/api/web/brave-key" and method == "GET":
        return 200, {"configured": False}
    if not READER:
        return None
    if path == "/api/web/reader" and method == "GET":
        return 200, READER
    m = re.fullmatch(r"/api/web/reader/sites/(.+)", path)
    if m and method == "DELETE":
        lib.CALLS.append(f"DELETE {path}")
        READER["warmed"] = [w for w in READER["warmed"] if w["host"] != m.group(1)]
        return 204, b""
    if path == "/api/web/reader/profile" and method == "DELETE":
        lib.CALLS.append("DELETE /api/web/reader/profile")
        READER["warmed"] = []
        return 204, b""
    if path == "/api/web/reader/open" and method == "POST":
        lib.CALLS.append(f"POST {path} {json.dumps(body)}")
        if lib.CFG.get("open_error"):
            return 409, {"error": {"code": "busy", "message": lib.CFG["open_error"]}}
        return 200, {"host": "news.example.com"}
    return None


lib.ROUTES.append(route)
ENGINES = "[...document.querySelectorAll('.web-engine')].map(e => e.dataset.engine + ':' + e.dataset.on)"
BTN = lambda label: f"[...document.querySelectorAll('button')].find(b => b.textContent.trim() === {json.dumps(label)} || b.getAttribute('aria-label') === {json.dumps(label)})"
NOTE = "(document.querySelector('.ui-status') || {textContent: ''}).textContent"


async def checks(env, results, theme, w, h, full=True):
    page = env.page
    t = f"{theme} {w}x{h}"
    await env.size(w, h)
    READER.clear()
    READER.update(view(warmed=("news.example.com", "shop.example.org")))
    # The stand-in keeps the settings the page saved for the life of the process: each run starts from none.
    for key in ("search", "web"):
        lib.STUB["settings"].pop(key, None)
    await env.open(theme, ready=".web-engine", hash=HASH, open_error=None)
    await page.until("!document.querySelector('.web-engine input').disabled", "the settings loaded")
    results[f"{t}: the page is the Web section"] = await page.js("document.querySelector('.ui-section-head h2').textContent") == "Web"
    results[f"{t}: the default chain is DuckDuckGo then the reader, Brave and SearXNG off"] = await page.js(ENGINES) == ["duckduckgo:true", "reader:true", "brave:false", "searxng:false"]
    results[f"{t}: no key or address card while Brave and SearXNG are off"] = await page.js("!document.querySelector('.web-key') && !document.querySelector('[aria-label=\"SearXNG address\"]')")
    await page.until("!!document.querySelector('.web-path')", "the reader card")
    results[f"{t}: the browser it found is shown"] = await page.js("document.querySelector('.web-path').textContent") == "/usr/bin/brave-browser"
    results[f"{t}: the warmed sites are listed"] = await page.js("[...document.querySelectorAll('.web-warmed-row > span:first-child')].map(e => e.textContent)") == ["news.example.com", "shop.example.org"]
    await page.shot(f"web-{theme}-{w}x{h}")
    if not full:
        results[f"{t}: nothing runs off the page sideways"] = await page.js("document.documentElement.scrollWidth <= window.innerWidth + 1")
        return
    # ---- the chain: move, switch on, switch off
    await page.js(f"{BTN('Move DuckDuckGo down')}.click()")
    await page.until(f"JSON.stringify({ENGINES}) === JSON.stringify(['reader:true','duckduckgo:true','brave:false','searxng:false'])", "the moved chain")
    results[f"{t}: Down saves the new order"] = any('"engines": ["reader", "duckduckgo"]' in c for c in await asyncio.to_thread(calls))
    await page.js("document.querySelector('.web-engine[data-engine=brave] input').click()")
    await page.until("!!document.querySelector('.web-key')", "the Brave key card")
    results[f"{t}: Brave goes last in the chain and brings its key card"] = await page.js(ENGINES) == ["reader:true", "duckduckgo:true", "brave:true", "searxng:false"] and await page.js("!!document.querySelector('[aria-label=\"Brave Search key\"]')")
    await page.js("document.querySelector('.web-engine[data-engine=searxng] input').click()")
    await page.until("!!document.querySelector('[aria-label=\"SearXNG address\"]')", "the SearXNG address")
    await page.js("document.querySelector('.web-engine[data-engine=brave] input').click()")
    await page.until("!document.querySelector('.web-key')", "the key card gone")
    results[f"{t}: switching an engine off takes it out of the chain and its card away"] = await page.js(ENGINES) == ["reader:true", "duckduckgo:true", "searxng:true", "brave:false"]
    results[f"{t}: the first engine cannot move up, the last cannot move down"] = await page.js(f"{BTN('Move The reader up')}.disabled && {BTN('Move SearXNG down')}.disabled")
    for engine in ("reader", "duckduckgo", "searxng"):
        await page.js(f"document.querySelector('.web-engine[data-engine={engine}] input').click()")
        await page.until(f"document.querySelector('.web-engine[data-engine={engine}]').dataset.on === 'false'", f"{engine} off")
    await page.until("!document.querySelector('.web-engine[data-on=true]')", "all engines off")
    results[f"{t}: all off says search is off"] = await page.js("document.querySelector('.web-off').textContent.includes('search is off')")
    await page.js("document.querySelector('.web-engine[data-engine=duckduckgo] input').click()")
    await page.until("!!document.querySelector('.web-engine[data-on=true]')", "one on again")
    # ---- the reader: the switch and the sites it is always used for
    await page.js("document.querySelector('.ui-row input[type=checkbox]').click()")
    await until_call(page, '"web": {"reader": false}', "the reader switch saved")
    results[f"{t}: the reader's switch is saved"] = True
    box = "document.querySelector('[aria-label=\"Site for the reader\"]')"
    await page.js(f"(() => {{ const el = {box}; Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, 'not a host!'); el.dispatchEvent(new Event('input', {{bubbles: true}})); }})()")
    await page.until("!!document.querySelector('.web-sites .ui-inline-bad')", "the bad host")
    results[f"{t}: a site that is not a host cannot be added"] = await page.js(f"{BTN('Add site')}.disabled")
    await page.js(f"(() => {{ const el = {box}; Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, 'https://News.Example.com/a/b'); el.dispatchEvent(new Event('input', {{bubbles: true}})); }})()")
    await page.until(f"!{BTN('Add site')}.disabled", "the site can be added")
    await page.js(f"{BTN('Add site')}.click()")
    await page.until("!!document.querySelector('.web-chip')", "the chip")
    results[f"{t}: a pasted address becomes a bare lowercase host"] = await page.js("document.querySelector('.web-chip span').textContent") == "news.example.com" and any('"readerSites": ["news.example.com"]' in c for c in await asyncio.to_thread(calls))
    await page.js(f"{BTN('Stop using the reader for news.example.com')}.click()")
    await page.until("!document.querySelector('.web-chip')", "the chip is gone")
    results[f"{t}: Remove takes the site off the list"] = True
    # ---- the warmed sites: Forget, and the profile
    await page.js(f"{BTN('Forget news.example.com')}.click()")
    await page.until("document.querySelectorAll('.web-warmed-row').length === 1", "one site forgotten")
    results[f"{t}: Forget calls the daemon and says the cookies are gone"] = "DELETE /api/web/reader/sites/news.example.com" in await asyncio.to_thread(calls) and "forgotten" in await page.js(NOTE)
    await page.js(f"{BTN("Clear the reader's profile")}.click()")
    await page.until("!!document.querySelector('.ui-confirm')", "the confirmation")
    results[f"{t}: clearing the profile asks first, and nothing is deleted yet"] = "DELETE /api/web/reader/profile" not in await asyncio.to_thread(calls)
    await page.js("[...document.querySelectorAll('.ui-confirm button')].find(b => b.textContent === 'Clear profile').click()")
    await page.until("!!document.querySelector('.web-sites[data-kind=warmed] .ui-empty')", "the warmed list is empty")
    results[f"{t}: Clear profile deletes it and empties the list"] = "DELETE /api/web/reader/profile" in await asyncio.to_thread(calls)
    # ---- no browser on the machine
    READER.clear()
    READER.update(view(browser=None))
    await env.open(theme, ready=".web-engine", hash=HASH)
    await page.until("!!document.querySelector('.ui-badge')", "the badge")
    results[f"{t}: no browser says so"] = await page.js("document.querySelector('.web-sites') && document.body.textContent.includes('None found')")
    # ---- the notice of a human check
    await env.open(theme, ready=".web-engine", hash=HASH, open_error=None)
    await page.js(f"window.__emit({json.dumps(NOTICE)})")
    await page.until("!!document.querySelector('.toast .toast-action')", "the toast with its button")
    results[f"{t}: the notice has an Open it button"] = await page.js("document.querySelector('.toast-action').textContent") == "Open it"
    await page.shot(f"notice-{theme}-{w}x{h}")
    await page.js("document.querySelector('.toast-action').click()")
    await page.until("!document.querySelector('.toast')", "the toast closed")
    results[f"{t}: Open it posts the address to the daemon and closes the notice"] = 'POST /api/web/reader/open {"url": "https://news.example.com/a"}' in await asyncio.to_thread(calls)
    await env.open(theme, ready=".web-engine", hash=HASH, open_error="a page is being read: try again in a moment")
    await page.js(f"window.__emit({json.dumps(NOTICE)})")
    await page.until("!!document.querySelector('.toast .toast-action')", "the toast again")
    await page.js("document.querySelector('.toast-action').click()")
    await page.until("[...document.querySelectorAll('.toast')].some(x => x.textContent.includes('Open it failed: a page is being read'))", "the refusal shown")
    results[f"{t}: a refusal of the daemon is shown, and the notice stays"] = await page.js("document.querySelectorAll('.toast .toast-action').length") == 1


async def until_call(page, text, what):
    for _ in range(100):
        if any(text in c for c in await asyncio.to_thread(calls)):
            return
        await asyncio.sleep(0.1)
    raise RuntimeError(f"timeout: {what}: {await asyncio.to_thread(calls)}")


async def main():
    results = {}
    async with Env("kagent-webreader", SHOTS) as env:
        for theme in ("dark", "light"):
            await checks(env, results, theme, 1200, 820)
        await checks(env, results, "dark", 700, 500, full=False)
        # The refusal of Open it is made on purpose (a 409): the browser logs it, the page shows it.
        errors = [e for e in env.page.errors if "/api/web/reader/open" not in e]
        results["no console errors"] = not errors
        if errors:
            print("ERRORS", errors[:5])
    return report(results)


sys.exit(asyncio.run(main()))

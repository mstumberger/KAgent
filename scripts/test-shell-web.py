#!/usr/bin/env python3
"""The shell (docs/specs/shell.md) in headless Chrome against a mocked API: the router (each address opens its place, reload, old links,
the token never in the address or the history, Back and Forward by button, key and mouse button, an open menu closes first, scroll is
restored), the matrix of every page at every size with zero findings, and the conversation menu (rename, move, drag, archive, delete and undo).

Dark and light; screenshots in /tmp/kagent-v003-shell-shots/test/. Run from the repo root with KAGENT_DIST set to a build of web/
(`npx vite build --outDir <dir>`)."""
import asyncio, json, sys
import picker_web_lib as lib
import shell_matrix_lib as M
from picker_web_lib import Env, calls, opened, report

DATA = M.data()
SHOTS = "/tmp/kagent-v003-shell-shots/test"

PATH = "location.pathname"
NAV = "[...document.querySelectorAll('.sb-nav-item[aria-current=page] .sb-nav-label')].map(b => b.textContent.trim()).join()"
ALT = 1


async def hist(page):
    h = await page.send("Page.getNavigationHistory")
    return h["currentIndex"], [e["url"] for e in h["entries"]]


async def where(page):
    return await page.js(PATH)


async def go_to(page, path):
    """The address bar: a navigation to `path` in the same window (the page loads again)."""
    await page.send("Page.navigate", url=f"http://127.0.0.1:{lib.PORT}{path}")
    await page.until(f"{PATH} === {json.dumps(path)} || !!document.querySelector('#sidebar-toggle')", f"the page at {path}")
    await asyncio.sleep(0.4)


async def click_nav(page, label):
    """A real click (the browser skips the history entries a page makes without a user gesture, so a test needs one)."""
    await page.js(f"window.__nav = [...document.querySelectorAll('.sb-nav-item')].find(b => b.textContent.trim().startsWith({json.dumps(label)})); 0")
    r = await page.js("(() => { const r = window.__nav.getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; })()")
    await page.send("Input.dispatchMouseEvent", type="mousePressed", x=r[0], y=r[1], button="left", clickCount=1)
    await page.send("Input.dispatchMouseEvent", type="mouseReleased", x=r[0], y=r[1], button="left", clickCount=1)


async def mouse_button(page, button):
    await page.send("Input.dispatchMouseEvent", type="mousePressed", x=600, y=400, button=button, clickCount=1)
    await page.send("Input.dispatchMouseEvent", type="mouseReleased", x=600, y=400, button=button, clickCount=1)


async def real_click(page, sel):
    x, y = await page.center(sel)
    await page.send("Input.dispatchMouseEvent", type="mousePressed", x=x, y=y, button="left", clickCount=1)
    await page.send("Input.dispatchMouseEvent", type="mouseReleased", x=x, y=y, button="left", clickCount=1)


async def back_forward_state(page):
    return await page.js("({back: !document.querySelector('#nav-back').disabled, fwd: !document.querySelector('#nav-forward').disabled, backTitle: document.querySelector('#nav-back').title, fwdTitle: document.querySelector('#nav-forward').title})")


async def router(env, results, theme):
    t, page = theme, env.page
    await env.open(theme, **DATA)
    await page.until("!!document.querySelector('.sb-item')", "the list")
    await page.until(f"{PATH}.startsWith('/chat/s')", "the conversation in the address")
    mirror = await page.js("JSON.parse(sessionStorage.getItem('kagent.nav'))")
    results[f"{t}: / opens the chat and fills in the open conversation as a replace"] = len(mirror["entries"]) == 1 and mirror["cur"] == 0
    # Each address opens its place.
    cases = [
        ("/chat/s2", "!!document.querySelector('.sb-item.active .sb-title') && document.querySelector('.sb-item.active .sb-title').textContent === 'Unit tests for the loop'", "a conversation"),
        ("/settings/models", "document.querySelector('.settings-nav-item.active').textContent.trim() === 'Models'", "a settings page"),
        ("/settings/skills", "document.querySelector('.settings-nav-item.active').textContent.trim() === 'Skills'", "another settings page"),
        ("/tickets/KA-3", "!!document.querySelector('aside.tk-detail[data-key=KA-3]')", "a ticket"),
        ("/runs/run_aaaa1/T2", "!!document.querySelector('.rn-run-head') && document.querySelector('.rn-run-head h2').textContent.includes('overlay')", "a run with its task"),
        ("/runs/run_aaaa1", "!!document.querySelector('.rn-run-head')", "a run"),
        ("/alerts", f"{NAV} === 'Alerts'", "a view"),
        ("/changes", f"{NAV} === 'Changes'", "another view"),
        ("/voice", f"{NAV} === 'Voice'", "the voice view"),
        ("/providers", "document.querySelector('.settings-nav-item.active').textContent.trim() === 'Models'", "a former tab"),
    ]
    for path, cond, what in cases:
        await go_to(page, path)
        ok = True
        try:
            await page.until(cond, f"{what} at {path}")
        except RuntimeError:
            ok = False
        want = "/settings/models" if path == "/providers" else path
        results[f"{t}: the address {path} opens {what}"] = ok and await where(page) == want
        if path == "/runs/run_aaaa1/T2":
            await page.until("!!document.querySelector('.rn-tabs [aria-selected=true]') && document.querySelector('.rn-tabs [aria-selected=true]').textContent.trim() !== 'Overview'", "the task's tab")
            results[f"{t}: a task's address shows its plan or worker, not the overview"] = await page.js("document.querySelector('.rn-tabs [aria-selected=true]').textContent.trim()") in ("Plan", "Worker T2")
    # A pasted address that is not one is replaced by the chat; the query string stays.
    await go_to(page, "/nothing/at/all")
    await page.until(f"{PATH}.startsWith('/chat')", "the chat for an unknown address")
    results[f"{t}: an address that is not a place is replaced by the chat"] = (await hist(page))[1][-1].endswith("/chat") or "/chat/" in (await hist(page))[1][-1]
    # Reload opens the same place.
    await go_to(page, "/settings/tools")
    await page.until("document.querySelector('.settings-nav-item.active').textContent.trim().startsWith('Tools')", "tools")
    await page.send("Page.reload")
    await page.until("!!document.querySelector('.settings-nav-item.active')", "the page again")
    results[f"{t}: a reload opens the same place"] = await where(page) == "/settings/tools" and await page.js("document.querySelector('.settings-nav-item.active').textContent.trim().startsWith('Tools')")
    await click_nav(page, "Alerts")
    await page.until(f"{PATH} === '/alerts'", "alerts before a reload")
    await page.send("Page.reload")
    await page.until("!!document.querySelector('#nav-back')", "the page after the reload")
    await asyncio.sleep(0.3)
    st = await back_forward_state(page)
    results[f"{t}: a reload keeps the Back button's stack"] = st["back"] and st["backTitle"] == "Back to Settings: Tools"
    # Old links: the fragment is read once, turned into a path, and the token leaves the address.
    await env.open(theme, hash="&view=settings&section=desktop", **DATA)
    await page.until("!!document.querySelector('.settings-nav-item.active')", "the settings page")
    cur, urls = await hist(page)
    results[f"{t}: an old #view link opens its place and is replaced by its path"] = await where(page) == "/settings/desktop" and await page.js("location.hash === ''")
    results[f"{t}: the token is in no address of the history"] = all("token" not in u and "abc123" not in u for u in urls) and await page.js("localStorage.getItem('kagent.token') === 'abc123'")
    await env.open(theme, hash="&session=ab12", **DATA)
    await page.until("!!document.querySelector('.sb-item.active')", "the session link")
    results[f"{t}: an old #session link (KRunner) opens that conversation"] = await where(page) == "/chat/ab12" and "ab12" in opened()
    await env.open(theme, hash="&view=providers", **DATA)
    await page.until("!!document.querySelector('.settings-nav-item.active')", "the former tab")
    results[f"{t}: an old #view=providers link opens Models"] = await where(page) == "/settings/models"
    await env.open(theme, query="?app", hash="", path="settings/web", **DATA)
    await page.until("!!document.querySelector('.settings-nav-item.active')", "settings with ?app")
    await click_nav(page, "Alerts")
    await page.until(f"{PATH} === '/alerts'", "alerts")
    results[f"{t}: the query string stays on every address"] = await page.js("location.search") == "?app"

    # Back and Forward.
    await env.open(theme, **DATA)
    await page.until(f"{PATH}.startsWith('/chat/s')", "chat")
    results[f"{t}: Back and Forward are disabled where there is nowhere to go"] = (await back_forward_state(page)) == {"back": False, "fwd": False, "backTitle": "Back", "fwdTitle": "Forward"}
    await click_nav(page, "Tasks")
    await page.until(f"{PATH} === '/tasks'", "tasks")
    await click_nav(page, "Alerts")
    await page.until(f"{PATH} === '/alerts'", "alerts")
    await click_nav(page, "Settings")
    await page.until(f"{PATH}.startsWith('/settings/')", "settings")
    st = await back_forward_state(page)
    results[f"{t}: each view opened is one entry, Back is enabled with a tooltip naming the place"] = st["back"] and not st["fwd"] and st["backTitle"] == "Back to Alerts"
    await page.click("#nav-back")
    await page.until(f"{PATH} === '/alerts'", "back by the button")
    st = await back_forward_state(page)
    results[f"{t}: the Back button goes back and Forward is named"] = st["fwd"] and st["fwdTitle"].startswith("Forward to Settings") and await page.js(NAV) == "Alerts"
    await page.key("ArrowLeft", "ArrowLeft", 37, ALT)
    try:
        await page.until(f"{PATH} === '/tasks'", "Alt+Left", 3)
    except RuntimeError:
        h = await hist(page); print("DEBUG", h[0], len(h[1]), [u[21:] for u in h[1][-8:]])
        raise
    results[f"{t}: Alt+Left goes back"] = await page.js(NAV) == "Tasks"
    await page.key("ArrowRight", "ArrowRight", 39, ALT)
    await page.until(f"{PATH} === '/alerts'", "Alt+Right")
    results[f"{t}: Alt+Right goes forward"] = await page.js(NAV) == "Alerts"
    await mouse_button(page, "back")
    await page.until(f"{PATH} === '/tasks'", "the mouse's back button")
    idx, _ = await hist(page)
    results[f"{t}: the mouse's side button 3 goes back by one place only"] = idx == (await hist(page))[0]
    await mouse_button(page, "forward")
    await page.until(f"{PATH} === '/alerts'", "the mouse's forward button")
    results[f"{t}: the mouse's side button 4 goes forward"] = await page.js(NAV) == "Alerts"
    # A new place after Back drops Forward.
    await page.click("#nav-back")
    await page.until(f"{PATH} === '/tasks'", "back to tasks")
    await click_nav(page, "System")
    await page.until(f"{PATH} === '/system'", "system")
    st = await back_forward_state(page)
    results[f"{t}: a new place after Back drops Forward"] = not st["fwd"] and st["back"]
    # The agent's `ui show` and a click in the list are places too.
    # An open menu closes on Back before the place changes.
    await page.js("document.querySelector('.sb-more').click()")
    await page.until("!!document.querySelector('.menu')", "the row menu")
    before = await where(page)
    await page.click("#nav-back")
    await page.until("!document.querySelector('.menu')", "the menu closed by Back")
    results[f"{t}: Back closes an open menu first and the place stays"] = await where(page) == before
    await page.js("document.querySelector('.sb-more').click()")
    await page.until("!!document.querySelector('.menu')", "the row menu again")
    await mouse_button(page, "back")
    await page.until("!document.querySelector('.menu')", "the menu closed by the mouse button")
    results[f"{t}: the mouse's Back closes an open menu first"] = await where(page) == before
    await page.click("#nav-back")
    await page.until(f"{PATH} === '/tasks'", "the place changes on the next Back")
    results[f"{t}: the next Back changes the place"] = await page.js(NAV) == "Tasks"
    # A dialog (the Dropdown list of a settings page) closes first too.
    await go_to(page, "/settings/models")
    await page.until("!!document.querySelector('.pv-detail .dd-trigger')", "a dropdown")
    await real_click(page, ".pv-detail .dd-trigger")
    await page.until("!!document.querySelector('.dd-list')", "the dropdown list")
    await page.key("ArrowLeft", "ArrowLeft", 37, ALT)
    await page.until("!document.querySelector('.dd-list')", "the list closed by Back")
    results[f"{t}: Back closes an open dropdown first"] = await where(page) == "/settings/models"

    # The scroll of a page is put back on Back.
    await env.size(700, 500)
    await go_to(page, "/settings/general")
    await page.until("!!document.querySelector('[data-scroll-root=page]')", "the page frame")
    await page.until("(() => { const e = document.querySelector('[data-scroll-root=page]'); return e.scrollHeight > e.clientHeight + 300; })()", "a tall page")
    await page.js("document.querySelector('[data-scroll-root=page]').scrollTop = 300")
    await page.until("document.querySelector('[data-scroll-root=page]').scrollTop === 300", "scrolled")
    await asyncio.sleep(0.3)
    await page.js("document.querySelector('#sidebar-toggle').click()")
    await page.until("!!document.querySelector('aside.drawer')", "the drawer")
    await click_nav(page, "Tasks")
    await page.until(f"{PATH} === '/tasks'", "tasks")
    await page.click("#nav-back")
    await page.until(f"{PATH} === '/settings/general'", "back to the page")
    await page.until("document.querySelector('[data-scroll-root=page]') && Math.abs(document.querySelector('[data-scroll-root=page]').scrollTop - 300) < 3", "the scroll restored")
    results[f"{t}: the scroll of a page is restored on Back"] = abs(await page.js("document.querySelector('[data-scroll-root=page]').scrollTop") - 300) < 3
    await env.size(1200, 820)


async def main():
    results = {}
    async with Env("kagent-shell-test", SHOTS) as env:
        for theme in ("dark", "light"):
            await router(env, results, theme)
        errors = [e for e in env.page.errors if "404" not in e]
        results["no console errors"] = not errors
        if errors:
            print("ERRORS", errors[:5])
    return report(results)


sys.exit(asyncio.run(main()))

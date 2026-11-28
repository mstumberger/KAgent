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


OT = "/home/virus/code/other"
CONVO = dict(DATA, dirs=[M.KA, OT], running=["s2"], projects=[{"path": M.KA, "name": "kagent", "agentsMd": True}, {"path": OT, "name": "other", "agentsMd": False}])
ROWS = "[...document.querySelectorAll('.sb-item .sb-title')].map(e => e.textContent)"
row = lambda title: f"[...document.querySelectorAll('.sb-item')].find(r => r.querySelector('.sb-title, .sb-rename') && (r.querySelector('.sb-title') ? r.querySelector('.sb-title').textContent : '') === {json.dumps(title)})"
group_of = lambda title: f"(() => {{ let n = {row(title)}; while (n && !n.classList.contains('sb-group')) n = n.parentElement; return n ? n.querySelector('.sb-section-name').textContent : null; }})()"
MENU = "[...document.querySelectorAll('.menu [role=menuitem]')].map(i => i.textContent.replace(/F2|Del$/, '').trim())"


async def open_menu(page, title):
    await page.js(f"{row(title)}.querySelector('.sb-more').click()")
    await page.until("!!document.querySelector('.menu')", f"the menu of {title}")


async def pick(page, label):
    await page.js(f"[...document.querySelectorAll('.menu [role=menuitem]')].find(i => i.textContent.startsWith({json.dumps(label)})).click()")
    await asyncio.sleep(0.15)


async def conversations(env, results, theme):
    t, page = theme, env.page
    knob = "try { localStorage.setItem('kagent.undo-ms', '700'); } catch (e) {}"
    await env.open(theme, init=knob, **CONVO)
    await page.until("!!document.querySelector('.sb-item')", "the list")
    # The menu: what was there, and what is new.
    await open_menu(page, "Why is the fan loud and what can be done about it in the long run")
    items = await page.js(MENU)
    results[f"{t}: the row menu has Rename, Pin, Move to project, Attach folder, Archive and Delete"] = items == ["Rename", "Pin", "Move to project...", "Attach folder...", "Archive", "Delete"]
    await page.key("Escape", "Escape", 27)
    long_title = "Why is the fan loud and what can be done about it in the long run"

    # Rename: from the menu, with F2, Enter saves, Escape and an empty title change nothing.
    await open_menu(page, long_title)
    await pick(page, "Rename")
    await page.until("!!document.querySelector('.sb-rename')", "the rename field")
    results[f"{t}: Rename turns the title into a field holding the title"] = await page.js("document.querySelector('.sb-rename').value") == long_title
    await page.js("(() => { const i = document.querySelector('.sb-rename'); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; set.call(i, '  The loud   fan '); i.dispatchEvent(new Event('input', {bubbles: true})); })()")
    await page.key("Enter", "Enter", 13)
    await page.until(f"{row('The loud fan')} !== undefined", "the new title")
    results[f"{t}: Enter saves the title, trimmed and on one line, with one PUT"] = 'PUT /api/sessions/s3/title {"title": "The loud fan"}' in calls() and not await page.js("!!document.querySelector('.sb-rename')")
    results[f"{t}: after a rename the focus is back on the row"] = await page.js("document.activeElement.classList.contains('sb-row-main') && document.activeElement.textContent.includes('The loud fan')")
    n = len(calls())
    await page.js(f"{row('The loud fan')}.querySelector('.sb-row-main').focus()")
    await page.key("F2", "F2", 113)
    await page.until("!!document.querySelector('.sb-rename')", "F2 renames")
    await page.key("Escape", "Escape", 27)
    await page.until("!document.querySelector('.sb-rename')", "Escape closes the field")
    results[f"{t}: F2 opens the field and Escape closes it without a call"] = len(calls()) == n
    await page.js(f"{row('The loud fan')}.querySelector('.sb-row-main').focus()")
    await page.key("F2", "F2", 113)
    await page.until("!!document.querySelector('.sb-rename')", "F2 again")
    await page.js("(() => { const i = document.querySelector('.sb-rename'); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; set.call(i, '   '); i.dispatchEvent(new Event('input', {bubbles: true})); })()")
    await page.key("Enter", "Enter", 13)
    await page.until("!document.querySelector('.sb-rename')", "an empty title is not saved")
    results[f"{t}: an empty title is not saved and the row keeps its title"] = len(calls()) == n and await page.js("!!" + row("The loud fan"))

    # Move to a project: the submenu, and "No project".
    await open_menu(page, "The loud fan")
    await pick(page, "Move to project")
    await page.until("[...document.querySelectorAll('.menu [role=menuitem]')].some(i => i.textContent.startsWith('No project'))", "the projects")
    results[f"{t}: Move to project lists the projects and No project, and checks the current one"] = (await page.js(MENU))[-1] == "No project" and "other" in await page.js(MENU) and await page.js("document.querySelector('.menu [aria-checked=true], .menu [role=menuitem].checked, .menu .pop-check') !== null")
    await pick(page, "other")
    await page.until(f"{group_of('The loud fan')} === 'other'", "the row in its new project")
    results[f"{t}: picking a project moves the conversation (PUT project) and the row changes section"] = f'PUT /api/sessions/s3/project {json.dumps({"path": OT})}' in calls()
    await open_menu(page, "The loud fan")
    await pick(page, "Move to project")
    await page.until("!!document.querySelector('.menu')", "the submenu again")
    await pick(page, "No project")
    await page.until(f"{group_of('The loud fan')} === 'This machine'", "back to no project")
    results[f"{t}: No project takes the conversation out of its project"] = 'PUT /api/sessions/s3/project {"path": null}' in calls()

    # Drag a row onto a project heading; the group lights up while it is over.
    drag = lambda title, target, step: f"""(() => {{ window.__dt = window.__dt || new DataTransfer(); const dt = window.__dt; const r = {row(title)};
      const g = [...document.querySelectorAll('.sb-group')].find(x => x.querySelector('.sb-section-name') && x.querySelector('.sb-section-name').textContent === {json.dumps(target)});
      if ({json.dumps(step)} === 'over') {{ r.dispatchEvent(new DragEvent('dragstart', {{bubbles: true, dataTransfer: dt}})); g.dispatchEvent(new DragEvent('dragover', {{bubbles: true, cancelable: true, dataTransfer: dt}})); }}
      else if ({json.dumps(step)} === 'leave') g.dispatchEvent(new DragEvent('dragleave', {{bubbles: true, relatedTarget: document.body, dataTransfer: dt}}));
      else {{ r.dispatchEvent(new DragEvent('dragstart', {{bubbles: true, dataTransfer: dt}})); g.dispatchEvent(new DragEvent('dragover', {{bubbles: true, cancelable: true, dataTransfer: dt}})); g.dispatchEvent(new DragEvent('drop', {{bubbles: true, cancelable: true, dataTransfer: dt}})); window.__dt = null; }}
    }})()"""
    await page.js(drag("The loud fan", "kagent", "over"))
    await page.until("!!document.querySelector('.sb-group.drop')", "the project lit while a row is over it")
    await page.js(drag("The loud fan", "kagent", "leave"))
    await page.until("!document.querySelector('.sb-group.drop')", "the light out after leaving")
    results[f"{t}: a row dragged over a project lights the project up, and leaving puts it out"] = True
    await page.js(drag("The loud fan", "kagent", "drop"))
    await page.until(f"{group_of('The loud fan')} === 'kagent'", "the dropped row")
    results[f"{t}: dropping a row on a project moves it (the same PUT as the menu)"] = calls().count(f'PUT /api/sessions/s3/project {json.dumps({"path": M.KA})}') == 1
    await page.js(drag("The loud fan", "This machine", "drop"))
    await page.until(f"{group_of('The loud fan')} === 'This machine'", "dropped on This machine")
    results[f"{t}: dropping a row on This machine takes it out of its project"] = calls().count('PUT /api/sessions/s3/project {"path": null}') == 2

    # Archive: a group at the foot, left out of the search unless the filter is pressed; a running one is refused.
    await open_menu(page, "The loud fan")
    await pick(page, "Archive")
    await page.until(f"{row('The loud fan')} === undefined", "the archived row leaves the list")
    results[f"{t}: Archive hides the row and puts it in a collapsed Archived group at the foot"] = 'PUT /api/sessions/s3/archive {"archived": true}' in calls() and await page.js("document.querySelector('.sb-archived .sb-section-name').textContent") == "Archived" and await page.js("document.querySelector('.sb-archived .sb-section').getAttribute('aria-expanded')") == "false" and await page.js("document.querySelectorAll('.sb-archived .sb-item').length") == 0
    await page.js("document.querySelector('.sb-search input').focus()")
    await page.js("(() => { const i = document.querySelector('.sb-search input'); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; set.call(i, 'loud'); i.dispatchEvent(new Event('input', {bubbles: true})); })()")
    await page.until("document.querySelector('.sb-empty') && document.querySelector('.sb-empty').textContent.includes('No session matches')", "the search without it")
    results[f"{t}: the search leaves archived conversations out"] = True
    await page.js("document.querySelector('.sb-filter').click()")
    await page.until("!!document.querySelector('.sb-item .sb-title')", "the search with the filter")
    results[f"{t}: the Archived filter brings them into the search"] = await page.js(ROWS) == ["The loud fan"] and await page.js("document.querySelector('.sb-filter').getAttribute('aria-pressed')") == "true"
    await page.js("document.querySelector('.sb-filter').click()")
    await page.js("(() => { const i = document.querySelector('.sb-search input'); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; set.call(i, ''); i.dispatchEvent(new Event('input', {bubbles: true})); })()")
    await page.until("!!document.querySelector('.sb-archived .sb-section')", "the list again")
    await page.js("document.querySelector('.sb-archived .sb-section').click()")
    await page.until("document.querySelectorAll('.sb-archived .sb-item').length === 1", "the archived group open")
    await open_menu(page, "The loud fan")
    results[f"{t}: an archived row's menu has Unarchive and no Pin or Move"] = await page.js(MENU) == ["Rename", "Unarchive", "Delete"]
    await pick(page, "Unarchive")
    await page.until(f"{group_of('The loud fan')} === 'This machine'", "unarchived")
    results[f"{t}: Unarchive brings the row back to its place"] = 'PUT /api/sessions/s3/archive {"archived": false}' in calls() and await page.js("!document.querySelector('.sb-archived')")
    n = len(calls())
    await open_menu(page, "Unit tests for the loop")
    await pick(page, "Archive")
    await page.until("!!document.querySelector('.toast')", "the refusal as a toast")
    results[f"{t}: a running conversation is not archived (a notice with the reason, no call)"] = "running" in await page.js("document.querySelector('.toast').textContent") and len(calls()) == n and await page.js("!!" + row("Unit tests for the loop"))
    await page.js("document.querySelector('.toast-close').click()")

    # Delete: hidden at once with an Undo toast; Undo cancels; the daemon deletes when the toast ends.
    hexed = "A conversation with a hex id"
    await open_menu(page, hexed)
    await pick(page, "Delete")
    await page.until(f"{row(hexed)} === undefined", "the deleted row hidden")
    results[f"{t}: Delete hides the row at once and says so with an Undo toast, with no call yet"] = await page.js("document.querySelector('.toast').textContent.includes('Undo')") and not any(c.startswith("DELETE") for c in calls())
    await open_menu(page, "The loud fan")
    await pick(page, "Delete")
    await page.until(f"{row('The loud fan')} === undefined", "the second row hidden")
    await page.js("[...document.querySelectorAll('.toast')].find(x => x.textContent.includes('hex id')).querySelector('.toast-action').click()")
    await page.until(f"{row(hexed)} !== undefined", "undone")
    await page.until("!document.querySelector('.toast')", "the other toast ended", 6)
    await until_call(page, "DELETE /api/sessions/s3")
    results[f"{t}: when the toast ends the daemon is called (DELETE), and the row is gone for good"] = not await page.js("!!" + row("The loud fan"))
    results[f"{t}: Undo showed the row again and its conversation is never deleted"] = "DELETE /api/sessions/ab12" not in calls() and await page.js("!!" + row(hexed))
    # The open conversation goes: the next one opens, and it is a replace in the history.
    mirror = await page.js("JSON.parse(sessionStorage.getItem('kagent.nav')).entries.length")
    await page.js(f"{row('Free the disk')}.querySelector('.sb-row-main').focus()")
    await page.key("Delete", "Delete", 46)
    await page.until(f"{row('Free the disk')} === undefined", "deleted by the key")
    await page.until(f"!{PATH}.endsWith('/s1')", "the next conversation opens")
    results[f"{t}: Delete on the focused row deletes it; when it was open the next one opens as a replace"] = await page.js("JSON.parse(sessionStorage.getItem('kagent.nav')).entries.length") == mirror
    await page.js("[...document.querySelectorAll('.toast-action')].find(b => b.textContent === 'Undo').click()")
    await page.until(f"{row('Free the disk')} !== undefined && {PATH}.endsWith('/s1')", "undo brings it back open")
    results[f"{t}: Undo of the open conversation opens it again"] = "DELETE /api/sessions/s1" not in calls()
    # A conversation that runs is refused here, with no call; one the daemon refuses at the end comes back with the reason.
    n = len(calls())
    await open_menu(page, "Unit tests for the loop")
    await pick(page, "Delete")
    await page.until("!!document.querySelector('.toast') && document.querySelector('.toast').textContent.includes('running')", "the running refusal")
    results[f"{t}: a running conversation is not deleted (a notice with the reason, no call, the row stays)"] = len(calls()) == n and await page.js("!!" + row("Unit tests for the loop"))
    await page.js("document.querySelector('.toast-close').click()")
    lib.CFG["deny_delete"] = ["ab12"]
    await open_menu(page, hexed)
    await pick(page, "Delete")
    await page.until(f"{row(hexed)} === undefined", "hidden before the refusal")
    await until_call(page, "DELETE /api/sessions/ab12")
    await page.until(f"{row(hexed)} !== undefined && [...document.querySelectorAll('.toast')].some(x => x.textContent.includes('Not deleted'))", "the row back with the reason")
    results[f"{t}: a delete the daemon refuses at the end brings the row back with its reason"] = "running" in await page.js("[...document.querySelectorAll('.toast')].find(x => x.textContent.includes('Not deleted')).textContent")
    lib.CFG["deny_delete"] = []
    # Closing the window during the undo time: the delete is done (a keepalive request), the conversation is not left half there.
    calls_before = calls().count("DELETE /api/sessions/ab12")
    await open_menu(page, hexed)
    await pick(page, "Delete")
    await page.until(f"{row(hexed)} === undefined", "hidden before the window closes")
    await page.send("Page.navigate", url="about:blank")
    await until_call(page, "DELETE /api/sessions/ab12", count=calls_before + 1)
    results[f"{t}: closing the window during the undo time sends the delete at once (it is done, not lost)"] = lib.SESSIONS.get("ab12") is None
    # The real ten seconds.
    await env.open(theme, **CONVO)
    await page.until("!!document.querySelector('.sb-item')", "the list again")
    await open_menu(page, long_title)
    await pick(page, "Delete")
    await page.until(f"{row(long_title)} === undefined", "hidden")
    t0 = asyncio.get_event_loop().time()
    await page.until("document.querySelectorAll('.toast').length === 1", "the toast")
    await asyncio.sleep(3)
    still = await page.js("document.querySelectorAll('.toast').length === 1") and "DELETE /api/sessions/s3" not in calls()
    await until_call(page, "DELETE /api/sessions/s3", seconds=14)
    took = asyncio.get_event_loop().time() - t0
    results[f"{t}: the toast and the wait are 10 seconds (the delete came after {took:.1f} s)"] = still and 8.5 < took < 13


async def until_call(page, call, count=1, seconds=10):
    end = asyncio.get_event_loop().time() + seconds
    while asyncio.get_event_loop().time() < end:
        if calls().count(call) >= count:
            return
        await asyncio.sleep(0.1)
    raise RuntimeError(f"timeout: {call} was not called: {calls()[-5:]}")


async def matrix(env, results):
    """The matrix of every page at every size, dark and light: any finding fails (shell.md section 3)."""
    found = await M.run_matrix(env, "/tmp/kagent-v003-shell-shots/after")
    bad = {k: v for k, v in found.items() if v}
    for (theme, page, size), fs in sorted(bad.items()):
        for f in fs[:3]:
            print(f"FINDING {theme} {page} {size}: {f['kind']} {f['at'][:60]} | {f['detail'][:80]}")
    results[f"the matrix of {len(found)} cells (every page, every size, dark and light) has no finding"] = not bad


async def main():
    results = {}
    async with Env("kagent-shell-test", SHOTS) as env:
        for theme in ("dark", "light"):
            await router(env, results, theme)
            await conversations(env, results, theme)
        await matrix(env, results)
        # The 404s are routes of the stand-in; the 409 is the refusal of a delete the suite asked for.
        errors = [e for e in env.page.errors if "404" not in e and "409" not in e]
        results["no console errors"] = not errors
        if errors:
            print("ERRORS", errors[:5])
    return report(results)


sys.exit(asyncio.run(main()))

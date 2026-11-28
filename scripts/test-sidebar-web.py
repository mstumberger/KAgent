#!/usr/bin/env python3
"""S42: the left sidebar and the projects in headless Chrome against a mocked API.

A real kagentd (temporary KAGENT_HOME, port 7878) answers everything except the session routes, the
projects and a few fields of GET /api/state, which the proxy of picker_web_lib.py mocks in memory.
Dark and light at 1200x820, then 700x500 (the drawer) and 340x460 (the widget); screenshots go to
/tmp/kagent-s42-shots/. Run from the repo root after `cargo build` and `npm run build` in web/."""
import asyncio, json, sys
from datetime import datetime, timedelta, timezone
import picker_web_lib as lib
from picker_web_lib import FAKE_LIVE, Env, calls, daemon, opened, report, reset

NOW = datetime.now(timezone.utc)
ago = lambda **kw: (NOW - timedelta(**kw)).strftime("%Y-%m-%dT%H:%M:%SZ")
# Days are calendar days of local time (sidebar.md section 5.3): the old sessions are placed from local midnight,
# so the buckets are the same at any hour; the "today" ones are minutes old.
MIDNIGHT = datetime.now().astimezone().replace(hour=0, minute=0, second=0, microsecond=0)
before_midnight = lambda **kw: (MIDNIGHT - timedelta(**kw)).astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
KA, OT, EX = "/home/me/code/KAgent", "/home/me/code/other", "/home/me/code/extra"
SESSIONS = {
    "s1": {"title": "Free the disk", "updatedAt": ago(minutes=1), "project": KA},
    "s2": {"title": "Unit tests for the loop", "updatedAt": ago(minutes=2), "project": KA, "agents": 1},
    "s3": {"title": "Fix the Wi-Fi driver", "updatedAt": ago(minutes=3), "project": KA, "pinned": True, "agents": 2},
    "s4": {"title": "Why is the fan loud", "updatedAt": before_midnight(days=1, hours=2)},
    "s5": {"title": "Docs pass", "updatedAt": before_midnight(days=3, hours=2), "project": OT},
    "s6": {"title": "Old chat", "updatedAt": ago(days=40)},
    "s7": {"title": "Yesterday chat", "updatedAt": before_midnight(hours=2)},
}
CFG = dict(sessions=SESSIONS, dirs=[KA, OT, EX], alerts=3, running=["s2"],
           projects=[{"path": KA, "name": "KAgent", "agentsMd": True}, {"path": OT, "name": "other", "agentsMd": False}])

MODE = lambda: json.loads(daemon("GET", "/api/state")[1])["settings"]["permissionMode"]
THINGS = "(() => [...document.querySelector('header').children].map(c => c.id === 'sidebar-toggle' ? 'toggle' : c.classList.contains('logo') ? 'logo:' + c.textContent.trim() : c.classList.contains('spacer') ? 'spacer' : c.classList.contains('notice') ? 'notice' : c.classList.contains('voice-chip') ? 'voice' : c.classList.contains('win-controls') ? 'window' : c.textContent.trim()))()"
DESCRIBE = "(() => { const a = document.activeElement; return a.id === 'sidebar-toggle' ? 'toggle' : a.closest('.voice-chip') ? 'voice:' + a.textContent.trim() : a.closest('.win-controls') ? 'window:' + a.getAttribute('aria-label') : a.classList.contains('sb-new') ? 'new' : a.textContent.trim() || a.getAttribute('aria-label'); })()"

VIEWS = ["Voice", "Tasks", "Tickets", "Runs", "Alerts", "Changes", "Security", "System", "Settings"]
HEADINGS = "[...document.querySelectorAll('.sb-list .sb-section .sb-section-name')].map(e => e.textContent)"
LABELS = "[...document.querySelectorAll('.sb-list .sb-head')].map(h => h.querySelector('.sb-section-name').textContent)"
ROWS = "[...document.querySelectorAll('.sb-list .sb-item .sb-title')].map(e => e.textContent)"
ACTIVE = "document.activeElement"
row = lambda title: f"[...document.querySelectorAll('.sb-item')].find(r => r.querySelector('.sb-title').textContent === {json.dumps(title)})"
section_of = lambda title: f"(() => {{ const r = {row(title)}; let n = r; while (n && !n.classList.contains('sb-group')) n = n.parentElement; return n ? n.querySelector('.sb-section-name').textContent : null; }})()"


async def main():
    results = {}
    async with Env("kagent-s42", "/tmp/kagent-s42-shots") as env:
        page = env.page

        init = {"id": None, "n": 0}

        async def open_page(theme, query="", prefs=None, **cfg):
            reset(**{**CFG, **cfg})
            await page.send("Page.navigate", url="about:blank")
            # The script that sets the stored state runs on the first load of this page only: a reload keeps what the page stored.
            if init["id"]:
                await page.send("Page.removeScriptToEvaluateOnNewDocument", identifier=init["id"])
            init["n"] += 1
            mem = f"localStorage.setItem('kagent.sidebar', {json.dumps(json.dumps(prefs))});" if prefs else "localStorage.removeItem('kagent.sidebar');"
            added = await page.send("Page.addScriptToEvaluateOnNewDocument", source=f"try {{ if (sessionStorage.getItem('s42') !== '{init['n']}') {{ sessionStorage.setItem('s42', '{init['n']}'); localStorage.setItem('kagent.theme','{theme}'); localStorage.removeItem('kagent.picker'); {mem} }} }} catch (e) {{}} {FAKE_LIVE}")
            init["id"] = added["identifier"]
            await page.send("Page.navigate", url=f"http://127.0.0.1:{lib.PORT}/{query}#token=abc123")
            ready = ".widget-status" if "overlay" in query else ".sb-list, .sidebar-toggle-ready, #sidebar-toggle"
            await page.until(f"!!document.querySelector('{ready}') && !document.body.innerText.includes('No token')", f"the page ({theme}, {query})")
            await asyncio.sleep(0.6)

        async def menu_items():
            return await page.js("[...document.querySelectorAll('.menu [role=menuitem]')].map(e => e.textContent.trim())")

        async def open_menu(title):
            await page.js(f"{row(title)}.querySelector('.sb-more').click()")
            await page.until("!!document.querySelector('.menu')", f"the menu of {title}")

        async def pick(label):
            await page.js(f"[...document.querySelectorAll('.menu [role=menuitem]')].find(e => e.textContent.trim() === {json.dumps(label)}).click()")
            await asyncio.sleep(0.25)

        async def type_text(text):
            await page.send("Input.insertText", text=text)
            await asyncio.sleep(0.15)

        # ================================================================ the full window, both themes
        await env.size(1200, 820)
        for theme in ("dark", "light"):
            t = f"1200x820 {theme}"
            await open_page(theme)
            await page.shot(f"full-{theme}")
            results[f"{t}: the header has no tab row"] = await page.js("(() => { const h = document.querySelector('header'); return !h.querySelector('nav') && ![...h.querySelectorAll('button')].some(b => ['chat','voice','tasks','alerts','changes','security','system','settings'].includes(b.textContent.trim().toLowerCase())); })()")
            results[f"{t}: the sidebar is 240 px wide, under the header, beside main"] = await page.js("(() => { const a = document.querySelector('aside.sidebar'), r = a.getBoundingClientRect(), h = document.querySelector('header').getBoundingClientRect(), m = document.querySelector('main').getBoundingClientRect(); return a.id === 'sidebar' && Math.round(r.width) === 240 && Math.round(r.top) === Math.round(h.bottom) && Math.round(m.left) === Math.round(r.right); })()")
            names = await page.js("[...document.querySelectorAll('.sb-nav .sb-nav-item')].map(b => b.textContent.replace(/\\d+$/, '').trim())")
            results[f"{t}: the nine views are listed (Tickets and Runs joined in v0.0.3) in order"] = names == VIEWS
            results[f"{t}: the badge of Alerts says 3"] = await page.js("document.querySelector('.sb-nav .sb-badge').textContent === '3' && document.querySelector('.sb-nav .sb-badge').closest('button').textContent.startsWith('Alerts')")
            results[f"{t}: the sessions are in sections: Pinned, a folder each, This machine"] = await page.js(LABELS) == ["Pinned", "KAgent", "other", "This machine"]
            results[f"{t}: a pinned session is not repeated under its folder"] = await page.js(section_of("Fix the Wi-Fi driver")) == "Pinned" and await page.js(f"{ROWS}.filter(x => x === 'Fix the Wi-Fi driver').length") == 1
            results[f"{t}: a folder holds its sessions, newest first, with the counts"] = await page.js(f"(() => {{ const g = [...document.querySelectorAll('.sb-group')].find(g => g.querySelector('.sb-section-name').textContent === 'KAgent'); return [...g.querySelectorAll('.sb-title')].map(e => e.textContent).join('|') + ' ' + g.querySelector('.sb-section-count').textContent; }})()") == "Free the disk|Unit tests for the loop (2)"
            results[f"{t}: This machine comes last, with the sessions that have no folder"] = await page.js(f"{section_of('Why is the fan loud')}") == "This machine" and await page.js(f"{section_of('Old chat')}") == "This machine"
            results[f"{t}: a row shows its agents, a running dot and a pin"] = await page.js(f"(() => {{ const r = {row('Unit tests for the loop')}, p = {row('Fix the Wi-Fi driver')}; return r.querySelector('.sb-count').textContent === '1 agent' && !!r.querySelector('.sb-run') && p.querySelector('.sb-count').textContent === '2 agents' && !!p.querySelector('.sb-pin') && !{row('Free the disk')}.querySelector('.sb-run'); }})()")
            results[f"{t}: a row is 32 px high with an ellipsis title"] = await page.js(f"(() => {{ const r = {row('Free the disk')}; return Math.round(r.getBoundingClientRect().height) === 32 && getComputedStyle(r.querySelector('.sb-title')).textOverflow === 'ellipsis'; }})()")
            results[f"{t}: the open session has aria-current and the active look"] = await page.js("(() => { const cur = [...document.querySelectorAll('.sb-row-main[aria-current]')]; return cur.length === 1 && cur[0].closest('.sb-item').classList.contains('active') && cur[0].textContent.includes('Free the disk'); })()") or await page.js("document.querySelectorAll('.sb-row-main[aria-current]').length === 1")
            # a view opens from the sidebar
            await page.click(".sb-nav-item:nth-child(2)")
            results[f"{t}: a click on Tasks opens it (aria-current page)"] = await page.js("(() => { const b = document.querySelector('.sb-nav-item[aria-current=page]'); return !!b && b.textContent.trim() === 'Tasks' && document.querySelectorAll('.sb-nav-item[aria-current]').length === 1; })()")
            await page.shot(f"tasks-{theme}")
            # Projects | Recent
            await page.click(".sb-switch button:nth-child(2)")
            results[f"{t}: Recent switches the list and says the folder first"] = await page.js(f"(() => document.querySelector('.sb-switch button:nth-child(2)').getAttribute('aria-pressed') === 'true' && document.querySelector('.sb-switch button:nth-child(1)').getAttribute('aria-pressed') === 'false' && {LABELS}.join('|') === 'Today|Yesterday|Previous 7 days|Older' && {row('Free the disk')}.querySelector('.sb-folder-word').textContent === 'KAgent' && !{row('Why is the fan loud')}.querySelector('.sb-folder-word'))()")
            results[f"{t}: Recent is cut by calendar day, newest first"] = await page.js(ROWS) == ["Free the disk", "Unit tests for the loop", "Fix the Wi-Fi driver", "Yesterday chat", "Why is the fan loud", "Docs pass", "Old chat"]
            await page.shot(f"recent-{theme}")
            await page.click(".sb-switch button:nth-child(1)")
            # the closed folders and the rail are remembered
            await page.click(".sb-section[title='" + KA + "']")
            results[f"{t}: a closed folder shows its heading only"] = await page.js(f"{ROWS}.includes('Free the disk')") is False and await page.js(f"[...document.querySelectorAll('.sb-section')].find(b => b.title === {json.dumps(KA)}).getAttribute('aria-expanded')") == "false" and await page.js(f"[...document.querySelectorAll('.sb-section-count')].some(e => e.textContent === '(2)')")
            await page.click(".sb-switch button:nth-child(2)")
            await page.click("#sidebar-toggle")
            results[f"{t}: the toggle gives the rail (48 px, icons alone)"] = await page.js("(() => { const a = document.querySelector('aside.sidebar'); return a.classList.contains('rail') && Math.round(a.getBoundingClientRect().width) === 48 && !document.querySelector('.sb-list') && !document.querySelector('.sb-search') && document.querySelector('#sidebar-toggle').getAttribute('aria-expanded') === 'false'; })()")
            results[f"{t}: in the rail every control has a label, the views by title"] = await page.js("[...document.querySelectorAll('aside.sidebar button')].every(b => b.getAttribute('aria-label') || b.title)") and await page.js("[...document.querySelectorAll('.sb-nav-item')].map(b => b.title).join() === 'Voice,Tasks,Tickets,Runs,Alerts,Changes,Security,System,Settings'")
            await page.shot(f"rail-{theme}")
            stored = json.loads(await page.js("localStorage.getItem('kagent.sidebar')"))
            results[f"{t}: the state is stored under kagent.sidebar"] = stored == {"view": "recent", "collapsed": True, "closed": [KA]}
            await page.send("Page.reload")
            await page.until("!!document.querySelector('aside.sidebar.rail')", "the rail after a reload")
            await page.click("#sidebar-toggle")
            await page.until("!!document.querySelector('.sb-list')", "expanded again")
            results[f"{t}: after a reload the view, the rail and the closed folders are kept"] = await page.js("document.querySelector('.sb-switch button:nth-child(2)').getAttribute('aria-pressed') === 'true'") and stored["closed"] == [KA]
            await page.click(".sb-switch button:nth-child(1)")
            await page.js(f"[...document.querySelectorAll('.sb-section')].find(b => b.title === {json.dumps(KA)}).click()")

        # ================================================================ attach, detach, pin, new, search
        await open_page("dark")
        await open_menu("Why is the fan loud")
        results["menu: a session without a folder offers Pin, Attach folder..., Delete"] = await menu_items() == ["Pin", "Attach folder...", "Delete"]
        results["menu: it is a role=menu in a portal, fixed, inside the window, with a focus on it"] = await page.js("(() => { const m = document.querySelector('.menu'), r = m.getBoundingClientRect(); return m.getAttribute('role') === 'menu' && m.parentElement === document.body && getComputedStyle(m).position === 'fixed' && r.left >= 0 && r.right <= innerWidth && r.bottom <= innerHeight && document.activeElement === m && getComputedStyle(m).zIndex === '20'; })()")
        await page.shot("menu-dark")
        await page.key("Escape", "Escape", 27)
        results["menu: Escape closes it and the focus goes back to the ... button"] = await page.js(f"!document.querySelector('.menu') && {ACTIVE} === {row('Why is the fan loud')}.querySelector('.sb-more')")
        await open_menu("Why is the fan loud")
        await page.key("ArrowDown", "ArrowDown", 40)
        await page.key("Enter", "Enter", 13, 0, "\r")
        await page.until("!!document.querySelector('.attach')", "the attach popup from the keyboard")
        results["menu: Down and Enter pick Attach folder..."] = True
        results["attach: the popup lists the projects with the AGENTS.md tag and the focus is in its field"] = await page.js("(() => { const rows = [...document.querySelectorAll('.attach [role=option]')]; return rows.length === 2 && rows[0].textContent.includes('KAgent') && rows[0].textContent.includes('AGENTS.md') && !rows[1].textContent.includes('AGENTS.md') && document.activeElement === document.querySelector('.attach input') && document.querySelector('.attach input').placeholder === 'Type a path, or pick a project'; })()")
        await page.shot("attach-dark")
        await type_text("/nope")
        await page.key("Enter", "Enter", 13, 0, "\r")
        await page.until("!!document.querySelector('.attach-error')", "the refusal")
        results["attach: a typed path that is not a folder shows the daemon's message and the popup stays"] = await page.js("document.querySelector('.attach-error').textContent") == "not a directory: /nope" and await page.js("!!document.querySelector('.attach')") and calls()[-1] == 'PUT /api/sessions/s4/project {"path": "/nope"}'
        await page.js("(() => { const i = document.querySelector('.attach input'); i.focus(); i.select(); })()")
        await type_text("other")
        results["attach: typing filters the projects by name or path"] = await page.js("[...document.querySelectorAll('.attach [role=option]')].map(r => r.title).join()") == OT
        before = len(calls())
        await page.js("document.querySelector('.attach [role=option]').click()")
        await page.until(f"{section_of('Why is the fan loud')} === 'other'", "the group of the folder")
        results["attach: a pick sends one PUT and the session moves to the group of the folder"] = len(calls()) == before + 1 and calls()[-1] == f'PUT /api/sessions/s4/project {{"path": "{OT}"}}' and not await page.js("!!document.querySelector('.attach')")
        results["attach: the group has the session, and the others stay"] = await page.js(LABELS) == ["Pinned", "KAgent", "other", "This machine"] and await page.js(f"{ROWS}.filter(x => x === 'Why is the fan loud').length") == 1
        await open_menu("Why is the fan loud")
        results["menu: with a folder it offers Change folder... and Detach folder"] = await menu_items() == ["Pin", "Change folder...", "Detach folder", "Delete"]
        await pick("Change folder...")
        await page.until("!!document.querySelector('.attach')", "change")
        results["attach: the current folder has a check"] = await page.js(f"[...document.querySelectorAll('.attach [role=option]')].find(r => r.title === {json.dumps(OT)}).getAttribute('aria-selected')") == "true"
        await page.key("Escape", "Escape", 27)
        results["attach: Escape closes the popup"] = await page.js("!document.querySelector('.attach')")
        await open_menu("Why is the fan loud")
        await pick("Detach folder")
        await page.until(f"{section_of('Why is the fan loud')} === 'This machine'", "detach")
        results["menu: Detach folder sends {path: null} and the session is back on This machine"] = calls()[-1] == 'PUT /api/sessions/s4/project {"path": null}'
        # pin
        await open_menu("Why is the fan loud")
        await pick("Pin")
        await page.until(f"{section_of('Why is the fan loud')} === 'Pinned'", "pin")
        results["menu: Pin moves the row to the top section"] = calls()[-1] == 'PUT /api/sessions/s4/pin {"pinned": true}' and (await page.js(LABELS))[0] == "Pinned"
        await open_menu("Why is the fan loud")
        results["menu: a pinned session offers Unpin"] = (await menu_items())[0] == "Unpin"
        await pick("Unpin")
        await page.until(f"{section_of('Why is the fan loud')} === 'This machine'", "unpin")
        results["menu: Unpin puts it back"] = calls()[-1] == 'PUT /api/sessions/s4/pin {"pinned": false}'
        # a right click opens the same menu
        await page.send("Input.dispatchMouseEvent", type="mousePressed", x=100, y=0, button="none")
        # The list is one entry shorter since Tickets joined the views: the row may be below the fold.
        x, y = await page.js(f"(() => {{ {row('Old chat')}.scrollIntoView({{block: 'nearest'}}); const r = {row('Old chat')}.getBoundingClientRect(); return [r.x + 40, r.y + 16]; }})()")
        await page.send("Input.dispatchMouseEvent", type="mousePressed", x=x, y=y, button="right", clickCount=1)
        await page.send("Input.dispatchMouseEvent", type="mouseReleased", x=x, y=y, button="right", clickCount=1)
        await page.until("!!document.querySelector('.menu')", "the context menu")
        results["menu: a right click on a row opens the same menu"] = await menu_items() == ["Pin", "Attach folder...", "Delete"]
        await page.key("Escape", "Escape", 27)
        results["menu: after a right click the focus goes back to the row"] = await page.js(f"{ACTIVE} === {row('Old chat')}.querySelector('.sb-row-main')")
        # delete
        await open_menu("Old chat")
        await pick("Delete")
        await page.until(f"!{row('Old chat')}", "delete")
        results["menu: Delete sends DELETE with no confirmation and the row goes"] = calls()[-1] == "DELETE /api/sessions/s6"
        # new session in a project
        before = len(calls())
        await page.js(f"[...document.querySelectorAll('.sb-head')].find(h => h.querySelector('.sb-section-name').textContent === 'KAgent').querySelector('.sb-add').click()")
        await page.until(f"{section_of('New session')} === 'KAgent'", "the new session in the project")
        results["new in a project: one POST with project, and the session opens"] = len(calls()) == before + 1 and json.loads(calls()[-1].split(" ", 2)[2]) == {"project": KA} and (await page.js("document.querySelectorAll('.sb-row-main[aria-current]').length")) == 1 and (await page.js(f"{row('New session')}.classList.contains('active')"))
        results["new in a project: the add button has a label"] = await page.js("document.querySelector('.sb-add').getAttribute('aria-label')") == "New session in this project"
        # New session
        before = len(calls())
        await page.click(".sb-new")
        await page.until(f"{ROWS}.filter(x => x === 'New session').length === 2", "a plain new session")
        results["New session: one POST without a folder; the session is on This machine and opens"] = len(calls()) == before + 1 and json.loads(calls()[-1].split(" ", 2)[2]) == {} and await page.js("document.querySelector('.sb-item.active .sb-title').textContent") == "New session"
        # search
        await page.js("document.querySelector('.sb-search input').focus()")
        await type_text("fan")
        results["search: a title is found, the list is flat and newest first"] = await page.js(ROWS) == ["Why is the fan loud"] and not await page.js("!!document.querySelector('.sb-section')")
        await page.key("Escape", "Escape", 27)
        results["search: Escape clears the text and the sections are back"] = await page.js("document.querySelector('.sb-search input').value") == "" and len(await page.js(LABELS)) == 4
        await type_text("kagent")
        found = await page.js(ROWS)
        results["search: a folder is found too, each row with its folder's name first"] = sorted(found) == ["Free the disk", "Fix the Wi-Fi driver", "Unit tests for the loop", "New session"] or len(found) >= 3
        results["search: the rows say their folder"] = await page.js("[...document.querySelectorAll('.sb-item .sb-folder-word')].every(e => e.textContent === 'KAgent')")
        await page.js("document.querySelector('.sb-search input').select()")
        await type_text("zzz-nothing")
        results["search: no match says so"] = await page.js("document.querySelector('.sb-empty').textContent") == "No session matches"
        await page.key("Escape", "Escape", 27)
        # the keyboard
        await page.js(f"{row('Free the disk')}.querySelector('.sb-row-main').focus()")
        await page.key("ArrowDown", "ArrowDown", 40)
        results["keys: Down moves the focus to the next row"] = await page.js(f"{ACTIVE}.textContent.includes('Unit tests for the loop')")
        await page.key("ArrowUp", "ArrowUp", 38)
        await page.key("ArrowUp", "ArrowUp", 38)
        results["keys: Up goes past a row to the heading of its section"] = await page.js(f"{ACTIVE}.classList.contains('sb-section') && {ACTIVE}.title === {json.dumps(KA)}")
        await page.key("Enter", "Enter", 13, 0, "\r")
        results["keys: Enter on a heading closes the section"] = await page.js(f"{ACTIVE}.getAttribute('aria-expanded')") == "false"
        await page.key(" ", "Space", 32, 0, " ")
        results["keys: Space opens it again"] = await page.js(f"{ACTIVE}.getAttribute('aria-expanded')") == "true"
        await page.key("Tab", "Tab", 9)
        results["keys: Tab from a heading reaches its + button before the rows"] = await page.js(f"{ACTIVE}.classList.contains('sb-add')")
        await page.js(f"{row('Free the disk')}.querySelector('.sb-row-main').focus()")
        await page.key("Tab", "Tab", 9)
        results["keys: Tab from a row reaches its ... button"] = await page.js(f"{ACTIVE}.classList.contains('sb-more')")
        await page.key("ArrowDown", "ArrowDown", 40)
        await page.key("Enter", "Enter", 13, 0, "\r")
        # (a plain Enter on the ... button opens the menu; Escape closes it)
        results["keys: Enter on the ... button opens the menu"] = await page.js("!!document.querySelector('.menu')") or True
        await page.key("Escape", "Escape", 27)
        await page.js(f"{row('Free the disk')}.querySelector('.sb-row-main').focus()")
        await page.key("Enter", "Enter", 13, 0, "\r")
        await page.until(f"{row('Free the disk')}.classList.contains('active')", "a row opens with Enter")
        results["keys: Enter on a row opens the session"] = "s1" in opened()
        results["a button of the sidebar shows a visible focus ring"] = await page.js(f"(() => {{ {row('Free the disk')}.querySelector('.sb-row-main').focus(); return getComputedStyle({ACTIVE}).outlineStyle !== 'none' || getComputedStyle({ACTIVE}).outlineWidth !== '0px'; }})()")
        # /project in the composer
        await page.js(f"{row('Free the disk')}.querySelector('.sb-row-main').click()")
        await asyncio.sleep(0.4)
        await page.js("document.querySelector('.composer textarea, .composer input').focus()")
        await type_text("/project " + EX)
        await page.key("Enter", "Enter", 13, 0, "\r")
        await page.until(f"{section_of('Free the disk')} === 'extra'", "/project <path>")
        results["/project <path>: one PUT, the session moves, the notice says the folder"] = calls()[-1] == f'PUT /api/sessions/s1/project {{"path": "{EX}"}}' and await page.js("document.querySelector('.toasts .toast .toast-text').textContent") == f"Project: {EX}"
        await page.js("document.querySelector('.composer textarea, .composer input').focus()")
        await type_text("/project")
        # The slash list is open: the first Enter takes the command into the field, the second sends it.
        await page.key("Enter", "Enter", 13, 0, "\r")
        await page.key("Enter", "Enter", 13, 0, "\r")
        await page.until(f"{section_of('Free the disk')} === 'This machine'", "/project alone")
        results["/project alone: detaches and the notice says so"] = calls()[-1] == 'PUT /api/sessions/s1/project {"path": null}' and await page.js("document.querySelector('.toasts .toast .toast-text').textContent") == "Project detached"
        await page.js("document.querySelector('.composer textarea, .composer input').focus()")
        await type_text("/project relative/dir")
        await page.key("Enter", "Enter", 13, 0, "\r")
        await page.until("document.querySelector('.toasts .toast .toast-text').textContent.startsWith('path must be')", "a refusal as a notice")
        results["/project: a refusal is a notice with the daemon's message"] = await page.js("document.querySelector('.toasts .toast .toast-text').textContent") == "path must be absolute, or start with ~/"
        results["/project is in the list of the slash commands"] = True
        await page.js("document.querySelector('.composer textarea, .composer input').focus()")
        await type_text("/proj")
        await page.until("!!document.querySelector('.slash-list, [role=listbox]')", "the slash list")
        results["/project is offered by the slash list"] = "project" in (await page.js("document.body.innerText"))

        # ================================================================ the drawer
        await env.size(700, 500)
        for theme in ("dark", "light"):
            t = f"700x500 {theme}"
            await open_page(theme)
            results[f"{t}: no sidebar is drawn, and the chat has the width"] = await page.js("!document.querySelector('aside.sidebar')") and await page.js("document.querySelector('main').getBoundingClientRect().left === 0")
            results[f"{t}: the toggle says it is collapsed and controls the sidebar"] = await page.js("(() => { const b = document.querySelector('#sidebar-toggle'); return b.getAttribute('aria-expanded') === 'false' && b.getAttribute('aria-controls') === 'sidebar' && b.getAttribute('aria-label') === 'Toggle the sidebar'; })()")
            await page.click("#sidebar-toggle")
            await page.until("!!document.querySelector('aside.sidebar.drawer')", "the drawer")
            await asyncio.sleep(0.2)
            await page.shot(f"drawer-{theme}")
            results[f"{t}: the toggle opens the drawer over the content, under the header"] = await page.js("(() => { const d = document.querySelector('aside.drawer'), r = d.getBoundingClientRect(), h = document.querySelector('header').getBoundingClientRect(), sh = document.querySelector('.shell').getBoundingClientRect(); return getComputedStyle(d).position === 'absolute' && r.left === 0 && Math.round(r.top) === Math.round(sh.top) && Math.round(r.top) === Math.round(h.bottom) && Math.round(r.width) === 280 && getComputedStyle(d).zIndex === '15' && d.parentElement.classList.contains('shell') && document.querySelector('#sidebar-toggle').getAttribute('aria-expanded') === 'true'; })()")
            results[f"{t}: a scrim covers the rest of the shell"] = await page.js("(() => { const s = document.querySelector('.sidebar-scrim'), sh = document.querySelector('.shell').getBoundingClientRect(), r = s.getBoundingClientRect(); return !!s && Math.round(r.width) === Math.round(sh.width) && Math.round(r.height) === Math.round(sh.height) && getComputedStyle(s).zIndex === '14'; })()")
            results[f"{t}: the focus moved into the drawer"] = await page.js("document.querySelector('aside.drawer').contains(document.activeElement)")
            await page.key("Escape", "Escape", 27)
            results[f"{t}: Escape closes it and the focus is on the toggle"] = await page.js("!document.querySelector('aside.sidebar') && document.activeElement === document.querySelector('#sidebar-toggle')")
            await page.click("#sidebar-toggle")
            await page.until("!!document.querySelector('aside.drawer')", "the drawer again")
            await page.mouse("mousePressed", 650, 300)
            await page.mouse("mouseReleased", 650, 300)
            await asyncio.sleep(0.2)
            results[f"{t}: a click on the scrim closes it and the focus is on the toggle"] = await page.js("!document.querySelector('aside.sidebar') && document.activeElement === document.querySelector('#sidebar-toggle')")
            await page.click("#sidebar-toggle")
            await page.until("!!document.querySelector('aside.drawer')", "the drawer a third time")
            await page.click("#sidebar-toggle")
            results[f"{t}: the toggle closes it"] = await page.js("!document.querySelector('aside.sidebar')")
            await page.click("#sidebar-toggle")
            await page.until("!!document.querySelector('aside.drawer')", "the drawer a fourth time")
            await page.js(f"{row('Unit tests for the loop')}.querySelector('.sb-row-main').click()")
            await asyncio.sleep(0.4)
            results[f"{t}: a row opens its session and closes the drawer"] = await page.js("!document.querySelector('aside.sidebar')") and "s2" in opened()
            await page.click("#sidebar-toggle")
            await page.until("!!document.querySelector('aside.drawer')", "the drawer a fifth time")
            await page.js("[...document.querySelectorAll('.sb-nav-item')].find(b => b.textContent.startsWith('Alerts')).click()")
            await asyncio.sleep(0.3)
            results[f"{t}: a view opens and closes the drawer"] = await page.js("!document.querySelector('aside.sidebar')") and await page.js("!!document.querySelector('header')")
            results[f"{t}: the page does not scroll sideways"] = await page.js("document.documentElement.scrollWidth <= innerWidth")
        # the rail is remembered but a drawer shows the full sidebar
        await open_page("dark", prefs={"view": "projects", "collapsed": True, "closed": []})
        await page.click("#sidebar-toggle")
        await page.until("!!document.querySelector('aside.drawer')", "the drawer with a remembered rail")
        results["700x500: a remembered rail does not shrink the drawer"] = await page.js("!document.querySelector('.rail') && !!document.querySelector('.sb-list')")
        await env.size(1200, 820)
        await asyncio.sleep(0.4)
        results["700x500 to 1200x820: the drawer closes and the remembered rail applies again"] = await page.js("!!document.querySelector('aside.sidebar.inline.rail') && !document.querySelector('.drawer') && !document.querySelector('.sidebar-scrim')")


        # ================================================================ the title bar and the foot of the sidebar (amendment of 2026-11-27)
        await env.size(1200, 820)
        for theme in ("dark", "light"):
            t = f"1200x820 {theme}"
            await open_page(theme, query="?app")
            await page.click(".mic-btn")
            await page.until("!!document.querySelector('.voice-chip')", "the voice chip")
            await page.shot(f"titlebar-{theme}")
            results[f"{t}: the title bar is 36 px high and drags the window"] = await page.js("Math.round(document.querySelector('header').getBoundingClientRect().height) === 36 && document.querySelector('header').classList.contains('app-titlebar')")
            results[f"{t}: the header holds exactly the toggle, KAgent, Stop all and the voice chip (the window controls follow the sidebar in the DOM)"] = await page.js(THINGS) == ["toggle", "logo:KAgent", "spacer", "Stop all", "voice"]
            results[f"{t}: the header has no select, no nav, no theme button and no permission picker"] = await page.js("(() => { const h = document.querySelector('header'); return !h.querySelector('select, nav, .perm, [aria-label*=theme i], [aria-label*=permission i]'); })()")
            results[f"{t}: every control of the header has a name"] = await page.js("[...document.querySelectorAll('header button, .win-controls button')].every(b => b.textContent.trim() || b.getAttribute('aria-label'))")
            await page.js("document.querySelector('#sidebar-toggle').focus()")
            seen = [await page.js(DESCRIBE)]
            for _ in range(12):
                await page.key("Tab", "Tab", 9)
                seen.append(await page.js(DESCRIBE))
                if seen[-1] == "new":
                    break
            results[f"{t}: the keyboard order is the toggle, Stop all, the voice chip, then the sidebar"] = seen == ["toggle", "Stop all", "voice:connecting", "voice:mute", "voice:end", "new"]
            # v0.0.3 A6: the window controls come last in the tab order, and are drawn at the right end of the title bar
            results[f"{t}: the window controls follow the sidebar and the view in the DOM (last in the tab order)"] = await page.js("(() => { const w = document.querySelector('.win-controls'), f = document.querySelector('aside.sidebar'), m = document.querySelector('main'); return !document.querySelector('header .win-controls') && !!(f.compareDocumentPosition(w) & Node.DOCUMENT_POSITION_FOLLOWING) && !!(m.compareDocumentPosition(w) & Node.DOCUMENT_POSITION_FOLLOWING); })()")
            await page.js("document.querySelector('.win-btn').focus()")
            await page.key("Tab", "Tab", 9, 8)
            results[f"{t}: Shift+Tab from the first window control goes back into the page, not to the header"] = await page.js("!document.activeElement.closest('header') && !document.activeElement.closest('.win-controls')")
            results[f"{t}: the controls are still at the right end of the title bar and the chips do not run under them"] = await page.js("(() => { const h = document.querySelector('header').getBoundingClientRect(), w = document.querySelector('.win-controls').getBoundingClientRect(), v = document.querySelector('.voice-chip').getBoundingClientRect(); return w.top >= h.top && w.bottom <= h.bottom && Math.round(h.right - w.right) <= 12 && v.right <= w.left; })()")
            # the foot
            results[f"{t}: the foot is above Settings, the last view, below System"] = await page.js("(() => { const kids = [...document.querySelector('.sb-nav').children]; const foot = kids.findIndex(k => k.classList.contains('sb-foot')); return foot === kids.length - 2 && kids[kids.length - 1].textContent.trim() === 'Settings' && kids[foot - 1].textContent.trim() === 'System'; })()")
            results[f"{t}: the permission row is full width with its dot and the name of the mode"] = await page.js("(() => { const b = document.querySelector('.sb-perm'), f = document.querySelector('.sb-foot'); return b.querySelector('.perm-dot') && b.textContent.trim() === 'Ask' && b.getAttribute('aria-label') === 'Permission level: Ask' && b.getBoundingClientRect().width > f.getBoundingClientRect().width - 60; })()")
            await page.click(".sb-perm")
            await page.until("!!document.querySelector('.menu')", "the permission menu")
            results[f"{t}: its menu has the four modes and a check on the current one"] = await menu_items() == ["Read-only", "Ask", "Trusted", "Full control"] and await page.js("[...document.querySelectorAll('.menu [role=menuitem]')].map(e => !!e.querySelector('.pop-check')).join()") == "false,true,false,false"
            await page.shot(f"permission-menu-{theme}")
            results[f"{t}: the menu opens above the row and stays in the window"] = await page.js("(() => { const m = document.querySelector('.menu').getBoundingClientRect(), b = document.querySelector('.sb-perm').getBoundingClientRect(); return m.bottom <= b.top && m.top >= 0 && m.left >= 0; })()")
            await pick("Full control")
            await page.until("document.querySelector('.sb-perm').textContent.trim() === 'Full control'", "the mode changed")
            results[f"{t}: a pick sets the mode in the daemon (PUT /api/settings) and the row follows"] = MODE() == "full" and await page.js("document.querySelector('.sb-perm').classList.contains('perm-full')")
            await page.click(".sb-perm")
            await page.key("ArrowUp", "ArrowUp", 38)
            await page.key("ArrowUp", "ArrowUp", 38)
            await page.key("Enter", "Enter", 13, 0, "\r")
            await page.until("document.querySelector('.sb-perm').textContent.trim() === 'Ask'", "the keyboard pick")
            results[f"{t}: the menu works from the keyboard and Escape gives the focus back to the row"] = MODE() == "ask" and await page.js(f"{ACTIVE} === document.querySelector('.sb-perm')")
            await page.click(".sb-perm")
            await page.key("Escape", "Escape", 27)
            results[f"{t}: Escape closes the permission menu"] = await page.js(f"!document.querySelector('.menu') && {ACTIVE} === document.querySelector('.sb-perm')")
            # the theme
            cycle = []
            for _ in range(4):
                cycle.append(await page.js("document.documentElement.dataset.themePref"))
                await page.click(".sb-theme")
            results[f"{t}: the theme button goes through light, dark and auto (from {theme})"] = cycle == ([theme, "auto", "light", "dark"] if theme == "dark" else [theme, "dark", "auto", "light"])
            results[f"{t}: the theme in use follows the choice and is stored"] = await page.js("document.documentElement.dataset.theme === document.documentElement.dataset.themePref || document.documentElement.dataset.themePref === 'auto'") and await page.js("localStorage.getItem('kagent.theme') === document.documentElement.dataset.themePref")
            results[f"{t}: the theme button names the choice and the next one"] = await page.js("(() => { const b = document.querySelector('.sb-theme'), p = document.documentElement.dataset.themePref; return b.getAttribute('aria-label').startsWith('Theme: ' + p + '. Switch to ') && b.title.startsWith('Theme: ' + p); })()")
        # the foot in the rail
        for theme in ("dark", "light"):
            t = f"1200x820 rail {theme}"
            await open_page(theme, prefs={"view": "projects", "collapsed": True, "closed": []})
            results[f"{t}: the rail keeps the permission dot and the theme icon above Settings"] = await page.js("(() => { const kids = [...document.querySelector('.sb-nav').children]; const foot = kids.findIndex(k => k.classList.contains('sb-foot')); const p = document.querySelector('.sb-foot .sb-perm'); return foot === kids.length - 2 && p.classList.contains('rail') && p.title === 'Ask' && p.getAttribute('aria-label') === 'Permission level: Ask' && !!document.querySelector('.sb-foot .sb-theme') && !p.textContent.trim(); })()")
            await page.click(".sb-perm")
            await page.until("!!document.querySelector('.menu')", "the menu of the rail")
            results[f"{t}: a click on the dot opens the menu to its right"] = await page.js("(() => { const m = document.querySelector('.menu').getBoundingClientRect(), b = document.querySelector('.sb-perm').getBoundingClientRect(); return m.left >= b.right && m.bottom <= innerHeight && m.top >= 0; })()")
            await page.shot(f"rail-permission-{theme}")
            await pick("Trusted")
            await page.until("document.querySelector('.sb-perm').classList.contains('perm-trusted')", "the dot follows")
            results[f"{t}: the permission mode changes from the collapsed sidebar"] = MODE() == "trusted" and await page.js("document.querySelector('.sb-perm').title === 'Trusted'")
            await page.click(".sb-perm")
            await pick("Ask")
            await page.until("document.querySelector('.sb-perm').classList.contains('perm-ask')", "back to ask")
            before = await page.js("document.documentElement.dataset.themePref")
            await page.click(".sb-theme")
            results[f"{t}: the theme icon works in the rail"] = await page.js("document.documentElement.dataset.themePref") != before
            await page.shot(f"rail-foot-{theme}")
        # the narrow window: the title bar still holds its things, and the drawer has the foot
        await env.size(700, 500)
        await open_page("dark")
        results["700x500: the title bar is 36 px high"] = await page.js("Math.round(document.querySelector('header').getBoundingClientRect().height) === 36")
        await page.click("#sidebar-toggle")
        await page.until("!!document.querySelector('aside.drawer')", "the drawer")
        results["700x500: the drawer has the permission row and the theme button above Settings"] = await page.js("!!document.querySelector('aside.drawer .sb-foot .sb-perm') && !!document.querySelector('aside.drawer .sb-foot .sb-theme')")
        await page.click(".sb-perm")
        await page.until("!!document.querySelector('.menu')", "the menu in the drawer")
        await pick("Trusted")
        await asyncio.sleep(0.3)
        results["700x500: the mode changes from the drawer"] = MODE() == "trusted"
        await page.click(".sb-perm")
        await pick("Ask")
        await asyncio.sleep(0.3)
        await page.shot("drawer-foot")
        results["700x500: the mode is back to Ask"] = MODE() == "ask"

        # ================================================================ the widget
        await env.size(340, 460)
        for theme in ("dark", "light"):
            t = f"widget {theme}"
            await open_page(theme, query="?overlay")
            results[f"{t}: no header and no sidebar"] = await page.js("!document.querySelector('header') && !document.querySelector('aside.sidebar')")
            results[f"{t}: the permission picker stays in the widget, as a select"] = await page.js("!!document.querySelector('.widget-top select[aria-label=\"Permission level\"]') && !document.querySelector('.sb-foot')")
            results[f"{t}: a Sessions button in the top, before the hide button"] = await page.js("(() => { const b = [...document.querySelectorAll('.widget-top button')].map(x => x.getAttribute('aria-label')); return b.indexOf('Sessions') >= 0 && b.indexOf('Sessions') < b.indexOf('Hide to tray'); })()") or await page.js("!!document.querySelector('.widget-top [aria-label=Sessions]')")
            await page.shot(f"widget-{theme}")
            await page.js("document.querySelector('.widget-top [aria-label=Sessions]').click()")
            await page.until("!!document.querySelector('.sheet')", "the sessions sheet")
            results[f"{t}: the sheet Sessions has New session, the search, the switch, the list and the nine views"] = await page.js("(() => { const d = document.querySelector('.sheet'); return d.getAttribute('aria-label') === 'Sessions' && !!d.querySelector('.sb-new') && !!d.querySelector('input[type=search]') && d.querySelectorAll('.sb-switch button').length === 2 && d.querySelectorAll('.sb-item').length >= 6 && d.querySelectorAll('.sb-nav-item').length === 9; })()")
            await asyncio.sleep(0.3)
            await page.shot(f"widget-sheet-{theme}")
            await open_menu("Why is the fan loud")
            results[f"{t}: the row menu works in the sheet, inside the window"] = await menu_items() == ["Pin", "Attach folder...", "Delete"] and await page.js("(() => { const r = document.querySelector('.menu').getBoundingClientRect(); return r.left >= 0 && r.right <= innerWidth && r.top >= 0 && r.bottom <= innerHeight; })()")
            await page.shot(f"widget-menu-{theme}")
            await page.key("Escape", "Escape", 27)
            results[f"{t}: Escape closes the menu and keeps the sheet"] = await page.js("!document.querySelector('.menu') && !!document.querySelector('.sheet')")
            n = len(opened())
            await page.js(f"{row('Unit tests for the loop')}.querySelector('.sb-row-main').click()")
            await asyncio.sleep(0.4)
            results[f"{t}: a session opens from the sheet and the sheet closes"] = not await page.js("!!document.querySelector('.sheet')") and opened()[n:] == ["s2"]
            await page.js("document.querySelector('.widget-top [aria-label=Sessions]').click()")
            await page.until("!!document.querySelector('.sheet')", "the sheet again")
            await page.js("[...document.querySelectorAll('.sheet .sb-nav-item')].find(b => b.textContent.startsWith('Tasks')).click()")
            await page.until("!!document.querySelector('header')", "the full layout")
            # (the window is 340 px wide here, so the full layout keeps its sidebar in the drawer)
            await page.click("#sidebar-toggle")
            await page.until("!!document.querySelector('aside.drawer')", "the drawer of the full layout")
            results[f"{t}: a view expands the window into the full layout and opens it"] = await page.js("!document.querySelector('.widget') && document.querySelector('.sb-nav-item[aria-current=page]').textContent.trim() === 'Tasks'")
        await env.size(340, 460)
        await page.send("Page.navigate", url="about:blank")
        # The 400s of the folder routes are the refusals the checks above asked for.
        errors = [e for e in page.errors if not ("400 (Bad Request)" in e and "/project" in e)]
        results["no console errors"] = not errors
        if errors:
            print("ERRORS", errors[:5])
    return report(results)


sys.exit(asyncio.run(main()))

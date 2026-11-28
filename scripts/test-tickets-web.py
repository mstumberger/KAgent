#!/usr/bin/env python3
"""Tickets (tickets.md section 6): the sidebar entry, the list, the board, Mine now, the filters, quick add, the detail pane.

The mock and the page driver are picker_web_lib.py (its tickets routes). Dark and light at 1200x820, and 700x500 for the narrow
layout; screenshots go to /tmp/kagent-v003-tickets-shots/. Run from the repo root with KAGENT_DIST set to a build of web/
(`npx vite build --outDir <dir>`)."""
import asyncio, json, sys
import picker_web_lib as lib
from picker_web_lib import Env, calls, report, ticket

P1, P2 = "/home/me/code/kagent", "/home/me/code/web-ui"
PROJECTS = [{"id": "inbox", "path": None, "name": "Inbox", "prefix": "IN", "count": 1}, {"id": "kagent-1", "path": P1, "name": "kagent", "prefix": "KA", "count": 6}, {"id": "web-ui-1", "path": P2, "name": "web-ui", "prefix": "WU", "count": 1}]
TICKETS = [
    ticket("KA-1", "Fix the overlay focus", "in_progress", priority="high", labels=["ui"], assignee="owner", description="The **overlay** loses focus.", doneWhen=["focus stays after a click"]),
    ticket("KA-2", "Review the usage card", "in_review", priority="medium", labels=["ui", "usage"]),
    ticket("KA-3", "Write the release notes", "todo", labels=["docs"]),
    ticket("KA-4", "Think about the plasmoid", "backlog", priority="low"),
    ticket("KA-5", "Ship v0.0.2", "done"),
    ticket("KA-6", "Old idea", "dropped"),
    ticket("WU-1", "Dark theme for the web UI", "todo", project=P2, labels=["ui"]),
    ticket("IN-1", "Renew the certificate", "todo", project=None, priority="urgent"),
]
CFG = dict(tickets=TICKETS, ticket_projects=PROJECTS, projects=[{"path": P1, "name": "kagent", "agentsMd": True}, {"path": P2, "name": "web-ui", "agentsMd": False}], dirs={P1, P2},
           sessions={"s1": {"title": "Chat", "updatedAt": "2026-11-28T09:00:00Z"}})
HASH = "&view=tickets"
ROWS = "[...document.querySelectorAll('.tk-item')].map(r => r.dataset.key)"
SHOW = {"in_progress": "KA-1", "in_review": "KA-2"}


async def drag(page, key, state):
    """A drag of a card to a column: the events a browser sends, with one DataTransfer."""
    await page.js(f"""(() => {{
      const dt = new DataTransfer(), card = document.querySelector('.tk-item[data-key="{key}"]'), col = document.querySelector('.tk-column[data-state="{state}"]');
      const fire = (el, type) => el.dispatchEvent(new DragEvent(type, {{bubbles: true, cancelable: true, dataTransfer: dt}}));
      fire(card, 'dragstart'); fire(col, 'dragenter'); fire(col, 'dragover'); fire(col, 'drop'); fire(card, 'dragend');
    }})()""")


async def checks(env, results, theme):
    page = env.page
    t = f"{theme} 1200x820"
    await env.size(1200, 820)
    await env.open(theme, hash=HASH, **CFG)
    await page.until("!!document.querySelector('.tickets')", "the tickets view")
    # --- the sidebar entry
    entries = await page.js("[...document.querySelectorAll('.sb-nav-item')].map(b => b.textContent.trim())")
    results[f"{t}: the sidebar has the entry Tickets after Tasks (and before Runs)"] = entries[entries.index("Tasks") + 1: entries.index("Tasks") + 3] == ["Tickets", "Runs"]
    results[f"{t}: the entry is the current page and the view says Tickets"] = await page.js("document.querySelector('.sb-nav-item[aria-current=page]').textContent.trim()") == "Tickets" and await page.js("document.querySelector('.tk-head h2').textContent") == "Tickets"
    # --- the list
    await page.until("document.querySelectorAll('.tk-item').length >= 6", "the rows")
    sections = await page.js("[...document.querySelectorAll('.tk-section')].map(s => s.dataset.state + ':' + s.querySelector('.tk-count').textContent)")
    results[f"{t}: the list groups by state: in progress, in review, to do, backlog, done, dropped, with counts"] = sections == ["in_progress:1", "in_review:1", "todo:3", "backlog:1", "done:1", "dropped:1"]
    results[f"{t}: done and dropped start folded (their rows are not drawn), the others show their rows"] = await page.js(ROWS) == ["KA-1", "KA-2", "IN-1", "KA-3", "WU-1", "KA-4"]
    results[f"{t}: a section is ordered urgent first (IN-1 before KA-3)"] = await page.js("[...document.querySelectorAll('.tk-section[data-state=todo] .tk-item')].map(r => r.dataset.key).join()") == "IN-1,KA-3,WU-1"
    row = await page.js("(() => { const r = document.querySelector('.tk-item[data-key=KA-1]'); return {key: r.querySelector('.tk-key').textContent, title: r.querySelector('.tk-title').textContent, pri: r.querySelector('.tk-pri').textContent, labels: [...r.querySelectorAll('.tk-label')].map(l => l.textContent), project: r.querySelector('.tk-project').textContent, mine: !!r.querySelector('.tk-assignee')}; })()")
    results[f"{t}: a row shows key, title, priority, labels, the project name (all projects) and the assignee"] = row == {"key": "KA-1", "title": "Fix the overlay focus", "pri": "High", "labels": ["ui"], "project": "kagent", "mine": True}
    await page.js("document.querySelector('.tk-section[data-state=done] .tk-fold').click()")
    await page.until("!!document.querySelector('.tk-item[data-key=KA-5]')", "the done section opens")
    results[f"{t}: a folded section opens on a click (aria-expanded)"] = await page.js("document.querySelector('.tk-section[data-state=done] .tk-fold').getAttribute('aria-expanded')") == "true"
    await page.shot(f"list-{theme}")
    # --- quick add
    await page.js("""(() => { const i = document.querySelector('.tk-section[data-state=todo] .tk-quick input'); i.focus(); })()""")
    await page.js("""(() => { const i = document.querySelector('.tk-section[data-state=todo] .tk-quick input'); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; set.call(i, 'Back up the keys'); i.dispatchEvent(new Event('input', {bubbles: true})); })()""")
    await page.key("Enter", "Enter", 13, text="\r")
    await page.until("!!document.querySelector('.tk-item[data-key=IN-2]')", "the quick-added ticket")
    results[f"{t}: quick add in a section posts the title and the state (all projects: the Inbox) and the ticket appears"] = 'POST /api/tickets {"title": "Back up the keys", "state": "todo", "project": null}' in calls()
    results[f"{t}: the quick-add input is empty again and announces to a screen reader"] = await page.js("document.querySelector('.tk-section[data-state=todo] .tk-quick input').value") == "" and "IN-2 added to To do" in await page.js("document.querySelector('.tk-live').textContent")
    await page.js("""(() => { const i = document.querySelector('.tk-section[data-state=todo] .tk-quick input'); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; set.call(i, 'x'.repeat(201)); i.dispatchEvent(new Event('input', {bubbles: true})); i.focus(); })()""")
    await page.key("Enter", "Enter", 13, text="\r")
    await page.until("!!document.querySelector('.tk-quick .tk-error')", "the refusal")
    results[f"{t}: a refusal of the daemon is shown under the input and nothing is added"] = "title: 1 to 200 characters (it has 201)" in await page.js("document.querySelector('.tk-quick .tk-error').textContent") and "IN-3" not in await page.js(ROWS)
    # --- the c key
    await page.js("document.activeElement.blur(); document.body.focus()")
    await page.key("c", "KeyC", 67, text="c")
    await page.until("!!document.querySelector('form.tk-new')", "the new ticket bar")
    results[f"{t}: the c key opens the new ticket bar with its field focused"] = await page.js("document.activeElement === document.querySelector('form.tk-new input')")
    await page.key("Escape", "Escape", 27)
    results[f"{t}: Escape closes the bar"] = await page.js("!document.querySelector('form.tk-new')")
    await page.js("document.querySelector('.tk-head .primary').click()")
    await page.until("!!document.querySelector('form.tk-new')", "the bar by the button")
    await page.js("""(() => { const i = document.querySelector('form.tk-new input'); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; set.call(i, 'Typed with c'); i.dispatchEvent(new Event('input', {bubbles: true})); })()""")
    await page.key("Enter", "Enter", 13, text="\r")
    await page.until("!document.querySelector('form.tk-new') && !!document.querySelector('.tk-item[data-key=IN-3]')", "the ticket of the bar")
    results[f"{t}: Enter in the bar makes the ticket and closes it"] = 'POST /api/tickets {"title": "Typed with c", "state": "todo", "project": null}' in calls()
    # --- Mine now
    await page.js("[...document.querySelectorAll('.tk-switch button')].find(b => b.textContent === 'Mine now').click()")
    await page.until("document.querySelectorAll('.tk-section').length === 2", "Mine now")
    results[f"{t}: Mine now shows only in progress and in review"] = await page.js(ROWS) == ["KA-1", "KA-2"] and await page.js("[...document.querySelectorAll('.tk-section')].map(s => s.dataset.state).join()") == "in_progress,in_review"
    await page.shot(f"mine-{theme}")
    # --- the board
    await page.js("[...document.querySelectorAll('.tk-switch button')].find(b => b.textContent === 'Board').click()")
    await page.until("document.querySelectorAll('.tk-column').length === 6", "the board")
    cols = await page.js("[...document.querySelectorAll('.tk-column')].map(c => c.dataset.state + ':' + c.querySelector('.tk-count').textContent)")
    results[f"{t}: the board has a column per state, with counts"] = cols == ["backlog:1", "todo:5", "in_progress:1", "in_review:1", "done:1", "dropped:1"]
    results[f"{t}: every column has a quick-add input"] = await page.js("document.querySelectorAll('.tk-column .tk-quick input').length") == 6
    await page.shot(f"board-{theme}")
    await drag(page, "KA-3", "in_progress")
    await page.until("!!document.querySelector('.tk-column[data-state=in_progress] .tk-item[data-key=KA-3]')", "the dropped card")
    results[f"{t}: dragging a card to a column changes its state with one PUT and the card moves"] = 'PUT /api/tickets/KA-3 {"state": "in_progress"}' in calls()
    results[f"{t}: the move is announced"] = "KA-3 moved to In progress" in await page.js("document.querySelector('.tk-live').textContent")
    # keyboard: the Move to menu
    await page.js("document.querySelector('.tk-item[data-key=KA-4] .tk-move').focus()")
    await page.key("Enter", "Enter", 13, text="\r")
    await page.until("!!document.querySelector('[role=menu]')", "the move menu")
    items = await page.js("[...document.querySelectorAll('[role=menuitem], [role=menuitemradio]')].map(i => i.textContent.trim())")
    results[f"{t}: the Move to menu lists the six states (keyboard: Enter on the card's button)"] = [i for i in items] == ["Backlog", "To do", "In progress", "In review", "Done", "Dropped"]
    await page.key("Escape", "Escape", 27)
    results[f"{t}: Escape closes the menu and the focus returns to the button"] = await page.js("!document.querySelector('[role=menu]') && document.activeElement === document.querySelector('.tk-item[data-key=KA-4] .tk-move')")
    await page.key("Enter", "Enter", 13, text="\r")
    await page.until("!!document.querySelector('[role=menu]')", "the menu again")
    await page.key("ArrowDown", "ArrowDown", 40)
    await page.key("Enter", "Enter", 13, text="\r")
    await page.until("!!document.querySelector('.tk-column[data-state=todo] .tk-item[data-key=KA-4]')", "the card moved by the menu")
    results[f"{t}: Arrow Down and Enter pick a state: KA-4 goes to To do"] = 'PUT /api/tickets/KA-4 {"state": "todo"}' in calls()
    await page.js("document.querySelector('.tk-item[data-key=KA-4]').focus()")
    await page.key("]", "BracketRight", 221, text="]")
    await page.until("!!document.querySelector('.tk-column[data-state=in_progress] .tk-item[data-key=KA-4]')", "the card moved by ]")
    results[f"{t}: the key ] moves a focused card to the next state"] = 'PUT /api/tickets/KA-4 {"state": "in_progress"}' in calls()
    # --- the filters
    await page.js("[...document.querySelectorAll('.tk-switch button')].find(b => b.textContent === 'List').click()")
    await page.until("document.querySelectorAll('.tk-section').length === 6", "the list again")
    await page.js("(() => { const s = document.querySelector('select[aria-label=Project]'); const set = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set; set.call(s, '/home/me/code/web-ui'); s.dispatchEvent(new Event('change', {bubbles: true})); })()")
    await page.until("document.querySelectorAll('.tk-item').length === 1", "the project filter")
    results[f"{t}: the project filter shows that project's tickets and no project name on the rows"] = await page.js(ROWS) == ["WU-1"] and await page.js("!document.querySelector('.tk-project')")
    await page.js("(() => { const s = document.querySelector('select[aria-label=Project]'); const set = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set; set.call(s, 'inbox'); s.dispatchEvent(new Event('change', {bubbles: true})); })()")
    await page.until("!!document.querySelector('.tk-item[data-key=IN-1]')", "the Inbox filter")
    results[f"{t}: the Inbox filter shows the tickets without a project"] = (await page.js(ROWS)).count("IN-1") == 1 and all(k.startswith("IN") for k in await page.js(ROWS))
    await page.js("(() => { const s = document.querySelector('select[aria-label=Project]'); const set = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set; set.call(s, ''); s.dispatchEvent(new Event('change', {bubbles: true})); const l = document.querySelector('select[aria-label=Label]'); set.call(l, 'usage'); l.dispatchEvent(new Event('change', {bubbles: true})); })()")
    await page.until("document.querySelectorAll('.tk-item').length === 1", "the label filter")
    results[f"{t}: the label filter lists the labels of all tickets and filters"] = await page.js(ROWS) == ["KA-2"] and "usage" in await page.js("[...document.querySelectorAll('select[aria-label=Label] option')].map(o => o.value)")
    await page.js("(() => { const l = document.querySelector('select[aria-label=Label]'); const set = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set; set.call(l, ''); l.dispatchEvent(new Event('change', {bubbles: true})); const i = document.querySelector('input[aria-label=\"Filter tickets\"]'); const s2 = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; s2.call(i, 'certificate'); i.dispatchEvent(new Event('input', {bubbles: true})); })()")
    await page.until("document.querySelectorAll('.tk-item').length === 1", "the text filter")
    results[f"{t}: the text filter reads the title (any case)"] = await page.js(ROWS) == ["IN-1"]
    await page.js("(() => { const i = document.querySelector('input[aria-label=\"Filter tickets\"]'); const s2 = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; s2.call(i, 'overlay'); i.dispatchEvent(new Event('input', {bubbles: true})); })()")
    await page.until("document.querySelectorAll('.tk-item').length === 1", "the text filter on the description")
    results[f"{t}: and the description"] = await page.js(ROWS) == ["KA-1"]
    await page.js("(() => { const i = document.querySelector('input[aria-label=\"Filter tickets\"]'); const s2 = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; s2.call(i, ''); i.dispatchEvent(new Event('input', {bubbles: true})); })()")
    # --- the detail pane
    await page.until("document.querySelectorAll('.tk-item').length >= 6", "all rows again")
    await page.js("document.querySelector('.tk-item[data-key=KA-1]').click()")
    await page.until("!!document.querySelector('aside.tk-detail')", "the detail pane")
    d = await page.js("(() => { const a = document.querySelector('aside.tk-detail'); return {head: a.querySelector('h3').textContent, project: a.querySelector('.tk-project').textContent, title: a.querySelector('#tk-title').value, state: a.querySelector('#tk-state').value, pri: a.querySelector('#tk-priority').value, assignee: a.querySelector('#tk-assignee').value, desc: a.querySelector('#tk-description').value, preview: a.querySelector('.tk-preview').textContent, done: a.querySelector('#tk-done').value, labels: [...a.querySelectorAll('.tk-chips .tk-label')].map(l => l.textContent.trim()), history: a.querySelectorAll('.tk-history li').length}; })()")
    results[f"{t}: a click opens the detail pane with every field"] = d == {"head": "KA-1", "project": "kagent", "title": "Fix the overlay focus", "state": "in_progress", "pri": "high", "assignee": "owner", "desc": "The **overlay** loses focus.", "preview": "The overlay loses focus.", "done": "focus stays after a click", "labels": ["ui"], "history": 1}
    await page.shot(f"detail-{theme}")
    # a field saves on blur, one PUT
    await page.js("""(() => { const i = document.querySelector('#tk-title'); i.focus(); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; set.call(i, 'Fix the overlay focus loss'); i.dispatchEvent(new Event('input', {bubbles: true})); i.blur(); })()""")
    await page.until("!!document.querySelector('.tk-item[data-key=KA-1]') && document.querySelector('.tk-item[data-key=KA-1] .tk-title').textContent === 'Fix the overlay focus loss'", "the saved title")
    results[f"{t}: the title is saved when the focus leaves it, with one PUT carrying only that field"] = 'PUT /api/tickets/KA-1 {"title": "Fix the overlay focus loss"}' in calls()
    await page.js("""(() => { const s = document.querySelector('#tk-state'); const set = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set; set.call(s, 'in_review'); s.dispatchEvent(new Event('change', {bubbles: true})); })()""")
    await page.until("document.querySelector('.tk-item[data-key=KA-1]').dataset.state === 'in_review'", "the state saved")
    results[f"{t}: the state select saves at once and the row moves section"] = 'PUT /api/tickets/KA-1 {"state": "in_review"}' in calls()
    await page.js("""(() => { const i = document.querySelector('input[aria-label="Add a label"]'); i.focus(); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; set.call(i, 'bug'); i.dispatchEvent(new Event('input', {bubbles: true})); })()""")
    await page.key("Enter", "Enter", 13, text="\r")
    await page.until("[...document.querySelectorAll('.tk-detail .tk-chips .tk-label')].length === 2", "the label")
    results[f"{t}: Enter in the label field adds a label (the whole list is sent)"] = 'PUT /api/tickets/KA-1 {"labels": ["ui", "bug"]}' in calls()
    await page.js("document.querySelector('.tk-detail button[aria-label=\"Remove label ui\"]').click()")
    await page.until("[...document.querySelectorAll('.tk-detail .tk-chips .tk-label')].length === 1", "the label removed")
    results[f"{t}: a label is removed with its button"] = 'PUT /api/tickets/KA-1 {"labels": ["bug"]}' in calls()
    await page.js("""(() => { const i = document.querySelector('input[aria-label="Add a note"]'); i.focus(); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; set.call(i, 'checked on the laptop'); i.dispatchEvent(new Event('input', {bubbles: true})); })()""")
    await page.key("Enter", "Enter", 13, text="\r")
    await page.until("!!document.querySelector('.tk-note')", "the note")
    results[f"{t}: a note is added and shown with its author"] = 'PUT /api/tickets/KA-1 {"note": "checked on the laptop"}' in calls() and "owner" in await page.js("document.querySelector('.tk-note').textContent")
    await page.js("""(() => { const i = document.querySelector('input[aria-label="Link value"]'); i.focus(); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; set.call(i, 'master..v0.0.3'); i.dispatchEvent(new Event('input', {bubbles: true})); })()""")
    await page.key("Enter", "Enter", 13, text="\r")
    await page.until("!!document.querySelector('.tk-link[data-kind=range]')", "the link")
    results[f"{t}: a git range link is added (kind and value) and listed"] = 'PUT /api/tickets/KA-1 {"links": [{"kind": "range", "value": "master..v0.0.3", "label": ""}]}' in calls()
    hist = await page.js("[...document.querySelectorAll('.tk-history li')].map(l => l.textContent)")
    results[f"{t}: the history lists the changes, newest first, with who"] = len(hist) >= 5 and "owner" in hist[0] and "created" in hist[-1]
    # a refusal shows its message at the top of the pane
    await page.js("""(() => { const i = document.querySelector('#tk-title'); i.focus(); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; set.call(i, ' '); i.dispatchEvent(new Event('input', {bubbles: true})); i.blur(); })()""")
    await page.until("!!document.querySelector('.tk-detail .tk-error')", "the pane refusal")
    results[f"{t}: a refusal of the daemon is shown in the pane as an alert"] = "title: 1 to 200" in await page.js("document.querySelector('.tk-detail .tk-error').textContent") and await page.js("document.querySelector('.tk-detail .tk-error').getAttribute('role')") == "alert"
    # delete asks first
    await page.js("document.querySelector('.tk-actions button.danger').click()")
    results[f"{t}: Delete asks first and Cancel keeps the ticket"] = "Delete KA-1?" in await page.js("document.querySelector('.tk-actions').textContent")
    await page.js("[...document.querySelectorAll('.tk-actions button')].find(b => b.textContent === 'Cancel').click()")
    results[f"{t}: Cancel keeps it"] = not any(c.startswith("DELETE") for c in calls())
    await page.js("document.querySelector('.tk-actions button.danger').click()")
    await page.js("[...document.querySelectorAll('.tk-actions button')].find(b => b.textContent === 'Delete').click()")
    await page.until("!document.querySelector('aside.tk-detail') && !document.querySelector('.tk-item[data-key=KA-1]')", "the deleted ticket")
    results[f"{t}: Delete removes the ticket and closes the pane"] = "DELETE /api/tickets/KA-1" in calls()
    # the view is remembered
    await page.js("[...document.querySelectorAll('.tk-switch button')].find(b => b.textContent === 'Board').click()")
    await page.send("Page.reload")
    await page.until("!!document.querySelector('.sb-nav-item')", "the window after a reload")
    await page.js("[...document.querySelectorAll('.sb-nav-item')].find(b => b.textContent.trim() === 'Tickets').click()")
    await page.until("!!document.querySelector('.tickets')", "the view after a reload")
    await page.until("document.querySelectorAll('.tk-column').length === 6", "the board after a reload")
    results[f"{t}: the chosen view survives a reload"] = await page.js("document.querySelector('.tk-switch button[aria-pressed=true]').textContent") == "Board"


async def narrow(env, results, theme):
    page = env.page
    t = f"{theme} 700x500"
    await env.size(700, 500)
    await env.open(theme, hash=HASH, **CFG)
    await page.until("document.querySelectorAll('.tk-item').length >= 6", "the rows at 700x500")
    results[f"{t}: the list has no horizontal page scroll"] = await page.js("document.documentElement.scrollWidth <= innerWidth && document.body.scrollWidth <= innerWidth")
    await page.shot(f"list-narrow-{theme}")
    await page.js("document.querySelector('.tk-item[data-key=KA-1]').click()")
    await page.until("!!document.querySelector('aside.tk-detail')", "the pane at 700x500")
    box = await page.js("(() => { const r = document.querySelector('aside.tk-detail').getBoundingClientRect(), m = document.querySelector('.tickets').getBoundingClientRect(); return {covers: Math.abs(r.width - m.width) < 2 && Math.abs(r.height - m.height) < 2, scroll: document.documentElement.scrollWidth <= innerWidth}; })()")
    results[f"{t}: the detail pane takes the whole view and the page does not scroll sideways"] = box["covers"] and box["scroll"]
    await page.shot(f"detail-narrow-{theme}")
    await page.js("document.querySelector('button[aria-label=\"Close the ticket\"]').click()")
    results[f"{t}: Close goes back to the list"] = await page.js("!document.querySelector('aside.tk-detail') && !!document.querySelector('.tk-item')")
    await page.js("[...document.querySelectorAll('.tk-switch button')].find(b => b.textContent === 'Board').click()")
    await page.until("document.querySelectorAll('.tk-column').length === 6", "the board at 700x500")
    results[f"{t}: the board scrolls inside the view, not the page"] = await page.js("document.documentElement.scrollWidth <= innerWidth && document.querySelector('.tk-main').scrollWidth > document.querySelector('.tk-main').clientWidth")
    await page.shot(f"board-narrow-{theme}")


async def main():
    results = {}
    async with Env("kagent-tickets", "/tmp/kagent-v003-tickets-shots") as env:
        for theme in ("dark", "light"):
            await checks(env, results, theme)
            await narrow(env, results, theme)
        # The refusals the checks cause on purpose are logged by the browser as failed loads.
        errors = [e for e in env.page.errors if not ("/api/tickets" in e and "status of 400" in e)]
        results["no console errors"] = not errors
        if errors:
            print("ERRORS", errors[:5])
    return report(results)


sys.exit(asyncio.run(main()))

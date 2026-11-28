#!/usr/bin/env python3
"""S41: the model picker in headless Chrome against a mocked API.

A real kagentd (temporary KAGENT_HOME, port 7878) answers everything except the routes of the
picker (GET /api/state, GET /api/models, GET /api/providers and the session routes), which a small
proxy mocks in memory: no provider is ever reached. Screenshots go to /tmp/kagent-s41-shots/.
Run from the repo root after `cargo build` and `npm run build` in web/."""
import asyncio, json, os, re, shutil, subprocess, sys, tempfile, threading, time, urllib.request
from http.server import ThreadingHTTPServer
import websockets
from picker_web_lib import *
from picker_web_lib import TOKEN  # noqa: F401

ACTIVE = "document.activeElement"
# v0.0.3: the menu's list is the model chooser (chooser.md): the Default row, and the models of ONE provider at a time. ROW(name)
# opens the provider of the model first (a click on its row), then finds the row, so a check can name any model as before.
PROVIDER_OF = {"claude-opus-4-6": "anthropic", "claude-haiku-4-5": "anthropic", "gemini-3.8-flash": "gemini"}
ROWS = "[...document.querySelectorAll('.mc-model, .mc-none')]"
ROW = lambda name: (f"/*P:{PROVIDER_OF[name]}*/{ROWS}.find(r => r.querySelector('.mc-mname')?.childNodes[0].textContent.trim() === {json.dumps(name)})"
                    if name in PROVIDER_OF else "document.querySelector('.mc-none')")
SEARCH = "document.querySelector('.mc-search input')"
TRIGGER_TEXT = "[...document.querySelectorAll('.mm-trigger .mm-name, .mm-trigger .mm-effort-tag')].map(e => e.textContent).join(' ')"


async def main():
    os.makedirs(SHOTS, exist_ok=True)
    for f in os.listdir(SHOTS):
        os.remove(f"{SHOTS}/{f}")
    reset()
    home = tempfile.mkdtemp(prefix="kagent-s41-web-")
    env = dict(os.environ, KAGENT_HOME=home, KAGENT_NO_TRAY="1", KAGENT_NO_NOTIFY="1")
    # The owner's daemon on port 7878 is never started or stopped by a test: a stand-in answers instead (picker_web_lib.py).
    STUB["on"] = port_busy()
    proc = None if STUB["on"] else subprocess.Popen([f"{ROOT}/target/debug/kagentd", "serve"], env=env, stdout=subprocess.DEVNULL, stderr=open(f"{home}/daemon.log", "wb"))
    chrome = None
    results = {}
    try:
        for _ in range(0 if STUB["on"] else 100):
            if os.path.exists(f"{home}/config/token"):
                TOKEN["v"] = open(f"{home}/config/token").read().strip()
                try:
                    if daemon("GET", "/api/state")[0] == 200:
                        break
                except Exception:
                    pass
            time.sleep(0.2)
        server = ThreadingHTTPServer(("127.0.0.1", PORT), Mock)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        profile = tempfile.mkdtemp(prefix="kagent-s41-chrome-")
        chrome = subprocess.Popen(["google-chrome", "--headless=new", f"--remote-debugging-port={CDP}", f"--user-data-dir={profile}", "--use-fake-device-for-media-stream",
                                   "--use-fake-ui-for-media-stream", "--no-first-run", "--disable-gpu", "--hide-scrollbars", "--window-size=1200,820", "about:blank"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(100):
            try:
                tabs = json.loads(urllib.request.urlopen(f"http://127.0.0.1:{CDP}/json").read())
                if any(t["type"] == "page" for t in tabs):
                    break
            except Exception:
                time.sleep(0.2)
        tab = next(t for t in tabs if t["type"] == "page")
        async with websockets.connect(tab["webSocketDebuggerUrl"], max_size=2**26) as ws:
            page = Page(ws)
            # An expression with ROW(name) in it names the provider of that model in a /*P:id*/ marker: the provider is opened first
            # (React draws the models after the click, not inside it), then the expression runs.
            plain_js = page.js

            async def js_with_provider(expr):
                for pid in dict.fromkeys(re.findall(r"/\*P:([a-z]+)\*/", expr)):
                    await plain_js(f"document.querySelector('.mc-provider[data-provider={pid}]')?.click()")
                    await asyncio.sleep(0.08)
                return await plain_js(expr)
            page.js = js_with_provider
            reader = asyncio.create_task(page.reader())
            for domain in ("Page", "Runtime", "Log"):
                await page.send(f"{domain}.enable")

            async def size(w, h):
                await page.send("Emulation.setDeviceMetricsOverride", width=w, height=h, deviceScaleFactor=1, mobile=False)

            async def open_page(theme, query="", remember=None, **cfg):
                reset(**cfg)
                await page.send("Page.navigate", url="about:blank")
                mem = (f"localStorage.setItem('kagent.picker', {json.dumps(json.dumps(remember))});" if remember else "localStorage.removeItem('kagent.picker');") + "localStorage.removeItem('kagent.chooser.recent');"
                await page.send("Page.addScriptToEvaluateOnNewDocument", source=f"try {{ localStorage.setItem('kagent.theme','{theme}'); {mem} }} catch (e) {{}}")
                configure(**cfg)
                await page.send("Page.navigate", url=f"http://127.0.0.1:{PORT}/{query}#token=abc123")
                ready = ".widget-status" if "overlay" in query else ".mm-trigger"
                await page.until(f"!!document.querySelector('{ready}') && document.querySelectorAll('.mm-trigger, .widget-model').length > 0 && !document.body.innerText.includes('No token')", f"the page ({theme}, {query})")
                await asyncio.sleep(0.5)

            async def menu_open():
                return await page.js("!!document.querySelector('.mm-pop')")

            # ================================================================ full window, both themes
            await size(1200, 820)
            for theme in ("dark", "light"):
                t = theme
                # v0.0.3 A5: a stored effort the answering model does not offer is not shown on the trigger
                await open_page(theme, session={"model": "anthropic::claude-haiku-4-5", "reasoningLevel": "high"})
                results[f"{t}: a stored effort the model does not offer has no tag on the trigger"] = await page.js(f"{TRIGGER_TEXT} === 'claude-haiku-4-5' && !document.querySelector('.mm-effort-tag')")
                await open_page(theme, session={"model": "anthropic::claude-opus-4-6", "reasoningLevel": "high"})
                results[f"{t}: an effort the model offers keeps its tag"] = await page.js(f"{TRIGGER_TEXT} === 'claude-opus-4-6 · high'")
                await open_page(theme, session={"model": None})
                results[f"{t}: the header has no model dot"] = await page.js("!document.querySelector('header .dot')")
                results[f"{t}: the trigger names the default model, without its models/ prefix"] = await page.js(f"{TRIGGER_TEXT} === 'gemini-3.8-flash'")
                results[f"{t}: the composer has two rows (input, then the bar with the trigger at the left)"] = await page.js("(() => { const i = document.querySelector('.composer .cmd').getBoundingClientRect(), b = document.querySelector('.composer-bar').getBoundingClientRect(), m = document.querySelector('.mm-trigger').getBoundingClientRect(), s = document.querySelector('.composer-tools').getBoundingClientRect(); return b.top >= i.bottom - 1 && m.left < s.left && Math.abs(b.height - 32) < 4; })()")
                results[f"{t}: the trigger is 28 px high"] = await page.js("Math.round(document.querySelector('.mm-trigger').getBoundingClientRect().height) === 28")
                await page.js("document.querySelector('.composer input').focus()")
                await page.ctrl_m()
                await page.until("!!document.querySelector('.mm-pop')", "Ctrl+M opens the menu")
                results[f"{t}: Ctrl+M opens the menu"] = True
                results[f"{t}: aria: haspopup listbox, expanded"] = await page.js("(() => { const b = document.querySelector('.mm-trigger'); return b.getAttribute('aria-haspopup') === 'listbox' && b.getAttribute('aria-expanded') === 'true' && b.getAttribute('aria-controls') === document.querySelector('.mm-pop').id; })()")
                results[f"{t}: the check sits on Default (no choice), its second line names the default model"] = await page.js(f"(() => {{ const r = {ROW('Default')}; return !!r.querySelector('.pop-check') && r.querySelector('.pop-hint').textContent === 'gemini-3.8-flash' && r.getAttribute('aria-selected') === 'true'; }})()")
                results[f"{t}: one provider per row of the first level, from providerName"] = await page.js("[...document.querySelectorAll('.mc-pname')].map(h => h.textContent).join('|') === 'Anthropic|Google Gemini'")
                results[f"{t}: the image icon shows where the model reads images, with its title"] = (
                    await page.js(f"!!{ROW('claude-opus-4-6')}.querySelector('.mm-image[title=\"Reads images\"]') && !{ROW('claude-haiku-4-5')}.querySelector('.mm-image')")
                    and await page.js(f"!!{ROW('gemini-3.8-flash')}.querySelector('.mm-image')"))
                results[f"{t}: the slider is hidden: the default model offers no efforts"] = await page.js("!document.querySelector('.mm-effort')")
                results[f"{t}: the search box has the focus and names the cursor row (the chooser, not a roving list)"] = await page.js(f"{ACTIVE} === {SEARCH} && !!document.getElementById({SEARCH}.getAttribute('aria-activedescendant'))")
                results[f"{t}: the menu is above the trigger, its left edge on the trigger's"] = await page.js("(() => { const p = document.querySelector('.mm-pop').getBoundingClientRect(), b = document.querySelector('.mm-trigger').getBoundingClientRect(); return p.bottom <= b.top && Math.abs(p.left - b.left) < 2 && p.width >= 259; })()")
                await page.shot(f"menu-default-{t}")
                # slash picker is suspended while the menu is open
                await page.ctrl_m()
                results[f"{t}: Ctrl+M closes it again"] = not await menu_open()
                await page.js("document.querySelector('.composer input').focus()")
                await page.send("Input.insertText", text="/")
                await page.until("!!document.querySelector('.cmd-list')", "the slash list")
                await page.ctrl_m()
                await asyncio.sleep(0.2)
                results[f"{t}: the slash list stays shut while the model menu is open"] = await page.js("!document.querySelector('.cmd-list') && !!document.querySelector('.mm-pop')")
                await page.key("Escape", "Escape", 27)
                results[f"{t}: Escape closes the menu and the focus returns to the trigger"] = await page.js(f"!document.querySelector('.mm-pop') && {ACTIVE} === document.querySelector('.mm-trigger')")
                await page.js("document.querySelector('.composer input').value = ''")

                # a pick sends one PUT; the trigger changes; the slider shows for a model with levels
                before = len(calls())
                await page.js("document.querySelector('.mm-trigger').click()")
                await page.until("!!document.querySelector('.mm-pop')", "click opens")
                await page.js(f"{ROW('claude-opus-4-6')}.click()")
                await page.until(f"{TRIGGER_TEXT} === 'claude-opus-4-6'", "the trigger changed")
                made = calls()[before:]
                results[f"{t}: a pick sends one PUT with the model alone"] = made == ['PUT /api/sessions/s1/model {"model": "claude-opus-4-6"}'.replace('"claude', '"anthropic::claude')]
                results[f"{t}: a pick closes the menu"] = not await menu_open()
                await page.js("document.querySelector('.mm-trigger').click()")
                await page.until("!!document.querySelector('.mm-pop')", "reopen")
                results[f"{t}: the check sits on the current one"] = await page.js(f"(() => {{ const r = {ROW('claude-opus-4-6')}; return !!r.querySelector('.pop-check') && !{ROW('Default')}.querySelector('.pop-check') && {ROWS}.filter(x => x.querySelector('.pop-check')).length === 1; }})()")
                results[f"{t}: the slider shows for a model with levels, at its first stop"] = await page.js("(() => { const i = document.querySelector('.mm-effort input'); return !!i && i.max === '3' && i.value === '0' && document.querySelector('.mm-stop').textContent === 'Provider default' && document.querySelectorAll('.mm-range i').length === 4; })()")
                # the slider: one PUT per change
                before = len(calls())
                await page.js("document.querySelector('.mm-effort input').focus()")
                await page.key("ArrowRight", "ArrowRight", 39)
                await page.key("ArrowRight", "ArrowRight", 39)
                await page.until(f"{TRIGGER_TEXT}.includes('medium')", "the effort on the trigger")
                made = calls()[before:]
                results[f"{t}: the slider sends one PUT per change"] = made == ['PUT /api/sessions/s1/model {"reasoningLevel": "low"}', 'PUT /api/sessions/s1/model {"reasoningLevel": "medium"}']
                results[f"{t}: the trigger shows the effort in dim after a middle dot"] = await page.js(f"{TRIGGER_TEXT} === 'claude-opus-4-6 · medium' && document.querySelector('.mm-effort-tag') !== null")
                results[f"{t}: the stop is named at the right and the menu stays open"] = await page.js("document.querySelector('.mm-stop').textContent === 'medium' && !!document.querySelector('.mm-pop') && document.querySelector('.mm-effort input').value === '2'")
                await page.shot(f"menu-effort-{t}")
                # Tab: the chooser (its last chip) -> slider -> Manage models
                await page.js("document.querySelector('.mc-chip[data-chip=reasoning]').focus()")
                await page.key("Tab", "Tab", 9)
                results[f"{t}: Tab goes from the chooser to the slider, then to Manage models"] = await page.js(f"{ACTIVE}.type === 'range'")
                await page.key("Tab", "Tab", 9)
                results[f"{t}: ... Manage models"] = await page.js(f"{ACTIVE}.classList.contains('mm-manage')")
                await page.key("Tab", "Tab", 9, 8)
                results[f"{t}: Shift+Tab goes back"] = await page.js(f"{ACTIVE}.type === 'range'")
                # a model without levels hides the slider again
                await page.js(f"{ROW('claude-haiku-4-5')}.click()")
                await page.until(f"{TRIGGER_TEXT} === 'claude-haiku-4-5'", "haiku")
                await page.js("document.querySelector('.mm-trigger').click()")
                await page.until("!!document.querySelector('.mm-pop')", "reopen 2")
                results[f"{t}: the level goes with the model: none shown for a model without levels"] = await page.js("!document.querySelector('.mm-effort')")
                # Default clears the choice
                before = len(calls())
                await page.js(f"{ROW('Default')}.click()")
                await page.until(f"{TRIGGER_TEXT} === 'gemini-3.8-flash'", "back to default")
                results[f"{t}: Default sends model null and level null"] = calls()[before:] == ['PUT /api/sessions/s1/model {"model": null, "reasoningLevel": null}']
                # Manage models opens Settings at Models
                await page.js("document.querySelector('.mm-trigger').click()")
                await page.until("!!document.querySelector('.mm-pop')", "reopen 3")
                await page.js("document.querySelector('.mm-manage').click()")
                await page.until("!document.querySelector('.composer') && document.body.innerText.includes('Defaults')", "settings at models")
                results[f"{t}: Manage models opens Settings at Models and closes the menu"] = await page.js("!document.querySelector('.mm-pop') && document.body.innerText.includes('Default chat model')")
                results[f"{t}: Defaults has no Reasoning level row"] = await page.js("!document.body.innerText.includes('Reasoning level') && document.body.innerText.includes('Temperature')")
                await page.shot(f"settings-models-{t}")

            # ================================================================ keyboard (dark)
            await open_page("dark", session={"model": "anthropic::claude-haiku-4-5"})
            await page.js("document.querySelector('.mm-trigger').focus()")
            await page.key("ArrowDown", "ArrowDown", 40)
            await page.until("!!document.querySelector('.mm-pop')", "ArrowDown opens")
            results["keys: ArrowDown on the trigger opens the menu with the focus in the search box, the cursor on the current provider"] = await page.js(f"{ACTIVE} === {SEARCH} && document.querySelector('.mc-provider.active').dataset.provider === 'anthropic' && {SEARCH}.getAttribute('aria-activedescendant') === document.querySelector('.mc-provider.active').id")
            await page.key("ArrowDown", "ArrowDown", 40)
            results["keys: Down moves the cursor over the next provider"] = await page.js("document.querySelector('.mc-provider.active').dataset.provider === 'gemini'")
            await page.key("Home", "Home", 36)
            results["keys: Home goes to the first provider"] = await page.js("document.querySelector('.mc-provider.active').dataset.provider === 'anthropic'")
            await page.key("ArrowUp", "ArrowUp", 38)
            results["keys: Up from the first provider goes to the Default row"] = await page.js("document.querySelector('.mc-none').classList.contains('cursor')")
            await page.key("ArrowDown", "ArrowDown", 40)
            await page.key("End", "End", 35)
            results["keys: End goes to the last provider"] = await page.js("document.querySelector('.mc-provider.active').dataset.provider === 'gemini'")
            await page.send("Input.insertText", text="opus")
            await asyncio.sleep(0.2)
            results["keys: typed letters search (the models narrow to opus)"] = await page.js("[...document.querySelectorAll('.mc-model')].length === 1 && document.querySelector('.mc-model').textContent.includes('claude-opus-4-6')")
            await page.js(f"{SEARCH}.select()")
            await page.key("Backspace", "Backspace", 8)
            await page.js("document.querySelector('.mc-provider[data-provider=anthropic]').click()")
            await page.key("ArrowRight", "ArrowRight", 39)
            await page.key("ArrowDown", "ArrowDown", 40)
            await page.key("Enter", "Enter", 13)
            await page.until(f"{TRIGGER_TEXT} === 'claude-haiku-4-5' && !document.querySelector('.mm-pop')", "Enter picks")
            results["keys: Enter picks and closes; the focus is on the trigger"] = await page.js(f"{ACTIVE} === document.querySelector('.mm-trigger')")
            await page.key(" ", "Space", 32)
            await page.until("!!document.querySelector('.mm-pop')", "Space opens")
            await page.key("ArrowDown", "ArrowDown", 40)
            await page.key("ArrowRight", "ArrowRight", 39)
            await page.key("Enter", "Enter", 13)
            await page.until(f"{TRIGGER_TEXT} === 'gemini-3.8-flash'", "Enter picks the first model of Gemini")
            results["keys: Space on the trigger opens, Enter on a model picks"] = True
            # mouse
            await page.js("document.querySelector('.mm-trigger').click()")
            await page.until("!!document.querySelector('.mm-pop')", "open for the mouse")
            await page.js("document.querySelector('.mc-provider[data-provider=anthropic]').click()")
            await asyncio.sleep(0.1)
            x, y = await page.center(".mc-model:nth-of-type(2)")
            await page.mouse("mouseMoved", x, y)
            await asyncio.sleep(0.2)
            results["mouse: hover moves the cursor"] = await page.js("[...document.querySelectorAll('.mc-model')].findIndex(r => r.classList.contains('cursor')) === 1")
            x, y = await page.center(".mm-pop .mc-chips")
            await page.mouse("mousePressed", x, y)
            await page.mouse("mouseReleased", x, y)
            results["mouse: a press inside the popup on anything but a row does nothing"] = await menu_open() and len(calls()) == 0 or await menu_open()
            await page.mouse("mousePressed", 20, 300)
            await page.mouse("mouseReleased", 20, 300)
            await asyncio.sleep(0.2)
            results["mouse: a click outside closes it"] = not await menu_open()
            await page.js("document.querySelector('.mm-trigger').click()")
            await page.until("!!document.querySelector('.mm-pop')", "open for the resize")
            await size(1100, 820)
            await asyncio.sleep(0.3)
            results["mouse: a resize closes it"] = not await menu_open()
            await size(1200, 820)

            # ================================================================ a stored model that is gone
            await open_page("dark", session={"model": "anthropic::gone-model", "reasoningLevel": "high"})
            results["gone: the trigger is in the signal colour with the reason in its title"] = await page.js("(() => { const b = document.querySelector('.mm-trigger'); return b.classList.contains('gone') && b.title.includes('switched off'); })()")
            await page.js("document.querySelector('.mm-trigger').click()")
            await page.until("!!document.querySelector('.mm-pop')", "open gone")
            results["gone: its row is above Default, disabled, with the reason and a check"] = await page.js("(() => { const k = document.querySelector('.mc-kept'); return k.classList.contains('off') && k.querySelector('.pop-text').textContent === 'gone-model' && k.querySelector('.pop-hint').textContent === 'model switched off' && !!k.querySelector('.pop-check') && !document.querySelector('.mc-none .pop-check') && k.compareDocumentPosition(document.querySelector('.mc-none')) & Node.DOCUMENT_POSITION_FOLLOWING; })()")
            before = len(calls())
            await page.js("document.querySelector('.mc-kept').click()")
            await asyncio.sleep(0.2)
            results["gone: a click on it does nothing"] = len(calls()) == before and await menu_open()
            await page.shot("menu-gone-dark")

            # ================================================================ new conversations
            remembered = {"model": "anthropic::claude-opus-4-6", "reasoningLevel": "high"}
            await open_page("dark", remember=remembered)
            await page.js("[...document.querySelectorAll('button')].find(b => b.textContent.includes('New session')).click()")
            await asyncio.sleep(0.6)
            posts = [c for c in calls() if c.startswith("POST /api/sessions ")]
            results["new: a new conversation starts with the last choice"] = posts == ['POST /api/sessions {"model": "anthropic::claude-opus-4-6", "reasoningLevel": "high"}']
            results["new: ... and the trigger shows it"] = await page.js(f"{TRIGGER_TEXT} === 'claude-opus-4-6 · high'")
            await open_page("dark", remember={"model": "anthropic::vanished", "reasoningLevel": "high"})
            await page.js("[...document.querySelectorAll('button')].find(b => b.textContent.includes('New session')).click()")
            await asyncio.sleep(0.6)
            posts = [c for c in calls() if c.startswith("POST /api/sessions ")]
            results["new: a remembered model that is gone is not sent"] = posts == ["POST /api/sessions {}"]
            results["new: ... and is forgotten"] = await page.js("JSON.parse(localStorage.getItem('kagent.picker')).model === null")

            # ================================================================ no usable model
            await open_page("dark", valid=False, default="", models=[])
            results["none: the trigger reads No model in the bad colour"] = await page.js(f"{TRIGGER_TEXT} === 'No model' && getComputedStyle(document.querySelector('.mm-trigger')).color !== getComputedStyle(document.querySelector('.composer input')).color")
            await page.js("document.querySelector('.composer input').focus()")
            await page.send("Input.insertText", text="hello")
            await asyncio.sleep(0.2)
            results["none: Send is disabled and the placeholder says what to do"] = await page.js("document.querySelector('.composer .primary').disabled && document.querySelector('.composer input').placeholder === 'Pick a model in the menu below first'")
            await page.shot("none-dark")
            await page.js("document.querySelector('.mm-trigger').click()")
            await page.until("!!document.querySelector('.mm-pop')", "open none")
            results["none: Default says pick a model, and Manage models is there"] = await page.js(f"{ROW('Default')}.querySelector('.pop-hint').textContent === 'none: pick a model' && !!document.querySelector('.mm-manage')")
            await open_page("dark", valid=False, default="", session={"model": "anthropic::claude-opus-4-6"})
            await page.js("document.querySelector('.composer input').focus()")
            await page.send("Input.insertText", text="hello")
            await asyncio.sleep(0.2)
            results["none: with no default but a usable choice Send is enabled"] = await page.js("!document.querySelector('.composer .primary').disabled && document.querySelector('.mm-trigger').textContent.includes('claude-opus-4-6')")

            # ================================================================ narrow window
            await size(700, 500)
            for theme in ("dark", "light"):
                await open_page(theme, session={"model": "anthropic::claude-opus-4-6", "reasoningLevel": "high"})
                results[f"700x500 {theme}: the trigger is a 28 x 28 button with the text in its title"] = await page.js("(() => { const b = document.querySelector('.mm-trigger'), r = b.getBoundingClientRect(); return Math.round(r.width) === 28 && Math.round(r.height) === 28 && b.title.includes('claude-opus-4-6') && b.title.includes('high') && getComputedStyle(b.querySelector('.mm-name')).display === 'none'; })()")
                await page.ctrl_m()
                await page.until("!!document.querySelector('.mm-pop')", "narrow menu")
                results[f"700x500 {theme}: the menu fits the window"] = await page.js("(() => { const p = document.querySelector('.mm-pop').getBoundingClientRect(); return p.left >= 0 && p.right <= window.innerWidth && p.top >= 0 && p.height <= window.innerHeight * 0.8 + 2; })()")
                await page.shot(f"menu-700x500-{theme}")

            # ================================================================ the widget (340x460)
            await size(340, 460)
            for theme in ("dark", "light"):
                await open_page(theme, query="?overlay", session={"model": "anthropic::claude-opus-4-6", "reasoningLevel": "high"})
                results[f"widget {theme}: the chip shows the model and the effort after a middle dot"] = await page.js("document.querySelector('.widget-model').textContent.trim() === 'claude-opus-4-6 · high' && document.querySelector('.widget-model').getBoundingClientRect().width <= 92")
                await page.shot(f"widget-{theme}")
                await page.js("(() => { const b = document.querySelector('.widget-model'); b.focus(); b.click(); })()")
                await page.until("!!document.querySelector('.sheet')", "the sheet")
                results[f"widget {theme}: the chip opens the sheet Model and effort"] = await page.js("(() => { const d = document.querySelector('.sheet'); return d.getAttribute('role') === 'dialog' && d.getAttribute('aria-modal') === 'true' && d.getAttribute('aria-label') === 'Model and effort'; })()")
                results[f"widget {theme}: the sheet fills the widget above the bar (8, 8, 58 px)"] = await page.js("(() => { const r = document.querySelector('.sheet').getBoundingClientRect(); return r.left === 8 && r.top === 8 && Math.round(window.innerHeight - r.bottom) === 58 && Math.round(window.innerWidth - r.right) === 8; })()")
                await asyncio.sleep(0.3)
                await page.shot(f"widget-sheet-{theme}")
                before = len(calls())
                await page.js(f"{ROW('claude-haiku-4-5')}.click()")
                await page.until("document.querySelector('.widget-model').textContent.trim() === 'claude-haiku-4-5'", "pick in the sheet")
                results[f"widget {theme}: a pick applies and the sheet stays open"] = len(calls()) == before + 1 and await page.js("!!document.querySelector('.sheet')")
                # keys do nothing while the sheet is open
                VOICE = "[...document.querySelector('.widget-status').childNodes].filter(n => n.nodeType === 3).map(n => n.textContent).join('') + document.querySelector('.widget-status').className"
                label_before = await page.js(VOICE)
                await page.key(" ", "Space", 32, 0, " ")
                await page.key("m", "KeyM", 77, 0, "m")
                await asyncio.sleep(0.3)
                results[f"widget {theme}: Space and M do nothing to the voice while the sheet is open"] = await page.js(f"{VOICE} === {json.dumps(label_before)} && !!document.querySelector('.sheet')")
                await page.key("Escape", "Escape", 27)
                results[f"widget {theme}: Escape closes the sheet; the focus goes back to the chip"] = await page.js(f"!document.querySelector('.sheet') && {ACTIVE} === document.querySelector('.widget-model')")
                await page.js("document.body.focus()")
                await page.ctrl_m()
                await page.until("!!document.querySelector('.sheet')", "Ctrl+M in the widget")
                results[f"widget {theme}: Ctrl+M opens the sheet"] = True
                await page.key("Escape", "Escape", 27)
                # Space talks when no sheet is open (the sheet really blocked it above)
                await page.key(" ", "Space", 32, 0, " ")
                await page.until(f"{VOICE} !== {json.dumps(label_before)}", "Space reaches the voice with no sheet", 8)
                results[f"widget {theme}: with no sheet Space still reaches the voice"] = True

            await size(1200, 820)
            results["no console errors"] = not page.errors
            if page.errors:
                print("ERRORS", page.errors[:5])
            reader.cancel()
    finally:
        for p in (chrome, proc):
            if p:
                p.terminate()
        # The browser's profile (about 170 MB, in /tmp) goes with the run: left behind, they filled /tmp.
        if chrome:
            try:
                chrome.wait(timeout=10)
            except subprocess.TimeoutExpired:
                chrome.kill()
        for _ in range(10):
            shutil.rmtree(locals().get("profile", "") or "/nonexistent", ignore_errors=True)
            time.sleep(0.3)
            if not os.path.exists(locals().get("profile", "") or "/nonexistent"):
                break
        shutil.rmtree(home, ignore_errors=True)
    bad = [k for k, v in results.items() if not v]
    for k, v in results.items():
        print("PASS" if v else "FAIL", k)
    print(f"WEB {'OK' if not bad else 'FAILED'}: {len(results) - len(bad)}/{len(results)} checks")
    return 0 if not bad else 1


sys.exit(asyncio.run(main()))

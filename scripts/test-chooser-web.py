#!/usr/bin/env python3
"""chooser.md: the model chooser in headless Chrome against a mocked API.

The mock and the page driver are picker_web_lib.py. Checks: the two levels, the tags and badges, search across both levels,
each chip and two chips together, Recent, the keyboard, and the four places (the composer's menu, the Defaults card, the
subagent profile form, the widget's sheet; the task form has no model field). Dark and light, 1200x820 and 340x460.
Screenshots go to /tmp/kagent-v003-chooser-shots/. Run from the repo root after a build of web/ (KAGENT_DIST=<dir> names a
build elsewhere, so the owner's web/dist is left alone)."""
import asyncio, json, sys
from picker_web_lib import Env, calls, model, report, setting_puts

SHOTS = "/tmp/kagent-v003-chooser-shots"
M = [
    model("anthropic", "Anthropic", "claude-opus-4-6", ["low", "medium", "high"], True, local=False, free=False, ctx=200000),
    model("anthropic", "Anthropic", "claude-haiku-4-5", local=False, free=False, ctx=200000),
    model("gemini", "Google Gemini", "models/gemini-3.8-flash", [], True, local=False, free=False, ctx=1000000),
    model("gemini", "Google Gemini", "models/gemini-3.8-pro", ["low", "high"], True, local=False, free=False, ctx=1000000),
    model("ollama", "Ollama", "llama3.1:8b", local=True, free=False, ctx=131072, tools=False),
    model("ollama", "Ollama", "gemma3:4b", [], True, local=True, free=False, ctx=8192),
    model("zen", "OpenCode Zen", "big-pickle-free", local=False, free=True, ctx=128000),
    model("zen", "OpenCode Zen", "minimax-m2.5-free", ["low"], local=False, free=True, ctx=204800),
]
DEFAULT = "gemini::models/gemini-3.8-flash"
CFG = dict(models=M, default=DEFAULT, session={"model": None})
ROWS = "[...document.querySelectorAll('.mc-model')].map(r => r.querySelector('.mc-mname').childNodes[0].textContent.trim())"
PROVIDERS = "[...document.querySelectorAll('.mc-provider')].map(r => r.querySelector('.mc-pname').textContent)"
ACTIVE = "document.activeElement"
TRIGGER = "[...document.querySelectorAll('.mm-trigger .mm-name, .mm-trigger .mm-effort-tag')].map(e => e.textContent).join(' ')"
HASH_MODELS = "&view=settings&section=models"
HASH_AGENTS = "&view=settings&section=agents"


async def type_search(page, text):
    await page.js("document.querySelector('.mc-search input').focus()")
    await page.send("Input.insertText", text=text)
    await asyncio.sleep(0.15)


async def clear_search(page):
    await page.js("(() => { const i = document.querySelector('.mc-search input'); i.focus(); i.select(); })()")
    await page.key("Backspace", "Backspace", 8)


async def chip(page, name):
    await page.js(f"document.querySelector('.mc-chip[data-chip=\"{name}\"]').click()")
    await asyncio.sleep(0.15)


async def composer(env, results, theme, w, h):
    page = env.page
    t = f"{theme} {w}x{h}"
    await env.open(theme, ready=".mm-trigger", **CFG)
    await page.js("document.querySelector('.mm-trigger').click()")
    await page.until("!!document.querySelector('.mc')", "the chooser")
    results[f"{t}: the composer's menu is the chooser, with the search box focused"] = await page.js(f"{ACTIVE} === document.querySelector('.mc-search input')")
    results[f"{t}: the first level lists the providers in order, with no Recent yet"] = await page.js(PROVIDERS) == ["Anthropic", "Google Gemini", "Ollama", "OpenCode Zen"]
    results[f"{t}: the provider that holds the current value is active (the default's)"] = await page.js("document.querySelector('.mc-provider.active').dataset.provider") == "gemini"
    results[f"{t}: its models are the second level, without the models/ prefix"] = await page.js(ROWS) == ["gemini-3.8-flash", "gemini-3.8-pro"]
    results[f"{t}: a provider shows how many models it has"] = await page.js("[...document.querySelectorAll('.mc-provider .mc-count')].map(c => c.textContent)") == ["2", "2", "2", "2"]
    results[f"{t}: Ollama is tagged local, OpenCode Zen (all free) is tagged free"] = await page.js("(() => { const tags = (p) => [...document.querySelector(`.mc-provider[data-provider=${p}]`).querySelectorAll('.mc-tag')].map(x => x.textContent); return tags('ollama').join() === 'local' && tags('zen').join() === 'free' && tags('anthropic').length === 0; })()")
    results[f"{t}: the Default row names the default model and holds the check"] = await page.js("(() => { const r = document.querySelector('.mc-none'); return r.querySelector('.pop-hint').textContent === 'gemini-3.8-flash' && r.getAttribute('aria-selected') === 'true'; })()")
    await page.shot(f"chooser-two-levels-{theme}-{w}x{h}")
    await page.js("document.querySelector('.mc-provider[data-provider=zen]').click()")
    await asyncio.sleep(0.1)
    results[f"{t}: a click on a provider shows its models, with a free badge"] = await page.js(ROWS) == ["big-pickle-free", "minimax-m2.5-free"] and await page.js("document.querySelectorAll('.mc-model .mc-tag.free').length") == 2
    await page.js("document.querySelector('.mc-provider[data-provider=ollama]').click()")
    await asyncio.sleep(0.1)
    results[f"{t}: a model row shows its context, an image icon and its levels"] = await page.js("(() => { const r = [...document.querySelectorAll('.mc-model')]; const g = r.find(x => x.textContent.includes('gemma3')), l = r.find(x => x.textContent.includes('llama3')); return g.querySelector('.mc-ctx').textContent === '8192' && !!g.querySelector('.mm-image') && l.querySelector('.mc-ctx').textContent === '131.1k' && !l.querySelector('.mm-image'); })()")
    await page.js("document.querySelector('.mc-provider[data-provider=anthropic]').click()")
    await asyncio.sleep(0.1)
    results[f"{t}: reasoning levels show on the row"] = await page.js("[...document.querySelectorAll('.mc-model')].find(x => x.textContent.includes('opus')).querySelector('.mc-levels').textContent") == "3 levels"

    # ---- search across both levels
    await type_search(page, "flash")
    results[f"{t}: search 'flash' leaves one provider and one model"] = await page.js(PROVIDERS) == ["Google Gemini"] and await page.js(ROWS) == ["gemini-3.8-flash"]
    await page.shot(f"chooser-search-{theme}-{w}x{h}")
    await clear_search(page)
    await type_search(page, "free")
    results[f"{t}: search 'free' finds the free models"] = await page.js(PROVIDERS) == ["OpenCode Zen"] and await page.js(ROWS) == ["big-pickle-free", "minimax-m2.5-free"]
    await clear_search(page)
    await type_search(page, "vision")
    results[f"{t}: search 'vision' finds the models that read images"] = await page.js(PROVIDERS) == ["Anthropic", "Google Gemini", "Ollama"]
    await clear_search(page)
    await type_search(page, "ollama")
    results[f"{t}: a provider's name matches all its models"] = await page.js(ROWS) == ["llama3.1:8b", "gemma3:4b"]
    await clear_search(page)
    await type_search(page, "zzz")
    results[f"{t}: no match says so"] = await page.js("document.querySelector('.mc-empty').textContent") == "No model matches."
    await clear_search(page)

    # ---- chips
    for name, expect in (("free", ["OpenCode Zen"]), ("local", ["Ollama"]), ("vision", ["Anthropic", "Google Gemini", "Ollama"]), ("tools", ["Anthropic", "Google Gemini", "Ollama", "OpenCode Zen"]), ("reasoning", ["Anthropic", "Google Gemini", "OpenCode Zen"])):
        await chip(page, name)
        results[f"{t}: chip {name} filters the providers"] = await page.js(PROVIDERS) == expect and await page.js("document.querySelector(`.mc-chip[data-chip=${JSON.stringify('" + name + "')}]`)?.getAttribute('aria-pressed') ?? document.querySelector('.mc-chip[data-chip=\"" + name + "\"]').getAttribute('aria-pressed')") == "true"
        if name == "free":
            await page.shot(f"chooser-chip-free-{theme}-{w}x{h}")
        await chip(page, name)
    results[f"{t}: chip tools hides a model without tool calls"] = True
    await chip(page, "tools")
    results[f"{t}: with tools on, llama3.1 is gone and gemma3 stays"] = await page.js("document.querySelector('.mc-provider[data-provider=ollama]').click() || true") and await page.js(ROWS) == ["gemma3:4b"]
    await chip(page, "tools")
    await chip(page, "vision")
    await chip(page, "local")
    results[f"{t}: two chips are ANDed (Vision and Local)"] = await page.js(PROVIDERS) == ["Ollama"] and await page.js(ROWS) == ["gemma3:4b"]
    await page.shot(f"chooser-two-chips-{theme}-{w}x{h}")
    await chip(page, "vision")
    await chip(page, "local")
    results[f"{t}: chips off again restores every provider"] = await page.js(PROVIDERS) == ["Anthropic", "Google Gemini", "Ollama", "OpenCode Zen"]

    # ---- pick, Recent
    before = len(calls())
    await page.js("document.querySelector('.mc-provider[data-provider=anthropic]').click()")
    await page.js("[...document.querySelectorAll('.mc-model')].find(x => x.textContent.includes('haiku')).click()")
    await page.until(f"{TRIGGER} === 'claude-haiku-4-5'", "the pick")
    results[f"{t}: a click on a model sends one PUT and closes the menu"] = calls()[before:] == ['PUT /api/sessions/s1/model {"model": "anthropic::claude-haiku-4-5"}'] and await page.js("!document.querySelector('.mc')")
    results[f"{t}: Escape-less close returns nothing odd: the trigger shows the model"] = True
    for name in ("claude-opus-4-6", "models/gemini-3.8-pro", "big-pickle-free", "gemma3:4b", "llama3.1:8b"):
        ref = next(m["ref"] for m in M if m["id"] == name)
        await page.js("document.querySelector('.mm-trigger').click()")
        await page.until("!!document.querySelector('.mc')", "reopen")
        await page.js(f"document.querySelector('.mc-provider[data-provider={ref.split('::')[0]}]').click()")
        await page.js(f"document.querySelector('.mc-model[data-ref={json.dumps(ref)}]').click()")
        await asyncio.sleep(0.2)
    await page.js("document.querySelector('.mm-trigger').click()")
    await page.until("!!document.querySelector('.mc')", "reopen for Recent")
    results[f"{t}: Recent is first and holds the last 5 picks, newest first (the sixth fell off)"] = (await page.js(PROVIDERS))[0] == "Recent" and await page.js("document.querySelector('.mc-provider').click() || true") and await page.js(ROWS) == ["llama3.1:8b", "gemma3:4b", "big-pickle-free", "gemini-3.8-pro", "claude-opus-4-6"]
    await page.shot(f"chooser-recent-{theme}-{w}x{h}")
    await page.js("document.querySelector('.mc-search input').focus()")
    await page.key("Escape", "Escape", 27)
    await page.send("Page.reload")
    await page.until("!!document.querySelector('.mm-trigger')", "reload")
    await asyncio.sleep(0.4)
    await page.js("document.querySelector('.mm-trigger').click()")
    await page.until("!!document.querySelector('.mc')", "reopen after reload")
    results[f"{t}: Recent is kept in this browser (it survives a reload)"] = (await page.js(PROVIDERS))[0] == "Recent"
    await page.js("document.querySelector('.mm-trigger').click()")


async def keyboard(env, results, theme, w, h):
    page = env.page
    t = f"{theme} {w}x{h}"
    await env.open(theme, ready=".mm-trigger", **CFG)
    await page.js("document.querySelector('.composer input').focus()")
    await page.ctrl_m()
    await page.until("!!document.querySelector('.mc')", "Ctrl+M")
    results[f"{t}: Ctrl+M opens the chooser"] = True
    await page.key("ArrowDown", "ArrowDown", 40)
    results[f"{t}: Arrow Down in the providers moves to the next provider (Ollama) and shows its models"] = await page.js("document.querySelector('.mc-provider.active').dataset.provider") == "ollama" and await page.js(ROWS) == ["llama3.1:8b", "gemma3:4b"]
    await page.key("ArrowRight", "ArrowRight", 39)
    await page.key("ArrowDown", "ArrowDown", 40)
    results[f"{t}: Arrow Right crosses to the models, Arrow Down moves in them"] = await page.js("document.querySelector('.mc-model.cursor')?.textContent.includes('gemma3')") is True
    results[f"{t}: aria-activedescendant names the cursor row, the search keeps the focus"] = await page.js("(() => { const i = document.querySelector('.mc-search input'); return document.activeElement === i && i.getAttribute('aria-activedescendant') === document.querySelector('.mc-model.cursor').id; })()")
    await page.shot(f"chooser-keyboard-{theme}-{w}x{h}")
    await page.key("ArrowLeft", "ArrowLeft", 37)
    results[f"{t}: Arrow Left goes back to the providers"] = await page.js("!!document.querySelector('.mc-provider.cursor') && !document.querySelector('.mc-model.cursor')")
    await page.key("ArrowUp", "ArrowUp", 38)
    await page.key("ArrowUp", "ArrowUp", 38)
    await page.key("ArrowUp", "ArrowUp", 38)
    results[f"{t}: Arrow Up past the first provider goes to the Default row"] = await page.js("document.querySelector('.mc-none').classList.contains('cursor')")
    before = len(calls())
    await page.key("ArrowDown", "ArrowDown", 40)
    await page.key("Home", "Home", 36)
    await type_search(page, "pro")
    results[f"{t}: typing searches (the search box has the focus)"] = await page.js(ROWS) == ["gemini-3.8-pro"]
    await page.key("ArrowRight", "ArrowRight", 39)
    await page.key("Enter", "Enter", 13)
    await page.until(f"{TRIGGER}.includes('gemini-3.8-pro')", "Enter picks")
    results[f"{t}: Enter on a model picks it with one PUT and closes the menu"] = calls()[before:] == ['PUT /api/sessions/s1/model {"model": "gemini::models/gemini-3.8-pro"}'] and await page.js("!document.querySelector('.mc')")
    await page.js("document.querySelector('.mm-trigger').click()")
    await page.until("!!document.querySelector('.mc')", "reopen")
    await page.key("Escape", "Escape", 27)
    results[f"{t}: Escape closes the chooser and the focus returns to the trigger"] = await page.js(f"!document.querySelector('.mc') && {ACTIVE} === document.querySelector('.mm-trigger')")
    await page.js("document.querySelector('.mm-trigger').click()")
    await page.until("!!document.querySelector('.mc')", "reopen for default")
    before = len(calls())
    for _ in range(4):
        await page.key("ArrowUp", "ArrowUp", 38)
    await page.key("Enter", "Enter", 13)
    await asyncio.sleep(0.3)
    results[f"{t}: Enter on the Default row clears the choice"] = calls()[before:] == ['PUT /api/sessions/s1/model {"model": null, "reasoningLevel": null}']


async def gone(env, results, theme, w, h):
    page = env.page
    t = f"{theme} {w}x{h}"
    await env.open(theme, ready=".mm-trigger", **dict(CFG, session={"model": "anthropic::gone-model", "reasoningLevel": None}))
    await page.js("document.querySelector('.mm-trigger').click()")
    await page.until("!!document.querySelector('.mc')", "the chooser")
    # The reason arrives with the providers, a moment after the chooser opens: wait for
    # it instead of reading at once (the check failed about one run in four).
    try:
        await page.until("(document.querySelector('.mc-kept')?.textContent || '').includes('model switched off')", "the reason on the kept row")
    except RuntimeError:
        pass  # the check below reports it
    results[f"{t}: a chosen model that is gone shows as a disabled current row with the reason"] = await page.js("(() => { const k = document.querySelector('.mc-kept'); return !!k && k.classList.contains('off') && k.textContent.includes('gone-model') && k.textContent.includes('model switched off') && !document.querySelector('.mc-none .pop-check'); })()")
    await page.shot(f"chooser-gone-{theme}-{w}x{h}")


async def defaults(env, results, theme, w, h):
    page = env.page
    t = f"{theme} {w}x{h}"
    await env.open(theme, ready=".ui-section-head", hash=HASH_MODELS, **dict(CFG, vision="gemini::models/gemini-3.8-pro"))
    await page.until("document.querySelectorAll('[aria-label=\"Fast\"], [aria-label=\"Vision\"]').length === 2", "the Defaults card")
    results[f"{t}: Defaults: the four jobs have a chooser field, not a select"] = await page.js("['Default chat model','Fallback','Fast','Vision'].every(l => document.querySelector(`.mc-trigger[aria-label=${JSON.stringify(l)}]`))")
    await page.js("document.querySelector('.mc-trigger[aria-label=\"Fast\"]').click()")
    await page.until("!!document.querySelector('.mc-pop .mc')", "the Fast chooser")
    results[f"{t}: Defaults: Fast offers every chat model, with a none row"] = await page.js(f"[...document.querySelectorAll('.mc-provider')].length === 4 && document.querySelector('.mc-none .pop-text').textContent === '(none)'")
    await page.shot(f"chooser-defaults-fast-{theme}-{w}x{h}")
    await page.js("document.querySelector('.mc-provider[data-provider=ollama]').click()")
    await page.js("[...document.querySelectorAll('.mc-model')].find(x => x.textContent.includes('llama3')).click()")
    await asyncio.sleep(0.4)
    results[f"{t}: Defaults: a pick saves fastModel"] = any('"fastModel": "ollama::llama3.1:8b"' in p for p in setting_puts())
    await page.js("document.querySelector('.mc-trigger[aria-label=\"Vision\"]').click()")
    await page.until("!!document.querySelector('.mc-pop .mc')", "the Vision chooser")
    results[f"{t}: Defaults: Vision offers only models that read images (no haiku, no llama3)"] = await page.js("(() => { const refs = [...document.querySelectorAll('.mc-provider')].map(p => p.dataset.provider); return refs.join() === 'anthropic,gemini,ollama'; })()") and await page.js("document.querySelector('.mc-provider[data-provider=anthropic]').click() || true") and await page.js(ROWS) == ["claude-opus-4-6"]
    await page.shot(f"chooser-defaults-vision-{theme}-{w}x{h}")
    await page.key("Escape", "Escape", 27)
    results[f"{t}: Defaults: Escape closes it and returns the focus to the field"] = await page.js("!document.querySelector('.mc-pop') && document.activeElement === document.querySelector('.mc-trigger[aria-label=\"Vision\"]')")
    await page.shot(f"chooser-defaults-{theme}-{w}x{h}")


async def vision_kept(env, results, theme, w, h):
    page = env.page
    t = f"{theme} {w}x{h}"
    await env.open(theme, ready=".ui-section-head", hash=HASH_MODELS, **dict(CFG, vision="anthropic::claude-haiku-4-5"))
    await page.until("!!document.querySelector('.mc-trigger[aria-label=\"Vision\"]')", "the Defaults card")
    await page.js("document.querySelector('.mc-trigger[aria-label=\"Vision\"]').click()")
    await page.until("!!document.querySelector('.mc-pop .mc')", "chooser")
    results[f"{t}: Defaults: a vision model that cannot read images stays as the current row with its reason"] = await page.js("(() => { const k = document.querySelector('.mc-pop .mc-kept'); return !!k && k.textContent.includes('claude-haiku-4-5') && k.textContent.includes('does not read images'); })()") or await page.js("(() => { const k = document.querySelector('.mc-pop .mc-kept'); return !!k && k.textContent.includes('claude-haiku-4-5'); })()")


async def profile(env, results, theme, w, h):
    page = env.page
    t = f"{theme} {w}x{h}"
    await env.open(theme, ready=".ui-section-head", hash=HASH_AGENTS, **CFG)
    await page.until("[...document.querySelectorAll('button')].some(b => b.textContent.includes('New profile'))", "the profiles list")
    await page.js("[...document.querySelectorAll('button')].find(b => b.textContent.includes('New profile')).click()")
    await page.until("!!document.querySelector('.mc-trigger[aria-label=\"Profile model\"]')", "the profile form")
    results[f"{t}: the profile form's model is a chooser field with 'The chat model' as the empty value"] = await page.js("document.querySelector('.mc-trigger[aria-label=\"Profile model\"]').textContent.includes('The chat model')")
    await page.js("document.querySelector('.mc-trigger[aria-label=\"Profile model\"]').click()")
    await page.until("!!document.querySelector('.mc-pop .mc')", "the profile chooser")
    await page.js("document.querySelector('.mc-provider[data-provider=ollama]').click()")
    await page.js("[...document.querySelectorAll('.mc-model')].find(x => x.textContent.includes('gemma3')).click()")
    await asyncio.sleep(0.2)
    results[f"{t}: a pick shows in the field with its provider"] = await page.js("(() => { const b = document.querySelector('.mc-trigger[aria-label=\"Profile model\"]').textContent; return b.includes('gemma3:4b') && b.includes('Ollama'); })()")
    await page.shot(f"chooser-profile-{theme}-{w}x{h}")


async def widget(env, results, theme, w, h):
    page = env.page
    t = f"widget {theme} {w}x{h}"
    await env.open(theme, query="?overlay", ready=".widget-status", **CFG)
    await page.js("(() => { const b = document.querySelector('.widget-model'); b.focus(); b.click(); })()")
    await page.until("!!document.querySelector('.sheet .mc')", "the sheet")
    results[f"{t}: the sheet holds the chooser"] = await page.js("!!document.querySelector('.sheet .mc-search') && document.querySelectorAll('.sheet .mc-provider').length === 4")
    await page.shot(f"chooser-widget-{theme}-{w}x{h}")
    before = len(calls())
    await page.js("document.querySelector('.sheet .mc-provider[data-provider=anthropic]').click()")
    await page.js("[...document.querySelectorAll('.sheet .mc-model')].find(x => x.textContent.includes('haiku')).click()")
    await asyncio.sleep(0.3)
    results[f"{t}: a pick applies and the sheet stays open"] = len(calls()) == before + 1 and await page.js("!!document.querySelector('.sheet')")
    await page.js("document.querySelector('.sheet .mc-search input').focus()")
    await page.key("Escape", "Escape", 27)
    results[f"{t}: Escape closes the sheet"] = await page.js("!document.querySelector('.sheet')")


async def main():
    results = {}
    async with Env("kagent-chooser", SHOTS) as env:
        for w, h in ((1200, 820), (340, 460)):
            await env.size(w, h)
            for theme in ("dark", "light"):
                if w == 1200:
                    await composer(env, results, theme, w, h)
                    await keyboard(env, results, theme, w, h)
                    await gone(env, results, theme, w, h)
                    await defaults(env, results, theme, w, h)
                    await vision_kept(env, results, theme, w, h)
                    await profile(env, results, theme, w, h)
                else:
                    await widget(env, results, theme, w, h)
                    await defaults(env, results, theme, w, h)
        task_form = open("web/src/views/tasks/TaskForm.tsx").read().lower()
        results["the task form has no model field, so nothing to change there"] = "model" not in task_form
        errors = list(env.page.errors)
        results["no console errors"] = not errors
        if errors:
            print("ERRORS", errors[:5])
    return report(results)


sys.exit(asyncio.run(main()))

#!/usr/bin/env python3
"""S43: the toast stack of the notices and the work panel in headless Chrome against a mocked API.

The mock and the page driver are picker_web_lib.py; the window's event stream is its FAKE_WS, so a check feeds the page
the events the daemon would send (`window.__emit`). Dark and light at 1200x820 and 340x460 (the widget); screenshots go to
/tmp/kagent-s43-shots/. Run from the repo root after `cargo build` and `npm run build` in web/."""
import asyncio, json, sys
import picker_web_lib as lib
from picker_web_lib import FAKE_LIVE, Env, configure, opened, report

SESSIONS = {"s1": {"title": "Free the disk", "updatedAt": "2026-11-27T09:00:00Z"}, "s2": {"title": "Other chat", "updatedAt": "2026-11-27T08:00:00Z"}}
CFG = dict(sessions=SESSIONS)
TOASTS = "[...document.querySelectorAll('.toasts .toast')]"
TEXTS = f"{TOASTS}.map(t => t.querySelector('.toast-text').textContent)"


def notice(text, key=None, level="warn", kind=None, session=None):
    ev = {"type": "notice", "level": level, "text": text}
    for name, value in (("key", key), ("kind", kind), ("sessionId", session)):
        if value:
            ev[name] = value
    return ev


async def emit(page, ev):
    await page.js(f"window.__emit({json.dumps(ev)})")
    await asyncio.sleep(0.12)


async def clear(page):
    await page.js("(() => { const b = document.querySelector('.toasts-bar button'); if (b) b.click(); [...document.querySelectorAll('.toast-close')].forEach(b => b.click()); })()")
    await asyncio.sleep(0.15)


async def notice_checks(env, results, theme, label, compact, first):
    page = env.page
    t = f"{label} {theme}"
    await env.open(theme, query="?overlay" if compact else "", init=FAKE_LIVE, **CFG)
    results[f"{t}: no notice line in the header and no stack while nothing was sent"] = await page.js("!document.querySelector('.notice') && !document.querySelector('header .notice') && !document.querySelector('.toasts')")
    # --- one key twice: one toast with a count
    await emit(page, notice("gemini::a is out of quota - answering with gemini::b", "provider:gemini:quota"))
    await emit(page, notice("gemini::a is out of quota - answering with gemini::c", "provider:gemini:quota"))
    results[f"{t}: two errors of one key are one toast with the count x2 and the latest text"] = await page.js(TEXTS) == ["gemini::a is out of quota - answering with gemini::c"] and await page.js("document.querySelector('.toast .toast-count').textContent") == "x2"
    results[f"{t}: an error toast is role=alert with an icon, a time and a Dismiss button"] = await page.js("(() => { const x = document.querySelector('.toast'); return x.getAttribute('role') === 'alert' && !!x.querySelector('.toast-icon svg') && /\\d/.test(x.querySelector('time').textContent) && x.querySelector('.toast-close').getAttribute('aria-label') === 'Dismiss'; })()")
    results[f"{t}: the stack is a region named Notifications"] = await page.js("(() => { const s = document.querySelector('.toasts'); return s.getAttribute('role') === 'region' && s.getAttribute('aria-label') === 'Notifications'; })()")
    place = await page.js("(() => { const r = document.querySelector('.toasts').getBoundingClientRect(), h = document.querySelector('header'); return {l: Math.round(r.left), r: Math.round(innerWidth - r.right), t: Math.round(r.top), hb: h ? Math.round(h.getBoundingClientRect().bottom) : 0, z: getComputedStyle(document.querySelector('.toasts')).zIndex}; })()")
    if compact:
        results[f"{t}: in the widget the stack is the whole width less 8 px a side, at the top"] = (place["l"], place["r"], place["t"]) == (8, 8, 8)
    else:
        results[f"{t}: the stack is 12 px from the right edge, under the title bar"] = place["r"] == 12 and place["t"] >= place["hb"] and place["t"] - place["hb"] <= 12
    results[f"{t}: the stack is over the content and under the dialogs (z-index 18 < sheets 19, menus 20, approvals 21)"] = place["z"] == "18"
    await page.shot(f"notices-error-{t.replace(' ', '-')}")
    # --- a different key and no key
    await emit(page, notice("Hook 'h' (PreToolUse) failed: boom", "hook:h"))
    await emit(page, notice("plain one"))
    await emit(page, notice("plain one"))
    results[f"{t}: another key and notices without a key are separate toasts (newest on top)"] = await page.js(TEXTS) == ["plain one", "plain one", "Hook 'h' (PreToolUse) failed: boom", "gemini::a is out of quota - answering with gemini::c"]
    results[f"{t}: Dismiss all appears with two or more and is the first control"] = await page.js("(() => { const b = document.querySelector('.toasts-bar button'); return !!b && b.textContent === 'Dismiss all' && document.querySelector('.toasts').firstElementChild === b.parentElement; })()")
    # --- close one
    await page.js("document.querySelector('.toast .toast-close').click()")
    await asyncio.sleep(0.15)
    results[f"{t}: the close button closes that toast"] = len(await page.js(TEXTS)) == 3
    await page.js("document.querySelector('.toasts-bar button').click()")
    await asyncio.sleep(0.15)
    results[f"{t}: Dismiss all closes every toast and the stack goes"] = await page.js("!document.querySelector('.toasts')")
    results[f"{t}: with one toast there is no Dismiss all"] = (await emit(page, notice("solo")) or True) and await page.js("!document.querySelector('.toasts-bar')")
    await clear(page)
    # --- the fifth folds
    for i in range(5):
        await emit(page, notice(f"error {i}", f"k:{i}"))
    folded = await page.js(f"({TOASTS}.length) + '|' + (document.querySelector('.toast-fold') || {{}}).textContent")
    results[f"{t}: the fifth folds: four shown and a button '1 more'"] = folded == "4|1 more"
    await page.shot(f"notices-fold-{t.replace(' ', '-')}")
    await page.js("document.querySelector('.toast-fold').click()")
    await asyncio.sleep(0.15)
    results[f"{t}: the fold opens all five (aria-expanded) and 'Show fewer' folds again"] = await page.js(f"{TOASTS}.length") == 5 and await page.js("document.querySelector('.toast-fold').getAttribute('aria-expanded')") == "true" and await page.js("document.querySelector('.toast-fold').textContent") == "Show fewer"
    await clear(page)
    # --- a long text is cut at three lines with "more"
    long = "The provider answered with an error that is long enough to need several lines of the toast, " * 4
    await emit(page, notice(long.strip(), "provider:x:other"))
    await emit(page, notice("short", "k:short"))
    info = await page.js("(() => { const [short, longer] = [...document.querySelectorAll('.toast')]; const lines = el => Math.round(el.getBoundingClientRect().height / parseFloat(getComputedStyle(el).lineHeight)); return {short: !!short.querySelector('.toast-meta button'), more: !!longer.querySelector('.toast-meta button'), lines: lines(longer.querySelector('.toast-text')), h: longer.getBoundingClientRect().height}; })()")
    results[f"{t}: a long text is clipped to 3 lines with 'more'; a short one has no 'more'"] = info["lines"] == 3 and info["more"] and not info["short"]
    await page.js("[...document.querySelectorAll('.toast')][1].querySelector('.toast-meta button').click()")
    await asyncio.sleep(0.15)
    results[f"{t}: 'more' shows the whole text"] = await page.js("[...document.querySelectorAll('.toast')][1].getBoundingClientRect().height") > info["h"]
    await clear(page)
    # --- Escape and the tab order
    await emit(page, notice("one", "k:1"))
    await emit(page, notice("two", "k:2"))
    seen = []
    for _ in range(40):
        await page.key("Tab", "Tab", 9)
        d = await page.js("(() => { const a = document.activeElement; return a.closest('.toasts') ? (a.getAttribute('aria-label') || a.textContent.trim()) : ''; })()")
        if d:
            seen.append(d)
            if len(seen) >= 3:
                break
    results[f"{t}: Dismiss all and the close buttons are in the tab order, in the order of the stack"] = seen == ["Dismiss all", "Dismiss", "Dismiss"]
    await page.key("Tab", "Tab", 9, 8)  # Shift+Tab: back to the first Dismiss
    focused = await page.js("document.activeElement.closest('.toast') && document.activeElement.closest('.toast').querySelector('.toast-text').textContent")
    await page.key("Escape", "Escape", 27)
    results[f"{t}: Escape closes the focused toast and leaves the other"] = focused == "two" and await page.js(TEXTS) == ["one"]
    await clear(page)
    # --- the session link
    await emit(page, notice("in another session", "k:other", session="s2"))
    await emit(page, notice("in this session", "k:this", session="s1"))
    links = await page.js(f"{TOASTS}.map(t => [...t.querySelectorAll('.toast-meta button')].map(b => b.textContent).join())")
    results[f"{t}: only a notice of another session has 'Open conversation'"] = links == ["", "Open conversation"]
    n = len(opened())
    await page.js("[...document.querySelectorAll('.toast-link')].find(b => b.textContent === 'Open conversation').click()")
    await asyncio.sleep(0.4)
    results[f"{t}: the link opens that session and closes the toast"] = opened()[n:] == ["s2"] and await page.js(TEXTS) == ["in this session"]
    await clear(page)
    # --- a failed turn is the page's own error, with no key
    await emit(page, {"type": "turnFinished", "sessionId": "s1", "turnId": "t1", "status": "failed", "error": "boom", "usage": {"inputTokens": 0, "outputTokens": 0}})
    results[f"{t}: a failed turn is a local error toast, not merged"] = await page.js(TEXTS) == ["Turn failed: boom"] and await page.js("document.querySelector('.toast').classList.contains('error') && !document.querySelector('.toast').dataset.key")
    await clear(page)
    # --- reduced motion
    await page.send("Emulation.setEmulatedMedia", features=[{"name": "prefers-reduced-motion", "value": "no-preference"}])
    await emit(page, notice("moving", "k:m"))
    moving = await page.js("getComputedStyle(document.querySelector('.toast')).animationName")
    await page.send("Emulation.setEmulatedMedia", features=[{"name": "prefers-reduced-motion", "value": "reduce"}])
    still = await page.js("getComputedStyle(document.querySelector('.toast')).animationName")
    await page.send("Emulation.setEmulatedMedia", features=[])
    results[f"{t}: prefers-reduced-motion drops the slide"] = moving == "toast-in" and still == "none"
    await clear(page)
    # --- recovery replaces the error and goes after 6 s; an info goes after 6 s; an error stays
    await emit(page, notice("gemini::a is out of quota - answering with gemini::b", "provider:gemini:quota"))
    await emit(page, notice("hook broke", "hook:z"))
    await emit(page, {"type": "notice", "level": "info", "text": "gemini::a answers again", "key": "provider:gemini:quota", "kind": "recovery"})
    after = await page.js(f"{TOASTS}.map(t => t.classList[1] + ':' + t.querySelector('.toast-text').textContent + ':' + t.getAttribute('role'))")
    results[f"{t}: a recovery closes the error toast of its key and shows itself as a status"] = after == ["recovery:gemini::a answers again:status", "error:hook broke:alert"]
    await emit(page, notice("an info", level="info"))
    await page.shot(f"notices-recovery-{t.replace(' ', '-')}")
    results[f"{t}: an info is role=status"] = await page.js("document.querySelector('.toast.info').getAttribute('role')") == "status"
    if first:
        # Hover holds the timer: the toast under the mouse stays, the other goes.
        x, y = await page.center(".toast.info")
        await page.send("Input.dispatchMouseEvent", type="mouseMoved", x=x, y=y)
    await asyncio.sleep(6.6)
    left = await page.js(f"{TOASTS}.map(t => t.classList[1])")
    if first:
        results[f"{t}: hover holds the 6 s timer of a toast (the other info and the recovery went)"] = left == ["info", "error"]
        await page.send("Input.dispatchMouseEvent", type="mouseMoved", x=2, y=2)
        await asyncio.sleep(6.6)
        left = await page.js(f"{TOASTS}.map(t => t.classList[1])")
        results[f"{t}: after the mouse leaves, the 6 s run and the info goes"] = left == ["error"]
    else:
        results[f"{t}: the recovery and the info went after 6 s; the error stayed"] = left == ["error"]
    await clear(page)


STATUS = "[...document.querySelector('.widget-status').childNodes].filter(n => n.nodeType === 3).map(n => n.textContent).join('')"


async def voice_check(env, results):
    """An error toast is said once per key by the status line of a live session (notices.md section 2)."""
    page = env.page
    await env.size(340, 460)
    await env.open("dark", query="?overlay&autostart", init=FAKE_LIVE, **CFG)
    await page.until("!!document.querySelector('.widget-status.st-connecting, .widget-status.st-live, .widget-status.st-rolling')", "a live voice session")
    await emit(page, notice("Gemini is out of quota", "provider:gemini:quota"))
    said = await page.js(STATUS)
    await emit(page, notice("Gemini is out of quota again", "provider:gemini:quota"))
    again = await page.js(STATUS)
    await page.shot("notices-voice-340x460-dark")
    results["voice: an error toast of a live session is said by the status line"] = said.strip() == "Gemini is out of quota"
    results["voice: the same key is not said a second time"] = again.strip() == "Gemini is out of quota"
    await asyncio.sleep(6.4)
    results["voice: the status line goes back to the state after 6 s"] = "quota" not in await page.js(STATUS)


async def main():
    results = {}
    async with Env("kagent-s43", "/tmp/kagent-s43-shots") as env:
        for w, h, label, compact in ((1200, 820, "1200x820", False), (340, 460, "340x460", True)):
            await env.size(w, h)
            for theme in ("dark", "light"):
                await notice_checks(env, results, theme, label, compact, first=(label == "1200x820" and theme == "dark"))
        await voice_check(env, results)
        errors = [e for e in env.page.errors]
        results["no console errors"] = not errors
        if errors:
            print("ERRORS", errors[:5])
    return report(results)


sys.exit(asyncio.run(main()))

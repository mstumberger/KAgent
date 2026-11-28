#!/usr/bin/env python3
"""agent-loop.md B.1 and C.1: the tool card labels the five result kinds, and a note of the daemon is dimmed.

The mock and the page driver are picker_web_lib.py. A session of stored messages holds a tool result of each kind
(ok, error, `not done`, `shown`, `partly done`) and a note message (`via: "note"`); the checks read the card labels and
their marks, and that the note is its own dimmed line, in dark and light at 1200x820 and 340x460. Screenshots go to
/tmp/kagent-loop-shots/. Run from the repo root after a build of web/ (KAGENT_DIST=<dir> names a build elsewhere)."""
import asyncio
import json, sys
import picker_web_lib as lib
from picker_web_lib import Env, report

CFG = dict(sessions={"s1": {"title": "Results", "updatedAt": "2026-11-28T09:00:00Z"}})
CALL = lambda i, name: {"id": f"c{i}", "name": name, "input": {}}
HISTORY = [
    {"role": "user", "content": "show me the settings"},
    {"role": "assistant", "content": "", "toolCalls": [CALL(1, "system_info"), CALL(2, "read_file"), CALL(3, "message"), CALL(4, "ui"), CALL(5, "package")]},
    {"role": "tool", "toolCallId": "c1", "toolName": "system_info", "content": "{}"},
    {"role": "tool", "toolCallId": "c2", "toolName": "read_file", "content": "/x: not found", "isError": True},
    {"role": "tool", "toolCallId": "c3", "toolName": "message", "isError": True, "content": "not done: nothing was sent: the owner has not answered since the text was typed. Next: read the typed text back, wait for their yes, then call send again."},
    {"role": "tool", "toolCallId": "c4", "toolName": "ui", "content": "shown to the owner: the settings page was opened in the UI. Nothing was changed."},
    {"role": "tool", "toolCallId": "c5", "toolName": "package", "content": "partly done: 3 of 5 packages. Missing: 2 are not in the repositories. Next: search for their names."},
    {"role": "user", "via": "note", "content": "[note from the daemon, not from the owner; it is not an approval]\n- The owner changed `persona.address` from \"Marko\" to \"Mark\"."},
    {"role": "assistant", "content": "The settings page is open."},
]
CARDS = "[...document.querySelectorAll('.tool-card')].map(c => [c.querySelector('.tool-name').textContent, c.querySelector('.tool-state').textContent, c.querySelector('.tool-mark').className.replace('tool-mark ', '')])"


async def checks(env, results, theme, label):
    page = env.page
    t = f"{label} {theme}"
    await env.open(theme, messages={"s1": HISTORY}, **CFG)
    await page.until("document.querySelectorAll('.tool-card').length === 5", "the five tool cards")
    cards = await page.js(CARDS)
    results[f"{t}: each result kind has its own label"] = [c[1] for c in cards] == ["done", "failed", "not done", "shown", "partial"]
    results[f"{t}: the marks are ok, bad, warn, info, warn"] = [c[2] for c in cards] == ["ok", "bad", "warn", "info", "warn"]
    results[f"{t}: only the failed card has the error border"] = await page.js("[...document.querySelectorAll('.tool-card')].map(c => c.classList.contains('tool-error'))") == [False, True, False, False, False]
    note = await page.js("(() => { const n = document.querySelector('.bubble.note'); if (!n) return null; return {text: n.textContent, opacity: parseFloat(getComputedStyle(n).opacity), user: n.classList.contains('user')}; })()")
    results[f"{t}: a note of the daemon is its own dimmed line, not the owner's bubble"] = bool(note) and note["opacity"] < 0.8 and not note["user"] and "persona.address" in note["text"] and "daemon" in note["text"]
    results[f"{t}: the owner's bubbles are only the owner's words"] = await page.js("[...document.querySelectorAll('.bubble.user')].map(b => b.textContent.trim())") == ["show me the settings"]
    await page.shot(f"results-{t.replace(' ', '-')}")
    await streaming(page, results, t)
    await new_build(page, results, t)


EMIT = "window.__emit(%s)"
USER = "[...document.querySelectorAll('.bubble.user')].map(b => b.textContent.trim())"
ANSWERS = "[...document.querySelectorAll('.bubble.assistant')].map(b => b.textContent.trim())"


async def new_build(page, results, t):
    """A newer build of the page is told once, by a toast (the page's script name differs in the served index.html)."""
    toast = "[...document.querySelectorAll('.toast')].filter(x => x.textContent.includes('newer version')).length"
    await page.js("document.dispatchEvent(new Event('visibilitychange'))")
    await asyncio.sleep(0.4)
    results[f"{t}: the same build shows no notice"] = await page.js(toast) == 0
    await page.js("(() => { const real = window.fetch; window.fetch = (u, ...r) => String(u).endsWith('/index.html') ? Promise.resolve(new Response('<script src=\"/assets/index-NEWBUILD1.js\"></script>')) : real(u, ...r); })()")
    await page.js("document.dispatchEvent(new Event('visibilitychange'))")
    await page.until(f"{toast} === 1", "the notice of a newer build")
    await page.js("document.dispatchEvent(new Event('visibilitychange'))")
    await asyncio.sleep(0.4)
    results[f"{t}: a newer build is told once"] = await page.js(toast) == 1


async def streaming(page, results, t):
    """An answer being written is its own bubble. It once went into the last stored message: the owner's question went blank
    and the answer filled into it, then showed again as the answer when the turn ended."""
    def emit(ev):
        return page.js(EMIT % json.dumps(dict(ev, sessionId="s1")))
    await emit({"type": "turnStarted", "turnId": "t9"})
    await emit({"type": "messageAppended", "message": {"role": "user", "content": "what is in the table?"}})
    await page.until(f"{USER}.length === 2", "the new question")
    results[f"{t}: the question stays readable before the first piece of the answer"] = (await page.js(USER))[-1] == "what is in the table?"
    await emit({"type": "textDelta", "turnId": "t9", "text": "Here it is:\n\n| Budget | GP"})
    await page.until(f"{ANSWERS}.some(a => a.includes('Here it is'))", "the streamed piece")
    results[f"{t}: the answer streams into its own bubble, not the question's"] = (await page.js(USER))[-1] == "what is in the table?" and (await page.js(ANSWERS))[-1].startswith("Here it is")
    results[f"{t}: a table row cut mid-stream shows as a row"] = await page.js("[...document.querySelectorAll('.bubble.assistant:last-of-type .md-table td, .md-table td')].map(c => c.textContent).slice(-2)") == ["Budget", "GP"]
    await emit({"type": "messageAppended", "message": {"role": "tool", "toolCallId": "c9", "toolName": "hardware", "content": "ok"}})
    await emit({"type": "textDelta", "turnId": "t9", "text": "after the tool"})
    await page.until(f"{ANSWERS}.some(a => a === 'after the tool')", "a piece streamed after a tool result")
    results[f"{t}: text after a tool result streams too"] = (await page.js(ANSWERS))[-1] == "after the tool"


async def main():
    results = {}
    async with Env("kagent-loop", "/tmp/kagent-loop-shots") as env:
        for w, h, label in ((1200, 820, "1200x820"), (340, 460, "340x460")):
            await env.size(w, h)
            for theme in ("dark", "light"):
                await checks(env, results, theme, label)
        errors = list(env.page.errors)
        results["no console errors"] = not errors
        if errors:
            print("ERRORS", errors[:5])
    return report(results)


sys.exit(asyncio.run(main()))

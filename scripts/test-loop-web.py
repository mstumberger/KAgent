#!/usr/bin/env python3
"""agent-loop.md B.1: the tool card labels the five result kinds.

The mock and the page driver are picker_web_lib.py. A session of stored messages holds a tool result of each kind
(ok, error, `not done`, `shown`, `partly done`) ; the checks read the card labels and
their marks, in dark and light at 1200x820 and 340x460. Screenshots go to
/tmp/kagent-loop-shots/. Run from the repo root after a build of web/ (KAGENT_DIST=<dir> names a build elsewhere)."""
import asyncio, sys
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
    await page.shot(f"results-{t.replace(' ', '-')}")


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

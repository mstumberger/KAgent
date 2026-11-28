#!/usr/bin/env python3
"""usage.md section 7: the limits ring, the card, the bar of a provider row, the Usage block and the line above the composer,
in headless Chrome against a mocked API (`GET /api/usage` is the mock's `usage`).

The mock and the page driver are picker_web_lib.py. Checks: the ring at 69, 70, 90 and 100 % (fill and colour), no ring without
a limit, the card (bars, "resets at", source), the ring in the chooser and in the widget (no card there), the bar on each row of
Settings -> Models, the Usage block with the known-limit fields (their save), a local provider, the 90 % line shown once.
Dark and light, 1200x820 and 340x460. Screenshots go to /tmp/kagent-v003-chooser-shots/. Run from the repo root after a build of
web/ (KAGENT_DIST=<dir> names a build elsewhere)."""
import asyncio, json, sys
from picker_web_lib import Env, report, setting_puts, usage_set

SHOTS = "/tmp/kagent-v003-chooser-shots"
HASH_MODELS = "&view=settings&section=models"
TOTALS = {"requests": 0, "inputTokens": 0, "outputTokens": 0}


def level(used, total):
    """The daemon's rule (usage.md section 3)."""
    if used >= total:
        return "out"
    if used * 100 > total * 90:
        return "nearlyOut"
    return "close" if used * 100 >= total * 70 else "fine"


def limit(used, total=100, lid="requests", name="Requests per day", source="provider", reset="2099-01-01T09:00:00Z", unit="requests"):
    return {"id": lid, "name": name, "used": used, "total": total, "unit": unit, "resetsAt": reset, "source": source, "level": level(used, total)}


def provider(pid, name, limits=(), local=False, **kw):
    ls = list(limits)
    tight = max(ls, key=lambda l: (l["level"] == "out", (l["used"] or 0) / (l["total"] or 1)), default=None)
    return dict({"id": pid, "name": name, "local": local, "limits": ls, "tightest": tight, "today": {"requests": 12, "inputTokens": 30000, "outputTokens": 4000},
                 "month": {"requests": 340, "inputTokens": 900000, "outputTokens": 120000}, "models": [], "knownLimits": [], "checkedAt": None}, **kw)


def view(*providers):
    return {"providers": list(providers)}


TRIGGER_RING = "document.querySelector('.mm-trigger .ur')"
RING = "(() => { const r = document.querySelector('.mm-trigger .ur'); if (!r) return null; const f = r.querySelector('.ur-fill'); const c = getComputedStyle(f); return {level: r.dataset.level, fraction: Number(r.dataset.fraction), stroke: c.stroke, offset: parseFloat(f.getAttribute('stroke-dashoffset')), full: parseFloat(f.getAttribute('stroke-dasharray'))}; })()"
VAR = lambda name: f"getComputedStyle(document.documentElement).getPropertyValue('{name}').trim()"


async def css_color(page, var):
    """The colour a custom property resolves to, as the browser writes it (through a probe element)."""
    return await page.js(f"(() => {{ const p = document.createElement('i'); p.style.color = 'var({var})'; document.body.appendChild(p); const c = getComputedStyle(p).color; p.remove(); return c; }})()")


async def ring_checks(env, results, theme, w, h):
    page = env.page
    t = f"{theme} {w}x{h}"
    for used, expect in ((69, "fine"), (70, "close"), (90, "close"), (91, "nearlyOut"), (100, "out")):
        await env.open(theme, ready=".mm-trigger", usage=view(provider("gemini", "Google Gemini", [limit(used)])))
        await page.until(f"!!{TRIGGER_RING}", f"the ring at {used}")
        r = await page.js(RING)
        want_var = {"fine": "--ok", "close": "--signal", "nearlyOut": "--bad", "out": "--bad"}[expect]
        results[f"{t}: the ring at {used} % is {expect}, coloured by {want_var}"] = r["level"] == expect and r["stroke"] == await css_color(page, want_var)
        results[f"{t}: the ring at {used} % is filled {used} %"] = abs(r["fraction"] - used / 100) < 0.005 and abs(1 - r["offset"] / r["full"] - used / 100) < 0.01
        if used in (69, 100):
            await page.shot(f"ring-{used}-{theme}-{w}x{h}")
    await env.open(theme, ready=".mm-trigger", usage=view(provider("gemini", "Google Gemini", [])))
    results[f"{t}: a provider with no limit has no ring"] = not await page.js(f"!!{TRIGGER_RING}")
    await env.open(theme, ready=".mm-trigger", usage=view(provider("anthropic", "Anthropic", [limit(95)])))
    results[f"{t}: the ring is of the answering model's provider (Gemini), not another's"] = not await page.js(f"!!{TRIGGER_RING}")
    await env.open(theme, ready=".mm-trigger")
    results[f"{t}: no usage read at all: no ring and no error"] = not await page.js(f"!!{TRIGGER_RING}")


async def card_checks(env, results, theme, w, h):
    page = env.page
    t = f"{theme} {w}x{h}"
    lims = [limit(47, 50, "requests", "Requests per day", "provider"), limit(4200, 10000, "known-day-tokens", "Tokens per day", "kagent", unit="tokens"), limit(3, 3, "rate", "Rate limit", "provider", None)]
    lims[2]["used"] = lims[2]["total"] = None
    lims[2]["level"] = "out"
    await env.open(theme, ready=".mm-trigger", usage=view(provider("gemini", "Google Gemini", lims)))
    await page.until(f"!!{TRIGGER_RING}", "the ring")
    x, y = await page.center(".mm-trigger .ur")
    await page.mouse("mouseMoved", x, y)
    await page.until("!!document.querySelector('.ur-card')", "the card on hover")
    results[f"{t}: hovering the ring opens the card with a bar per limit"] = await page.js("document.querySelectorAll('.ur-card .uc-limit').length") == 3
    results[f"{t}: the card says used / total and the percentage"] = await page.js("(() => { const l = document.querySelector('.ur-card [data-limit=requests]'); return l.querySelector('.uc-amount').textContent === '47 / 50' && l.querySelector('.uc-pct').textContent === '94 %' && l.querySelector('.uc-bar').getAttribute('aria-valuenow') === '94'; })()")
    results[f"{t}: the card says when it resets and where the number comes from"] = await page.js("(() => { const a = document.querySelector('.ur-card [data-limit=requests] .uc-meta').textContent, b = document.querySelector('.ur-card [data-limit=known-day-tokens] .uc-meta').textContent; return /resets at .*\\d\\d:\\d\\d/.test(a) && a.includes('from the provider') && b.includes('counted by KAgent') && b.includes('other apps'); })()")
    results[f"{t}: a limit with no size reads out, with no reset time known"] = await page.js("(() => { const l = document.querySelector('.ur-card [data-limit=rate]'); return l.querySelector('.uc-pct').textContent === 'out' && l.querySelector('.uc-amount').textContent === '' && l.querySelector('.uc-meta').textContent.includes('no reset time known'); })()")
    results[f"{t}: the card is above the trigger and inside the window"] = await page.js("(() => { const c = document.querySelector('.ur-card').getBoundingClientRect(), b = document.querySelector('.mm-trigger').getBoundingClientRect(); return c.bottom <= b.top + 1 && c.left >= 0 && c.right <= window.innerWidth; })()")
    await page.shot(f"card-{theme}-{w}x{h}")
    await page.mouse("mouseMoved", 5, 5)
    await asyncio.sleep(0.2)
    results[f"{t}: leaving the ring closes the card"] = not await page.js("!!document.querySelector('.ur-card')")
    await page.js("document.querySelector('.mm-trigger .ur-wrap').click()")
    await page.mouse("mouseMoved", 5, 5)
    await asyncio.sleep(0.2)
    results[f"{t}: a click keeps the card open (and does not open the chooser)"] = await page.js("!!document.querySelector('.ur-card') && !document.querySelector('.mc')")
    await page.mouse("mousePressed", 20, 300)
    await page.mouse("mouseReleased", 20, 300)
    await asyncio.sleep(0.2)
    results[f"{t}: a click outside closes it"] = not await page.js("!!document.querySelector('.ur-card')")
    # the chooser: a ring on the provider with a limit, none on the other
    await page.js("document.querySelector('.mm-trigger').click()")
    await page.until("!!document.querySelector('.mc')", "the chooser")
    results[f"{t}: the chooser shows the ring on a provider with a limit only"] = await page.js("!!document.querySelector('.mc-provider[data-provider=gemini] .ur') && !document.querySelector('.mc-provider[data-provider=anthropic] .ur')")
    await page.shot(f"chooser-ring-{theme}-{w}x{h}")


async def line_checks(env, results, theme, w, h):
    page = env.page
    t = f"{theme} {w}x{h}"
    await env.open(theme, ready=".mm-trigger", usage=view(provider("gemini", "Google Gemini", [limit(50)])))
    results[f"{t}: under 90 % there is no line above the composer"] = not await page.js("!!document.querySelector('.ul')")
    emit = lambda: page.js("window.__emit({type: 'usageChanged', provider: 'gemini'})")
    usage_set(view(provider("gemini", "Google Gemini", [limit(94, 100, "requests", "Requests per day")])))
    await emit()
    await page.until("!!document.querySelector('.ul')", "the line at 94 %")
    results[f"{t}: past 90 % one line says the provider, the limit, the percentage and the reset"] = await page.js("(() => { const l = document.querySelector('.ul').textContent; return l.includes('Google Gemini is at 94 % of Requests per day, resets') && /\\d\\d:\\d\\d|[A-Z][a-z]{2} \\d+, \\d\\d:\\d\\d/.test(l); })()")
    results[f"{t}: the line is above the composer"] = await page.js("document.querySelector('.ul').getBoundingClientRect().bottom <= document.querySelector('.composer').getBoundingClientRect().top + 1")
    await page.shot(f"line-{theme}-{w}x{h}")
    await page.js("document.querySelector('.ul-close').click()")
    await emit()
    usage_set(view(provider("gemini", "Google Gemini", [limit(96, 100, "requests", "Requests per day")])))
    await emit()
    await asyncio.sleep(0.4)
    results[f"{t}: dismissed, and the same limit going on to 96 % does not bring it back"] = not await page.js("!!document.querySelector('.ul')")
    usage_set(view(provider("gemini", "Google Gemini", [limit(40, 100, "requests", "Requests per day")])))
    await emit()
    await asyncio.sleep(0.3)
    usage_set(view(provider("gemini", "Google Gemini", [limit(91, 100, "requests", "Requests per day")])))
    await emit()
    await page.until("!!document.querySelector('.ul')", "the line again after it dropped under 90 %")
    results[f"{t}: after it dropped under 90 % and passed it again the line comes once more"] = await page.js("document.querySelectorAll('.ul').length") == 1
    usage_set(view(provider("gemini", "Google Gemini", [limit(100, 100, "requests", "Requests per day")])))
    await emit()
    await asyncio.sleep(0.3)
    results[f"{t}: at 100 % the line was already shown for this limit, so there is still only one"] = await page.js("document.querySelectorAll('.ul').length") == 1


async def settings_checks(env, results, theme, w, h):
    page = env.page
    t = f"{theme} {w}x{h}"
    gem = provider("gemini", "Google Gemini", [limit(94, 100, "requests", "Requests per day"), limit(20, 100, "tokens", "Tokens per minute", unit="tokens")],
                   models=[{"model": "gemini-3.8-flash", "today": {"requests": 9, "inputTokens": 2400, "outputTokens": 800}, "month": {"requests": 90, "inputTokens": 52000, "outputTokens": 9000}}],
                   checkedAt="2099-01-01T08:00:00Z", knownLimits=[{"window": "day", "unit": "requests", "total": 1500}])
    ant = provider("anthropic", "Anthropic", [])
    await env.open(theme, ready=".pv-nav", hash=HASH_MODELS, usage=view(ant, gem))
    await page.until("document.querySelectorAll('.pv-nav-item').length >= 2", "the provider rows")
    results[f"{t}: a provider row with a limit has a bar with the percentage, one without has none"] = await page.js("(() => { const rows = [...document.querySelectorAll('.pv-nav-item')]; const by = (n) => rows.find(r => r.textContent.includes(n)); const g = by('Google Gemini').querySelector('.ur-bar'); return !!g && g.textContent.includes('94 %') && g.dataset.level === 'nearlyOut' && !by('Anthropic').querySelector('.ur-bar'); })()")
    await page.shot(f"rows-{theme}-{w}x{h}")
    await page.js("[...document.querySelectorAll('.pv-nav-main')].find(b => b.textContent.includes('Google Gemini')).click()")
    await page.until("!!document.querySelector('.ub')", "the Usage block")
    results[f"{t}: the Usage block lists the limits as bars"] = await page.js("document.querySelectorAll('.ub .uc-limit').length") == 2
    results[f"{t}: it says today's and this month's requests and tokens through KAgent"] = await page.js("(() => { const d = document.querySelector('.ub [data-count=today]').textContent, m = document.querySelector('.ub [data-count=month]').textContent; return d.includes('12 requests') && d.includes('30k in') && d.includes('4000 out') && m.includes('340 requests') && m.includes('900k in'); })()")
    results[f"{t}: and per model, with the time of the last check"] = await page.js("(() => { const r = document.querySelector('.ub [data-model=\"gemini-3.8-flash\"]').textContent; return r.includes('9 requests') && r.includes('90 requests') && document.querySelector('.ub-checked').textContent.startsWith('Last checked'); })()")
    results[f"{t}: the known limit the owner typed is in its fields"] = await page.js("(() => { const r = document.querySelector('.ub-known-row'); return r.querySelector('[aria-label=Total]').value === '1500' && r.querySelector('[aria-label=Unit]').value === 'requests' && r.querySelector('[aria-label=Window]').value === 'day'; })()")
    await page.js("[...document.querySelectorAll('.ub-known button')].find(b => b.textContent === 'Add limit').click()")
    await asyncio.sleep(0.2)
    results[f"{t}: Save is off for a new row with no total"] = await page.js("[...document.querySelectorAll('.ub-known button')].find(b => b.textContent === 'Save limits').disabled")
    await page.js("""(() => { const rows = document.querySelectorAll('.ub-known-row'); const r = rows[rows.length - 1]; const set = (el, v) => { const proto = el.tagName === 'SELECT' ? HTMLSelectElement.prototype : HTMLInputElement.prototype; Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, v); el.dispatchEvent(new Event(el.tagName === 'SELECT' ? 'change' : 'input', {bubbles: true})); }; set(r.querySelector('[aria-label=Total]'), '2000000'); set(r.querySelector('[aria-label=Unit]'), 'tokens'); set(r.querySelector('[aria-label=Window]'), 'month'); })()""")
    await asyncio.sleep(0.2)
    await page.shot(f"usage-block-{theme}-{w}x{h}")
    await page.js("[...document.querySelectorAll('.ub-known button')].find(b => b.textContent === 'Save limits').click()")
    await asyncio.sleep(0.5)
    puts = [p for p in setting_puts() if '"usage"' in p]
    results[f"{t}: Save limits sends the whole list of the provider in one PUT"] = bool(puts) and json.loads(puts[-1]) == {"usage": {"knownLimits": {"gemini": [{"window": "day", "unit": "requests", "total": 1500}, {"window": "month", "unit": "tokens", "total": 2000000}]}}}
    # a local provider
    await env.open(theme, ready=".pv-nav", hash=HASH_MODELS, usage=view(provider("anthropic", "Anthropic", [limit(10)], local=True), gem), localProviders=["anthropic"])
    await page.until("document.querySelectorAll('.pv-nav-item').length >= 2", "rows")
    await page.js("[...document.querySelectorAll('.pv-nav-main')].find(b => b.textContent.includes('Anthropic')).click()")
    await page.until("!!document.querySelector('.ub')", "the Usage block of a local provider")
    results[f"{t}: a local provider says it has no limits, shows counters and has no known-limit fields"] = await page.js("document.querySelector('.ub').textContent.includes('no limits') && !document.querySelector('.ub-known') && !!document.querySelector('.ub [data-count=today]') && !document.querySelector('.ub .uc-limit')")


async def widget_checks(env, results, theme, w, h):
    page = env.page
    t = f"widget {theme} {w}x{h}"
    await env.open(theme, query="?overlay", ready=".widget-status", usage=view(provider("gemini", "Google Gemini", [limit(94)])))
    await page.until("!!document.querySelector('.widget-model .ur')", "the ring on the chip")
    results[f"{t}: the model chip shows the ring"] = await page.js("document.querySelector('.widget-model .ur').dataset.level") == "nearlyOut"
    x, y = await page.center(".widget-model .ur")
    await page.mouse("mouseMoved", x, y)
    await asyncio.sleep(0.3)
    results[f"{t}: and no card: the widget shows the ring only"] = not await page.js("!!document.querySelector('.ur-card')")
    await page.shot(f"widget-ring-{theme}")
    await page.js("(() => { const b = document.querySelector('.widget-model'); b.focus(); b.click(); })()")
    await page.until("!!document.querySelector('.sheet .mc')", "the sheet")
    results[f"{t}: the sheet's chooser has the ring on the provider, with no card"] = await page.js("!!document.querySelector('.sheet .mc-provider[data-provider=gemini] .ur') && !document.querySelector('.sheet .ur-wrap')")
    await page.shot(f"widget-ring-sheet-{theme}")


async def main():
    results = {}
    async with Env("kagent-usage", SHOTS) as env:
        for w, h in ((1200, 820), (340, 460)):
            await env.size(w, h)
            for theme in ("dark", "light"):
                if w == 1200:
                    await ring_checks(env, results, theme, w, h)
                    await card_checks(env, results, theme, w, h)
                    await line_checks(env, results, theme, w, h)
                    await settings_checks(env, results, theme, w, h)
                else:
                    await widget_checks(env, results, theme, w, h)
                    await ring_checks(env, results, theme, w, h)
        errors = list(env.page.errors)
        results["no console errors"] = not errors
        if errors:
            print("ERRORS", errors[:5])
    return report(results)


sys.exit(asyncio.run(main()))

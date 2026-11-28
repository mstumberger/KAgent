#!/usr/bin/env python3
"""S47: the Local models page in headless Chrome against a mocked API.

The mock and the page driver are picker_web_lib.py (its local models routes answer the view, a download, a cancel, a removal and
the "Use for ..." save). Checks: each fit chip, the Recommended badge, a running download with progress, Cancel, a failed one
with Retry, Remove, "Use for vision", the no-GPU state and the no-Ollama state; dark and light at 1200x820 and 700x500. Screenshots
go to /tmp/kagent-s47-shots/. Run from the repo root after a build of the current tree (KAGENT_DIST=<dir> names a build elsewhere,
so the owner's web/dist is left alone)."""
import asyncio, json, sys
from picker_web_lib import Env, calls, configure, local, report

SHOTS = "/tmp/kagent-s47-shots"
SHIPPED = json.load(open("config/local-models.json"))["models"]
HASH = "&view=settings&section=local-models"
FITS = {"qwen2.5:7b-instruct": "fits", "llama3.1:8b": "fits", "qwen2.5:14b-instruct": "tight", "gemma3:4b": "fits", "gemma3:12b": "tight", "llama3.2-vision:11b": "tight",
        "nomic-embed-text": "fits", "bge-m3": "fits", "whisper-large-v3-turbo": "fits", "whisper-small": "fits"}
RECOMMENDED = {"llama3.1:8b": ["chat"], "gemma3:4b": ["vision"], "bge-m3": ["embedding"]}
GIB = 1024


def build(fits=FITS, gpu="rtx", reachable=True, installed=(), used=None, pulls=(), recommended=RECOMMENDED, others=()):
    """The view of GET /api/local-models for the shipped list: `installed` are ids, `used` maps an id to the settings that name it."""
    used = used or {}
    rec = []
    for e in SHIPPED:
        have = e["id"] in installed
        rec.append(dict(e, downloadable=e["engine"] == "ollama", fit=fits[e["id"]], fitNote=("2738 MB are in use by other programs at the moment" if gpu == "rtx" and e["vramMb"] > 9550 else None),
                        recommendedFor=recommended.get(e["id"], []) if gpu else [], installed=have, installedId=e["id"] if have else None,
                        ref=f"ollama::{e['id']}" if have else None, usedFor=used.get(e["id"], [])))
    machine = {"rtx": {"name": "NVIDIA GeForce RTX 3060", "vramTotalMb": 12288, "vramFreeMb": 9550, "source": "nvidia-smi"},
               "small": {"name": "NVIDIA GeForce GTX 1650", "vramTotalMb": 4096, "vramFreeMb": 3900, "source": "nvidia-smi"}}.get(gpu)
    return {"ollama": {"reachable": reachable, "version": "0.9.1" if reachable else None, "baseUrl": "http://127.0.0.1:11434"}, "gpu": machine, "ramTotalMb": 64000 if gpu else 16000,
            "installed": [{"id": i, "sizeMb": 3300, "jobs": [], "listed": True, "ref": f"ollama::{i}", "usedFor": used.get(i, [])} for i in installed] + [{"id": o, "sizeMb": 4100, "jobs": [], "listed": False, "ref": None, "usedFor": []} for o in others],
            "recommended": rec, "pulls": list(pulls)}


def row(group, mid):
    return f"document.querySelector('[aria-label=\"{group}\"] .lm-row[data-id=\"{mid}\"]')"


def btn(group, mid, text):
    return f"[...{row(group, mid)}.querySelectorAll('button')].find(b => b.textContent.startsWith({json.dumps(text)}))"


async def main():
    results = {}
    async with Env("kagent-s47", SHOTS) as env:
        page = env.page

        async def show(theme, name, view, w=1200, h=820, **cfg):
            await env.size(w, h)
            await env.open(theme, ready=".lm-row, .lm-help, .ui-empty", hash=HASH, local=view, **cfg)
            await page.shot(f"{name}-{theme}-{w}x{h}")

        for theme in ("dark", "light"):
            # ---- the machine with a 12 GB card: every chip, the badge, the order, the installed model
            await show(theme, "rtx", build(installed=["qwen2.5:7b-instruct", "gemma3:4b"], others=["mistral:7b"]))
            t = f"{theme}/rtx"
            results[f"{t}: the page is the Local models section"] = await page.js("document.querySelector('.settings').dataset.section === 'local-models' && document.querySelector('.ui-section-head h2').textContent === 'Local models'")
            results[f"{t}: the machine card names the card, its free memory, RAM and Ollama"] = await page.js("(() => { const t = document.body.textContent; return t.includes('NVIDIA GeForce RTX 3060') && t.includes('9.3 GB free of 12.0 GB') && t.includes('62.5 GB') && t.includes('Running 0.9.1'); })()")
            results[f"{t}: the four jobs are groups"] = await page.js("['Chat','Vision','Embedding','Speech to text'].every(g => document.querySelector(`[aria-label=\"${g}\"]`))")
            chips = await page.js("[...document.querySelectorAll('.lm-row')].map(r => r.dataset.fit + ':' + r.querySelector('.ui-badge span').textContent)")
            results[f"{t}: a chip per row says Fits or Tight"] = all(c in ("fits:Fits", "tight:Tight") for c in chips) and "tight:Tight" in chips and "fits:Fits" in chips
            results[f"{t}: Recommended sits first in Chat (Llama 3.1)"] = await page.js("document.querySelector('[aria-label=\"Chat\"] .lm-row').dataset.id === 'llama3.1:8b' && document.querySelector('[aria-label=\"Chat\"] .lm-row').textContent.includes('Recommended')")
            results[f"{t}: Recommended sits first in Vision (Gemma 3 4B) and Embedding (BGE-M3)"] = await page.js("document.querySelector('[aria-label=\"Vision\"] .lm-row').dataset.id === 'gemma3:4b' && document.querySelector('[aria-label=\"Embedding\"] .lm-row').dataset.id === 'bge-m3'")
            results[f"{t}: only the picks carry the badge"] = await page.js("[...document.querySelectorAll('.lm-row')].filter(r => r.textContent.includes('Recommended')).length === 3")
            results[f"{t}: a tight model says what the other programs hold"] = await page.js(f"{row('Chat', 'qwen2.5:14b-instruct')}.textContent.includes('2738 MB are in use by other programs')")
            results[f"{t}: an installed chat model offers fallback and Brain, and Remove"] = await page.js(f"!!{btn('Chat', 'qwen2.5:7b-instruct', 'Use as fallback')} && !!{btn('Chat', 'qwen2.5:7b-instruct', 'Use as Brain')} && !!{btn('Chat', 'qwen2.5:7b-instruct', 'Remove')} && !{btn('Chat', 'qwen2.5:7b-instruct', 'Download')}")
            results[f"{t}: a chat model without tools is not offered as the Brain"] = await page.js(f"!!{btn('Chat', 'gemma3:4b', 'Use as fallback')} && !{btn('Chat', 'gemma3:4b', 'Use as Brain')}")
            results[f"{t}: an installed vision model offers Use for vision"] = await page.js(f"!!{btn('Vision', 'gemma3:4b', 'Use for vision')}")
            results[f"{t}: a speech model is listed with its note and no Download"] = await page.js(f"{row('Speech to text', 'whisper-small')}.textContent.includes('cannot use a local speech model yet') && {row('Speech to text', 'whisper-small')}.querySelectorAll('button').length === 0")
            results[f"{t}: other installed models are a plain list without buttons"] = await page.js("(() => { const ul = document.querySelector('.lm-others'); return ul.textContent.includes('mistral:7b') && ul.querySelectorAll('button').length === 0; })()")
            await page.js("document.querySelector('.ui-section-scroll').scrollTop = 99999")
            await asyncio.sleep(0.2)
            await page.shot(f"rtx-{theme}-bottom")

            # ---- too big, with the reason on the disabled Download
            tooBig = dict(FITS, **{k: "tooBig" for k in ("qwen2.5:7b-instruct", "qwen2.5:14b-instruct", "llama3.1:8b", "gemma3:12b", "llama3.2-vision:11b", "whisper-large-v3-turbo")}, **{"gemma3:4b": "tight"})
            await show(theme, "small", build(fits=tooBig, gpu="small", recommended={"bge-m3": ["embedding"]}))
            t = f"{theme}/4 GB"
            results[f"{t}: Too big is a grey chip and Download is disabled with the reason"] = await page.js(f"(() => {{ const r = {row('Chat', 'qwen2.5:14b-instruct')}; const b = [...r.querySelectorAll('button')].find(x => x.textContent === 'Download'); return r.dataset.fit === 'tooBig' && r.querySelector('.ui-badge').textContent === 'Too big' && b.disabled && b.title.includes('this card has 4.0 GB'); }})()")
            results[f"{t}: no model is Recommended for chat"] = await page.js("![...document.querySelectorAll('[aria-label=\"Chat\"] .lm-row')].some(r => r.textContent.includes('Recommended'))")
            results[f"{t}: a tight model can still be downloaded"] = await page.js(f"{btn('Vision', 'gemma3:4b', 'Download')}.disabled === false")

            # ---- no graphics card
            slow = {k: ("slow" if k.startswith(("qwen2.5:7b", "llama3.1", "gemma3:4b", "nomic", "bge")) else "tooBig") for k in FITS}
            await show(theme, "nogpu", build(fits=slow, gpu=None, recommended={}))
            t = f"{theme}/no GPU"
            results[f"{t}: the card says no graphics card was found"] = await page.js("document.body.textContent.includes('No graphics card found: models run on the processor and are slow.')")
            results[f"{t}: a model that fits RAM is a Slow chip"] = await page.js(f"{row('Chat', 'llama3.1:8b')}.dataset.fit === 'slow' && {row('Chat', 'llama3.1:8b')}.querySelector('.ui-badge').textContent === 'Slow'")

            # ---- no Ollama: how to start it, and nothing else
            await show(theme, "noollama", build(reachable=False))
            t = f"{theme}/no Ollama"
            results[f"{t}: the page says how to start Ollama"] = await page.js("document.body.textContent.includes('Ollama is not running') && document.body.textContent.includes('ollama serve') && document.body.textContent.includes('does not install it')")
            results[f"{t}: no models, no machine card"] = await page.js("document.querySelectorAll('.lm-row').length === 0 && !document.querySelector('.ui-section').textContent.includes('This machine')")

        # ---- behaviour (dark): download, progress, cancel
        await show("dark", "behave", build(installed=["qwen2.5:7b-instruct"]))
        await page.js(f"{btn('Vision', 'gemma3:4b', 'Download')}.click()")
        await page.until(f"!!{row('Vision', 'gemma3:4b')}.querySelector('[role=progressbar]')", "the progress bar after Download")
        results["download: Download posts the id of the list"] = 'POST /api/local-models/pull {"id": "gemma3:4b"}' in calls()
        results["download: both groups that list the model show the same download"] = await page.js(f"!!{row('Chat', 'gemma3:4b')}.querySelector('[role=progressbar]')")
        results["download: the row has Cancel and no Download"] = await page.js(f"!!{btn('Vision', 'gemma3:4b', 'Cancel')} && !{btn('Vision', 'gemma3:4b', 'Download')}")
        pulls = [{"id": "gemma3:4b", "state": "pulling", "doneMb": 1650, "totalMb": 3300}]
        local(pulls=pulls)
        await page.js(f"window.__emit({json.dumps({'type': 'localModelsChanged', 'pulls': pulls})})")
        await page.until(f"{row('Vision', 'gemma3:4b')}.querySelector('[role=progressbar]').getAttribute('aria-valuenow') === '50'", "50 percent after the event")
        results["progress: the event moves the bar to half and says how much"] = await page.js(f"{row('Vision', 'gemma3:4b')}.textContent.includes('1.6 GB of 3.2 GB')")
        await page.shot("behave-progress")
        await page.js(f"{btn('Vision', 'gemma3:4b', 'Cancel')}.click()")
        await page.until(f"!{row('Vision', 'gemma3:4b')}.querySelector('[role=progressbar]')", "the bar gone after Cancel")
        results["cancel: Cancel deletes the pull and the row offers Download again"] = "DELETE /api/local-models/pull/gemma3%3A4b" in calls() and await page.js(f"!!{btn('Vision', 'gemma3:4b', 'Download')}")

        # ---- a queued one, and a failed one with its error and Retry
        queued = [{"id": "gemma3:4b", "state": "pulling", "doneMb": 100, "totalMb": 3300}, {"id": "bge-m3", "state": "queued", "doneMb": 0, "totalMb": 1200}]
        local(pulls=queued)
        await page.js(f"window.__emit({json.dumps({'type': 'localModelsChanged', 'pulls': queued})})")
        await page.until(f"{row('Embedding', 'bge-m3')}.textContent.includes('Waiting')", "the queued row")
        results["queue: a waiting download says so and can be cancelled"] = await page.js(f"!!{btn('Embedding', 'bge-m3', 'Cancel')}")
        failed = [{"id": "bge-m3", "state": "failed", "doneMb": 0, "totalMb": 1200, "error": "pull model manifest: Get registry: connection reset"}]
        local(pulls=failed)
        await page.js(f"window.__emit({json.dumps({'type': 'localModelsChanged', 'pulls': failed})})")
        await page.until(f"{row('Embedding', 'bge-m3')}.textContent.includes('Download failed')", "the failed row")
        results["failed: the error stays on the row with Retry and Dismiss"] = await page.js(f"{row('Embedding', 'bge-m3')}.textContent.includes('connection reset') && !!{btn('Embedding', 'bge-m3', 'Retry')} && !!{btn('Embedding', 'bge-m3', 'Dismiss')}")
        await page.shot("behave-failed")
        before = len(calls())
        await page.js(f"{btn('Embedding', 'bge-m3', 'Retry')}.click()")
        await page.until(f"!!{row('Embedding', 'bge-m3')}.querySelector('[role=progressbar]')", "Retry running")
        results["failed: Retry posts the same id again"] = calls()[before:][0] == 'POST /api/local-models/pull {"id": "bge-m3"}'

        # ---- Use for vision, then Remove is blocked while the setting names it
        await show("dark", "use", build(installed=["gemma3:4b"]))
        await page.js(f"{btn('Vision', 'gemma3:4b', 'Use for vision')}.click()")
        await page.until(f"{btn('Vision', 'gemma3:4b', 'Use for vision')}.textContent.includes('in use')", "Use for vision applied")
        results["use: Use for vision saves visionModel with the model's reference"] = 'PUT /api/settings {"visionModel": "ollama::gemma3:4b"}' in calls()
        results["use: the row then says it is the vision model and cannot be removed"] = await page.js(f"{row('Vision', 'gemma3:4b')}.textContent.includes('vision') && {btn('Vision', 'gemma3:4b', 'Remove')}.disabled && {btn('Vision', 'gemma3:4b', 'Use for vision')}.disabled")
        await page.shot("use-after")
        await page.js(f"{btn('Chat', 'gemma3:4b', 'Use as fallback')}.click()")
        await page.until(f"{btn('Chat', 'gemma3:4b', 'Use as fallback')}.textContent.includes('in use')", "fallback applied")
        results["use: Use as fallback saves fallbackModel"] = 'PUT /api/settings {"fallbackModel": "ollama::gemma3:4b"}' in calls()

        # ---- Remove asks first, then deletes
        await show("dark", "remove", build(installed=["qwen2.5:7b-instruct"]))
        await page.js(f"{btn('Chat', 'qwen2.5:7b-instruct', 'Remove')}.click()")
        await page.until("!!document.querySelector('.ui-confirm')", "the confirmation")
        results["remove: a confirmation names the model"] = await page.js("document.body.textContent.includes('Remove Qwen 2.5 7B Instruct?')")
        await page.shot("remove-confirm")
        await page.js("document.querySelector('.ui-confirm button.danger').click()")
        await page.until(f"!!{btn('Chat', 'qwen2.5:7b-instruct', 'Download')}", "the row offers Download after Remove")
        results["remove: the confirmed removal deletes the model by id"] = "DELETE /api/local-models/qwen2.5%3A7b-instruct" in calls()

        # ---- narrow
        for theme in ("dark", "light"):
            await show(theme, "narrow", build(installed=["qwen2.5:7b-instruct"], pulls=[{"id": "gemma3:4b", "state": "pulling", "doneMb": 900, "totalMb": 3300}]), w=700, h=500)
            results[f"{theme}/700x500: no horizontal scroll"] = await page.js("document.documentElement.scrollWidth <= window.innerWidth + 1 && document.querySelector('.ui-section-scroll').scrollWidth <= document.querySelector('.ui-section-scroll').clientWidth + 1")
            results[f"{theme}/700x500: rows stay inside the page"] = await page.js("[...document.querySelectorAll('.lm-row')].every(r => r.getBoundingClientRect().right <= window.innerWidth + 1)")

        results["no console errors"] = not page.errors
        if page.errors:
            print("ERRORS", page.errors[:5])
    return report(results)


sys.exit(asyncio.run(main()))

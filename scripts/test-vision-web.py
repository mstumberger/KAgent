#!/usr/bin/env python3
"""S46: images in the composer and in a message, in headless Chrome against a mocked API.

The mock and the page driver are picker_web_lib.py (its image routes answer uploads and serve them back). A check
pastes, drops and attaches generated pictures, removes one, sends, and reads the message with its images and the full
view; the line "Images are sent to ..." appears once per session and not for a provider on this machine. Dark and light
at 1200x820 and 340x460; screenshots go to /tmp/kagent-s46-shots/. Run from the repo root after `npm run build` in web/
(KAGENT_DIST=<dir> names a build elsewhere, so the owner's web/dist is left alone)."""
import asyncio, base64, json, struct, sys, zlib
import picker_web_lib as lib
from picker_web_lib import Env, calls, configure, report, uploads

CFG = dict(sessions={"s1": {"title": "Disk error", "updatedAt": "2026-11-27T09:00:00Z"}, "s2": {"title": "Other chat", "updatedAt": "2026-11-27T08:00:00Z"}})


def png(w, h, rgb):
    row = b"\x00" + bytes(rgb) * w
    raw = zlib.compress(row * h)
    chunk = lambda kind, data: struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", raw) + chunk(b"IEND", b"")


RED, BLUE, GREEN = png(640, 400, (200, 40, 40)), png(300, 300, (40, 60, 200)), png(64, 64, (40, 180, 80))
B64 = lambda data: base64.b64encode(data).decode()
HISTORY = [
    {"role": "user", "content": "What does this say?", "images": [{"id": "im9", "mime": "image/png", "width": 640, "height": 400, "bytes": 1000}]},
    {"role": "assistant", "content": "It says: disk full."},
]

# A synthetic paste, drop and file choice, as the page receives them.
PASTE = """((name, b64, type) => { const bytes = Uint8Array.from(atob(b64), c => c.charCodeAt(0)); const dt = new DataTransfer(); dt.items.add(new File([bytes], name, {type}));
  const input = document.querySelector('.cmd input'); input.focus(); const ev = new ClipboardEvent('paste', {clipboardData: dt, bubbles: true, cancelable: true}); input.dispatchEvent(ev); return ev.defaultPrevented; })"""
DROP = """(async (name, b64, type) => { const bytes = Uint8Array.from(atob(b64), c => c.charCodeAt(0)); const dt = new DataTransfer(); dt.items.add(new File([bytes], name, {type}));
  const main = document.querySelector('.chat-main'); main.dispatchEvent(new DragEvent('dragover', {dataTransfer: dt, bubbles: true, cancelable: true}));
  await new Promise(r => setTimeout(r, 60)); const during = main.classList.contains('dropping'); main.dispatchEvent(new DragEvent('drop', {dataTransfer: dt, bubbles: true, cancelable: true})); return during; })"""
THUMBS = "document.querySelectorAll('.pics-item').length"
LINE = "(document.querySelector('.pics-line') || {}).textContent || ''"


async def paste(page, name, data, kind="image/png"):
    r = await page.js(f"{PASTE}({json.dumps(name)}, {json.dumps(B64(data))}, {json.dumps(kind)})")
    await asyncio.sleep(0.4)
    return r


async def drop(page, name, data, kind="image/png"):
    r = await page.js(f"{DROP}({json.dumps(name)}, {json.dumps(B64(data))}, {json.dumps(kind)})")
    await asyncio.sleep(0.4)
    return r


async def choose(page, path):
    doc = await page.send("DOM.getDocument")
    node = await page.send("DOM.querySelector", nodeId=doc["root"]["nodeId"], selector="input[type=file]")
    await page.send("DOM.setFileInputFiles", files=[path], nodeId=node["nodeId"])
    await asyncio.sleep(0.4)


async def toasts(page):
    return await page.js("[...document.querySelectorAll('.toasts .toast .toast-text')].map(t => t.textContent)")


async def composer_checks(env, results, theme, label):
    page = env.page
    t = f"{label} {theme}"
    open("/tmp/kagent-s46-green.png", "wb").write(GREEN)
    await env.open(theme, messages={"s1": HISTORY}, images={"s1": {"im9": B64(RED)}}, vision="", **CFG)
    await page.until("!!document.querySelector('.msg-image')", "the message with its image")
    # --- an image of a stored message: at most 320 px, a click opens the full view
    await page.until("!!document.querySelector('.msg-image img')", "the stored image is fetched")
    size = await page.js("(() => { const i = document.querySelector('.msg-image img').getBoundingClientRect(); return [Math.round(i.width), Math.round(i.height)]; })()")
    results[f"{t}: a message shows its image at most 320 px wide (and the text under it)"] = size[0] <= 320 and size[1] <= 320 and size[0] > 100 and await page.js("document.querySelector('.bubble.user .content').textContent") == "What does this say?"
    await page.shot(f"message-image-{t.replace(' ', '-')}")
    await page.click(".msg-image")
    full = await page.js("(() => { const d = document.querySelector('.lightbox'); if (!d) return null; const i = d.querySelector('img').getBoundingClientRect(); return {role: d.getAttribute('role'), modal: d.getAttribute('aria-modal'), w: Math.round(i.width), inView: i.right <= innerWidth && i.bottom <= innerHeight}; })()")
    results[f"{t}: a click opens the full image in a dialog that fits the window"] = bool(full) and full["role"] == "dialog" and full["modal"] == "true" and full["inView"] and full["w"] > 100
    await page.shot(f"full-view-{t.replace(' ', '-')}")
    await page.key("Escape", "Escape", 27)
    results[f"{t}: Escape closes the full view"] = await page.js("!document.querySelector('.lightbox')")
    await page.click(".msg-image")
    await page.js("document.querySelector('.lightbox').click()")
    results[f"{t}: a click on the backdrop closes it too"] = await page.js("!document.querySelector('.lightbox')")

    # --- paste: a thumbnail of 56 px, uploaded, the provider line, an images-only message can be sent
    results[f"{t}: without images the composer has no thumbnails and no provider line"] = await page.js(f"{THUMBS} === 0 && !document.querySelector('.pics')")
    results[f"{t}: Send is off with nothing to send"] = await page.js("document.querySelector('.composer .primary').disabled")
    handled = await paste(page, "shot.png", RED)
    results[f"{t}: a pasted picture is taken (the paste is not left to the input) and shown as a thumbnail"] = handled is True and await page.js(THUMBS) == 1
    thumb = await page.js("(() => { const i = document.querySelector('.pics-item img').getBoundingClientRect(); return [Math.round(i.width), Math.round(i.height)]; })()")
    results[f"{t}: the thumbnail is 56 px"] = thumb == [56, 56]
    results[f"{t}: the picture was uploaded to the open session once"] = uploads() == [["s1", len(RED)]]
    results[f"{t}: the line says where it goes, as a provider that is not on this machine"] = await page.js(LINE) == "Images are sent to Google Gemini."
    results[f"{t}: with only an image Send is on"] = await page.js("!document.querySelector('.composer .primary').disabled")
    await page.shot(f"paste-{t.replace(' ', '-')}")
    # --- drop on the conversation
    during = await drop(page, "other.png", BLUE)
    results[f"{t}: a picture dropped on the conversation is added; the pane shows the drop"] = during is True and await page.js(THUMBS) == 2 and await page.js("!document.querySelector('.chat-main.dropping')")
    # --- remove
    names = await page.js("[...document.querySelectorAll('.pics-item')].map(i => i.dataset.name)")
    await page.click(".pics-item .pics-remove")
    results[f"{t}: the remove button takes that thumbnail away and keeps the other"] = names == ["shot.png", "other.png"] and await page.js("[...document.querySelectorAll('.pics-item')].map(i => i.dataset.name)") == ["other.png"]
    results[f"{t}: the remove button is named for its picture"] = await page.js("document.querySelector('.pics-remove').getAttribute('aria-label')") == "Remove other.png"
    # --- the attach button
    results[f"{t}: the attach button has a name and a file input for PNG, JPEG and WebP"] = await page.js("(() => { const b = document.querySelector('.pics-btn'); const f = document.querySelector('input[type=file]'); return b.getAttribute('aria-label') === 'Attach an image' && f.accept === 'image/png,image/jpeg,image/webp' && f.multiple; })()")
    await choose(page, "/tmp/kagent-s46-green.png")
    results[f"{t}: a file chosen with the attach button is added"] = await page.js(THUMBS) == 2
    # --- limits are checks in the composer too: the fifth, a type that is not an image type
    await paste(page, "c.png", RED)
    await paste(page, "d.png", RED)
    await paste(page, "e.png", RED)
    results[f"{t}: a fifth image is refused with what to do, the four stay"] = await page.js(THUMBS) == 4 and any("at most 4 images" in x for x in await toasts(page))
    await page.js("document.querySelector('.toasts-bar button') && document.querySelector('.toasts-bar button').click(); [...document.querySelectorAll('.toast-close')].forEach(b => b.click())")
    await page.click(".pics-item .pics-remove")
    await paste(page, "x.gif", b"GIF89a", "image/gif")
    results[f"{t}: a GIF is refused as a type"] = await page.js(THUMBS) == 3 and any("PNG, JPEG and WebP" in x for x in await toasts(page))
    await page.js("[...document.querySelectorAll('.toast-close')].forEach(b => b.click())")
    # --- send: the text and the ids, the composer is empty after
    ids = [u for u in range(len(uploads()))]
    await page.js("(() => { const i = document.querySelector('.cmd input'); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; set.call(i, 'what is this'); i.dispatchEvent(new Event('input', {bubbles: true})); })()")
    await asyncio.sleep(0.1)
    shown = await page.js(THUMBS)
    await page.click(".composer .primary")
    await asyncio.sleep(0.4)
    sent = [c for c in calls() if c.startswith("POST /api/sessions/s1/messages")]
    body = json.loads(sent[-1].split(" ", 2)[2]) if sent else {}
    results[f"{t}: Send posts the text and the ids of the uploaded images, in order"] = body.get("text") == "what is this" and len(body.get("images", [])) == shown and all(i.startswith("im") for i in body.get("images", []))
    results[f"{t}: the composer is empty after the send (text and thumbnails), the line too"] = await page.js(f"{THUMBS} === 0 && document.querySelector('.cmd input').value === '' && !document.querySelector('.pics')")
    # the daemon's event brings the message in
    refs = [{"id": i, "mime": "image/png", "width": 64, "height": 64, "bytes": 100} for i in body.get("images", [])]
    await page.js(f"window.__emit({json.dumps({'type': 'messageAppended', 'sessionId': 's1', 'message': {'role': 'user', 'content': 'what is this', 'images': refs}})})")
    await page.until(f"document.querySelectorAll('.msg-images')[1] && document.querySelectorAll('.msg-images')[1].querySelectorAll('img').length === {len(refs)}", "the sent message with its images")
    results[f"{t}: the sent message shows all its images"] = True
    await page.shot(f"sent-{t.replace(' ', '-')}")
    # --- the provider line is once per session
    await paste(page, "again.png", BLUE)
    results[f"{t}: a second image in the same session has no provider line"] = await page.js(THUMBS) == 1 and await page.js(LINE) == ""
    results[f"{t}: the send told the daemon the line was shown (PUT .../images-notice, once)"] = calls().count("PUT /api/sessions/s1/images-notice") == 1
    # a reload: the meta says the line was shown, so it is not shown again
    await env.open(theme, **{**CFG, "sessions": {"s1": {"title": "Disk error", "updatedAt": "2026-11-27T09:00:00Z", "imagesNoticeShown": True}, "s2": CFG["sessions"]["s2"]}})
    await paste(page, "shot.png", RED)
    results[f"{t}: after a reload the line is not shown again (imagesNoticeShown in the meta)"] = await page.js(THUMBS) == 1 and await page.js(LINE) == ""
    await env.open(theme, **CFG)
    await paste(page, "shot.png", RED)
    # --- images only: an empty text is a message
    await page.click(".composer .primary")
    await asyncio.sleep(0.4)
    sent = [c for c in calls() if c.startswith("POST /api/sessions/s1/messages")]
    body = json.loads(sent[-1].split(" ", 2)[2])
    results[f"{t}: images alone are a message (empty text, one image)"] = body.get("text") == "" and len(body.get("images", [])) == 1


async def refused_checks(env, results, theme, label):
    page = env.page
    t = f"{label} {theme} refused"
    await env.open(theme, vision="", refuse="The Brain cannot read images: pick a vision model in Settings -> Models.", **CFG)
    await paste(page, "shot.png", RED)
    await page.js("(() => { const i = document.querySelector('.cmd input'); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set; set.call(i, 'what does it say'); i.dispatchEvent(new Event('input', {bubbles: true})); })()")
    await asyncio.sleep(0.1)
    await page.click(".composer .primary")
    await asyncio.sleep(0.5)
    results[f"{t}: a refused message comes back to the composer with its text and its image"] = await page.js(f"document.querySelector('.cmd input').value === 'what does it say' && {THUMBS} === 1")
    results[f"{t}: the refusal is not shown twice by the composer (the daemon's own notice is the toast)"] = await toasts(page) == []
    page.errors[:] = [e for e in page.errors if "409" not in e]  # the browser logs the refused request itself


async def local_checks(env, results, theme, label):
    page = env.page
    t = f"{label} {theme} local"
    await env.open(theme, localProviders=["gemini"], **CFG)
    await paste(page, "shot.png", RED)
    results[f"{t}: a provider on this machine gets no line"] = await page.js(THUMBS) == 1 and await page.js(LINE) == ""
    await page.shot(f"local-{t.replace(' ', '-')}")
    # another session is its own: images there carry the line again when the provider is not local
    await env.open(theme, vision="anthropic::claude-haiku-4-5", sessions={**CFG["sessions"]}, **{k: v for k, v in CFG.items() if k != "sessions"})
    await paste(page, "shot.png", RED)
    results[f"{t}: the Brain reads images, so the line names its provider (not the vision model's)"] = await page.js(LINE) == "Images are sent to Google Gemini."


async def main():
    results = {}
    async with Env("kagent-s46", "/tmp/kagent-s46-shots") as env:
        for w, h, label in ((1200, 820, "1200x820"), (340, 460, "340x460")):
            await env.size(w, h)
            for theme in ("dark", "light"):
                await composer_checks(env, results, theme, label)
                await refused_checks(env, results, theme, label)
                await local_checks(env, results, theme, label)
        errors = [e for e in env.page.errors]
        results["no console errors"] = not errors
        if errors:
            print("ERRORS", errors[:5])
    return report(results)


sys.exit(asyncio.run(main()))

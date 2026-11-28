// A newer build of the page: the window is told once, with a toast, instead of going on with the old page
// (the owner once tested a fix on the page of the build before it). The page's own script has its content's
// hash in its name, so a different name in the served index.html is a different build.
import { pushNotice } from "./components/toasts";

const SCRIPT = /assets\/index-[\w-]+\.js/;

/** The name of the page's script in an index.html, or null (a dev server has none). */
export function scriptOf(html: string): string | null {
  return SCRIPT.exec(html)?.[0] ?? null;
}

export function watchForNewBuild(everyMs = 60_000) {
  const own = scriptOf([...document.querySelectorAll("script[src]")].map((s) => s.getAttribute("src")).join(" "));
  if (!own) return;
  let told = false;
  const check = async () => {
    if (told || document.visibilityState !== "visible") return;
    try {
      const served = scriptOf(await (await fetch("/index.html", { cache: "no-store" })).text());
      if (served && served !== own) {
        told = true;
        pushNotice({ key: "page-version", level: "info", kind: "error", text: "A newer version of this page is ready. Reload the window (F5) to use it." });
      }
    } catch {
      // The daemon is restarting or gone: the connection notice says so.
    }
  };
  window.setInterval(() => { void check(); }, everyMs);
  document.addEventListener("visibilitychange", () => { void check(); });
}

import { useEffect, useState, useSyncExternalStore } from "react";
import { api, captureToken } from "./api";
import { openSession, refreshSessions, setView, useStore } from "./store";
import Sidebar from "./components/Sidebar";
import { useNarrow } from "./components/sidebar/useNarrow";
import { toggleSidebar } from "./components/sidebar/store";
import { init } from "./storeEvents";
import { installDiagnostics } from "./diag";
import { startDrag, startResize, toggleMaximize } from "./frame";
import Icon from "./components/Icon";
import ResizeEdges from "./components/ResizeEdges";
import ErrorBoundary from "./components/ErrorBoundary";
import Toasts from "./components/Toasts";
import { voice } from "./voice/controller";
import { useVoice } from "./voice/useVoice";
import { isPref, readPref, storePref, subscribeSystemTheme, systemTheme, themeOf, type ThemePref } from "./theme";
import SettingsPage from "./settings/SettingsPage";
import Chat from "./views/Chat";
import System from "./views/System";
import Voice from "./views/Voice";
import Overlay from "./views/Overlay";
import Alerts from "./views/Alerts";
import Changes from "./views/Changes";
import Security from "./views/Security";
import Tasks from "./views/Tasks";

/** Voice keeps running across tabs: show it (and mute/stop) everywhere. */
function VoiceChip() {
  const v = useVoice();
  if (!v.active) return null;
  return (
    <span className={`voice-chip st-${v.status}${v.muted ? " muted" : ""}`}>
      <button className="tab" onClick={() => setView("voice")} title="voice session">
        {v.muted ? "mic muted" : v.status === "live" ? "listening" : v.status}
      </button>
      <button className="tab" onClick={() => v.setMuted(!v.muted)}>{v.muted ? "unmute" : "mute"}</button>
      <button className="tab danger" onClick={() => v.stop()}>end</button>
    </span>
  );
}

/** Widget (compact) and full layout live in one page so voice survives the switch. */
function initialCompact(): boolean {
  const q = new URLSearchParams(location.search);
  return q.has("compact") || q.has("overlay");
}

function setCompactInUrl(compact: boolean) {
  const q = new URLSearchParams(location.search);
  q.delete("overlay");
  q.delete("autostart");
  if (compact) q.set("compact", "");
  else q.delete("compact");
  const qs = q.toString().replace(/=(&|$)/g, "$1");
  history.replaceState(null, "", `${location.pathname}${qs ? `?${qs}` : ""}${location.hash}`);
}

export default function App() {
  const s = useStore();
  const narrow = useNarrow();
  const [compact, setCompact] = useState(initialCompact);
  const isApp = new URLSearchParams(location.search).has("app");
  // Local switch (from the daemon's `layout` command) vs. a click here, which also asks
  // the daemon to resize and pin the window.
  const applyLayout = (mode: string) => {
    const c = mode === "compact";
    setCompact(c);
    setCompactInUrl(c);
    if (!c) document.documentElement.classList.remove("overlay-mode");
  };
  const switchLayout = (mode: "compact" | "full") => {
    applyLayout(mode);
    if (isApp) void api.post("/api/ui/window", { mode });
  };
  useEffect(() => {
    const onLayout = (e: Event) => applyLayout(String((e as CustomEvent).detail));
    window.addEventListener("kagent-layout", onLayout);
    // On load only: a widget layout opened in a big window (restart) becomes the full
    // layout. Never while the user resizes (that once flipped layouts mid-drag).
    if (isApp && new URLSearchParams(location.search).has("compact") && window.innerWidth >= 720 && window.innerHeight >= 520) applyLayout("full");
    return () => {
      window.removeEventListener("kagent-layout", onLayout);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const [token, setToken] = useState<string | null>(null);
  // The owner's choice (auto by default) and the theme in use (theme.ts).
  const [pref, setPref] = useState<ThemePref>(readPref);
  const system = useSyncExternalStore(subscribeSystemTheme, systemTheme, systemTheme);
  const theme = themeOf(pref, system);

  useEffect(() => {
    const t = captureToken();
    setToken(t);
    if (t) {
      init();
      installDiagnostics();
      voice.onSession((id) => void refreshSessions().then(() => openSession(id)));
    }
    const onTheme = (e: Event) => {
      const v = (e as CustomEvent).detail;
      if (isPref(v)) setPref(v);
    };
    window.addEventListener("kagent-theme", onTheme);
    return () => window.removeEventListener("kagent-theme", onTheme);
  }, []);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.themePref = pref;
    storePref(pref);
  }, [theme, pref]);

  if (token && compact) {
    return (
      <ErrorBoundary name="widget">
        <ResizeEdges />
        <Toasts compact />
        <Overlay onExpand={() => switchLayout("full")} onHide={() => void api.post("/api/ui/window", { mode: "hidden" })} />
      </ErrorBoundary>
    );
  }

  if (!token) {
    return (
      <div className="gate">
        <h1>KAgent</h1>
        <p>No token. Open the daemon URL (printed by kagentd), or start the daemon first.</p>
      </div>
    );
  }

  return (
    <div className="app">
      <header className={isApp ? "app-titlebar" : ""} onPointerDown={startDrag} onDoubleClick={toggleMaximize}>
        <button
          id="sidebar-toggle"
          className="tab icon-tab"
          title="Toggle the sidebar"
          aria-label="Toggle the sidebar"
          aria-expanded={narrow ? s.sidebarDrawer : !s.sidebarCollapsed}
          aria-controls="sidebar"
          onClick={toggleSidebar}
        >
          <Icon name="sidebar" size={15} />
        </button>
        <span className="logo">KAgent</span>
        <span className="spacer" />
        {(s.state?.running?.length ?? 0) > 0 && (
          <button className="tab danger" title="Kill switch: cancel every running turn and tool" onClick={() => void api.post("/api/stop-all", {})}>
            Stop all
          </button>
        )}
        <VoiceChip />
        {isApp && (
          <div className="win-controls" role="group" aria-label="Window">
            <button className="win-btn" title="Shrink to the small always-on-top widget" aria-label="Shrink to widget" onClick={() => switchLayout("compact")}>
              <Icon name="shrink" size={15} />
            </button>
            <button className="win-btn" title="Hide to the tray (keeps listening)" aria-label="Hide to tray" onClick={() => void api.post("/api/ui/window", { mode: "hidden" })}>
              <Icon name="tray" size={15} />
            </button>
            <button className="win-btn" title="Maximize or restore (or double-click the title bar)" aria-label="Maximize" onClick={() => toggleMaximize()}>
              <Icon name="max" size={13} />
            </button>
          </div>
        )}
      </header>
      <div className="shell">
      <Sidebar />
      <main>
        <ErrorBoundary key={s.view} name={s.view}>
        {s.view === "chat" && <Chat />}
        {s.view === "voice" && <Voice />}
        {s.view === "tasks" && <Tasks />}
        {s.view === "alerts" && <Alerts />}
        {s.view === "changes" && <Changes />}
        {s.view === "security" && <Security />}
        {s.view === "settings" && <SettingsPage />}
        {s.view === "system" && <System />}
        </ErrorBoundary>
      </main>
      </div>
      <ResizeEdges />
      <Toasts />
      {isApp && (
        <div className="resize-grip" title="Drag to resize" onPointerDown={(e) => startResize(e, "se")}>
          <Icon name="grip" size={14} />
        </div>
      )}
    </div>
  );
}

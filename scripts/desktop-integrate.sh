#!/usr/bin/env bash
# Owner-run (no sudo): add "KAgent" and "KAgent Talk" to the app menu on any desktop,
# plus the desktop-specific parts: KDE gets the KRunner plugin, GNOME gets a
# push-to-talk shortcut (Super+Alt+Space) and the Window Calls hint.
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
apps="$HOME/.local/share/applications"; mkdir -p "$apps"
cat > "$apps/kagent-talk.desktop" <<DESK
[Desktop Entry]
Type=Application
Name=KAgent Talk
Comment=Open the KAgent overlay and start talking
Exec=$repo/scripts/kagent-overlay.sh --talk
Icon=$repo/app/src-tauri/icons/icon.png
Categories=Utility;
DESK
cat > "$apps/kagent.desktop" <<DESK
[Desktop Entry]
Type=Application
Name=KAgent
Comment=Local system agent
Exec=sh -c 'xdg-open "http://127.0.0.1:7878/#token=\$(cat \$HOME/.config/kagent/token)"'
Icon=$repo/app/src-tauri/icons/icon.png
Categories=System;Utility;
DESK
update-desktop-database "$apps" 2>/dev/null || true
desktop="${XDG_CURRENT_DESKTOP:-}"
echo "Added KAgent and KAgent Talk to the app menu."
case "${desktop,,}" in
  *gnome*|*unity*)
    # Custom shortcut: append to the existing list, never replace it.
    base=/org/gnome/settings-daemon/plugins/media-keys
    path="$base/custom-keybindings/kagent-talk/"
    list=$(gsettings get org.gnome.settings-daemon.plugins.media-keys custom-keybindings)
    if [[ "$list" != *"kagent-talk"* ]]; then
      if [[ "$list" == "@as []" || "$list" == "[]" ]]; then new="['$path']"; else new="${list%]}, '$path']"; fi
      gsettings set org.gnome.settings-daemon.plugins.media-keys custom-keybindings "$new"
    fi
    schema="org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:$path"
    gsettings set "$schema" name "KAgent Talk"
    gsettings set "$schema" command "$repo/scripts/kagent-overlay.sh --talk"
    gsettings set "$schema" binding "<Super><Alt>space"
    echo "GNOME: Super+Alt+Space starts talking (change it in Settings -> Keyboard -> Custom Shortcuts)."
    if [[ "${XDG_SESSION_TYPE:-}" == "wayland" ]]; then
      echo "GNOME Wayland: for window control install the 'Window Calls' extension: https://extensions.gnome.org/extension/4724/window-calls/"
    fi
    exit 0
    ;;
  *kde*|*plasma*) ;;
  *)
    echo "Bind a key to 'KAgent Talk' in your desktop's keyboard settings for push-to-talk."
    exit 0
    ;;
esac
# KDE: KRunner plugin (Alt+Space "ka <question>"): served by kagentd on D-Bus.
runners="$HOME/.local/share/krunner/dbusplugins"; mkdir -p "$runners"
cat > "$runners/kagent.desktop" <<DESK
[Desktop Entry]
Name=KAgent
Comment=Ask KAgent: type "ka <question>", "ka doctor", "ka stop"
Icon=audio-input-microphone
Type=Service
X-KDE-ServiceTypes=Plasma/Runner
X-KDE-PluginInfo-Name=kagent
X-KDE-PluginInfo-Version=0.1
X-KDE-PluginInfo-License=MIT
X-KDE-PluginInfo-EnabledByDefault=true
X-Plasma-API=DBus
X-Plasma-DBusRunner-Service=org.kagent.KRunner
X-Plasma-DBusRunner-Path=/runner
DESK
kquitapp6 krunner 2>/dev/null || true
echo "KDE: added the KRunner plugin (Alt+Space: ka <question>)."
echo "Bind a key to 'KAgent Talk' in System Settings -> Shortcuts for push-to-talk."

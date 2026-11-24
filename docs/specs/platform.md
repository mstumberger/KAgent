# Spec: platform backends (desktop + package manager)

KAgent runs on any systemd Linux. Desktop- and distro-specific calls go through
`kagent_core::platform` instead of being hard-coded in tools. Non-systemd distros are
out of scope (services, journal, boot analysis and most watchers need systemd).

## 1. Detection

| What | Source | Override |
|---|---|---|
| desktop: `kde`, `gnome`, `other` | `XDG_CURRENT_DESKTOP` (colon list; `KDE`/`plasma` -> kde, `GNOME`/`gnome-classic`/`Unity` -> gnome) | `KAGENT_DESKTOP` |
| session: Wayland or X11 | `XDG_SESSION_TYPE`, else `WAYLAND_DISPLAY` without `DISPLAY` | |
| packages: `apt`, `dnf`, `pacman`, `unknown` | `/etc/os-release` `ID` + `ID_LIKE` family (debian/ubuntu, fedora/rhel/centos, arch) when its binary exists, else the first of apt-get, dnf/dnf5, pacman | `KAGENT_PM` |

The root helper detects the package manager itself with the same rules (the caller
cannot pick a binary). Shown in `/api/state.platform` and the system prompt.

## 2. Desktop backends

| Feature | KDE | GNOME | other |
|---|---|---|---|
| clipboard get/set | Klipper D-Bus, then the generic tools | wl-clipboard (Wayland), xclip / xsel (X11) | same |
| dark/light mode | `plasma-apply-colorscheme BreezeDark/BreezeLight` | `gsettings org.gnome.desktop.interface color-scheme prefer-dark/default` + `gtk-theme` Adwaita/Yaru `-dark` variant for GTK3 apps; one undo restores both | error |
| named scheme | Plasma scheme | GTK theme name | error |
| brightness | brightnessctl, else report KDE ScreenBrightness displays | brightnessctl, else gnome-settings-daemon `Power.Screen.Brightness` | brightnessctl |
| windows | KWin scripting (X11 + Wayland); list via xprop on X11 | Wayland: "Window Calls" extension (`org.gnome.Shell.Extensions.Windows` List/GetTitle/Activate/Close/Minimize/MoveToWorkspace); X11: wmctrl | X11: wmctrl |
| active window (focus_context) | xprop (X11) | Window Calls `focus` (Wayland), xprop (X11) | xprop |
| input | xdotool (X11), ydotool (Wayland) | same | same |
| launcher integration | KRunner D-Bus runner (only registered on KDE), plasmoid | Super+Alt+Space custom shortcut via `scripts/desktop-integrate.sh` | menu entries |
| app aliases | "files" -> Dolphin, "terminal" -> Konsole | Nautilus, Ptyxis / GNOME Terminal | Thunar, Nemo, kitty, ... |
| editors | Kate, KWrite | GNOME Text Editor, gedit (`+LINE`) | VS Code, JetBrains, Zed everywhere |

ydotool key combos use Linux input event codes (`ctrl+s` -> `29:1 31:1 31:0 29:0`);
scroll is not supported on Wayland.

## 3. Package backends

| Feature | apt | dnf (4 and 5) | pacman |
|---|---|---|---|
| search | `apt-cache search --names-only` | `dnf search -q` | `pacman -Ss` |
| info | `apt-cache policy` + `show` | `dnf info -q` | `pacman -Qi` + `-Si` |
| dry run (approval card) | helper, else `apt-get -s` | helper only (dnf needs root to resolve) | helper, else `pacman --print` |
| install/remove + undo | helper op `package` (priv-ops.md) | same | same |
| pending updates | `apt list --upgradable` | `dnf check-update -q` | `checkupdates` (pacman-contrib), else `pacman -Qu` |
| security updates | lines from `-security` pockets | `dnf updateinfo list --security -q` | none (Arch has no advisories): doctor reports the update count instead |
| history (what_changed) | `/var/log/dpkg.log{,.1}` | `rpm -qa --last` (install time; installs and upgrades look alike) | `/var/log/pacman.log` |
| reboot needed | `/var/run/reboot-required(.pkgs)` | newest `/usr/lib/modules/*` newer than the running kernel | running kernel's modules removed (Arch) or newer kernel |
| file owner hint | `dpkg -S` | `rpm -qf` | `pacman -Qo` |
| firewall | ufw | firewalld | nftables (or ufw/firewalld if installed) |

Parsers are tested on output captured 2026-11-24 from Fedora 45 (dnf5 5.4), Rocky 9
(dnf4) and Arch; the daemon and helper were run end to end in Fedora and Arch
containers (search, info, doctor, what_changed, install + undo).

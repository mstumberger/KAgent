# KAgent desktop shell (Tauri 2)

Windows around kagentd: main window, transparent always-on-top overlay, tray, global
hotkey Super+Space (show overlay + start talking; again = stop + hide).

Separate cargo workspace because Tauri needs GTK/WebKit dev headers.

Owner setup (once):

    sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev
    cd web && pnpm build && cd ..
    cargo build --release --manifest-path app/src-tauri/Cargo.toml
    ./app/src-tauri/target/release/kagent-app

No Tauri yet? `scripts/kagent-overlay.sh` opens the same overlay in Brave/Chrome app mode,
and `scripts/kde-integrate.sh` adds a launcher you can bind to a KDE shortcut.

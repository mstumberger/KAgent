//! Platform backends (docs/specs/platform.md): which desktop (KDE, GNOME, other) and
//! which package manager (apt, dnf, pacman) this machine has. Tools ask here instead of
//! hard-coding one distro or desktop. Detection is pure over its inputs so it is tested
//! with real os-release samples.

pub mod desktop;
pub mod gpu;
pub mod packages;

use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desktop {
    Kde,
    Gnome,
    Other,
}

impl Desktop {
    pub fn name(self) -> &'static str {
        match self {
            Desktop::Kde => "kde",
            Desktop::Gnome => "gnome",
            Desktop::Other => "other",
        }
    }
}

/// `XDG_CURRENT_DESKTOP` (colon list, e.g. `ubuntu:GNOME`, `KDE`, `X-Cinnamon`).
pub fn detect_desktop(xdg_current_desktop: &str) -> Desktop {
    let parts: Vec<String> = xdg_current_desktop.split(':').map(|p| p.trim().to_ascii_lowercase()).collect();
    if parts.iter().any(|p| p == "kde" || p == "plasma") {
        Desktop::Kde
    } else if parts.iter().any(|p| p == "gnome" || p == "gnome-classic" || p == "unity") {
        Desktop::Gnome
    } else {
        Desktop::Other
    }
}

/// The current desktop. `KAGENT_DESKTOP` (kde|gnome|other) overrides detection.
pub fn desktop() -> Desktop {
    match std::env::var("KAGENT_DESKTOP").unwrap_or_default().as_str() {
        "kde" => return Desktop::Kde,
        "gnome" => return Desktop::Gnome,
        "other" => return Desktop::Other,
        _ => {}
    }
    detect_desktop(&std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default())
}

/// Wayland session (X11 tools like xprop/xdotool only see XWayland windows there).
pub fn wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE").map(|t| t == "wayland").unwrap_or(false) || (std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("DISPLAY").is_none())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Apt,
    Dnf,
    Pacman,
    Unknown,
}

impl PackageManager {
    pub fn name(self) -> &'static str {
        match self {
            PackageManager::Apt => "apt",
            PackageManager::Dnf => "dnf",
            PackageManager::Pacman => "pacman",
            PackageManager::Unknown => "unknown",
        }
    }
    /// Command that names the package owning a file (for alert suggestions).
    pub fn owner_cmd(self) -> &'static str {
        match self {
            PackageManager::Apt => "dpkg -S",
            PackageManager::Dnf => "rpm -qf",
            PackageManager::Pacman => "pacman -Qo",
            PackageManager::Unknown => "your package manager",
        }
    }
}

/// From /etc/os-release (`ID`, `ID_LIKE`) and which binaries exist. The family named by
/// os-release wins when its tool is installed; otherwise the first tool found.
pub fn detect_package_manager(os_release: &str, has: impl Fn(&str) -> bool) -> PackageManager {
    let field = |k: &str| {
        os_release
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{k}=")))
            .map(|v| v.trim().trim_matches('"').to_ascii_lowercase())
            .unwrap_or_default()
    };
    let ids = format!("{} {}", field("ID"), field("ID_LIKE"));
    let ids: Vec<&str> = ids.split_whitespace().collect();
    let family = |names: &[&str]| ids.iter().any(|i| names.contains(i));
    let apt = has("/usr/bin/apt-get");
    let dnf = has("/usr/bin/dnf") || has("/usr/bin/dnf5");
    let pacman = has("/usr/bin/pacman");
    if family(&["debian", "ubuntu"]) && apt {
        PackageManager::Apt
    } else if family(&["fedora", "rhel", "centos"]) && dnf {
        PackageManager::Dnf
    } else if family(&["arch", "archlinux"]) && pacman {
        PackageManager::Pacman
    } else if apt {
        PackageManager::Apt
    } else if dnf {
        PackageManager::Dnf
    } else if pacman {
        PackageManager::Pacman
    } else {
        PackageManager::Unknown
    }
}

/// This machine's package manager (cached). `KAGENT_PM` overrides detection.
pub fn package_manager() -> PackageManager {
    static PM: OnceLock<PackageManager> = OnceLock::new();
    *PM.get_or_init(|| match std::env::var("KAGENT_PM").unwrap_or_default().as_str() {
        "apt" => PackageManager::Apt,
        "dnf" => PackageManager::Dnf,
        "pacman" => PackageManager::Pacman,
        _ => detect_package_manager(&std::fs::read_to_string("/etc/os-release").unwrap_or_default(), |p| std::path::Path::new(p).exists()),
    })
}

/// Install command to show the owner (KAgent never runs it).
pub fn install_cmd(pkg: &str) -> String {
    match package_manager() {
        PackageManager::Dnf => format!("sudo dnf install {pkg}"),
        PackageManager::Pacman => format!("sudo pacman -S {pkg}"),
        _ => format!("sudo apt install {pkg}"),
    }
}

/// One-line platform summary for the prompt and `/api/state`.
pub fn summary() -> String {
    format!("desktop {}{}, packages {}", desktop().name(), if wayland() { " (Wayland)" } else { " (X11)" }, package_manager().name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktops() {
        assert_eq!(detect_desktop("KDE"), Desktop::Kde);
        assert_eq!(detect_desktop("ubuntu:GNOME"), Desktop::Gnome);
        assert_eq!(detect_desktop("GNOME-Classic:GNOME"), Desktop::Gnome);
        assert_eq!(detect_desktop("Unity"), Desktop::Gnome);
        assert_eq!(detect_desktop("X-Cinnamon"), Desktop::Other);
        assert_eq!(detect_desktop(""), Desktop::Other);
    }

    #[test]
    fn package_managers_from_real_os_release() {
        let all = |_: &str| true;
        let ubuntu = "NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\n";
        let fedora = "NAME=\"Fedora Linux\"\nID=fedora\nVERSION_ID=45\n";
        let rocky = "ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n";
        let arch = "NAME=\"Arch Linux\"\nID=arch\n";
        let manjaro = "ID=manjaro\nID_LIKE=arch\n";
        let mint = "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n";
        assert_eq!(detect_package_manager(ubuntu, all), PackageManager::Apt);
        assert_eq!(detect_package_manager(mint, all), PackageManager::Apt);
        assert_eq!(detect_package_manager(fedora, all), PackageManager::Dnf);
        assert_eq!(detect_package_manager(rocky, all), PackageManager::Dnf);
        assert_eq!(detect_package_manager(arch, all), PackageManager::Pacman);
        assert_eq!(detect_package_manager(manjaro, all), PackageManager::Pacman);
        // Family tool missing: fall back to whatever exists (dnf5-only Fedora).
        assert_eq!(detect_package_manager(fedora, |p| p == "/usr/bin/dnf5"), PackageManager::Dnf);
        assert_eq!(detect_package_manager("ID=nixos\n", |_| false), PackageManager::Unknown);
    }
}

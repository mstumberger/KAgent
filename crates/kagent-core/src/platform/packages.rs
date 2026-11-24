//! Unprivileged package queries per package manager: search, info, pending updates,
//! security updates, install history, "reboot needed". Privileged install/remove goes
//! through `kagent-priv` (op `package`). Parsers are tested against output captured
//! from Ubuntu, Fedora 45 (dnf5), Rocky 9 (dnf4) and Arch.

use chrono::{DateTime, Local, NaiveDateTime, TimeZone};

use super::PackageManager;
use crate::tool::ToolContext;
use crate::tools::exec::exec;

fn v(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

/// Package names accepted anywhere (Fedora has `NetworkManager`, Arch `lib32-...`).
pub fn valid_name(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 100
        && p.starts_with(|c: char| c.is_ascii_alphanumeric())
        && p.chars().all(|c| c.is_ascii_alphanumeric() || "+._@-".contains(c))
}

pub fn search_argv(pm: PackageManager, name: &str) -> Option<Vec<String>> {
    Some(match pm {
        PackageManager::Apt => v(&["apt-cache", "search", "--names-only", name]),
        PackageManager::Dnf => v(&["dnf", "search", "-q", name]),
        PackageManager::Pacman => v(&["pacman", "-Ss", name]),
        PackageManager::Unknown => return None,
    })
}

/// Commands whose outputs together describe one package.
pub fn info_argv(pm: PackageManager, name: &str) -> Vec<Vec<String>> {
    match pm {
        PackageManager::Apt => vec![v(&["apt-cache", "policy", name]), v(&["apt-cache", "show", "--no-all-versions", name])],
        PackageManager::Dnf => vec![v(&["dnf", "info", "-q", name])],
        PackageManager::Pacman => vec![v(&["pacman", "-Qi", name]), v(&["pacman", "-Si", name])],
        PackageManager::Unknown => vec![],
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Updates {
    pub all: Vec<String>,
    /// None = this distro does not publish security advisories (Arch).
    pub security: Option<Vec<String>>,
}

/// `apt list --upgradable`: `htop/noble-updates 3.3.0-4 amd64 [upgradable from: 3.2]`.
pub fn parse_apt_upgradable(out: &str) -> Updates {
    let all: Vec<String> = out.lines().filter(|l| l.contains("[upgradable")).map(String::from).collect();
    let security = all.iter().filter(|l| l.contains("-security")).cloned().collect();
    Updates { all, security: Some(security) }
}

/// `dnf check-update -q` (dnf4 and dnf5): `curl.x86_64  8.21.0-9.fc45  updates`.
pub fn parse_dnf_check_update(out: &str) -> Vec<String> {
    out.lines()
        .filter(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            t.len() == 3 && t[0].contains('.') && !l.starts_with(' ')
        })
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

/// `dnf updateinfo list --security -q`: dnf5 has a `security` type column, dnf4 `/Sec.`.
pub fn parse_dnf_security(out: &str) -> Vec<String> {
    out.lines()
        .filter(|l| l.split_whitespace().any(|t| t == "security" || t.ends_with("/Sec.")))
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

/// `pacman -Qu` / `checkupdates`: `gpgme 2.2.0-1 -> 2.3.1-1`.
pub fn parse_pacman_updates(out: &str) -> Vec<String> {
    out.lines().filter(|l| l.contains(" -> ")).map(|l| l.trim().to_string()).collect()
}

/// Pending updates (and security updates where the distro has advisories).
pub async fn updates(ctx: &ToolContext, pm: PackageManager) -> Updates {
    let out = |argv: Vec<String>| async move { exec(ctx, &argv).run().await.map(|r| r.stdout).unwrap_or_default() };
    match pm {
        PackageManager::Apt => parse_apt_upgradable(&out(v(&["apt", "list", "--upgradable"])).await),
        PackageManager::Dnf => Updates {
            all: parse_dnf_check_update(&out(v(&["dnf", "check-update", "-q"])).await),
            security: Some(parse_dnf_security(&out(v(&["dnf", "updateinfo", "list", "--security", "-q"])).await)),
        },
        PackageManager::Pacman => {
            // checkupdates (pacman-contrib) syncs a private db copy; plain -Qu uses the
            // last `pacman -Sy` and may be stale.
            let argv = if crate::tools::xdg::which("checkupdates").is_some() { v(&["checkupdates"]) } else { v(&["pacman", "-Qu"]) };
            Updates { all: parse_pacman_updates(&out(argv).await), security: None }
        }
        PackageManager::Unknown => Updates::default(),
    }
}

/// `[2026-11-24T08:57:48+0000] [ALPM] upgraded htop (3.2-1 -> 3.3-1)`.
pub fn parse_pacman_log(log: &str, since: DateTime<Local>) -> Vec<(DateTime<Local>, String)> {
    log.lines()
        .filter_map(|l| {
            let (ts, rest) = l.strip_prefix('[')?.split_once("] [ALPM] ")?;
            let ts = DateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S%z").ok()?.with_timezone(&Local);
            let (action, rest) = rest.split_once(' ')?;
            let verb = match action {
                "installed" => "installed",
                "upgraded" => "upgraded",
                "downgraded" => "downgraded",
                "removed" => "removed",
                _ => return None,
            };
            let (pkg, ver) = rest.split_once(' ').unwrap_or((rest, ""));
            (ts >= since).then(|| (ts, format!("package {verb}: {pkg} {}", ver.trim_matches(|c| c == '(' || c == ')'))))
        })
        .collect()
}

/// `rpm -qa --last`: `htop-3.5.3-1.fc45.x86_64   Tue Nov 24 08:56:53 2026` (LC_ALL=C).
/// rpm records install time only, so installs and upgrades look the same.
pub fn parse_rpm_last(out: &str, since: DateTime<Local>) -> Vec<(DateTime<Local>, String)> {
    out.lines()
        .filter_map(|l| {
            let (pkg, when) = l.split_once("  ")?;
            let pkg = pkg.trim();
            if pkg.starts_with("gpg-pubkey") {
                return None;
            }
            let when = when.split_whitespace().take(5).collect::<Vec<_>>().join(" ");
            let ts = NaiveDateTime::parse_from_str(&when, "%a %b %d %H:%M:%S %Y").ok()?;
            let ts = Local.from_local_datetime(&ts).single()?;
            (ts >= since).then(|| (ts, format!("package installed/upgraded: {pkg}")))
        })
        .collect()
}

/// Package events since `since` for `what_changed`.
pub async fn history(ctx: &ToolContext, pm: PackageManager, since: DateTime<Local>) -> Vec<(DateTime<Local>, String)> {
    match pm {
        PackageManager::Apt => {
            let mut ev = Vec::new();
            // dpkg.log is world-readable on Debian/Ubuntu; .1 covers rotation.
            for f in ["/var/log/dpkg.log.1", "/var/log/dpkg.log"] {
                if let Ok(log) = tokio::fs::read_to_string(f).await {
                    ev.extend(crate::tools::timeline::parse_dpkg(&log, since));
                }
            }
            ev
        }
        PackageManager::Dnf => {
            let out = exec(ctx, &["rpm", "-qa", "--last"]).run().await.map(|r| r.stdout).unwrap_or_default();
            parse_rpm_last(&out, since)
        }
        PackageManager::Pacman => parse_pacman_log(&tokio::fs::read_to_string("/var/log/pacman.log").await.unwrap_or_default(), since),
        PackageManager::Unknown => vec![],
    }
}

/// Natural version order: `6.12.10` > `6.12.9`.
pub fn version_key(s: &str) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    let mut num = String::new();
    let mut txt = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            if !txt.is_empty() {
                out.push((0, std::mem::take(&mut txt)));
            }
            num.push(c);
        } else {
            if !num.is_empty() {
                out.push((num.parse().unwrap_or(0), String::new()));
                num.clear();
            }
            txt.push(c);
        }
    }
    if !num.is_empty() {
        out.push((num.parse().unwrap_or(0), String::new()));
    }
    if !txt.is_empty() {
        out.push((0, txt));
    }
    out
}

/// Reboot needed because a newer kernel is installed (or the running kernel's modules
/// were removed, as on Arch). `module_dirs` = names under /usr/lib/modules.
pub fn kernel_reboot_reason(running: &str, module_dirs: &[String]) -> Option<String> {
    let dirs: Vec<&String> = module_dirs.iter().filter(|d| d.chars().next().is_some_and(|c| c.is_ascii_digit())).collect();
    if dirs.is_empty() || running.is_empty() {
        return None;
    }
    if !dirs.iter().any(|d| d.as_str() == running) {
        return Some(format!("running kernel {running} was removed by an update"));
    }
    let newest = dirs.iter().max_by_key(|d| version_key(d))?;
    (version_key(newest) > version_key(running)).then(|| format!("kernel {newest} installed, running {running}"))
}

/// Why a reboot is pending, if it is.
pub async fn reboot_required(pm: PackageManager) -> Option<String> {
    if pm == PackageManager::Apt {
        // Debian/Ubuntu keep old kernels; their flag file is precise.
        if tokio::fs::metadata("/var/run/reboot-required").await.is_err() {
            return None;
        }
        let pkgs = tokio::fs::read_to_string("/var/run/reboot-required.pkgs").await.unwrap_or_default();
        let pkgs = pkgs.split_whitespace().collect::<Vec<_>>().join(", ");
        return Some(if pkgs.is_empty() { "yes".into() } else { pkgs });
    }
    let running = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default().trim().to_string();
    let dirs: Vec<String> = std::fs::read_dir("/usr/lib/modules").map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
    kernel_reboot_reason(&running, &dirs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long_ago() -> DateTime<Local> {
        Local.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap()
    }

    #[test]
    fn names() {
        assert!(valid_name("NetworkManager") && valid_name("lib32-glibc") && valid_name("python3.12") && valid_name("g++"));
        assert!(!valid_name("-o=x") && !valid_name("a b") && !valid_name("x;rm") && !valid_name(""));
    }

    #[test]
    fn dnf_outputs() {
        // Fedora 45 (dnf5) `check-update -q`.
        let cu = "Upgrades\ncurl.x86_64            8.21.0-9.fc45     updates\ndnf5.x86_64            5.4.8.0-2.fc45    updates\n";
        assert_eq!(parse_dnf_check_update(cu), vec!["curl.x86_64 8.21.0-9.fc45 updates", "dnf5.x86_64 5.4.8.0-2.fc45 updates"]);
        // Rocky 9 (dnf4) starts with a blank line.
        assert_eq!(parse_dnf_check_update("\ncurl-minimal.x86_64                 7.76.1-44.el9_9.1                     baseos\n").len(), 1);
        let sec5 = "Name                   Type     Severity                             Package              Issued\nFEDORA-2026-0866c99872 security Moderate        tpm2-tss-4.2.2-1.fc45.x86_64 2026-11-16 15:55:37\n";
        let sec4 = "RLSA-2026:100686 Important/Sec. curl-minimal-7.76.1-44.el9_9.1.x86_64\n";
        assert_eq!(parse_dnf_security(sec5).len(), 1);
        assert_eq!(parse_dnf_security(sec4), vec!["RLSA-2026:100686 Important/Sec. curl-minimal-7.76.1-44.el9_9.1.x86_64"]);
        let rpm = "hwloc-libs-2.14.0-3.fc45.x86_64               Tue Nov 24 08:56:53 2026\ngpg-pubkey-4f50a6114cd5-6888bc98 Sun Nov  1 06:47:42 2026\nxz-5.8.3-2.fc45.x86_64                        Sun Nov  1 06:47:38 2026\n";
        let ev = parse_rpm_last(rpm, long_ago());
        assert_eq!(ev.len(), 2, "gpg-pubkey skipped: {ev:?}");
        assert_eq!(ev[0].1, "package installed/upgraded: hwloc-libs-2.14.0-3.fc45.x86_64");
        assert_eq!(ev[1].0.format("%Y-%m-%d %H:%M").to_string(), "2026-11-01 06:47");
    }

    #[test]
    fn pacman_and_apt_outputs() {
        assert_eq!(parse_pacman_updates("gpgme 2.2.0-1 -> 2.3.1-1\nlibseccomp 2.6.0-1 -> 2.6.2-1\n").len(), 2);
        let log = "[2026-11-24T08:57:48+0000] [ALPM] transaction started\n[2026-11-24T08:57:48+0000] [ALPM] installed htop (3.5.3-1)\n[2026-11-24T08:58:00+0000] [ALPM] upgraded gpgme (2.2.0-1 -> 2.3.1-1)\n[2026-11-24T08:58:01+0000] [ALPM] running '35-systemd-update.hook'...\n";
        let ev = parse_pacman_log(log, long_ago());
        assert_eq!(ev.iter().map(|e| e.1.as_str()).collect::<Vec<_>>(), vec!["package installed: htop 3.5.3-1", "package upgraded: gpgme 2.2.0-1 -> 2.3.1-1"]);
        let apt = "Listing...\nopenssl/noble-security 3.0.13-0ubuntu3.5 amd64 [upgradable from: 3.0.13-0ubuntu3.4]\nhtop/noble-updates 3.3.0-4 amd64 [upgradable from: 3.3.0-3]\n";
        let u = parse_apt_upgradable(apt);
        assert_eq!((u.all.len(), u.security.unwrap().len()), (2, 1));
    }

    #[test]
    fn kernel_reboot() {
        let d = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(kernel_reboot_reason("6.12.9-arch1-1", &d(&["6.12.10-arch1-1"])).unwrap(), "running kernel 6.12.9-arch1-1 was removed by an update");
        assert_eq!(kernel_reboot_reason("6.11.5-300.fc41.x86_64", &d(&["6.11.5-300.fc41.x86_64", "6.11.10-300.fc41.x86_64"])).unwrap(), "kernel 6.11.10-300.fc41.x86_64 installed, running 6.11.5-300.fc41.x86_64");
        assert!(kernel_reboot_reason("6.11.10-300.fc41.x86_64", &d(&["6.11.5-300.fc41.x86_64", "6.11.10-300.fc41.x86_64", "extramodules-6.11"])).is_none());
        assert!(kernel_reboot_reason("6.1", &[]).is_none());
    }
}

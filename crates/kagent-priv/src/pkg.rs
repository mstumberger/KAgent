//! Package install/remove for apt, dnf and pacman, and firewall status for ufw,
//! firewalld and nftables (docs/specs/priv-ops.md, op `package` / `firewall_status`).
//! The helper detects the package manager itself; the caller cannot choose a binary.
//!
//! Undo: apt from its simulation (`Inst`/`Remv` lines); dnf and pacman by diffing the
//! installed package names before and after (their dry-run tables are not stable to
//! parse: dnf4 wraps columns, dnf5 changed the layout).

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use serde_json::json;

use crate::ops::{argv, fail, output, re_package, Fail, Response};
use crate::system::{RunOutput, System};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pm {
    Apt,
    Dnf,
    Pacman,
}

/// Same rules as kagent-core's platform detection: os-release family first, then
/// whichever tool exists.
pub fn detect(sys: &dyn System) -> Option<Pm> {
    let osr = sys.read(Path::new("/etc/os-release")).unwrap_or_default().to_ascii_lowercase();
    let field = |k: &str| osr.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).map(|v| v.trim_matches('"').to_string()).unwrap_or_default();
    let ids = format!("{} {}", field("id"), field("id_like"));
    let fam = |n: &[&str]| ids.split_whitespace().any(|i| n.contains(&i));
    let apt = sys.exists(Path::new("/usr/bin/apt-get"));
    let dnf = sys.exists(Path::new("/usr/bin/dnf"));
    let pacman = sys.exists(Path::new("/usr/bin/pacman"));
    if fam(&["debian", "ubuntu"]) && apt {
        Some(Pm::Apt)
    } else if fam(&["fedora", "rhel", "centos"]) && dnf {
        Some(Pm::Dnf)
    } else if fam(&["arch"]) && pacman {
        Some(Pm::Pacman)
    } else if apt {
        Some(Pm::Apt)
    } else if dnf {
        Some(Pm::Dnf)
    } else if pacman {
        Some(Pm::Pacman)
    } else {
        None
    }
}

fn installed(sys: &mut dyn System, pm: Pm) -> BTreeSet<String> {
    let a = match pm {
        Pm::Apt => argv(&["/usr/bin/dpkg-query", "-W", "-f", "${Package}\n"]),
        Pm::Dnf => argv(&["/usr/bin/rpm", "-qa", "--qf", "%{NAME}\n"]),
        Pm::Pacman => argv(&["/usr/bin/pacman", "-Qq"]),
    };
    sys.run(&a, Duration::from_secs(60)).stdout.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

fn simulate_argv(pm: Pm, action: &str, packages: &[String]) -> Vec<String> {
    let mut a = match (pm, action) {
        (Pm::Apt, _) => argv(&["/usr/bin/apt-get", "-y", "-s", action, "--"]),
        (Pm::Dnf, _) => argv(&["/usr/bin/dnf", "--assumeno", action, "--"]),
        (Pm::Pacman, "install") => argv(&["/usr/bin/pacman", "-S", "--needed", "--print", "--print-format", "%n %v", "--"]),
        (Pm::Pacman, _) => argv(&["/usr/bin/pacman", "-R", "--print", "--print-format", "%n %v", "--"]),
    };
    a.extend(packages.iter().cloned());
    a
}

fn apply_argv(pm: Pm, action: &str, packages: &[String]) -> Vec<String> {
    let mut a = match (pm, action) {
        (Pm::Apt, _) => argv(&["/usr/bin/apt-get", "-y", action, "--"]),
        (Pm::Dnf, _) => argv(&["/usr/bin/dnf", "-y", action, "--"]),
        (Pm::Pacman, "install") => argv(&["/usr/bin/pacman", "-S", "--noconfirm", "--needed", "--"]),
        (Pm::Pacman, _) => argv(&["/usr/bin/pacman", "-R", "--noconfirm", "--"]),
    };
    a.extend(packages.iter().cloned());
    a
}

/// Did the dry run fail? dnf's exit code is useless here (1 for a fine aborted
/// install, 0 for an aborted remove), so look at the text.
pub fn simulation_failed(pm: Pm, sim: &RunOutput) -> bool {
    let text = format!("{}\n{}", sim.stdout, sim.stderr);
    match pm {
        Pm::Apt => !sim.ok(),
        Pm::Dnf => ["Error:", "Problem:", "No match for argument", "Failed to resolve", "No packages to remove", "No package"].iter().any(|m| text.contains(m)) || sim.timed_out,
        Pm::Pacman => text.lines().any(|l| l.starts_with("error:")) || sim.timed_out || sim.exit.is_some_and(|c| c != 0),
    }
}

/// Package op. Limits: the agent asks for at most 20 names; undo lists (dependencies
/// included) may carry up to 200.
pub(crate) fn run(sys: &mut dyn System, action: &str, packages: &[String], simulate: bool, r: &mut Response) -> Result<(), Fail> {
    if action != "install" && action != "remove" {
        return fail("invalid_input", "action must be install or remove");
    }
    if packages.is_empty() || packages.len() > 200 || !packages.iter().all(|p| re_package().is_match(p)) {
        return fail("invalid_input", "1-200 valid package names required");
    }
    let Some(pm) = detect(sys) else { return fail("not_allowed", "no supported package manager (apt, dnf, pacman)") };
    let sim = sys.run(&simulate_argv(pm, action, packages), Duration::from_secs(180));
    let sim_failed = simulation_failed(pm, &sim);
    if simulate {
        output(r, sim);
        r.ok = !sim_failed;
        r.code = sim_failed.then(|| "failed".into());
        return Ok(());
    }
    if sim_failed {
        output(r, sim);
        r.ok = false;
        r.code = Some("failed".into());
        return Ok(());
    }
    let undo_action = if action == "install" { "remove" } else { "install" };
    let changed: Vec<String> = if pm == Pm::Apt {
        let marker = if action == "install" { "Inst " } else { "Remv " };
        let c: Vec<String> = sim.stdout.lines().filter_map(|l| l.strip_prefix(marker)).filter_map(|l| l.split_whitespace().next().map(String::from)).collect();
        output(r, sys.run(&apply_argv(pm, action, packages), Duration::from_secs(600)));
        c
    } else {
        let before = installed(sys, pm);
        output(r, sys.run(&apply_argv(pm, action, packages), Duration::from_secs(600)));
        let after = installed(sys, pm);
        if action == "install" { after.difference(&before).cloned().collect() } else { before.difference(&after).cloned().collect() }
    };
    if !changed.is_empty() {
        r.after = Some(format!("{} {}: {}", if action == "install" { "installed" } else { "removed" }, changed.len(), changed.join(" ")));
        r.undo = Some(json!({"op": "package", "action": undo_action, "packages": changed}));
    }
    Ok(())
}

/// ufw, else firewalld, else the raw nftables ruleset (first 400 lines).
pub fn firewall_status(sys: &mut dyn System, r: &mut Response) {
    let t = Duration::from_secs(15);
    if sys.exists(Path::new("/usr/sbin/ufw")) {
        output(r, sys.run(&argv(&["/usr/sbin/ufw", "status", "verbose"]), t));
    } else if sys.exists(Path::new("/usr/bin/firewall-cmd")) {
        output(r, sys.run(&argv(&["/usr/bin/firewall-cmd", "--list-all"]), t));
    } else if sys.exists(Path::new("/usr/sbin/nft")) {
        let mut out = sys.run(&argv(&["/usr/sbin/nft", "list", "ruleset"]), t);
        out.stdout = out.stdout.lines().take(400).collect::<Vec<_>>().join("\n");
        output(r, out);
    } else {
        r.ok = true;
        r.stdout = "no firewall tool found (ufw, firewalld, nftables)".into();
    }
}

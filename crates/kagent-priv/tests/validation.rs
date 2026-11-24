//! Validation, undo and execution planning of every op, against a fake system.
//! No root, no real commands.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

use kagent_priv::ops::handle;
use kagent_priv::system::{RunOutput, System};
use serde_json::{json, Value};

#[derive(Default)]
struct Fake {
    files: BTreeMap<PathBuf, String>,
    dirs: BTreeSet<PathBuf>,
    symlinks: BTreeSet<PathBuf>,
    runs: Vec<Vec<String>>,
    kills: Vec<(i32, String)>,
    stdout_for: BTreeMap<String, String>,
    /// Successive replies for one command (installed-package lists before/after).
    queue_for: BTreeMap<String, VecDeque<String>>,
    exit_for: BTreeMap<String, i32>,
}

impl Fake {
    fn file(mut self, p: &str, c: &str) -> Self {
        self.files.insert(p.into(), c.into());
        self
    }
    fn dir(mut self, p: &str) -> Self {
        self.dirs.insert(p.into());
        self
    }
    fn reply(mut self, argv_contains: &str, out: &str) -> Self {
        self.stdout_for.insert(argv_contains.into(), out.into());
        self
    }
    fn replies(mut self, argv_contains: &str, outs: &[&str]) -> Self {
        self.queue_for.insert(argv_contains.into(), outs.iter().map(|s| s.to_string()).collect());
        self
    }
    fn exit(mut self, argv_contains: &str, code: i32) -> Self {
        self.exit_for.insert(argv_contains.into(), code);
        self
    }
}

impl System for Fake {
    fn read(&self, p: &Path) -> Option<String> {
        self.files.get(p).cloned()
    }
    fn exists(&self, p: &Path) -> bool {
        self.files.contains_key(p) || self.dirs.contains(p)
    }
    fn canonicalize(&self, p: &Path) -> Option<PathBuf> {
        if self.symlinks.contains(p) {
            return Some(PathBuf::from("/elsewhere"));
        }
        self.exists(p).then(|| p.to_path_buf())
    }
    fn is_symlink(&self, p: &Path) -> bool {
        self.symlinks.contains(p)
    }
    fn write(&mut self, p: &Path, c: &str, _mode: u32) -> std::io::Result<()> {
        self.files.insert(p.to_path_buf(), c.into());
        Ok(())
    }
    fn remove(&mut self, p: &Path) -> std::io::Result<()> {
        self.files.remove(p);
        Ok(())
    }
    fn run(&mut self, argv: &[String], _t: Duration) -> RunOutput {
        self.runs.push(argv.to_vec());
        let joined = argv.join(" ");
        let queued = self.queue_for.iter_mut().find(|(k, _)| joined.contains(k.as_str())).and_then(|(_, q)| q.pop_front());
        let stdout = queued.unwrap_or_else(|| {
            self.stdout_for
                .iter()
                .find(|(k, _)| joined.contains(k.as_str()))
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        });
        let exit = self.exit_for.iter().find(|(k, _)| joined.contains(k.as_str())).map(|(_, c)| *c).unwrap_or(0);
        RunOutput { exit: Some(exit), stdout, ..Default::default() }
    }
    fn kill(&mut self, pid: i32, sig: &str) -> Result<(), String> {
        self.kills.push((pid, sig.into()));
        Ok(())
    }
    fn caller_home(&self) -> Option<PathBuf> {
        Some("/home/u".into())
    }
    fn now_stamp(&self) -> String {
        "1700000000".into()
    }
}

fn go(sys: &mut Fake, req: Value) -> Value {
    serde_json::to_value(handle(&req.to_string(), sys)).unwrap()
}

fn code(v: &Value) -> &str {
    v["code"].as_str().unwrap_or("")
}

#[test]
fn rejects_garbage_and_unknown() {
    let mut s = Fake::default();
    assert_eq!(code(&serde_json::to_value(handle("{nope", &mut s)).unwrap()), "invalid_input");
    assert_eq!(code(&go(&mut s, json!({"op": "format_disk"}))), "invalid_input");
    assert_eq!(code(&go(&mut s, json!({"op": "kill", "pid": 5, "signal": "TERM", "extra": 1}))), "invalid_input");
    assert!(s.runs.is_empty());
}

#[test]
fn sysctl_set_validates_persists_and_undoes() {
    let mut s = Fake::default().file("/proc/sys/vm/swappiness", "60\n");
    for bad in [json!({"op":"sysctl_set","key":"vm.swappiness","value":"10; rm -rf /"}),
                json!({"op":"sysctl_set","key":"../etc","value":"1"}),
                json!({"op":"sysctl_set","key":"vm.nope","value":"1"})] {
        assert!(!go(&mut s, bad)["ok"].as_bool().unwrap());
    }
    let denied = go(&mut s, json!({"op":"sysctl_set","key":"kernel.core_pattern","value":"x"}));
    assert_eq!(code(&denied), "not_allowed");

    let r = go(&mut s, json!({"op":"sysctl_set","key":"vm.swappiness","value":"10"}));
    assert!(r["ok"].as_bool().unwrap(), "{r}");
    assert_eq!(r["before"], "60");
    assert_eq!(r["undo"], json!({"op":"sysctl_set","key":"vm.swappiness","value":"60","persist":true}));
    assert_eq!(s.runs.last().unwrap(), &vec!["/usr/sbin/sysctl", "-w", "vm.swappiness=10"]);
    let conf = s.files[Path::new("/etc/sysctl.d/99-kagent.conf")].clone();
    assert!(conf.contains("vm.swappiness = 10"), "{conf}");
    // Second set replaces the line instead of appending.
    go(&mut s, json!({"op":"sysctl_set","key":"vm.swappiness","value":"20"}));
    let conf = &s.files[Path::new("/etc/sysctl.d/99-kagent.conf")];
    assert_eq!(conf.matches("vm.swappiness").count(), 1);
}

#[test]
fn dry_run_executes_nothing() {
    let mut s = Fake::default().file("/proc/sys/vm/swappiness", "60");
    let r = go(&mut s, json!({"op":"sysctl_set","key":"vm.swappiness","value":"10","dryRun":true}));
    assert!(r["ok"].as_bool().unwrap() && r["dryRun"].as_bool().unwrap());
    assert!(r["undo"].is_object());
    assert!(s.runs.is_empty() && !s.files.contains_key(Path::new("/etc/sysctl.d/99-kagent.conf")));
}

#[test]
fn modprobe_load_remove_and_params() {
    let mut s = Fake::default();
    assert_eq!(code(&go(&mut s, json!({"op":"modprobe","module":"bt usb"}))), "invalid_input");
    assert_eq!(code(&go(&mut s, json!({"op":"modprobe","module":"btusb","params":{"a":"$(id)"}}))), "invalid_input");
    let r = go(&mut s, json!({"op":"modprobe","module":"btusb"}));
    assert_eq!(r["undo"], json!({"op":"modprobe","module":"btusb","remove":true}));
    let mut s = Fake::default().dir("/sys/module/snd_hda_intel");
    let r = go(&mut s, json!({"op":"modprobe","module":"snd-hda-intel","remove":true}));
    assert_eq!(r["undo"], json!({"op":"modprobe","module":"snd-hda-intel","remove":false}));
    assert_eq!(s.runs[0], vec!["/usr/sbin/modprobe", "-r", "snd-hda-intel"]);
    // Removing something not loaded: nothing to undo.
    let mut s = Fake::default();
    assert!(go(&mut s, json!({"op":"modprobe","module":"zram","remove":true}))["undo"].is_null());
}

#[test]
fn modprobe_conf_writes_and_restores() {
    let mut s = Fake::default().dir("/etc/modprobe.d").file("/etc/modprobe.d/kagent-btusb.conf", "options btusb enable_autosuspend=1\n");
    let r = go(&mut s, json!({"op":"modprobe_conf","module":"btusb","params":{"enable_autosuspend":"0"}}));
    assert!(r["ok"].as_bool().unwrap(), "{r}");
    assert_eq!(r["undo"]["params"], json!({"enable_autosuspend":"1"}));
    assert!(s.files[Path::new("/etc/modprobe.d/kagent-btusb.conf")].contains("enable_autosuspend=0"));
    go(&mut s, json!({"op":"modprobe_conf","module":"btusb","params":{}}));
    assert!(!s.files.contains_key(Path::new("/etc/modprobe.d/kagent-btusb.conf")));
}

#[test]
fn systemctl_rules_and_undo() {
    let mut s = Fake::default().reply("is-active", "inactive").reply("is-enabled", "disabled");
    assert_eq!(code(&go(&mut s, json!({"op":"systemctl","action":"mask","unit":"x"}))), "invalid_input");
    assert_eq!(code(&go(&mut s, json!({"op":"systemctl","action":"start","unit":"a b"}))), "invalid_input");
    for unit in ["dbus", "systemd-journald.service", "sshd", "kagentd.service", "polkit"] {
        assert_eq!(code(&go(&mut s, json!({"op":"systemctl","action":"stop","unit":unit}))), "not_allowed", "{unit}");
    }
    let r = go(&mut s, json!({"op":"systemctl","action":"start","unit":"bluetooth"}));
    assert_eq!(r["undo"], json!({"op":"systemctl","action":"stop","unit":"bluetooth.service"}));
    let r = go(&mut s, json!({"op":"systemctl","action":"restart","unit":"bluetooth.service"}));
    assert!(r["undo"].is_null());
    assert!(s.runs.iter().any(|a| a == &vec!["/usr/bin/systemctl", "restart", "bluetooth.service"]));
}

#[test]
fn apt_simulates_then_records_installed_packages() {
    let mut s = Fake::default().file("/etc/os-release", "ID=ubuntu\nID_LIKE=debian\n").file("/usr/bin/apt-get", "").reply("-s install", "Inst htop (3.3 Ubuntu)\nInst libnl (1 Ubuntu)\nConf htop");
    assert_eq!(code(&go(&mut s, json!({"op":"apt","action":"purge","packages":["htop"]}))), "invalid_input");
    assert_eq!(code(&go(&mut s, json!({"op":"apt","action":"install","packages":["-o=x"]}))), "invalid_input");
    assert_eq!(code(&go(&mut s, json!({"op":"apt","action":"install","packages":[]}))), "invalid_input");
    let r = go(&mut s, json!({"op":"apt","action":"install","packages":["htop"],"simulate":true}));
    assert!(r["stdout"].as_str().unwrap().contains("Inst htop"));
    assert_eq!(s.runs.len(), 1);
    let r = go(&mut s, json!({"op":"apt","action":"install","packages":["htop"]}));
    assert_eq!(r["undo"], json!({"op":"package","action":"remove","packages":["htop","libnl"]}));
    assert_eq!(s.runs.last().unwrap(), &vec!["/usr/bin/apt-get", "-y", "install", "--", "htop"]);
    // New op name, same path; dryRun only simulates.
    let n = s.runs.len();
    let r = go(&mut s, json!({"op":"package","action":"install","packages":["htop"],"dryRun":true}));
    assert!(r["ok"].as_bool().unwrap());
    assert_eq!(s.runs.len(), n + 1);
}

// Real dnf5 output captured on Fedora 45 (2026-11-24).
const DNF_SIM: &str = "Package     Arch   Version         Repository      Size\nInstalling:\n htop       x86_64 0:3.5.3-1.fc45  fedora     520.8 KiB\nInstalling dependencies:\n hwloc-libs x86_64 0:2.14.0-3.fc45 fedora       2.9 MiB\n\nTransaction Summary:\n Installing:         2 packages\nOperation aborted by the user.\n";

#[test]
fn dnf_install_undo_from_installed_diff() {
    let mut s = Fake::default()
        .file("/etc/os-release", "NAME=\"Fedora Linux\"\nID=fedora\n")
        .file("/usr/bin/dnf", "")
        .reply("--assumeno install", DNF_SIM)
        .exit("--assumeno", 1) // dnf5 exits 1 on an aborted (fine) install
        .replies("rpm -qa", &["bash\nglibc\n", "bash\nglibc\nhtop\nhwloc-libs\n"]);
    let r = go(&mut s, json!({"op":"package","action":"install","packages":["htop"]}));
    assert!(r["ok"].as_bool().unwrap(), "{r}");
    assert_eq!(r["undo"], json!({"op":"package","action":"remove","packages":["htop","hwloc-libs"]}));
    assert!(s.runs.iter().any(|a| a == &vec!["/usr/bin/dnf", "-y", "install", "--", "htop"]));
    // Resolution problems are failures even with exit 0 (dnf5 remove of a protected package).
    let mut s = Fake::default()
        .file("/etc/os-release", "ID=fedora\n")
        .file("/usr/bin/dnf", "")
        .reply("--assumeno remove", "Failed to resolve the transaction:\nProblem: The operation would result in removing the following protected packages: sudo\n");
    let r = go(&mut s, json!({"op":"package","action":"remove","packages":["sudo"]}));
    assert!(!r["ok"].as_bool().unwrap());
    assert!(!s.runs.iter().any(|a| a.contains(&"-y".to_string())), "nothing applied");
    assert!(r["undo"].is_null());
}

#[test]
fn pacman_remove_undo_and_errors() {
    let mut s = Fake::default()
        .file("/etc/os-release", "NAME=\"Arch Linux\"\nID=arch\n")
        .file("/usr/bin/pacman", "")
        .reply("-R --print", "htop 3.5.3-1\n")
        .replies("pacman -Qq", &["bash\nhtop\n", "bash\n"]);
    let r = go(&mut s, json!({"op":"package","action":"remove","packages":["htop"]}));
    assert_eq!(r["undo"], json!({"op":"package","action":"install","packages":["htop"]}));
    assert!(s.runs.iter().any(|a| a == &vec!["/usr/bin/pacman", "-R", "--noconfirm", "--", "htop"]));
    let mut s = Fake::default().file("/etc/os-release", "ID=arch\n").file("/usr/bin/pacman", "").reply("-S --needed --print", "error: target not found: nosuchpkg\n").exit("--print", 1);
    let r = go(&mut s, json!({"op":"package","action":"install","packages":["nosuchpkg"]}));
    assert!(!r["ok"].as_bool().unwrap());
    assert_eq!(s.runs.len(), 1, "only the dry run ran");
    // No package manager at all.
    let mut s = Fake::default().file("/etc/os-release", "ID=nixos\n");
    assert_eq!(code(&go(&mut s, json!({"op":"package","action":"install","packages":["htop"]}))), "not_allowed");
    // Fedora-style names are valid; option injection is not.
    let mut s = Fake::default().file("/etc/os-release", "ID=fedora\n").file("/usr/bin/dnf", "");
    assert_ne!(code(&go(&mut s, json!({"op":"package","action":"install","packages":["NetworkManager"],"dryRun":true}))), "invalid_input");
    assert_eq!(code(&go(&mut s, json!({"op":"package","action":"install","packages":["--setopt=x"]}))), "invalid_input");
}

#[test]
fn firewall_status_per_tool() {
    let mut s = Fake::default().file("/usr/bin/firewall-cmd", "").reply("--list-all", "public (active)\n  services: ssh\n");
    let r = go(&mut s, json!({"op":"firewall_status"}));
    assert!(r["stdout"].as_str().unwrap().contains("public"));
    let mut s = Fake::default().file("/usr/sbin/ufw", "");
    go(&mut s, json!({"op":"ufw_status"}));
    assert_eq!(s.runs.last().unwrap(), &vec!["/usr/sbin/ufw", "status", "verbose"]);
}

#[test]
fn write_file_allowlist_symlinks_and_backup() {
    let mut s = Fake::default().dir("/etc/sysctl.d").dir("/etc/modprobe.d");
    for bad in ["/etc/passwd", "/etc/sysctl.d/../shadow", "/etc/sysctl.d/10-other.conf", "etc/sysctl.d/99-kagent.conf", "/etc/modprobe.d/kagent-x.conf/../../x"] {
        assert_eq!(code(&go(&mut s, json!({"op":"write_file","path":bad,"content":"x"}))), "not_allowed", "{bad}");
    }
    assert_eq!(code(&go(&mut s, json!({"op":"write_file","path":"/etc/sysctl.d/99-kagent.conf","content":"x","mode":"4755"}))), "invalid_input");
    s.symlinks.insert("/etc/modprobe.d/kagent-evil.conf".into());
    assert_eq!(code(&go(&mut s, json!({"op":"write_file","path":"/etc/modprobe.d/kagent-evil.conf","content":"x"}))), "not_allowed");

    let r = go(&mut s, json!({"op":"write_file","path":"/etc/modprobe.d/kagent-new.conf","content":"options a b=1\n"}));
    assert_eq!(r["undo"], json!({"op":"write_file","path":"/etc/modprobe.d/kagent-new.conf","delete":true}));
    let r = go(&mut s, json!({"op":"write_file","path":"/etc/modprobe.d/kagent-new.conf","content":"options a b=2\n"}));
    assert_eq!(r["undo"]["content"], "options a b=1\n");
    assert!(r["backup"].as_str().unwrap().starts_with("/var/lib/kagent/backup/1700000000-etc_modprobe.d_kagent-new.conf"));
}

#[test]
fn read_root_allowlist() {
    let mut s = Fake::default().dir("/var/log").file("/var/log/auth.log", "a\nb\nc\n").file("/etc/shadow", "secret");
    assert_eq!(code(&go(&mut s, json!({"op":"read_root","path":"/etc/shadow"}))), "not_allowed");
    assert_eq!(code(&go(&mut s, json!({"op":"read_root","path":"/var/log/../../etc/shadow"}))), "not_allowed");
    let r = go(&mut s, json!({"op":"read_root","path":"/var/log/auth.log","tailLines":2}));
    assert_eq!(r["stdout"], "b\nc");
}

#[test]
fn kill_protects_init_and_kernel_threads() {
    let mut s = Fake::default()
        .file("/proc/42/comm", "sleep\n").file("/proc/42/cmdline", "sleep\u{0}600")
        .file("/proc/7/comm", "kworker\n").file("/proc/7/cmdline", "")
        .file("/proc/9/comm", "systemd\n").file("/proc/9/cmdline", "/lib/systemd/systemd");
    assert_eq!(code(&go(&mut s, json!({"op":"kill","pid":1,"signal":"KILL"}))), "invalid_input");
    assert_eq!(code(&go(&mut s, json!({"op":"kill","pid":42,"signal":"SEGV"}))), "invalid_input");
    assert_eq!(code(&go(&mut s, json!({"op":"kill","pid":7,"signal":"TERM"}))), "not_allowed");
    assert_eq!(code(&go(&mut s, json!({"op":"kill","pid":9,"signal":"TERM"}))), "not_allowed");
    assert_eq!(code(&go(&mut s, json!({"op":"kill","pid":99,"signal":"TERM"}))), "invalid_input");
    assert!(go(&mut s, json!({"op":"kill","pid":42,"signal":"TERM"}))["ok"].as_bool().unwrap());
    assert_eq!(s.kills, vec![(42, "TERM".to_string())]);
}

#[test]
fn smartctl_bpftrace_and_root_shell() {
    let mut s = Fake::default();
    assert_eq!(code(&go(&mut s, json!({"op":"smartctl","device":"/dev/../etc"}))), "invalid_input");
    assert!(go(&mut s, json!({"op":"smartctl","device":"/dev/nvme0n1"}))["ok"].as_bool().unwrap());
    assert_eq!(code(&go(&mut s, json!({"op":"bpftrace","script":"io-by-process","seconds":60}))), "invalid_input");
    assert_eq!(code(&go(&mut s, json!({"op":"bpftrace","script":"rm -rf","seconds":5}))), "invalid_input");
    assert_eq!(code(&go(&mut s, json!({"op":"bpftrace","script":"open-snoop","seconds":5,"pathPrefix":"/home/u/\"x"}))), "invalid_input");
    assert!(go(&mut s, json!({"op":"bpftrace","script":"open-snoop","seconds":5,"pathPrefix":"/home/u/.aws"}))["ok"].as_bool().unwrap());
    assert_eq!(s.runs.last().unwrap()[0], "/usr/bin/bpftrace");
    assert_eq!(code(&go(&mut s, json!({"op":"shell_root","command":"id"}))), "not_allowed");
    let mut s = Fake::default().file("/etc/kagent/allow-root-shell", "");
    assert!(go(&mut s, json!({"op":"shell_root","command":"id"}))["ok"].as_bool().unwrap());
}

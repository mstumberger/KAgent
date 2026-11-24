//! Potentially destructive actions: always confirmed by the user, in every permission
//! mode including full control (docs/specs/tools.md section 16). Owner report: with
//! full control, "Should we reboot?" (a question) and the mis-heard words "Ezel" and "b"
//! each rebooted the PC without asking. The daemon enforces this; the model cannot skip it.

/// A shell command line that can lose data or the session: deletes, overwrites, kills,
/// power actions, disk/partition tools, destructive git.
pub fn looks_destructive(cmd: &str) -> bool {
    let c = cmd.to_lowercase();
    let words: Vec<&str> = c.split(|ch: char| ch.is_whitespace() || ";|&()`$".contains(ch)).filter(|w| !w.is_empty()).collect();
    let has = |w: &str| words.contains(&w);
    let power = ["reboot", "poweroff", "shutdown", "halt"].iter().any(|w| has(w))
        || c.contains("systemctl reboot")
        || c.contains("systemctl poweroff")
        || c.contains("systemctl halt")
        || c.contains("systemctl kexec")
        || c.contains("loginctl terminate")
        || c.contains("loginctl kill");
    let delete = has("rm") || has("rmdir") || has("shred") || has("unlink") || has("wipefs") || c.contains("find ") && c.contains(" -delete");
    let disk = words.iter().any(|w| w.starts_with("mkfs") || *w == "dd" || *w == "fdisk" || *w == "parted" || *w == "sgdisk" || *w == "mkswap");
    let kill = ["kill", "pkill", "killall", "xkill"].iter().any(|w| has(w));
    let git = c.contains("git reset --hard") || c.contains("git clean") || c.contains("git push --force") || c.contains("git push -f") || c.contains("git checkout -- ") || c.contains("git branch -d");
    let truncate = c.contains("> /") || c.contains(">/") || has("truncate");
    let pkg = (has("apt") || has("apt-get") || has("dnf") || has("pacman")) && (has("remove") || has("purge") || has("autoremove") || has("-r") || has("-rns"));
    power || delete || disk || kill || git || truncate || pkg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destructive_commands() {
        for c in ["reboot", "sudo systemctl reboot", "shutdown -h now", "rm -rf build", "cd /tmp && rm x", "find . -name '*.o' -delete", "mkfs.ext4 /dev/sdb1", "dd if=/dev/zero of=/dev/sda", "pkill firefox", "kill -9 1234", "git reset --hard HEAD~1", "echo x > /etc/hosts", "apt remove htop"] {
            assert!(looks_destructive(c), "{c}");
        }
        for c in ["ls -la", "cat /etc/hosts", "git status", "journalctl -b", "echo hello", "grep -r remove .", "systemctl status ssh", "ps aux | grep kill-switch"] {
            assert!(!looks_destructive(c), "{c}");
        }
    }
}

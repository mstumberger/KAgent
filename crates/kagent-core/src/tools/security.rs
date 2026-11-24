//! security_report (docs/specs/tools.md section 10): the machine's exposure in one call.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::exec::exec;
use crate::tool::{Tool, ToolContext, ToolContract, ToolOutput};

pub struct SecurityReport {
    contract: ToolContract,
}

impl SecurityReport {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "security_report",
                "Security posture: listening ports with the owning process, firewall state, accounts with a login shell and sudo members, recent logins and SSH failures, Secure Boot/lockdown, and who is using the microphone, camera and network right now. Optional `section`.",
                json!({"type":"object","properties":{"section":{"type":"string","enum":["ports","firewall","users","logins","boot","devices"]}}}),
            ),
        }
    }
}

async fn run(ctx: &ToolContext, argv: &[&str]) -> String {
    match exec(ctx, argv).run().await {
        Ok(r) => r.render(),
        Err(e) => format!("unavailable: {e}\n"),
    }
}

#[async_trait]
impl Tool for SecurityReport {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let want = input.get("section").and_then(Value::as_str);
        let on = |s: &str| want.is_none_or(|w| w == s);
        let mut out = String::new();
        if on("ports") {
            out.push_str("## listening ports (process)\n");
            out.push_str(&run(&ctx, &["ss", "-tulpnH"]).await);
        }
        if on("firewall") {
            out.push_str("\n## firewall\n");
            // ufw / firewalld rules need root; the helper answers when installed.
            let txt = match super::privop::query_root(&ctx, &json!({"op": "firewall_status"})).await {
                Ok(r) if r.ok => r.stdout,
                _ => {
                    let mut t = String::new();
                    for svc in ["ufw", "firewalld", "nftables"] {
                        let active = run(&ctx, &["systemctl", "is-active", svc]).await;
                        let word = active.lines().next().unwrap_or("").trim();
                        let state = if ["active", "inactive", "failed", "activating", "deactivating"].contains(&word) { word } else { "unknown" };
                        t.push_str(&format!("{svc} service: {state}\n"));
                    }
                    t.push_str("(rule details need the root helper)\n");
                    t
                }
            };
            out.push_str(&txt);
        }
        if on("users") {
            out.push_str("\n## accounts with a login shell\n");
            let passwd = tokio::fs::read_to_string("/etc/passwd").await.unwrap_or_default();
            for l in passwd.lines() {
                let f: Vec<&str> = l.split(':').collect();
                if f.len() >= 7 && !f[6].ends_with("nologin") && !f[6].ends_with("false") && !f[6].ends_with("sync") {
                    out.push_str(&format!("{} uid {} shell {}\n", f[0], f[2], f[6]));
                }
            }
            out.push_str("sudo group: ");
            out.push_str(run(&ctx, &["getent", "group", "sudo"]).await.trim());
            out.push('\n');
        }
        if on("logins") {
            out.push_str("\n## recent logins\n");
            out.push_str(&run(&ctx, &["last", "-n", "15", "-w"]).await);
            out.push_str("\n## SSH failures/accepts (24 h)\n");
            out.push_str(&run(&ctx, &["journalctl", "--no-pager", "-q", "-u", "ssh", "-u", "sshd", "--since", "-24h", "-g", "Failed|Accepted|Invalid", "-n", "30", "-o", "short-iso"]).await);
        }
        if on("boot") {
            out.push_str("\n## boot security\n");
            out.push_str(&run(&ctx, &["mokutil", "--sb-state"]).await);
            out.push_str(&format!("lockdown: {}\n", super::read_trim("/sys/kernel/security/lockdown").await.unwrap_or_else(|| "unknown".into())));
            out.push_str(&format!("tainted: {}\n", super::read_trim("/proc/sys/kernel/tainted").await.unwrap_or_default()));
        }
        if on("devices") {
            out.push_str("\n## microphone users\n");
            let dump = exec(&ctx, &["pw-dump"]).run().await.map(|r| r.stdout).unwrap_or_default();
            let nodes: Value = serde_json::from_str(&dump).unwrap_or(Value::Null);
            let mut mic = Vec::new();
            for n in nodes.as_array().into_iter().flatten() {
                let p = &n["info"]["props"];
                if p["media.class"].as_str() == Some("Stream/Input/Audio") {
                    mic.push(format!("{} (pid {})", p["application.name"].as_str().unwrap_or("?"), p["application.process.id"]));
                }
            }
            out.push_str(&if mic.is_empty() { "none\n".to_string() } else { mic.join("\n") + "\n" });
            out.push_str("\n## camera users\n");
            let mut cams = Vec::new();
            if let Ok(mut rd) = tokio::fs::read_dir("/proc").await {
                while let Ok(Some(e)) = rd.next_entry().await {
                    let Ok(mut fds) = tokio::fs::read_dir(e.path().join("fd")).await else { continue };
                    while let Ok(Some(fd)) = fds.next_entry().await {
                        if tokio::fs::read_link(fd.path()).await.is_ok_and(|l| l.to_string_lossy().starts_with("/dev/video")) {
                            let comm = super::read_trim(e.path().join("comm")).await.unwrap_or_default();
                            cams.push(format!("{comm} (pid {})", e.file_name().to_string_lossy()));
                            break;
                        }
                    }
                }
            }
            cams.sort();
            cams.dedup();
            out.push_str(&if cams.is_empty() { "none\n".to_string() } else { cams.join("\n") + "\n" });
            out.push_str("\n## established connections (process)\n");
            let est = run(&ctx, &["ss", "-tpnH", "state", "established"]).await;
            out.push_str(&est.lines().take(40).collect::<Vec<_>>().join("\n"));
            out.push('\n');
        }
        Ok(ToolOutput::ok(out))
    }
}

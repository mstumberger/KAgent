//! The subcommands of `kagentd` other than `serve`.

use crate::{mcp, net, paths};

/// Run subcommand `arg` (anything else prints the usage and exits).
pub fn run(arg: &str) {
    match arg {
        "audit" => {
            if std::env::args().nth(2).as_deref() == Some("verify") {
                let path = paths::Paths::resolve().data.join("audit.jsonl");
                match kagent_core::audit::verify(&path) {
                    Ok(n) => println!("OK: {n} records, chain intact ({})", path.display()),
                    Err((line, reason)) => {
                        println!("BROKEN at line {line}: {reason} ({})", path.display());
                        std::process::exit(1);
                    }
                }
            } else {
                usage();
            }
        }
        "sudoers" => print_sudoers(),
        "mikrotik-setup" => net::print_setup(&std::env::args().nth(2).unwrap_or_else(|| "<this-PC-IP>".into())),
        "router-unblock" => {
            // Undo path for router_block_ip (recorded in the change journal).
            let addr = std::env::args().nth(2).unwrap_or_default();
            let token = std::fs::read_to_string(paths::Paths::resolve().token()).unwrap_or_default();
            let out = std::process::Command::new("curl")
                .args(["-fsS", "-X", "POST", "-H", &format!("Authorization: Bearer {}", token.trim()), "-H", "Content-Type: application/json", "-d", &serde_json::json!({"address": addr}).to_string(), "http://127.0.0.1:7878/api/net/unblock"])
                .status();
            std::process::exit(if out.is_ok_and(|s| s.success()) { 0 } else { 1 });
        }
        "mcp" => {
            // stdout is the protocol channel: logs must not go there.
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            if let Err(e) = rt.block_on(mcp::serve_stdio(&paths::Paths::resolve())) {
                eprintln!("kagentd mcp: {e:#}");
                std::process::exit(1);
            }
        }
        _ => usage(),
    }
}

/// Owner action (never executed by KAgent): install the root helper + sudoers rule.
fn print_sudoers() {
    let user = std::env::var("USER").unwrap_or_else(|_| "<your-user>".into());
    let repo = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| ".".into());
    if let Some(line) = kagent_core::tools::privop::setup_oneliner() {
        println!("# KAgent root helper setup: paste this ONE line into a terminal (asks for your password once):");
        println!("{line}");
        println!();
        println!("# Or step by step from the source tree:");
    } else {
        println!("# KAgent root helper setup. Review, then run these yourself (KAgent never runs sudo).");
    }
    println!("cd {repo}");
    println!("cargo build --release -p kagent-priv");
    println!("sudo install -o root -g root -m 0755 target/release/kagent-priv {}", kagent_core::tools::privop::DEFAULT_HELPER);
    println!("echo '{user} ALL=(root) NOPASSWD: {}' | sudo tee /etc/sudoers.d/kagent", kagent_core::tools::privop::DEFAULT_HELPER);
    println!("sudo chmod 0440 /etc/sudoers.d/kagent && sudo visudo -c");
    println!();
    println!("# Optional: allow free-form root shell commands (each one still asks in KAgent):");
    println!("# sudo mkdir -p /etc/kagent && sudo touch /etc/kagent/allow-root-shell");
    println!("# Remove everything: sudo rm /etc/sudoers.d/kagent {}", kagent_core::tools::privop::DEFAULT_HELPER);
}

pub fn usage() {
    eprintln!("usage: kagentd [serve | audit verify | sudoers | mcp]");
    std::process::exit(2);
}

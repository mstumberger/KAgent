//! Call every read-only tool with `{}` and print the first 300 chars of its output
//! (IMPLEMENTATION.md S1 check). Non-{} tools get a minimal valid input.

use kagent_core::tool::ToolContext;
use kagent_core::tools::builtin_registry;
use serde_json::json;
use std::time::Duration;

#[tokio::main]
async fn main() {
    let registry = builtin_registry();
    let mut names: Vec<String> = registry.contracts().iter().map(|c| c.name.clone()).collect();
    names.sort();
    for name in &names {
        let tool = registry.get(name).unwrap();
        let input = match name.as_str() {
            "read_file" => json!({"path": "/etc/hostname"}),
            "list_dir" => json!({"path": "/tmp"}),
            "search_files" => json!({"pattern": "root", "path": "/etc/hostname"}),
            "process_signal" | "shell" | "kernel_module" => {
                // mutating-only tools: verify the not-available/guard path
                match name.as_str() {
                    "process_signal" => json!({"pid": 1}),
                    "shell" => json!({"command": "echo hi"}),
                    _ => json!({"action": "info", "module": "nvme"}),
                }
            }
            "kernel_info" => json!({"section": "version"}),
            "journal" => json!({"lines": 5}),
            "systemd" => json!({"action": "list", "state": "failed"}),
            "sysctl" => json!({"action": "get", "key": "vm.swappiness"}),
            _ => json!({}),
        };
        let ctx = ToolContext::standalone(Duration::from_secs(10), 24_000);
        let out = tool.call(input, ctx).await;
        let text = match out {
            Ok(o) => o.content,
            Err(e) => format!("error: {e:#}"),
        };
        let head: String = text.chars().take(300).collect();
        println!("=== {} ===\n{}\n", name, head);
    }
}

use std::io::{Read, Write};

fn main() {
    let mut raw = String::new();
    let limit = 1024 * 1024;
    let mut handle = std::io::stdin().take(limit as u64 + 1);
    if handle.read_to_string(&mut raw).is_err() || raw.len() > limit {
        println!(r#"{{"ok":false,"op":"?","code":"invalid_input","message":"stdin unreadable or over 1 MiB"}}"#);
        std::process::exit(2);
    }
    let mut sys = kagent_priv::system::RealSystem;
    let resp = kagent_priv::ops::handle(&raw, &mut sys);
    let line = serde_json::to_string(&resp).unwrap_or_else(|_| r#"{"ok":false,"op":"?","code":"failed"}"#.into());
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
}

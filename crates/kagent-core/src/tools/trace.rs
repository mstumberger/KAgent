//! trace (docs/specs/tools.md section 12): short, read-only kernel tracing through the
//! root helper's allowlisted bpftrace scripts. Answers "what is writing to my disk",
//! "why is it laggy", "who is spamming syscalls", "is the network dropping packets".

use async_trait::async_trait;
use serde_json::{json, Value};

use super::privop::query_root;
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};

pub const PROBES: [&str; 6] = ["io-by-process", "runq-latency", "syscalls-top", "tcp-retransmits", "exec-snoop", "open-snoop"];

pub struct Trace {
    contract: ToolContract,
}

impl Trace {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "trace",
                "Kernel tracing with eBPF for a few seconds (root helper, asks first). Probes: io-by-process (who does disk I/O), runq-latency (CPU scheduling delay histogram = lag), syscalls-top (busiest processes by syscalls), tcp-retransmits (lossy connections), exec-snoop (programs started), open-snoop (files opened under path_prefix).",
                json!({"type":"object","required":["probe"],"properties":{
                    "probe":{"type":"string","enum":PROBES},
                    "seconds":{"type":"integer","description":"1-10, default 5"},
                    "path_prefix":{"type":"string","description":"open-snoop only, e.g. /home/marko/.ssh"}
                }}),
            )
            .mutating(SideEffect::Kernel, Risk::Low)
            .privileged()
            .timeout(30_000, 40_000),
        }
    }
}

/// bpftrace prints maps as `@name[key]: value`; keep the top rows, sorted by value.
pub fn summarize(out: &str, limit: usize) -> String {
    let mut rows: Vec<(String, u64)> = Vec::new();
    let mut other = Vec::new();
    // bpftrace compiler warnings: "stdin:2:104-129: WARNING: ..." + source + caret lines.
    let mut skip = 0;
    for line in out.lines() {
        let l = line.trim();
        if l.starts_with("stdin:") && l.contains("WARNING") {
            skip = 2;
            continue;
        }
        if skip > 0 {
            skip -= 1;
            continue;
        }
        if l.is_empty() || l.starts_with("Attaching") {
            continue;
        }
        if let (true, Some((k, v))) = (l.starts_with('@'), l.rsplit_once(':')) {
            if let Ok(n) = v.trim().parse::<u64>() {
                rows.push((k.trim().to_string(), n));
                continue;
            }
        }
        other.push(l.to_string());
    }
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    let mut s = String::new();
    for (k, v) in rows.iter().take(limit) {
        s.push_str(&format!("{v:>14}  {k}\n"));
    }
    if rows.len() > limit {
        s.push_str(&format!("... {} more\n", rows.len() - limit));
    }
    // Histograms and event lines (exec/open snoop) pass through, capped.
    let rest: Vec<&String> = other.iter().take(80).collect();
    if !rest.is_empty() {
        s.push_str(&rest.iter().map(|x| x.as_str()).collect::<Vec<_>>().join("\n"));
        s.push('\n');
        if other.len() > 80 {
            s.push_str(&format!("... {} more lines\n", other.len() - 80));
        }
    }
    if s.is_empty() {
        s.push_str("(no events in the sampling window)\n");
    }
    s
}

#[async_trait]
impl Tool for Trace {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let probe = input.get("probe").and_then(Value::as_str).unwrap_or("?");
        let secs = input.get("seconds").and_then(Value::as_u64).unwrap_or(5);
        CallAssessment::from_contract(&self.contract, format!("eBPF trace {probe} for {secs}s (read-only)"), format!("trace {probe}"))
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let probe = input.get("probe").and_then(Value::as_str).unwrap_or("");
        if !PROBES.contains(&probe) {
            return Ok(ToolOutput::error(format!("probe must be one of {}", PROBES.join(", "))));
        }
        let seconds = input.get("seconds").and_then(Value::as_u64).unwrap_or(5).clamp(1, 10);
        let mut op = json!({"op": "bpftrace", "script": probe, "seconds": seconds});
        if probe == "open-snoop" {
            let Some(p) = input.get("path_prefix").and_then(Value::as_str).filter(|p| p.starts_with('/')) else {
                return Ok(ToolOutput::error("open-snoop needs path_prefix (absolute path)"));
            };
            op["pathPrefix"] = json!(p);
        }
        let r = query_root(&ctx, &op).await?;
        if !r.ok && r.stdout.trim().is_empty() {
            return Ok(ToolOutput::error(format!("bpftrace failed ({}): {} {}", r.code.unwrap_or_default(), r.message.unwrap_or_default(), crate::util::truncate_str(r.stderr.trim(), 1500))));
        }
        Ok(ToolOutput::ok(format!("## {probe} ({seconds}s)\n{}", summarize(&r.stdout, 15))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_maps_and_passes_events() {
        let out = "Attaching 2 probes...\n@bytes[firefox]: 4096\n@bytes[kworker/u8:1]: 1048576\n@bytes[cp]: 52428800\n\n";
        let s = summarize(out, 2);
        let lines: Vec<&str> = s.lines().collect();
        assert!(lines[0].ends_with("@bytes[cp]") && lines[1].ends_with("@bytes[kworker/u8:1]"), "{s}");
        assert!(s.contains("1 more"));
        let ev = summarize("1234 1 bash /usr/bin/ls\n", 10);
        assert!(ev.contains("/usr/bin/ls"));
        assert!(summarize("Attaching 1 probe...\n", 5).contains("no events"));
    }
}

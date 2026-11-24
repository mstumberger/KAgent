//! Embedded bpftrace scripts (docs/specs/priv-ops.md section 4). Only these run; the
//! caller passes an id. Placeholders: {S} seconds, {P} path prefix, {N} prefix length.

pub const SCRIPTS: &[(&str, &str)] = &[
    (
        "io-by-process",
        "tracepoint:block:block_rq_issue { @bytes[comm] = sum(args.bytes); }\ninterval:s:{S} { exit(); }",
    ),
    (
        "runq-latency",
        "tracepoint:sched:sched_wakeup, tracepoint:sched:sched_wakeup_new { @q[args.pid] = nsecs; }\n\
tracepoint:sched:sched_switch /@q[args.next_pid]/ { @usecs = hist((nsecs - @q[args.next_pid]) / 1000); delete(@q[args.next_pid]); }\n\
interval:s:{S} { clear(@q); exit(); }",
    ),
    ("syscalls-top", "tracepoint:raw_syscalls:sys_enter { @[comm] = count(); }\ninterval:s:{S} { exit(); }"),
    (
        "tcp-retransmits",
        "tracepoint:tcp:tcp_retransmit_skb { @[ntop(args.daddr), args.dport] = count(); }\ninterval:s:{S} { exit(); }",
    ),
    (
        "exec-snoop",
        "tracepoint:syscalls:sys_enter_execve { printf(\"%d %d %s %s\\n\", pid, curtask->real_parent->tgid, comm, str(args.filename)); }\ninterval:s:{S} { exit(); }",
    ),
    (
        "open-snoop",
        "tracepoint:syscalls:sys_enter_openat /strncmp(str(args.filename), \"{P}\", {N}) == 0/ { printf(\"%d %d %s %s\\n\", pid, curtask->real_parent->tgid, comm, str(args.filename)); }\ninterval:s:{S} { exit(); }",
    ),
];

pub fn render(id: &str, seconds: u64, prefix: Option<&str>) -> Option<String> {
    let (_, body) = SCRIPTS.iter().find(|(k, _)| *k == id)?;
    let mut s = body.replace("{S}", &seconds.to_string());
    if id == "open-snoop" {
        let p = prefix?;
        s = s.replace("{P}", p).replace("{N}", &p.len().to_string());
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_all_scripts() {
        for (id, _) in SCRIPTS {
            let s = render(id, 5, Some("/home/u/.aws")).unwrap();
            assert!(!s.contains("{S}") && !s.contains("{P}") && !s.contains("{N}"), "{id}: {s}");
            assert!(s.contains("interval:s:5"));
        }
        assert!(render("open-snoop", 5, None).is_none());
        assert!(render("nope", 5, None).is_none());
        assert!(render("open-snoop", 3, Some("/tmp/x")).unwrap().contains("\"/tmp/x\", 6"));
    }
}
